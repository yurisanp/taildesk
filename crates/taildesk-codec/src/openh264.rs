//! Backend openh264: Constrained Baseline, vídeo em tempo real, controle por bitrate.
//!
//! O crate não expõe trocar o bitrate de um encoder aberto; [`EncoderOpenH264::ajustar_bitrate`]
//! recria o encoder no próximo quadro, que sai como IDR. A seleção adaptativa muda o alvo a cada
//! poucos segundos, então o custo é um quadro-chave por ajuste.

use openh264::OpenH264API;
use openh264::decoder::{Decoder, DecoderConfig};
use openh264::encoder::{
    BitRate, Complexity, Encoder, EncoderConfig, FrameRate, FrameType, IntraFramePeriod,
    RateControlMode, UsageType, VuiConfig,
};
use openh264::formats::{YUVSlices, YUVSource};

use crate::cor::I420;
use crate::{
    ConfigEncoder, ErroCodec, Faixa, FormatoYuv, Matriz, PlanoYuv, QuadroRgb, QuadroYuv,
    TipoQuadro, VideoDecoder, VideoEncoder,
};

fn backend(e: openh264::Error) -> ErroCodec {
    ErroCodec::Backend(e.to_string())
}

/// Encoder H.264 em software.
pub struct EncoderOpenH264 {
    config: ConfigEncoder,
    encoder: Option<Encoder>,
    yuv: I420,
}

impl std::fmt::Debug for EncoderOpenH264 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EncoderOpenH264")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl EncoderOpenH264 {
    pub fn novo(config: ConfigEncoder) -> Result<Self, ErroCodec> {
        let mut e = EncoderOpenH264 {
            config,
            encoder: None,
            yuv: I420::default(),
        };
        e.encoder = Some(e.abrir()?);
        Ok(e)
    }

    fn abrir(&self) -> Result<Encoder, ErroCodec> {
        // O modo "câmera" codifica tela ~30% mais rápido e com PSNR maior (53 vs 44 dB) que o
        // "tela", ao custo de uns 3x de bitrate. `TAILDESK_OPENH264_USO=tela` volta ao antigo.
        let uso = match std::env::var("TAILDESK_OPENH264_USO").as_deref() {
            Ok("tela") => UsageType::ScreenContentRealTime,
            _ => UsageType::CameraVideoRealTime,
        };
        let threads = std::env::var("TAILDESK_OPENH264_THREADS")
            .ok()
            .and_then(|t| t.parse().ok())
            .unwrap_or(0);
        let cfg = EncoderConfig::new()
            .usage_type(uso)
            .num_threads(threads)
            .rate_control_mode(RateControlMode::Bitrate)
            .bitrate(BitRate::from_bps(self.config.bitrate_bps))
            .max_frame_rate(FrameRate::from_hz(self.config.fps_max))
            .skip_frames(true)
            .complexity(Complexity::Low)
            .intra_frame_period(IntraFramePeriod::auto())
            .vui(VuiConfig::bt709());
        Encoder::with_api_config(OpenH264API::from_source(), cfg).map_err(backend)
    }
}

impl VideoEncoder for EncoderOpenH264 {
    fn nome(&self) -> &'static str {
        "openh264"
    }

    fn codificar(
        &mut self,
        quadro: &QuadroRgb<'_>,
        forcar_chave: bool,
        saida: &mut Vec<u8>,
    ) -> Result<Option<TipoQuadro>, ErroCodec> {
        saida.clear();
        if !quadro.valido() {
            return Err(ErroCodec::QuadroInvalido);
        }
        let encoder = match &mut self.encoder {
            Some(e) => e,
            None => self.encoder.insert(self.abrir()?),
        };
        self.yuv.preparar(quadro.largura, quadro.altura);
        self.yuv.converter(quadro);
        let (l, a) = (self.yuv.largura as usize, self.yuv.altura as usize);
        let fonte = YUVSlices::new(
            (&self.yuv.y, &self.yuv.u, &self.yuv.v),
            (l, a),
            (l, l / 2, l / 2),
        );
        if forcar_chave {
            encoder.force_intra_frame();
        }
        let bits = encoder.encode(&fonte).map_err(backend)?;
        let tipo = match bits.frame_type() {
            FrameType::IDR => TipoQuadro::Chave,
            FrameType::I | FrameType::P => TipoQuadro::Delta,
            FrameType::Skip | FrameType::Invalid | FrameType::IPMixed => return Ok(None),
        };
        bits.write_vec(saida);
        Ok((!saida.is_empty()).then_some(tipo))
    }

    fn ajustar_bitrate(&mut self, bps: u32) {
        if bps != self.config.bitrate_bps {
            self.config.bitrate_bps = bps;
            self.encoder = None;
        }
    }
}

/// Decoder H.264 em software.
pub struct DecoderOpenH264 {
    decoder: Decoder,
}

impl std::fmt::Debug for DecoderOpenH264 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecoderOpenH264").finish_non_exhaustive()
    }
}

impl DecoderOpenH264 {
    pub fn novo() -> Result<Self, ErroCodec> {
        let decoder = Decoder::with_api_config(OpenH264API::from_source(), DecoderConfig::new())
            .map_err(backend)?;
        Ok(DecoderOpenH264 { decoder })
    }
}

impl VideoDecoder for DecoderOpenH264 {
    fn nome(&self) -> &'static str {
        "openh264"
    }

    fn decodificar(
        &mut self,
        access_unit: &[u8],
        ao_quadro: &mut dyn FnMut(&QuadroYuv<'_>),
    ) -> Result<bool, ErroCodec> {
        let Some(img) = self.decoder.decode(access_unit).map_err(backend)? else {
            return Ok(false);
        };
        let (l, a) = img.dimensions();
        let (py, pu, pv) = img.strides();
        ao_quadro(&QuadroYuv {
            largura: l as u32,
            altura: a as u32,
            formato: FormatoYuv::I420,
            planos: [
                PlanoYuv {
                    dados: img.y(),
                    passo: py as u32,
                },
                PlanoYuv {
                    dados: img.u(),
                    passo: pu as u32,
                },
                PlanoYuv {
                    dados: img.v(),
                    passo: pv as u32,
                },
            ],
            // O encoder marca BT.709 limitado no VUI; outros encoders H.264 do TailDesk também.
            matriz: Matriz::Bt709,
            faixa: Faixa::Limitada,
        });
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OrdemRgb;

    /// Gradiente horizontal BGRx que anda `passo` pixels por quadro.
    fn quadro(largura: u32, altura: u32, passo: u32, buf: &mut Vec<u8>) {
        let stride = largura as usize * 4 + 64;
        buf.clear();
        buf.resize(stride * altura as usize, 0);
        for y in 0..altura as usize {
            for x in 0..largura as usize {
                let v = ((x as u32 + passo) % largura * 255 / largura) as u8;
                buf[y * stride + x * 4..y * stride + x * 4 + 3].copy_from_slice(&[v, v, v]);
            }
        }
    }

    #[test]
    fn ida_e_volta_mantem_tamanho_e_brilho() {
        let (l, a) = (320, 240);
        let mut enc = EncoderOpenH264::novo(ConfigEncoder {
            bitrate_bps: 2_000_000,
            fps_max: 30.0,
        })
        .unwrap();
        let mut dec = DecoderOpenH264::novo().unwrap();
        let mut img = Vec::new();
        let mut au = Vec::new();
        let mut tipos = Vec::new();
        let mut decodificados = 0;
        for i in 0..10 {
            quadro(l, a, i * 4, &mut img);
            let q = QuadroRgb {
                largura: l,
                altura: a,
                stride: l as usize * 4 + 64,
                ordem: OrdemRgb::Bgrx,
                dados: &img,
            };
            let Some(tipo) = enc.codificar(&q, i == 5, &mut au).unwrap() else {
                continue;
            };
            tipos.push(tipo);
            assert_eq!(&au[..4], &[0, 0, 0, 1], "Annex-B");
            dec.decodificar(&au, &mut |y| {
                decodificados += 1;
                assert_eq!((y.largura, y.altura, y.formato), (l, a, FormatoYuv::I420));
                // Meio da primeira linha: o cinza do gradiente em faixa limitada.
                let cinza = (l / 2 + i * 4) % l * 255 / l;
                let esperado = 16 + cinza * 219 / 255;
                let meio = u32::from(y.planos[0].dados[l as usize / 2]);
                assert!(
                    meio.abs_diff(esperado) <= 8,
                    "luma {meio}, esperado {esperado}"
                );
            })
            .unwrap();
        }
        assert_eq!(tipos.first(), Some(&TipoQuadro::Chave));
        assert!(tipos.iter().filter(|&&t| t == TipoQuadro::Chave).count() >= 2);
        assert!(decodificados >= 8);
    }

    #[test]
    fn bitrate_novo_recria_com_chave() {
        let mut enc = EncoderOpenH264::novo(ConfigEncoder {
            bitrate_bps: 1_000_000,
            fps_max: 30.0,
        })
        .unwrap();
        let mut img = Vec::new();
        let mut au = Vec::new();
        quadro(64, 64, 0, &mut img);
        let q = QuadroRgb {
            largura: 64,
            altura: 64,
            stride: 64 * 4 + 64,
            ordem: OrdemRgb::Bgrx,
            dados: &img,
        };
        assert_eq!(
            enc.codificar(&q, false, &mut au).unwrap(),
            Some(TipoQuadro::Chave)
        );
        enc.ajustar_bitrate(500_000);
        assert_eq!(
            enc.codificar(&q, false, &mut au).unwrap(),
            Some(TipoQuadro::Chave)
        );
    }

    #[test]
    fn recusa_quadro_curto() {
        let mut enc = EncoderOpenH264::novo(ConfigEncoder {
            bitrate_bps: 1_000_000,
            fps_max: 30.0,
        })
        .unwrap();
        let q = QuadroRgb {
            largura: 64,
            altura: 64,
            stride: 256,
            ordem: OrdemRgb::Bgrx,
            dados: &[0; 100],
        };
        let mut au = Vec::new();
        assert!(matches!(
            enc.codificar(&q, false, &mut au),
            Err(ErroCodec::QuadroInvalido)
        ));
    }
}
