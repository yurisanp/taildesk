//! Decoder H.264, HEVC ou AV1 no NVDEC (GPUs NVIDIA), pelo `libnvcuvid.so.1` do driver.
//!
//! O parser do próprio NVDEC lê o Annex-B, cria o decoder no primeiro SPS e entrega cada quadro
//! sem atraso (`ulMaxDisplayDelay = 0`). O quadro decodificado (NV12 na GPU) é copiado para a
//! memória do processo e emprestado ao callback, como nos outros decoders.

use std::ffi::{c_int, c_void};
use std::sync::OnceLock;

use super::cuda::{Contexto, CuDevice, Cuda};
use super::sys::nvcuvid::*;
use crate::{CodecVideo, ErroCodec, Faixa, FormatoYuv, Matriz, PlanoYuv, QuadroYuv, VideoDecoder};

fn tipo_cuvid(c: CodecVideo) -> cudaVideoCodec {
    match c {
        CodecVideo::H264 => cudaVideoCodec_H264,
        CodecVideo::Hevc => cudaVideoCodec_HEVC,
        CodecVideo::Av1 => cudaVideoCodec_AV1,
    }
}

/// Codecs (8 bits, 4:2:0) que o NVDEC da GPU `dev` decodifica, perguntado uma vez por GPU.
pub fn codecs(dev: CuDevice) -> Vec<CodecVideo> {
    static CACHE: std::sync::Mutex<Vec<(CuDevice, Vec<CodecVideo>)>> =
        std::sync::Mutex::new(Vec::new());
    let mut cache = CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((_, c)) = cache.iter().find(|(d, _)| *d == dev) {
        return c.clone();
    }
    let consultar = || -> Option<Vec<CodecVideo>> {
        let lib = lib()?;
        let ctx = Contexto::novo(Cuda::carregar()?, dev).ok()?;
        ctx.com(|| {
            [CodecVideo::H264, CodecVideo::Hevc, CodecVideo::Av1]
                .into_iter()
                .filter(|&c| {
                    // SAFETY: estrutura POD do cuviddec.h, zerada; o contexto está atual.
                    unsafe {
                        let mut caps: CUVIDDECODECAPS = std::mem::zeroed();
                        caps.eCodecType = tipo_cuvid(c);
                        caps.eChromaFormat = cudaVideoChromaFormat_420;
                        caps.nBitDepthMinus8 = 0;
                        lib.cuvidGetDecoderCaps(&mut caps) == 0 && caps.bIsSupported != 0
                    }
                })
                .collect()
        })
        .ok()
    };
    let c = consultar().unwrap_or_default();
    cache.push((dev, c.clone()));
    c
}

fn lib() -> Option<&'static LibNvcuvid> {
    static LIB: OnceLock<Option<LibNvcuvid>> = OnceLock::new();
    LIB.get_or_init(|| {
        // SAFETY: biblioteca do driver da NVIDIA; o bindgen confere cada símbolo.
        unsafe { LibNvcuvid::new(super::biblioteca("libnvcuvid.so.1", "nvcuvid.dll")).ok() }
    })
    .as_ref()
}

fn erro(e: impl std::fmt::Display) -> ErroCodec {
    ErroCodec::Backend(format!("nvdec: {e}"))
}

fn checar(r: CUresult, o_que: &str) -> Result<(), ErroCodec> {
    if r == 0 {
        Ok(())
    } else {
        Err(erro(format!("{o_que}: CUresult {r}")))
    }
}

/// GPUs NVIDIA com NVDEC: dispositivo CUDA, nome e endereço PCI.
pub fn gpus() -> Vec<(CuDevice, String, String)> {
    match (Cuda::carregar(), lib()) {
        (Some(cuda), Some(_)) => cuda.gpus(),
        _ => Vec::new(),
    }
}

/// O que os callbacks do parser mexem (o parser recebe um ponteiro para isto).
struct Estado {
    lib: &'static LibNvcuvid,
    codec: cudaVideoCodec,
    decoder: CUvideodecoder,
    /// Tamanho do quadro (área de exibição) e altura da superfície do decoder.
    largura: u32,
    altura: u32,
    altura_superficie: u32,
    /// NV12 do último quadro: Y com `altura` linhas e UV com `altura / 2`, passo = `largura`.
    y: Vec<u8>,
    uv: Vec<u8>,
    novo: bool,
    erro: Option<ErroCodec>,
    ctx: *const Contexto,
}

impl Estado {
    fn destruir_decoder(&mut self) {
        if !self.decoder.is_null() {
            // SAFETY: decoder criado por `sequencia` e ainda não destruído.
            unsafe { self.lib.cuvidDestroyDecoder(self.decoder) };
            self.decoder = std::ptr::null_mut();
        }
    }
}

/// SPS novo: (re)cria o decoder. Devolve quantas superfícies o parser pode usar.
// `c_ulong` tem 64 bits no Linux e 32 no Windows: o `.into()` é conversão de verdade só no Linux.
#[allow(clippy::useless_conversion)]
unsafe extern "C" fn sequencia(dados: *mut c_void, formato: *mut CUVIDEOFORMAT) -> c_int {
    // SAFETY: o parser chama com o `Estado` que registramos e um formato válido.
    let (e, f) = unsafe { (&mut *dados.cast::<Estado>(), &*formato) };
    if f.codec != e.codec
        || f.chroma_format != cudaVideoChromaFormat_420
        || f.bit_depth_luma_minus8 != 0
    {
        e.erro = Some(erro("formato não suportado"));
        return 0;
    }
    e.destruir_decoder();
    let area = f.display_area;
    let (l, a) = (
        (area.right - area.left) as u32,
        (area.bottom - area.top) as u32,
    );
    let superficies = u32::from(f.min_num_decode_surfaces).max(1);
    // SAFETY: estrutura POD do cuviddec.h, zerada e preenchida como o header pede.
    let mut info: CUVIDDECODECREATEINFO = unsafe { std::mem::zeroed() };
    info.ulWidth = f.coded_width.into();
    info.ulHeight = f.coded_height.into();
    info.ulMaxWidth = f.coded_width.into();
    info.ulMaxHeight = f.coded_height.into();
    info.ulNumDecodeSurfaces = superficies.into();
    info.CodecType = f.codec;
    info.ChromaFormat = f.chroma_format;
    info.ulCreationFlags = cudaVideoCreate_PreferCUVID as _;
    info.OutputFormat = cudaVideoSurfaceFormat_NV12;
    info.DeinterlaceMode = cudaVideoDeinterlaceMode_Weave;
    info.display_area.left = area.left as i16;
    info.display_area.top = area.top as i16;
    info.display_area.right = area.right as i16;
    info.display_area.bottom = area.bottom as i16;
    info.ulTargetWidth = l.into();
    info.ulTargetHeight = a.into();
    info.ulNumOutputSurfaces = 2;
    // SAFETY: `info` preenchido acima; o contexto CUDA está atual (o parse roda em `com`).
    let r = unsafe { e.lib.cuvidCreateDecoder(&mut e.decoder, &mut info) };
    if let Err(x) = checar(r, "cuvidCreateDecoder") {
        e.erro = Some(x);
        return 0;
    }
    (e.largura, e.altura, e.altura_superficie) = (l, a, a);
    e.y.resize((l * a) as usize, 0);
    e.uv.resize((l * a / 2) as usize, 0);
    superficies as c_int
}

unsafe extern "C" fn decodificar(dados: *mut c_void, pic: *mut CUVIDPICPARAMS) -> c_int {
    // SAFETY: o parser chama com o `Estado` registrado e parâmetros válidos.
    let e = unsafe { &mut *dados.cast::<Estado>() };
    if e.decoder.is_null() {
        return 0;
    }
    // SAFETY: decoder criado na sequência; `pic` vem do parser.
    let r = unsafe { e.lib.cuvidDecodePicture(e.decoder, pic) };
    match checar(r, "cuvidDecodePicture") {
        Ok(()) => 1,
        Err(x) => {
            e.erro = Some(x);
            0
        }
    }
}

unsafe extern "C" fn exibir(dados: *mut c_void, info: *mut CUVIDPARSERDISPINFO) -> c_int {
    // SAFETY: o parser chama com o `Estado` registrado; `info` nulo = fim do stream.
    let (e, info) = unsafe { (&mut *dados.cast::<Estado>(), info.as_ref()) };
    let Some(info) = info else { return 1 };
    if e.decoder.is_null() {
        return 0;
    }
    // SAFETY: estrutura POD do cuviddec.h, zerada como o header pede.
    let mut proc: CUVIDPROCPARAMS = unsafe { std::mem::zeroed() };
    proc.progressive_frame = info.progressive_frame;
    proc.top_field_first = info.top_field_first;
    let (mut ptr, mut passo) = (0u64, 0u32);
    // SAFETY: mapeia a superfície `picture_index` do decoder; desmapeada logo abaixo.
    let r = unsafe {
        e.lib.cuvidMapVideoFrame64(
            e.decoder,
            info.picture_index,
            &mut ptr,
            &mut passo,
            &mut proc,
        )
    };
    if let Err(x) = checar(r, "cuvidMapVideoFrame") {
        e.erro = Some(x);
        return 0;
    }
    let (l, a, passo) = (e.largura as usize, e.altura as usize, passo as usize);
    // SAFETY: `ctx` aponta para o contexto do decoder, vivo durante o parse.
    let ctx = unsafe { &*e.ctx };
    let uv = ptr + (passo * ((e.altura_superficie as usize + 1) & !1)) as u64;
    let r = ctx
        .copiar_para_host(ptr, passo, &mut e.y, l, l, a)
        .and_then(|()| ctx.copiar_para_host(uv, passo, &mut e.uv, l, l, a / 2));
    // SAFETY: desfaz o map acima.
    unsafe { e.lib.cuvidUnmapVideoFrame64(e.decoder, ptr) };
    match r {
        Ok(()) => {
            e.novo = true;
            1
        }
        Err(x) => {
            e.erro = Some(erro(x));
            0
        }
    }
}

/// Decoder no NVDEC de uma GPU.
pub struct DecoderNvdec {
    lib: &'static LibNvcuvid,
    ctx: Box<Contexto>,
    parser: CUvideoparser,
    estado: Box<Estado>,
    gpu: String,
    codec: CodecVideo,
}

// SAFETY: parser, decoder e contexto são handles do driver usados por uma thread de cada vez
// (`&mut self`); o ponteiro do estado aponta para a caixa que mora junto.
unsafe impl Send for DecoderNvdec {}

impl std::fmt::Debug for DecoderNvdec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecoderNvdec")
            .field("gpu", &self.gpu)
            .field("codec", &self.codec)
            .finish_non_exhaustive()
    }
}

impl DecoderNvdec {
    pub fn novo(dev: CuDevice, gpu: String, codec: CodecVideo) -> Result<Self, ErroCodec> {
        let cuda = Cuda::carregar().ok_or_else(|| erro("sem libcuda"))?;
        let lib = lib().ok_or_else(|| erro("sem libnvcuvid"))?;
        let ctx = Box::new(Contexto::novo(cuda, dev).map_err(erro)?);
        let mut estado = Box::new(Estado {
            lib,
            codec: tipo_cuvid(codec),
            decoder: std::ptr::null_mut(),
            largura: 0,
            altura: 0,
            altura_superficie: 0,
            y: Vec::new(),
            uv: Vec::new(),
            novo: false,
            erro: None,
            ctx: &*ctx,
        });
        // SAFETY: estrutura POD do nvcuvid.h, zerada; o `Estado` vive numa caixa que fica
        // junto do parser até `drop`, e os callbacks têm as assinaturas do header.
        let parser = unsafe {
            let mut p: CUVIDPARSERPARAMS = std::mem::zeroed();
            p.CodecType = tipo_cuvid(codec);
            p.ulMaxNumDecodeSurfaces = 1;
            p.ulMaxDisplayDelay = 0;
            p.pUserData = (&mut *estado as *mut Estado).cast();
            p.pfnSequenceCallback = Some(sequencia);
            p.pfnDecodePicture = Some(decodificar);
            p.pfnDisplayPicture = Some(exibir);
            let mut parser = std::ptr::null_mut();
            let r = ctx
                .com(|| lib.cuvidCreateVideoParser(&mut parser, &mut p))
                .map_err(erro)?;
            checar(r, "cuvidCreateVideoParser")?;
            parser
        };
        Ok(DecoderNvdec {
            lib,
            ctx,
            parser,
            estado,
            gpu,
            codec,
        })
    }
}

impl Drop for DecoderNvdec {
    fn drop(&mut self) {
        let (lib, parser) = (self.lib, self.parser);
        let estado = &mut self.estado;
        let _ = self.ctx.com(|| {
            // SAFETY: parser criado em `novo`; o decoder é destruído depois dele, os dois
            // com o contexto atual.
            unsafe { lib.cuvidDestroyVideoParser(parser) };
            estado.destruir_decoder();
        });
    }
}

impl VideoDecoder for DecoderNvdec {
    fn nome(&self) -> &'static str {
        "nvdec"
    }

    fn gpu(&self) -> Option<&str> {
        Some(&self.gpu)
    }

    fn decodificar(
        &mut self,
        access_unit: &[u8],
        ao_quadro: &mut dyn FnMut(&QuadroYuv<'_>),
    ) -> Result<bool, ErroCodec> {
        // SAFETY: estrutura POD; o payload vive durante o parse, que é síncrono.
        let mut pacote: CUVIDSOURCEDATAPACKET = unsafe { std::mem::zeroed() };
        pacote.flags = CUVID_PKT_ENDOFPICTURE as _;
        pacote.payload = access_unit.as_ptr();
        pacote.payload_size = access_unit.len() as _;
        self.estado.novo = false;
        let (lib, parser) = (self.lib, self.parser);
        // SAFETY: parser criado em `novo`, com o contexto atual; os callbacks rodam aqui
        // dentro, nesta thread.
        let r = self
            .ctx
            .com(|| unsafe { lib.cuvidParseVideoData(parser, &mut pacote) })
            .map_err(erro)?;
        checar(r, "cuvidParseVideoData")?;
        if let Some(e) = self.estado.erro.take() {
            return Err(e);
        }
        if !self.estado.novo {
            return Ok(false);
        }
        let e = &self.estado;
        ao_quadro(&QuadroYuv {
            largura: e.largura,
            altura: e.altura,
            formato: FormatoYuv::Nv12,
            planos: [
                PlanoYuv {
                    dados: &e.y,
                    passo: e.largura,
                },
                PlanoYuv {
                    dados: &e.uv,
                    passo: e.largura,
                },
                PlanoYuv {
                    dados: &[],
                    passo: 0,
                },
            ],
            // Os encoders do TailDesk marcam BT.709 limitado no VUI.
            matriz: Matriz::Bt709,
            faixa: Faixa::Limitada,
        });
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ConfigEncoder, EncoderOpenH264, OrdemRgb, QuadroRgb, VideoEncoder};

    /// openh264 → NVDEC: tamanho, formato e brilho. Sem GPU NVIDIA o teste passa sem testar.
    #[test]
    fn decodifica_o_openh264() {
        let Some((dev, nome, _)) = gpus().into_iter().next() else {
            eprintln!("sem NVDEC: pulando");
            return;
        };
        let mut dec = DecoderNvdec::novo(dev, nome, CodecVideo::H264).unwrap();
        let mut enc = EncoderOpenH264::novo(ConfigEncoder {
            bitrate_bps: 4_000_000,
            fps_max: 30.0,
        })
        .unwrap();
        let (l, a) = (640u32, 360u32);
        let mut au = Vec::new();
        let mut vistos = 0;
        for i in 0..10u32 {
            let v = (i * 20 + 30) as u8;
            let img = vec![v; (l * a * 4) as usize];
            let q = QuadroRgb {
                largura: l,
                altura: a,
                stride: l as usize * 4,
                ordem: OrdemRgb::Bgrx,
                dados: &img,
            };
            if enc.codificar(&q, false, &mut au).unwrap().is_none() {
                continue;
            }
            dec.decodificar(&au, &mut |y| {
                assert_eq!((y.largura, y.altura, y.formato), (l, a, FormatoYuv::Nv12));
                let esperado = 16 + i32::from(v) * 219 / 255;
                let visto = i32::from(y.planos[0].dados[(a as usize / 2) * l as usize + 7]);
                assert!(
                    (visto - esperado).abs() <= 3,
                    "quadro {i}: {visto} vs {esperado}"
                );
                vistos += 1;
            })
            .unwrap();
        }
        assert!(vistos >= 8, "{vistos}");
    }
}
