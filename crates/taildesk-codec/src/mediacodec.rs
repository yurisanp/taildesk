//! Decoder H.264 do Android pelo `AMediaCodec` (spike S6, `mobile/android-detalhado.md` §4.1).
//!
//! Primeiro passo (este arquivo): saída em `ByteBuffer`, com os planos YUV lidos pelo layout que
//! o próprio codec informa (`stride`, `slice-height`, `color-format`, `crop-*`) e entregues como
//! [`QuadroYuv`], igual ao openh264 no desktop. O render sobe os planos para a GPU (uma cópia).
//! O caminho sem cópia (`AImageReader` → `AHardwareBuffer` → `VkImage`) é o gate do S6 e fica
//! registrado em `mobile/m0-resultados.md`.
//!
//! Baixa latência: `low-latency = 1` (Android 11+), `priority = 0` (tempo real) e nenhuma fila
//! própria: cada access unit entra e o decode espera até [`ESPERA_SAIDA`] pela imagem dele.
//!
//! O cálculo do layout ([`layout`]) é lógica pura e tem teste no Linux; a parte que chama o NDK
//! só compila no Android.

use crate::{FormatoYuv, PlanoYuv};

/// `MediaCodecInfo.CodecCapabilities.COLOR_FormatYUV420Planar` (I420).
pub const COR_I420: i32 = 19;
/// `COLOR_FormatYUV420SemiPlanar` (NV12).
pub const COR_NV12: i32 = 21;
/// `COLOR_FormatYUV420Flexible`: com `ByteBuffer`, os decoders de hardware entregam NV12.
pub const COR_FLEXIVEL: i32 = 0x7F42_0888;

/// Onde ficam os planos dentro do buffer de saída.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub largura: u32,
    pub altura: u32,
    pub formato: FormatoYuv,
    /// (início, passo) de cada plano; em NV12 o terceiro fica (0, 0).
    pub planos: [(usize, u32); 3],
    /// Bytes que o buffer precisa ter.
    pub tamanho: usize,
}

/// Formato de saída do codec, como o `AMediaFormat` informa.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FormatoSaida {
    pub largura: i32,
    pub altura: i32,
    pub stride: i32,
    pub slice_height: i32,
    pub cor: i32,
    /// `crop-left/top/right/bottom` (inclusivos); `None` = sem recorte.
    pub recorte: Option<(i32, i32, i32, i32)>,
}

/// Calcula os planos. `None` = layout que não sabemos ler (formato de cor proprietário,
/// tamanhos absurdos): o app cai para "sem decode por hardware".
pub fn layout(f: &FormatoSaida) -> Option<Layout> {
    let (largura, altura) = match f.recorte {
        Some((l, t, r, b)) if r >= l && b >= t => (r - l + 1, b - t + 1),
        _ => (f.largura, f.altura),
    };
    if !(2..=8192).contains(&largura) || !(2..=8192).contains(&altura) {
        return None;
    }
    let stride = if f.stride >= f.largura && f.stride > 0 {
        f.stride
    } else {
        f.largura
    };
    let linhas = if f.slice_height >= f.altura && f.slice_height > 0 {
        f.slice_height
    } else {
        f.altura
    };
    if stride > 16_384 || linhas > 16_384 {
        return None;
    }
    let (stride, linhas) = (stride as usize, linhas as usize);
    // O recorte começa em (left, top) dentro dos planos.
    let (cx, cy) = f.recorte.map_or((0, 0), |(l, t, _, _)| {
        (l.max(0) as usize, t.max(0) as usize)
    });
    let y0 = cy * stride + cx;
    let tamanho_y = stride * linhas;
    match f.cor {
        COR_NV12 | COR_FLEXIVEL => {
            let uv0 = tamanho_y + (cy / 2) * stride + (cx & !1);
            Some(Layout {
                largura: largura as u32,
                altura: altura as u32,
                formato: FormatoYuv::Nv12,
                planos: [(y0, stride as u32), (uv0, stride as u32), (0, 0)],
                tamanho: tamanho_y + stride * (linhas / 2)
                    - (stride - largura as usize).min(stride),
            })
        }
        COR_I420 => {
            let passo_c = stride / 2;
            let tamanho_c = passo_c * (linhas / 2);
            let u0 = tamanho_y + (cy / 2) * passo_c + cx / 2;
            let v0 = tamanho_y + tamanho_c + (cy / 2) * passo_c + cx / 2;
            Some(Layout {
                largura: largura as u32,
                altura: altura as u32,
                formato: FormatoYuv::I420,
                planos: [
                    (y0, stride as u32),
                    (u0, passo_c as u32),
                    (v0, passo_c as u32),
                ],
                tamanho: tamanho_y + 2 * tamanho_c - (passo_c - largura as usize / 2).min(passo_c),
            })
        }
        _ => None,
    }
}

/// Planos emprestados do buffer, conforme o layout. `None` se o buffer for menor que o layout.
pub fn planos<'a>(buf: &'a [u8], l: &Layout) -> Option<[PlanoYuv<'a>; 3]> {
    if buf.len() < l.tamanho {
        return None;
    }
    let plano = |(inicio, passo): (usize, u32)| PlanoYuv {
        dados: buf.get(inicio..).unwrap_or_default(),
        passo,
    };
    Some([plano(l.planos[0]), plano(l.planos[1]), plano(l.planos[2])])
}

#[cfg(target_os = "android")]
pub use android::DecoderMediaCodec;

#[cfg(target_os = "android")]
mod android {
    use std::time::Duration;

    use ndk::media::media_codec::{
        DequeuedInputBufferResult, DequeuedOutputBufferInfoResult, MediaCodec, MediaCodecDirection,
    };
    use ndk::media::media_format::MediaFormat;

    use super::{FormatoSaida, Layout, layout, planos};
    use crate::{ErroCodec, Faixa, Matriz, QuadroYuv, VideoDecoder};

    /// Quanto o decode espera a imagem do access unit que acabou de entrar.
    const ESPERA_SAIDA: Duration = Duration::from_millis(12);
    /// Espera por um buffer de entrada livre.
    const ESPERA_ENTRADA: Duration = Duration::from_millis(5);

    fn erro(e: impl std::fmt::Debug) -> ErroCodec {
        ErroCodec::Backend(format!("mediacodec: {e:?}"))
    }

    /// Decoder H.264 do sistema.
    pub struct DecoderMediaCodec {
        codec: MediaCodec,
        nome: String,
        layout: Option<Layout>,
        tempo_us: u64,
    }

    impl std::fmt::Debug for DecoderMediaCodec {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("DecoderMediaCodec")
                .field("nome", &self.nome)
                .field("layout", &self.layout)
                .finish_non_exhaustive()
        }
    }

    // SAFETY: o `AMediaCodec` pode ser usado de qualquer thread desde que não ao mesmo tempo; o
    // decoder vive numa thread só (a de decode) e só se move entre threads antes de começar.
    unsafe impl Send for DecoderMediaCodec {}

    impl DecoderMediaCodec {
        /// Abre o decoder `video/avc` para até `largura`×`altura` (o SPS manda no tamanho real).
        pub fn novo(largura: u32, altura: u32) -> Result<Self, ErroCodec> {
            let codec = MediaCodec::from_decoder_type("video/avc")
                .ok_or_else(|| ErroCodec::Backend("mediacodec: sem decoder H.264".into()))?;
            let mut f = MediaFormat::new();
            f.set_str("mime", "video/avc");
            f.set_i32("width", largura.clamp(64, 4096) as i32);
            f.set_i32("height", altura.clamp(64, 4096) as i32);
            f.set_i32("color-format", super::COR_FLEXIVEL);
            f.set_i32("low-latency", 1);
            f.set_i32("priority", 0);
            codec
                .configure(&f, None, MediaCodecDirection::Decoder)
                .map_err(erro)?;
            codec.start().map_err(erro)?;
            let nome = codec.name().unwrap_or_else(|_| "mediacodec".into());
            Ok(DecoderMediaCodec {
                codec,
                nome,
                layout: None,
                tempo_us: 0,
            })
        }

        /// Nome do componente do fabricante (`c2.qti.avc.decoder`), para o HUD e o teste.
        pub fn componente(&self) -> &str {
            &self.nome
        }
    }

    /// Layout dos planos a partir do formato de saída do codec.
    fn ler_formato(f: &MediaFormat) -> Option<Layout> {
        let g = |k: &str| f.i32(k).unwrap_or(0);
        let recorte = match (f.i32("crop-left"), f.i32("crop-right")) {
            (Some(l), Some(r)) => Some((l, g("crop-top"), r, g("crop-bottom"))),
            _ => None,
        };
        layout(&FormatoSaida {
            largura: g("width"),
            altura: g("height"),
            stride: g("stride"),
            slice_height: g("slice-height"),
            cor: g("color-format"),
            recorte,
        })
    }

    impl Drop for DecoderMediaCodec {
        fn drop(&mut self) {
            let _ = self.codec.stop();
        }
    }

    impl VideoDecoder for DecoderMediaCodec {
        fn nome(&self) -> &'static str {
            "mediacodec"
        }

        fn gpu(&self) -> Option<&str> {
            Some(&self.nome)
        }

        fn decodificar(
            &mut self,
            access_unit: &[u8],
            ao_quadro: &mut dyn FnMut(&QuadroYuv<'_>),
        ) -> Result<bool, ErroCodec> {
            match self
                .codec
                .dequeue_input_buffer(ESPERA_ENTRADA)
                .map_err(erro)?
            {
                DequeuedInputBufferResult::Buffer(mut b) => {
                    let destino = b.buffer_mut();
                    if destino.len() < access_unit.len() {
                        return Err(ErroCodec::QuadroInvalido);
                    }
                    for (d, &s) in destino.iter_mut().zip(access_unit) {
                        d.write(s);
                    }
                    self.tempo_us += 16_667;
                    self.codec
                        .queue_input_buffer(b, 0, access_unit.len(), self.tempo_us, 0)
                        .map_err(erro)?;
                }
                // Decoder entupido: descarta este AU (o recepção pede quadro-chave se faltar).
                DequeuedInputBufferResult::TryAgainLater => return Ok(false),
            }
            let mut saiu = false;
            let mut espera = ESPERA_SAIDA;
            loop {
                match self.codec.dequeue_output_buffer(espera).map_err(erro)? {
                    DequeuedOutputBufferInfoResult::Buffer(out) => {
                        if self.layout.is_none() {
                            let f = self.codec.output_format();
                            self.layout = ler_formato(&f);
                        }
                        let info = *out.info();
                        let inicio = info.offset().max(0) as usize;
                        let fim = inicio + info.size().max(0) as usize;
                        if let (Some(l), Some(dados)) = (self.layout, out.buffer().get(inicio..fim))
                            && let Some(p) = planos(dados, &l)
                        {
                            ao_quadro(&QuadroYuv {
                                largura: l.largura,
                                altura: l.altura,
                                formato: l.formato,
                                planos: p,
                                matriz: Matriz::Bt709,
                                faixa: Faixa::Limitada,
                            });
                            saiu = true;
                        }
                        self.codec.release_output_buffer(out, false).map_err(erro)?;
                        // Já saiu a imagem deste AU: só pega o que mais estiver pronto.
                        espera = Duration::ZERO;
                    }
                    DequeuedOutputBufferInfoResult::OutputFormatChanged => {
                        let f = self.codec.output_format();
                        self.layout = ler_formato(&f);
                        if self.layout.is_none() {
                            return Err(ErroCodec::Backend(
                                "mediacodec: formato de cor que o TailDesk não lê".into(),
                            ));
                        }
                    }
                    DequeuedOutputBufferInfoResult::OutputBuffersChanged => {}
                    DequeuedOutputBufferInfoResult::TryAgainLater => return Ok(saiu),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nv12_com_stride_e_slice_height() {
        // Qualcomm típico: 1920×1080 com stride 2048 e slice-height 1088, recorte 1080.
        let l = layout(&FormatoSaida {
            largura: 1920,
            altura: 1088,
            stride: 2048,
            slice_height: 1088,
            cor: COR_FLEXIVEL,
            recorte: Some((0, 0, 1919, 1079)),
        })
        .unwrap();
        assert_eq!((l.largura, l.altura), (1920, 1080));
        assert_eq!(l.formato, FormatoYuv::Nv12);
        assert_eq!(l.planos[0], (0, 2048));
        assert_eq!(l.planos[1], (2048 * 1088, 2048));
        // O buffer pode acabar logo depois do último pixel útil.
        let buf = vec![0u8; l.tamanho];
        assert!(planos(&buf, &l).is_some());
        assert!(planos(&buf[..l.tamanho - 1], &l).is_none());
    }

    #[test]
    fn i420_sem_stride_informado() {
        let l = layout(&FormatoSaida {
            largura: 1280,
            altura: 720,
            stride: 0,
            slice_height: 0,
            cor: COR_I420,
            recorte: None,
        })
        .unwrap();
        assert_eq!(l.formato, FormatoYuv::I420);
        assert_eq!(l.planos[1], (1280 * 720, 640));
        assert_eq!(l.planos[2], (1280 * 720 + 640 * 360, 640));
        assert_eq!(l.tamanho, 1280 * 720 * 3 / 2);
    }

    #[test]
    fn recorte_no_meio_e_formatos_estranhos() {
        let l = layout(&FormatoSaida {
            largura: 1920,
            altura: 1088,
            stride: 1920,
            slice_height: 1088,
            cor: COR_NV12,
            recorte: Some((2, 4, 1917, 1083)),
        })
        .unwrap();
        assert_eq!((l.largura, l.altura), (1916, 1080));
        assert_eq!(l.planos[0].0, 4 * 1920 + 2);
        assert_eq!(l.planos[1].0, 1920 * 1088 + 2 * 1920 + 2);
        // Formato proprietário (QCOM tiled) e tamanhos absurdos: não lê.
        let mut f = FormatoSaida {
            largura: 1920,
            altura: 1080,
            stride: 1920,
            slice_height: 1080,
            cor: 0x7FA3_0C04,
            recorte: None,
        };
        assert!(layout(&f).is_none());
        f.cor = COR_NV12;
        f.largura = 0;
        assert!(layout(&f).is_none());
        f.largura = 100_000;
        assert!(layout(&f).is_none());
    }
}
