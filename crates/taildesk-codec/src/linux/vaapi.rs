//! Encoder H.264 no VA-API (Intel pelo iHD, AMD e outras GPUs pelo Mesa).
//!
//! Só P depois do IDR, uma referência, POC tipo 2 (sem reordenação) e CBR com HRD de
//! [`QUADROS_HRD`] quadros e QP no máximo [`QP_MAXIMO`].
//! SPS e PPS vão como packed headers ([`crate::h264`], com o VUI em BT.709); o driver escreve os
//! cabeçalhos de slice. A imagem é
//! convertida para NV12 numa `VAImage` linear e copiada para a superfície com `vaPutImage`: o
//! `vaDeriveImage` do iHD expõe a superfície com tiling, e escrever nela como linear embaralha a
//! imagem.

use std::ffi::c_void;
use std::path::Path;

use super::sys::va::*;
use super::va::Display;
use crate::h264::{self, Parametros, Perfil};
use crate::{ConfigEncoder, ErroCodec, QuadroRgb, TipoQuadro, VideoEncoder, cor};

/// Superfícies de reconstrução: a referência atual e a do quadro em codificação.
const RECONSTRUCAO: usize = 2;
/// HRD em quadros. Com um quadro só, o controle de taxa do iHD vai para QP 51 e completa a taxa
/// com enchimento: texto rolando sai a 22 dB de PSNR em 1080p a 8 Mbit/s (36 dB com 8 quadros e
/// o teto de QP abaixo).
const QUADROS_HRD: u32 = 8;
/// Teto de QP: acima disso o texto fica ilegível; o quadro sai maior e o HRD absorve.
const QP_MAXIMO: u32 = 42;
/// `log2_max_frame_num_minus4`: `frame_num` anda módulo 256.
const LOG2_MAX_FRAME_NUM_MENOS4: u32 = 4;

/// Estruturas de parâmetros do libva em que todos os bits zero são um valor válido (inteiros,
/// arrays e uniões de inteiros, sem ponteiros nem enums do Rust).
trait Pod: Copy {}
impl Pod for VAEncSequenceParameterBufferH264 {}
impl Pod for VAEncPictureParameterBufferH264 {}
impl Pod for VAEncMiscParameterRateControl {}
impl Pod for VAEncMiscParameterFrameRate {}
impl Pod for VAEncMiscParameterHRD {}
impl Pod for VAImage {}

fn zerada<T: Pod>() -> T {
    // SAFETY: `Pod` só é implementado para estruturas em que zero é um valor válido.
    unsafe { std::mem::zeroed() }
}

fn erro(e: String) -> ErroCodec {
    ErroCodec::Backend(format!("vaapi: {e}"))
}

/// Perfil e entrypoint de encode H.264 que o display oferece (LP primeiro, que é o VDEnc da
/// Intel e o único no Gen12+).
fn escolher(d: &Display) -> Option<(VAProfile, VAEntrypoint)> {
    for perfil in [
        VAProfileH264High,
        VAProfileH264Main,
        VAProfileH264ConstrainedBaseline,
    ] {
        let eps = d.entrypoints(perfil);
        for ep in [VAEntrypointEncSliceLP, VAEntrypointEncSlice] {
            let cabecalhos = VA_ENC_PACKED_HEADER_SEQUENCE | VA_ENC_PACKED_HEADER_PICTURE;
            let packed = d.atributo(perfil, ep, VAConfigAttribEncPackedHeaders);
            if eps.contains(&ep)
                && packed != VA_ATTRIB_NOT_SUPPORTED
                && packed & cabecalhos == cabecalhos
                && d.atributo(perfil, ep, VAConfigAttribRateControl) & VA_RC_CBR != 0
                && d.atributo(perfil, ep, VAConfigAttribRTFormat) & VA_RT_FORMAT_YUV420 != 0
            {
                return Some((perfil, ep));
            }
        }
    }
    None
}

/// `true` se o nó tem encode H.264 por VA-API.
pub fn codifica_h264(no: &Path) -> Option<String> {
    let d = Display::abrir(no).ok()?;
    escolher(&d).map(|_| d.fornecedor.clone())
}

/// Recursos de um tamanho de imagem.
struct Sessao {
    config: VAConfigID,
    contexto: VAContextID,
    /// [0] = entrada; o resto, reconstruções.
    superficies: [VASurfaceID; 1 + RECONSTRUCAO],
    codificado: VABufferID,
    /// NV12 linear onde a CPU escreve antes do `vaPutImage`.
    imagem: VAImage,
    largura: u32,
    altura: u32,
}

/// Encoder H.264 VA-API de uma GPU.
pub struct EncoderVaapi {
    d: Display,
    gpu: String,
    perfil: VAProfile,
    ep: VAEntrypoint,
    config: ConfigEncoder,
    sessao: Option<Sessao>,
    /// Quadros desde o último IDR (dá o `frame_num` e o POC).
    desde_idr: u32,
    idr_id: u16,
    /// Reconstrução que serve de referência (índice em `superficies`).
    referencia: usize,
    taxa_mudou: bool,
    buffers: Vec<VABufferID>,
}

impl std::fmt::Debug for EncoderVaapi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EncoderVaapi")
            .field("gpu", &self.gpu)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl EncoderVaapi {
    /// Abre o encoder no nó de render `no` e confere que ele codifica um quadro.
    pub fn novo(no: &Path, gpu: String, config: ConfigEncoder) -> Result<Self, ErroCodec> {
        let d = Display::abrir(no).map_err(erro)?;
        let (perfil, ep) = escolher(&d).ok_or_else(|| erro("sem encode H.264".into()))?;
        let mut e = EncoderVaapi {
            d,
            gpu,
            perfil,
            ep,
            config,
            sessao: None,
            desde_idr: 0,
            idr_id: 0,
            referencia: 1,
            taxa_mudou: false,
            buffers: Vec::with_capacity(8),
        };
        let img = vec![0x80u8; 256 * 256 * 4];
        let q = QuadroRgb {
            largura: 256,
            altura: 256,
            stride: 256 * 4,
            ordem: crate::OrdemRgb::Bgrx,
            dados: &img,
        };
        let mut au = Vec::new();
        e.codificar(&q, true, &mut au)?;
        if au.is_empty() {
            return Err(erro("o quadro de teste saiu vazio".into()));
        }
        e.fechar();
        Ok(e)
    }

    fn fechar(&mut self) {
        let Some(s) = self.sessao.take() else { return };
        let va = &self.d.libs.va;
        // SAFETY: recursos criados em `abrir` neste display, destruídos uma vez, na ordem
        // inversa da criação.
        unsafe {
            if s.imagem.image_id != VA_INVALID_ID {
                va.vaDestroyImage(self.d.dpy, s.imagem.image_id);
            }
            va.vaDestroyBuffer(self.d.dpy, s.codificado);
            va.vaDestroyContext(self.d.dpy, s.contexto);
            let mut sup = s.superficies;
            va.vaDestroySurfaces(self.d.dpy, sup.as_mut_ptr(), sup.len() as i32);
            va.vaDestroyConfig(self.d.dpy, s.config);
        }
    }

    fn abrir(&mut self, largura: u32, altura: u32) -> Result<(), ErroCodec> {
        self.fechar();
        let (d, va) = (&self.d, &self.d.libs.va);
        let mut s = Sessao {
            config: VA_INVALID_ID,
            contexto: VA_INVALID_ID,
            superficies: [VA_INVALID_SURFACE; 1 + RECONSTRUCAO],
            codificado: VA_INVALID_ID,
            imagem: VAImage {
                image_id: VA_INVALID_ID,
                ..zerada()
            },
            largura,
            altura,
        };
        let r = (|| -> Result<(), String> {
            let mut atributos = [
                VAConfigAttrib {
                    type_: VAConfigAttribRTFormat,
                    value: VA_RT_FORMAT_YUV420,
                },
                VAConfigAttrib {
                    type_: VAConfigAttribRateControl,
                    value: VA_RC_CBR,
                },
                VAConfigAttrib {
                    type_: VAConfigAttribEncPackedHeaders,
                    value: VA_ENC_PACKED_HEADER_SEQUENCE | VA_ENC_PACKED_HEADER_PICTURE,
                },
            ];
            // SAFETY: display inicializado; os ponteiros apontam para arrays com o tamanho
            // passado junto e para as saídas da sessão.
            unsafe {
                d.checar(
                    va.vaCreateConfig(
                        d.dpy,
                        self.perfil,
                        self.ep,
                        atributos.as_mut_ptr(),
                        atributos.len() as i32,
                        &mut s.config,
                    ),
                    "vaCreateConfig",
                )?;
                d.checar(
                    va.vaCreateSurfaces(
                        d.dpy,
                        VA_RT_FORMAT_YUV420,
                        largura,
                        altura,
                        s.superficies.as_mut_ptr(),
                        s.superficies.len() as u32,
                        std::ptr::null_mut(),
                        0,
                    ),
                    "vaCreateSurfaces",
                )?;
                d.checar(
                    va.vaCreateContext(
                        d.dpy,
                        s.config,
                        largura as i32,
                        altura as i32,
                        VA_PROGRESSIVE as i32,
                        s.superficies.as_mut_ptr(),
                        s.superficies.len() as i32,
                        &mut s.contexto,
                    ),
                    "vaCreateContext",
                )?;
                let tamanho = largura * altura * 3 / 2 + 64 * 1024;
                d.checar(
                    va.vaCreateBuffer(
                        d.dpy,
                        s.contexto,
                        VAEncCodedBufferType,
                        tamanho,
                        1,
                        std::ptr::null_mut(),
                        &mut s.codificado,
                    ),
                    "vaCreateBuffer(coded)",
                )?;
                let mut formato = VAImageFormat {
                    fourcc: VA_FOURCC_NV12,
                    byte_order: VA_LSB_FIRST,
                    bits_per_pixel: 12,
                    ..Default::default()
                };
                d.checar(
                    va.vaCreateImage(
                        d.dpy,
                        &mut formato,
                        largura as i32,
                        altura as i32,
                        &mut s.imagem,
                    ),
                    "vaCreateImage",
                )?;
            }
            Ok(())
        })();
        self.sessao = Some(s);
        if let Err(e) = r {
            self.fechar();
            return Err(erro(e));
        }
        Ok(())
    }

    /// Cria um buffer de parâmetros com o conteúdo de `valor` e o guarda para o `vaRenderPicture`.
    fn parametro<T>(&mut self, tipo: VABufferType, valor: &T) -> Result<(), String> {
        let s = self.sessao.as_ref().expect("sessão aberta");
        let mut id = VA_INVALID_ID;
        // SAFETY: o libva copia `size_of::<T>()` bytes de `valor` para o buffer novo.
        let st = unsafe {
            self.d.libs.va.vaCreateBuffer(
                self.d.dpy,
                s.contexto,
                tipo,
                size_of::<T>() as u32,
                1,
                (valor as *const T).cast_mut().cast(),
                &mut id,
            )
        };
        self.d.checar(st, "vaCreateBuffer")?;
        self.buffers.push(id);
        Ok(())
    }

    /// Buffer de parâmetro "misc": o cabeçalho com o tipo seguido de `valor`.
    fn misc<T: Pod>(&mut self, tipo: VAEncMiscParameterType, valor: &T) -> Result<(), String> {
        #[repr(C)]
        struct Misc<T> {
            tipo: VAEncMiscParameterType,
            valor: T,
        }
        let m = Misc {
            tipo,
            valor: *valor,
        };
        self.parametro(VAEncMiscParameterBufferType, &m)
    }

    /// Um packed header (NAL em Annex-B, já com os bytes de prevenção de emulação).
    fn cabecalho(&mut self, tipo: VAEncPackedHeaderType, nal: &[u8]) -> Result<(), String> {
        let p = VAEncPackedHeaderParameterBuffer {
            type_: tipo,
            bit_length: nal.len() as u32 * 8,
            has_emulation_bytes: 1,
            ..Default::default()
        };
        self.parametro(VAEncPackedHeaderParameterBufferType, &p)?;
        let s = self.sessao.as_ref().expect("sessão aberta");
        let mut id = VA_INVALID_ID;
        // SAFETY: o libva copia `nal.len()` bytes de `nal` para o buffer novo.
        let st = unsafe {
            self.d.libs.va.vaCreateBuffer(
                self.d.dpy,
                s.contexto,
                VAEncPackedHeaderDataBufferType,
                nal.len() as u32,
                1,
                nal.as_ptr().cast_mut().cast(),
                &mut id,
            )
        };
        self.d.checar(st, "vaCreateBuffer(packed)")?;
        self.buffers.push(id);
        Ok(())
    }

    fn soltar_buffers(&mut self) {
        for id in self.buffers.drain(..) {
            // SAFETY: buffers criados por `parametro` neste display e ainda não destruídos.
            unsafe { self.d.libs.va.vaDestroyBuffer(self.d.dpy, id) };
        }
    }

    /// Converte o quadro para NV12 na imagem linear e copia para a superfície de entrada.
    fn carregar(&self, q: &QuadroRgb<'_>) -> Result<(), String> {
        let s = self.sessao.as_ref().expect("sessão aberta");
        let (d, va, img) = (&self.d, &self.d.libs.va, &s.imagem);
        // SAFETY: a imagem desta sessão fica mapeada só entre o map e o unmap, com `data_size`
        // bytes; os planos Y e UV ficam em `offsets` com `pitches` bytes por linha, e
        // `para_nv12` confere que cabem. O `vaPutImage` copia para a superfície da sessão.
        unsafe {
            let mut p: *mut c_void = std::ptr::null_mut();
            d.checar(va.vaMapBuffer(d.dpy, img.buf, &mut p), "vaMapBuffer")?;
            let tudo = std::slice::from_raw_parts_mut(p.cast::<u8>(), img.data_size as usize);
            let (o0, o1) = (img.offsets[0] as usize, img.offsets[1] as usize);
            let ok = o0 < o1 && o1 <= tudo.len() && {
                let (antes, uv) = tudo.split_at_mut(o1);
                cor::para_nv12(
                    q,
                    &mut antes[o0..],
                    img.pitches[0] as usize,
                    uv,
                    img.pitches[1] as usize,
                )
            };
            d.checar(va.vaUnmapBuffer(d.dpy, img.buf), "vaUnmapBuffer")?;
            if !ok {
                return Err("imagem menor que o quadro".to_string());
            }
            let (l, a) = (s.largura, s.altura);
            d.checar(
                va.vaPutImage(
                    d.dpy,
                    s.superficies[0],
                    img.image_id,
                    0,
                    0,
                    l,
                    a,
                    0,
                    0,
                    l,
                    a,
                ),
                "vaPutImage",
            )
        }
    }

    fn quadro(
        &mut self,
        q: &QuadroRgb<'_>,
        chave: bool,
        saida: &mut Vec<u8>,
    ) -> Result<TipoQuadro, String> {
        self.carregar(q)?;
        let s = self.sessao.as_ref().expect("sessão aberta");
        let (largura, altura) = (s.largura, s.altura);
        let (entrada, codificado, contexto) = (s.superficies[0], s.codificado, s.contexto);
        let reconstrucao = 1 + (self.referencia % RECONSTRUCAO);
        let rec_id = s.superficies[reconstrucao];
        let ref_id = s.superficies[self.referencia];
        if chave {
            self.desde_idr = 0;
            self.idr_id = self.idr_id.wrapping_add(1);
        }
        let frame_num = self.desde_idr % (1 << (LOG2_MAX_FRAME_NUM_MENOS4 + 4));
        let poc = (2 * self.desde_idr) as i32;
        let (mbs_l, mbs_a) = (largura.div_ceil(16), altura.div_ceil(16));
        let fps = self.config.fps_max.max(1.0).round() as u32;
        let bps = self.config.bitrate_bps.max(100_000);
        let cabac = self.perfil != VAProfileH264ConstrainedBaseline;

        // Estruturas do va_enc_h264.h, zeradas e preenchidas como o header descreve.
        if chave {
            let mut seq: VAEncSequenceParameterBufferH264 = zerada();
            seq.level_idc = 51;
            seq.intra_period = 0;
            seq.intra_idr_period = 0;
            seq.ip_period = 1;
            seq.bits_per_second = bps;
            seq.max_num_ref_frames = 1;
            seq.picture_width_in_mbs = mbs_l as u16;
            seq.picture_height_in_mbs = mbs_a as u16;
            // chroma_format_idc=1, frame_mbs_only_flag, direct_8x8_inference_flag,
            // log2_max_frame_num_minus4 e pic_order_cnt_type=2 (bits do va_enc_h264.h).
            seq.seq_fields.value =
                1 | (1 << 2) | (1 << 5) | (LOG2_MAX_FRAME_NUM_MENOS4 << 6) | (2 << 10);
            if largura != mbs_l * 16 || altura != mbs_a * 16 {
                seq.frame_cropping_flag = 1;
                seq.frame_crop_right_offset = (mbs_l * 16 - largura) / 2;
                seq.frame_crop_bottom_offset = (mbs_a * 16 - altura) / 2;
            }
            seq.vui_parameters_present_flag = 1;
            // timing_info_present_flag, bitstream_restriction_flag, log2_max_mv_length 15/15,
            // fixed_frame_rate_flag e motion_vectors_over_pic_boundaries_flag.
            seq.vui_fields.value =
                (1 << 1) | (1 << 2) | (15 << 3) | (15 << 8) | (1 << 13) | (1 << 15);
            seq.num_units_in_tick = 1;
            seq.time_scale = 2 * fps;
            self.parametro(VAEncSequenceParameterBufferType, &seq)?;
            let perfil = if self.perfil == VAProfileH264High {
                Perfil::High
            } else if self.perfil == VAProfileH264Main {
                Perfil::Main
            } else {
                Perfil::ConstrainedBaseline
            };
            let p = Parametros {
                perfil,
                level_idc: seq.level_idc,
                largura,
                altura,
                log2_max_frame_num_menos4: LOG2_MAX_FRAME_NUM_MENOS4,
                fps,
            };
            self.cabecalho(VAEncPackedHeaderSequence, &h264::sps(&p))?;
            self.cabecalho(VAEncPackedHeaderPicture, &h264::pps(&p))?;
        }
        // A taxa vai em todo IDR e, sem SPS novo, quando a seleção adaptativa muda o alvo.
        if chave || self.taxa_mudou {
            let mut rc: VAEncMiscParameterRateControl = zerada();
            rc.bits_per_second = bps;
            rc.target_percentage = 100;
            rc.window_size = 1000;
            rc.initial_qp = 26;
            rc.min_qp = 10;
            rc.max_qp = QP_MAXIMO;
            self.misc(VAEncMiscParameterTypeRateControl, &rc)?;
            let mut fr: VAEncMiscParameterFrameRate = zerada();
            fr.framerate = fps | (1 << 16);
            self.misc(VAEncMiscParameterTypeFrameRate, &fr)?;
            let mut hrd: VAEncMiscParameterHRD = zerada();
            hrd.buffer_size = bps / fps * QUADROS_HRD;
            hrd.initial_buffer_fullness = hrd.buffer_size / 2;
            self.misc(VAEncMiscParameterTypeHRD, &hrd)?;
            self.taxa_mudou = false;
        }

        let invalida = VAPictureH264 {
            picture_id: VA_INVALID_SURFACE,
            frame_idx: 0,
            flags: VA_PICTURE_H264_INVALID,
            TopFieldOrderCnt: 0,
            BottomFieldOrderCnt: 0,
            va_reserved: [0; 4],
        };
        let anterior = VAPictureH264 {
            picture_id: ref_id,
            frame_idx: frame_num.wrapping_sub(1) % (1 << (LOG2_MAX_FRAME_NUM_MENOS4 + 4)),
            flags: VA_PICTURE_H264_SHORT_TERM_REFERENCE,
            TopFieldOrderCnt: poc - 2,
            BottomFieldOrderCnt: poc - 2,
            va_reserved: [0; 4],
        };
        let mut pic: VAEncPictureParameterBufferH264 = zerada();
        pic.CurrPic = VAPictureH264 {
            picture_id: rec_id,
            frame_idx: frame_num,
            flags: 0,
            TopFieldOrderCnt: poc,
            BottomFieldOrderCnt: poc,
            va_reserved: [0; 4],
        };
        pic.ReferenceFrames = [invalida; 16];
        if !chave {
            pic.ReferenceFrames[0] = anterior;
        }
        pic.coded_buf = codificado;
        pic.frame_num = frame_num as u16;
        pic.pic_init_qp = 26;
        // idr_pic_flag, reference_pic_flag=1, entropy_coding_mode_flag,
        // transform_8x8_mode_flag (High) e deblocking_filter_control_present_flag.
        pic.pic_fields.value = u32::from(chave)
            | (1 << 1)
            | (u32::from(cabac) << 3)
            | (u32::from(self.perfil == VAProfileH264High) << 8)
            | (1 << 9);
        self.parametro(VAEncPictureParameterBufferType, &pic)?;

        let mut fatia = VAEncSliceParameterBufferH264 {
            macroblock_address: 0,
            num_macroblocks: mbs_l * mbs_a,
            macroblock_info: VA_INVALID_ID,
            // 2 = I, 0 = P.
            slice_type: if chave { 2 } else { 0 },
            idr_pic_id: self.idr_id,
            ..Default::default()
        };
        fatia.RefPicList0 = [invalida; 32];
        fatia.RefPicList1 = [invalida; 32];
        if !chave {
            fatia.RefPicList0[0] = anterior;
        }
        self.parametro(VAEncSliceParameterBufferType, &fatia)?;

        let (d, va) = (&self.d, &self.d.libs.va);
        // SAFETY: contexto, superfícies e buffers desta sessão; o segmento codificado fica
        // mapeado até o unmap e cada `buf` tem `size` bytes.
        let r = unsafe {
            (|| {
                d.checar(
                    va.vaBeginPicture(d.dpy, contexto, entrada),
                    "vaBeginPicture",
                )?;
                d.checar(
                    va.vaRenderPicture(
                        d.dpy,
                        contexto,
                        self.buffers.as_ptr().cast_mut(),
                        self.buffers.len() as i32,
                    ),
                    "vaRenderPicture",
                )?;
                d.checar(va.vaEndPicture(d.dpy, contexto), "vaEndPicture")?;
                d.checar(va.vaSyncSurface(d.dpy, entrada), "vaSyncSurface")?;
                let mut p: *mut c_void = std::ptr::null_mut();
                d.checar(
                    va.vaMapBuffer(d.dpy, codificado, &mut p),
                    "vaMapBuffer(coded)",
                )?;
                let mut seg = p.cast::<VACodedBufferSegment>();
                while !seg.is_null() {
                    let s = &*seg;
                    saida.extend_from_slice(std::slice::from_raw_parts(
                        s.buf.cast::<u8>(),
                        s.size as usize,
                    ));
                    seg = s.next.cast();
                }
                d.checar(va.vaUnmapBuffer(d.dpy, codificado), "vaUnmapBuffer(coded)")
            })()
        };
        self.soltar_buffers();
        r?;
        self.referencia = reconstrucao;
        self.desde_idr += 1;
        Ok(if chave {
            TipoQuadro::Chave
        } else {
            TipoQuadro::Delta
        })
    }
}

impl Drop for EncoderVaapi {
    fn drop(&mut self) {
        self.soltar_buffers();
        self.fechar();
    }
}

impl VideoEncoder for EncoderVaapi {
    fn nome(&self) -> &'static str {
        "vaapi"
    }

    fn gpu(&self) -> Option<&str> {
        Some(&self.gpu)
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
        let (l, a) = (quadro.largura & !1, quadro.altura & !1);
        let mut chave = forcar_chave;
        if self
            .sessao
            .as_ref()
            .is_none_or(|s| (s.largura, s.altura) != (l, a))
        {
            self.abrir(l, a)?;
            chave = true;
        }
        match self.quadro(quadro, chave, saida) {
            Ok(tipo) => Ok((!saida.is_empty()).then_some(tipo)),
            Err(e) => {
                // A próxima tentativa recomeça a sessão com IDR.
                self.fechar();
                Err(erro(e))
            }
        }
    }

    fn ajustar_bitrate(&mut self, bps: u32) {
        if bps != self.config.bitrate_bps {
            self.config.bitrate_bps = bps;
            self.taxa_mudou = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DecoderOpenH264, OrdemRgb, VideoDecoder};

    /// Codifica 30 quadros que mudam no primeiro nó com encode VA-API e decodifica no openh264.
    /// Sem VA-API o teste não tem o que testar e passa.
    #[test]
    fn ida_e_volta_no_openh264() {
        let Some(no) = super::super::va::nos_de_render()
            .into_iter()
            .find(|n| codifica_h264(n).is_some())
        else {
            eprintln!("sem encode VA-API: pulando");
            return;
        };
        let mut enc = EncoderVaapi::novo(
            &no,
            "teste".into(),
            ConfigEncoder {
                bitrate_bps: 4_000_000,
                fps_max: 30.0,
            },
        )
        .unwrap();
        let mut dec = DecoderOpenH264::novo().unwrap();
        // 1080 não é múltiplo de 16: exercita o recorte.
        let (l, a) = (1920u32, 1080u32);
        let mut img = vec![0u8; (l * a * 4) as usize];
        let mut au = Vec::new();
        let (mut chaves, mut decodificados) = (0, 0);
        for i in 0..30u32 {
            for (p, px) in img.chunks_exact_mut(4).enumerate() {
                let (x, y) = (p as u32 % l, p as u32 / l);
                let v = (((x + i * 8) / 64 + y / 64) % 2 * 200 + 20) as u8;
                px.copy_from_slice(&[v, v, 255 - v, 0]);
            }
            let q = QuadroRgb {
                largura: l,
                altura: a,
                stride: l as usize * 4,
                ordem: OrdemRgb::Bgrx,
                dados: &img,
            };
            let tipo = enc.codificar(&q, i == 20, &mut au).unwrap().unwrap();
            chaves += usize::from(tipo == TipoQuadro::Chave);
            dec.decodificar(&au, &mut |y| {
                assert_eq!((y.largura, y.altura), (l, a));
                // Pixel (8, 8): v = 20 + 200 * ((8 + i*8)/64 % 2) no B e no G.
                let v = (((8 + i * 8) / 64) % 2 * 200 + 20) as i32;
                let esperado = 16 + ((47 * (255 - v) + 157 * v + 16 * v + 128) >> 8);
                let visto = i32::from(y.planos[0].dados[8 * y.planos[0].passo as usize + 8]);
                assert!(
                    (visto - esperado).abs() <= 12,
                    "quadro {i}: Y {visto} vs {esperado}"
                );
                decodificados += 1;
            })
            .unwrap();
        }
        assert_eq!(chaves, 2);
        assert_eq!(decodificados, 30);
    }
}
