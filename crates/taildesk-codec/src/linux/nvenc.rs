//! Encoder H.264, HEVC ou AV1 no NVENC (GPUs NVIDIA), pela API do Video Codec SDK 12.1 carregada do
//! `libnvidia-encode.so.1` do driver.
//!
//! Preset P4 (H.264) ou P1 (HEVC, AV1) com tuning de latência ultrabaixa, CBR com VBV de [`QUADROS_VBV`] quadros e GOP
//! infinito (IDR só no começo e quando pedido). A imagem BGRx/RGBx vai direto para o buffer de entrada do NVENC, que
//! converte para YUV na GPU; nada passa pelo conversor da CPU.

use std::ffi::c_void;
use std::sync::OnceLock;

use super::cuda::{Contexto, CuDevice, Cuda};
use super::sys::nvenc::*;
use crate::{CodecVideo, ConfigEncoder, ErroCodec, OrdemRgb, QuadroRgb, TipoQuadro, VideoEncoder};

const fn versao(v: u32) -> u32 {
    NVENCAPI_VERSION | (v << 16) | (0x7 << 28)
}
const FUNCTION_LIST_VER: u32 = versao(2);
const OPEN_SESSION_EX_VER: u32 = versao(1);
const PRESET_CONFIG_VER: u32 = versao(4) | (1 << 31);
const CONFIG_VER: u32 = versao(8) | (1 << 31);
const INITIALIZE_PARAMS_VER: u32 = versao(6) | (1 << 31);
const RECONFIGURE_PARAMS_VER: u32 = versao(1) | (1 << 31);
const CREATE_INPUT_BUFFER_VER: u32 = versao(1);
const CREATE_BITSTREAM_BUFFER_VER: u32 = versao(1);
const LOCK_INPUT_BUFFER_VER: u32 = versao(1);
const PIC_PARAMS_VER: u32 = versao(6) | (1 << 31);
const LOCK_BITSTREAM_VER: u32 = versao(1) | (1 << 31);
const INFINITE_GOPLENGTH: u32 = 0xffff_ffff;
/// VBV em quadros. Com um quadro só, o controle de taxa achata a qualidade de tela que muda (texto
/// rolando: PSNR 31 dB contra 35 dB com 8 quadros, a 8 Mbit/s em 1080p) e ainda sobra taxa.
pub(crate) const QUADROS_VBV: f32 = 8.0;

const CODEC_H264: GUID = GUID {
    Data1: 0x6bc8_2762,
    Data2: 0x4e63,
    Data3: 0x4ca4,
    Data4: [0xaa, 0x85, 0x1e, 0x50, 0xf3, 0x21, 0xf6, 0xbf],
};
/// P4: ~1 ms a mais por quadro que o P1 em 1080p e uns 3 dB a mais de PSNR na mesma taxa.
const PRESET_P4: GUID = GUID {
    Data1: 0x90a7_b826,
    Data2: 0xdf06,
    Data3: 0x4862,
    Data4: [0xb9, 0xd2, 0xcd, 0x6d, 0x73, 0xa0, 0x86, 0x81],
};
const CODEC_HEVC: GUID = GUID {
    Data1: 0x790c_dc88,
    Data2: 0x4522,
    Data3: 0x4d7b,
    Data4: [0x94, 0x25, 0xbd, 0xa9, 0x97, 0x5f, 0x76, 0x03],
};
const CODEC_AV1: GUID = GUID {
    Data1: 0x0a35_2289,
    Data2: 0x0aa7,
    Data3: 0x4759,
    Data4: [0x86, 0x2d, 0x5d, 0x15, 0xcd, 0x16, 0xd2, 0x54],
};
const PERFIL_HEVC_MAIN: GUID = GUID {
    Data1: 0xb514_c39a,
    Data2: 0xb55b,
    Data3: 0x40fa,
    Data4: [0x87, 0x8f, 0xf1, 0x25, 0x3b, 0x4d, 0xfd, 0xec],
};
const PERFIL_AV1_MAIN: GUID = GUID {
    Data1: 0x5f2a_39f5,
    Data2: 0xf14e,
    Data3: 0x4f95,
    Data4: [0x9a, 0x9e, 0xb7, 0x6d, 0x56, 0x8f, 0xcf, 0x97],
};

fn guid_do_codec(c: CodecVideo) -> GUID {
    match c {
        CodecVideo::H264 => CODEC_H264,
        CodecVideo::Hevc => CODEC_HEVC,
        CodecVideo::Av1 => CODEC_AV1,
    }
}

fn mesmo_guid(a: &GUID, b: &GUID) -> bool {
    (a.Data1, a.Data2, a.Data3, a.Data4) == (b.Data1, b.Data2, b.Data3, b.Data4)
}

/// P1 para HEVC e AV1: na RTX 3060 (bateria) o HEVC leva 14,6 ms por quadro 1080p no P1 e 19,3
/// ms no P4, com o mesmo PSNR (37 dB a 8 Mbit/s).
const PRESET_P1: GUID = GUID {
    Data1: 0xfc0a_8d3e,
    Data2: 0x45f8,
    Data3: 0x4cf8,
    Data4: [0x80, 0xc7, 0x29, 0x88, 0x71, 0x59, 0x0e, 0xbf],
};

const PERFIL_HIGH: GUID = GUID {
    Data1: 0xe7cb_c309,
    Data2: 0x4f7a,
    Data3: 0x4b89,
    Data4: [0xaf, 0x2a, 0xd5, 0x37, 0xc9, 0x2b, 0xe3, 0x10],
};

/// Tabela de funções do NVENC, carregada uma vez por processo.
struct Api {
    _lib: libloading::Library,
    f: NV_ENCODE_API_FUNCTION_LIST,
}

// SAFETY: a tabela só guarda ponteiros de função do driver, que valem em qualquer thread.
unsafe impl Send for Api {}
// SAFETY: idem; ninguém escreve na tabela depois de carregada.
unsafe impl Sync for Api {}

fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(|| {
        // SAFETY: biblioteca do driver da NVIDIA; os dois símbolos têm as assinaturas de
        // nvEncodeAPI.h. A tabela é zerada (o formato que a API espera) e preenchida por ela.
        unsafe {
            let lib = libloading::Library::new("libnvidia-encode.so.1").ok()?;
            let max: libloading::Symbol<unsafe extern "C" fn(*mut u32) -> NVENCSTATUS> =
                lib.get(b"NvEncodeAPIGetMaxSupportedVersion\0").ok()?;
            let mut v = 0;
            if max(&mut v) != NV_ENC_SUCCESS
                || v < (NVENCAPI_MAJOR_VERSION << 4 | NVENCAPI_MINOR_VERSION)
            {
                return None;
            }
            let criar: libloading::Symbol<
                unsafe extern "C" fn(*mut NV_ENCODE_API_FUNCTION_LIST) -> NVENCSTATUS,
            > = lib.get(b"NvEncodeAPICreateInstance\0").ok()?;
            let mut f: NV_ENCODE_API_FUNCTION_LIST = std::mem::zeroed();
            f.version = FUNCTION_LIST_VER;
            if criar(&mut f) != NV_ENC_SUCCESS {
                return None;
            }
            Some(Api { _lib: lib, f })
        }
    })
    .as_ref()
}

fn checar(r: NVENCSTATUS, o_que: &str) -> Result<(), ErroCodec> {
    if r == NV_ENC_SUCCESS {
        Ok(())
    } else {
        Err(ErroCodec::Backend(format!(
            "nvenc: {o_que}: NVENCSTATUS {r}"
        )))
    }
}

macro_rules! chamar {
    ($api:expr, $f:ident ( $($a:expr),* $(,)? )) => {{
        let f = $api.f.$f.ok_or_else(|| {
            ErroCodec::Backend(concat!("nvenc: sem ", stringify!($f)).into())
        })?;
        checar(f($($a),*), stringify!($f))
    }};
}

/// GPUs NVIDIA com NVENC: dispositivo CUDA, nome e endereço PCI.
pub fn gpus() -> Vec<(CuDevice, String, String)> {
    match (Cuda::carregar(), api()) {
        (Some(cuda), Some(_)) => cuda.gpus(),
        _ => Vec::new(),
    }
}

/// Sessão aberta do NVENC para um tamanho.
struct Sessao {
    enc: *mut c_void,
    entrada: NV_ENC_INPUT_PTR,
    saida: NV_ENC_OUTPUT_PTR,
    largura: u32,
    altura: u32,
    formato: NV_ENC_BUFFER_FORMAT,
    init: NV_ENC_INITIALIZE_PARAMS,
    config: Box<NV_ENC_CONFIG>,
}

/// Codecs que o NVENC da GPU CUDA `dev` codifica (H.264 em toda GPU com NVENC; HEVC desde a
/// Maxwell 2; AV1 desde a Ada). Perguntado ao driver uma vez por GPU.
pub fn codecs(dev: CuDevice) -> Vec<CodecVideo> {
    static CACHE: std::sync::Mutex<Vec<(CuDevice, Vec<CodecVideo>)>> =
        std::sync::Mutex::new(Vec::new());
    let mut cache = CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((_, c)) = cache.iter().find(|(d, _)| *d == dev) {
        return c.clone();
    }
    let c = consultar_codecs(dev).unwrap_or_default();
    cache.push((dev, c.clone()));
    c
}

fn consultar_codecs(dev: CuDevice) -> Option<Vec<CodecVideo>> {
    let api = api()?;
    let ctx = Contexto::novo(Cuda::carregar()?, dev).ok()?;
    ctx.com(|| {
        // SAFETY: sessão aberta só para a consulta e destruída em seguida; o array de GUIDs
        // tem o tamanho que o driver disse.
        unsafe {
            let mut abrir: NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS = std::mem::zeroed();
            abrir.version = OPEN_SESSION_EX_VER;
            abrir.deviceType = NV_ENC_DEVICE_TYPE_CUDA;
            abrir.device = ctx.ctx;
            abrir.apiVersion = NVENCAPI_VERSION;
            let mut enc = std::ptr::null_mut();
            if api.f.nvEncOpenEncodeSessionEx?(&mut abrir, &mut enc) != NV_ENC_SUCCESS {
                return None;
            }
            let mut n = 0u32;
            let mut guids = Vec::new();
            if api.f.nvEncGetEncodeGUIDCount?(enc, &mut n) == NV_ENC_SUCCESS {
                guids = vec![GUID::default(); n as usize];
                let mut lidos = 0u32;
                if api.f.nvEncGetEncodeGUIDs?(enc, guids.as_mut_ptr(), n, &mut lidos)
                    == NV_ENC_SUCCESS
                {
                    guids.truncate(lidos as usize);
                } else {
                    guids.clear();
                }
            }
            api.f.nvEncDestroyEncoder?(enc);
            Some(
                [CodecVideo::H264, CodecVideo::Hevc, CodecVideo::Av1]
                    .into_iter()
                    .filter(|c| guids.iter().any(|g| mesmo_guid(g, &guid_do_codec(*c))))
                    .collect(),
            )
        }
    })
    .ok()
    .flatten()
}

/// Encoder no NVENC de uma GPU.
pub struct EncoderNvenc {
    api: &'static Api,
    ctx: Contexto,
    gpu: String,
    codec: CodecVideo,
    config: ConfigEncoder,
    sessao: Option<Sessao>,
    quadro: u64,
}

// SAFETY: a sessão do NVENC e o contexto CUDA são usados por uma thread de cada vez (o encoder é
// `&mut self` em tudo); os ponteiros são handles opacos do driver.
unsafe impl Send for EncoderNvenc {}

impl std::fmt::Debug for EncoderNvenc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EncoderNvenc")
            .field("gpu", &self.gpu)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl EncoderNvenc {
    /// Abre o NVENC na GPU CUDA `dev` (de [`gpus`]) para `codec` e confere que ele codifica um
    /// quadro.
    pub fn novo(
        dev: CuDevice,
        gpu: String,
        codec: CodecVideo,
        config: ConfigEncoder,
    ) -> Result<Self, ErroCodec> {
        let erro = |e: String| ErroCodec::Backend(format!("nvenc: {e}"));
        let cuda = Cuda::carregar().ok_or_else(|| erro("sem libcuda".into()))?;
        let api = api().ok_or_else(|| erro("sem libnvidia-encode".into()))?;
        let ctx = Contexto::novo(cuda, dev).map_err(erro)?;
        let mut e = EncoderNvenc {
            api,
            ctx,
            gpu,
            codec,
            config,
            sessao: None,
            quadro: 0,
        };
        // Probe: um quadro pequeno de verdade (driver velho ou sessões esgotadas falham aqui).
        let img = vec![0x80u8; 256 * 256 * 4];
        let q = QuadroRgb {
            largura: 256,
            altura: 256,
            stride: 256 * 4,
            ordem: OrdemRgb::Bgrx,
            dados: &img,
        };
        let mut au = Vec::new();
        e.codificar(&q, true, &mut au)?;
        if au.is_empty() {
            return Err(erro("o quadro de teste saiu vazio".into()));
        }
        e.fechar();
        e.quadro = 0;
        Ok(e)
    }

    fn fechar(&mut self) {
        let Some(s) = self.sessao.take() else { return };
        let api = self.api;
        let _ = self.ctx.com(|| {
            // SAFETY: handles criados por esta sessão, destruídos uma vez, buffers antes do
            // encoder, com o contexto dela atual.
            unsafe {
                if let Some(f) = api.f.nvEncDestroyInputBuffer {
                    f(s.enc, s.entrada);
                }
                if let Some(f) = api.f.nvEncDestroyBitstreamBuffer {
                    f(s.enc, s.saida);
                }
                if let Some(f) = api.f.nvEncDestroyEncoder {
                    f(s.enc);
                }
            }
        });
    }

    fn rc(&self, rc: &mut NV_ENC_RC_PARAMS) {
        let bps = self.config.bitrate_bps.max(100_000);
        let fps = self.config.fps_max.max(1.0);
        rc.rateControlMode = NV_ENC_PARAMS_RC_CBR;
        rc.averageBitRate = bps;
        rc.maxBitRate = bps;
        rc.vbvBufferSize = (bps as f32 / fps * QUADROS_VBV) as u32;
        rc.vbvInitialDelay = rc.vbvBufferSize;
    }

    /// Abre a sessão para o tamanho e o formato do quadro.
    fn abrir(
        &mut self,
        largura: u32,
        altura: u32,
        formato: NV_ENC_BUFFER_FORMAT,
    ) -> Result<(), ErroCodec> {
        self.fechar();
        let api = self.api;
        let ctx = self.ctx.ctx;
        let fps = self.config.fps_max.max(1.0).round() as u32;
        let (codec, perfil, guid_preset) = match self.codec {
            CodecVideo::H264 => (CODEC_H264, PERFIL_HIGH, PRESET_P4),
            CodecVideo::Hevc => (CODEC_HEVC, PERFIL_HEVC_MAIN, PRESET_P1),
            CodecVideo::Av1 => (CODEC_AV1, PERFIL_AV1_MAIN, PRESET_P1),
        };
        let qual = self.codec;
        let mut sessao = None;
        let r = self.ctx.com(|| -> Result<(), ErroCodec> {
            // SAFETY: todas as estruturas são POD do header, zeradas como a API pede e com a
            // versão certa; os ponteiros passados vivem durante cada chamada, e `config` fica
            // numa caixa que mora na sessão (o NVENC guarda o ponteiro do init).
            unsafe {
                let mut abrir: NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS = std::mem::zeroed();
                abrir.version = OPEN_SESSION_EX_VER;
                abrir.deviceType = NV_ENC_DEVICE_TYPE_CUDA;
                abrir.device = ctx;
                abrir.apiVersion = NVENCAPI_VERSION;
                let mut enc = std::ptr::null_mut();
                chamar!(api, nvEncOpenEncodeSessionEx(&mut abrir, &mut enc))?;
                let mut s = Sessao {
                    enc,
                    entrada: std::ptr::null_mut(),
                    saida: std::ptr::null_mut(),
                    largura,
                    altura,
                    formato,
                    init: std::mem::zeroed(),
                    config: Box::new(std::mem::zeroed()),
                };
                let r = (|| {
                    let mut preset: NV_ENC_PRESET_CONFIG = std::mem::zeroed();
                    preset.version = PRESET_CONFIG_VER;
                    preset.presetCfg.version = CONFIG_VER;
                    chamar!(
                        api,
                        nvEncGetEncodePresetConfigEx(
                            enc,
                            codec,
                            guid_preset,
                            NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY,
                            &mut preset,
                        )
                    )?;
                    let c = &mut *s.config;
                    *c = preset.presetCfg;
                    c.version = CONFIG_VER;
                    c.profileGUID = perfil;
                    c.gopLength = INFINITE_GOPLENGTH;
                    c.frameIntervalP = 1;
                    self.rc(&mut c.rcParams);
                    let bt709 = |vui: &mut NV_ENC_CONFIG_H264_VUI_PARAMETERS| {
                        vui.videoSignalTypePresentFlag = 1;
                        vui.videoFormat = NV_ENC_VUI_VIDEO_FORMAT_UNSPECIFIED;
                        vui.videoFullRangeFlag = 0;
                        vui.colourDescriptionPresentFlag = 1;
                        vui.colourPrimaries = NV_ENC_VUI_COLOR_PRIMARIES_BT709;
                        vui.transferCharacteristics = NV_ENC_VUI_TRANSFER_CHARACTERISTIC_BT709;
                        vui.colourMatrix = NV_ENC_VUI_MATRIX_COEFFS_BT709;
                    };
                    match qual {
                        CodecVideo::H264 => {
                            let h = &mut c.encodeCodecConfig.h264Config;
                            h.idrPeriod = INFINITE_GOPLENGTH;
                            h.set_repeatSPSPPS(1);
                            h.entropyCodingMode = NV_ENC_H264_ENTROPY_CODING_MODE_CABAC;
                            h.chromaFormatIDC = 1;
                            h.maxNumRefFrames = 1;
                            bt709(&mut h.h264VUIParameters);
                        }
                        CodecVideo::Hevc => {
                            let h = &mut c.encodeCodecConfig.hevcConfig;
                            h.idrPeriod = INFINITE_GOPLENGTH;
                            h.set_repeatSPSPPS(1);
                            h.set_chromaFormatIDC(1);
                            bt709(&mut h.hevcVUIParameters);
                        }
                        CodecVideo::Av1 => {
                            let a = &mut c.encodeCodecConfig.av1Config;
                            a.idrPeriod = INFINITE_GOPLENGTH;
                            a.set_repeatSeqHdr(1);
                            a.set_chromaFormatIDC(1);
                            a.colorPrimaries = NV_ENC_VUI_COLOR_PRIMARIES_BT709;
                            a.transferCharacteristics = NV_ENC_VUI_TRANSFER_CHARACTERISTIC_BT709;
                            a.matrixCoefficients = NV_ENC_VUI_MATRIX_COEFFS_BT709;
                            a.colorRange = 0;
                        }
                    }

                    let i = &mut s.init;
                    i.version = INITIALIZE_PARAMS_VER;
                    i.encodeGUID = codec;
                    i.presetGUID = guid_preset;
                    i.tuningInfo = NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY;
                    i.encodeWidth = largura;
                    i.encodeHeight = altura;
                    i.darWidth = largura;
                    i.darHeight = altura;
                    i.maxEncodeWidth = largura;
                    i.maxEncodeHeight = altura;
                    i.frameRateNum = fps;
                    i.frameRateDen = 1;
                    i.enablePTD = 1;
                    i.encodeConfig = &mut *s.config;
                    chamar!(api, nvEncInitializeEncoder(enc, &mut s.init))?;

                    let mut ent: NV_ENC_CREATE_INPUT_BUFFER = std::mem::zeroed();
                    ent.version = CREATE_INPUT_BUFFER_VER;
                    ent.width = largura;
                    ent.height = altura;
                    ent.memoryHeap = NV_ENC_MEMORY_HEAP_AUTOSELECT;
                    ent.bufferFmt = formato;
                    chamar!(api, nvEncCreateInputBuffer(enc, &mut ent))?;
                    s.entrada = ent.inputBuffer;

                    let mut sai: NV_ENC_CREATE_BITSTREAM_BUFFER = std::mem::zeroed();
                    sai.version = CREATE_BITSTREAM_BUFFER_VER;
                    chamar!(api, nvEncCreateBitstreamBuffer(enc, &mut sai))?;
                    s.saida = sai.bitstreamBuffer;
                    Ok(())
                })();
                sessao = Some(s);
                r
            }
        });
        // Guarda a sessão mesmo com erro: `fechar` solta o que chegou a ser criado.
        self.sessao = sessao;
        match r {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => {
                self.fechar();
                Err(e)
            }
            Err(e) => {
                self.fechar();
                Err(ErroCodec::Backend(format!("nvenc: {e}")))
            }
        }
    }
}

impl Drop for EncoderNvenc {
    fn drop(&mut self) {
        self.fechar();
    }
}

impl VideoEncoder for EncoderNvenc {
    fn nome(&self) -> &'static str {
        "nvenc"
    }

    fn codec(&self) -> CodecVideo {
        self.codec
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
        // H.264 4:2:0 pede tamanho par.
        let (l, a) = (quadro.largura & !1, quadro.altura & !1);
        // Byte a byte: ARGB do NVENC é B,G,R,A na memória (BGRx); ABGR é R,G,B,A (RGBx).
        let formato = match quadro.ordem {
            OrdemRgb::Bgrx => NV_ENC_BUFFER_FORMAT_ARGB,
            OrdemRgb::Rgbx => NV_ENC_BUFFER_FORMAT_ABGR,
        };
        let reabrir = self
            .sessao
            .as_ref()
            .is_none_or(|s| (s.largura, s.altura, s.formato) != (l, a, formato));
        let mut chave = forcar_chave;
        if reabrir {
            self.abrir(l, a, formato)?;
            chave = true;
        }
        let api = self.api;
        let s = self.sessao.as_ref().expect("aberta acima");
        let idx = self.quadro;
        self.quadro += 1;
        let r = self.ctx.com(|| -> Result<Option<TipoQuadro>, ErroCodec> {
            // SAFETY: estruturas POD zeradas com a versão certa; `entrada`/`saida` são desta
            // sessão; o ponteiro do lock vale até o unlock, com `pitch` bytes por linha e
            // `altura` linhas (o buffer foi criado com `l`×`a`), e cada linha copiada tem
            // `l * 4` bytes, dentro da linha de `quadro` (que é válido).
            unsafe {
                let mut lock: NV_ENC_LOCK_INPUT_BUFFER = std::mem::zeroed();
                lock.version = LOCK_INPUT_BUFFER_VER;
                lock.inputBuffer = s.entrada;
                chamar!(api, nvEncLockInputBuffer(s.enc, &mut lock))?;
                let pitch = lock.pitch as usize;
                let destino = lock.bufferDataPtr.cast::<u8>();
                let linha = l as usize * 4;
                for y in 0..a as usize {
                    std::ptr::copy_nonoverlapping(
                        quadro.dados.as_ptr().add(y * quadro.stride),
                        destino.add(y * pitch),
                        linha,
                    );
                }
                chamar!(api, nvEncUnlockInputBuffer(s.enc, s.entrada))?;

                let mut pic: NV_ENC_PIC_PARAMS = std::mem::zeroed();
                pic.version = PIC_PARAMS_VER;
                pic.inputWidth = l;
                pic.inputHeight = a;
                pic.inputPitch = lock.pitch;
                pic.inputBuffer = s.entrada;
                pic.outputBitstream = s.saida;
                pic.bufferFmt = formato;
                pic.pictureStruct = NV_ENC_PIC_STRUCT_FRAME;
                pic.inputTimeStamp = idx;
                pic.frameIdx = idx as u32;
                if chave {
                    pic.encodePicFlags = NV_ENC_PIC_FLAG_FORCEIDR | NV_ENC_PIC_FLAG_OUTPUT_SPSPPS;
                }
                chamar!(api, nvEncEncodePicture(s.enc, &mut pic))?;

                let mut bits: NV_ENC_LOCK_BITSTREAM = std::mem::zeroed();
                bits.version = LOCK_BITSTREAM_VER;
                bits.outputBitstream = s.saida;
                chamar!(api, nvEncLockBitstream(s.enc, &mut bits))?;
                saida.extend_from_slice(std::slice::from_raw_parts(
                    bits.bitstreamBufferPtr.cast::<u8>(),
                    bits.bitstreamSizeInBytes as usize,
                ));
                let tipo = if bits.pictureType == NV_ENC_PIC_TYPE_IDR {
                    TipoQuadro::Chave
                } else {
                    TipoQuadro::Delta
                };
                chamar!(api, nvEncUnlockBitstream(s.enc, s.saida))?;
                Ok((!saida.is_empty()).then_some(tipo))
            }
        });
        match r {
            Ok(r) => r,
            Err(e) => Err(ErroCodec::Backend(format!("nvenc: {e}"))),
        }
    }

    fn ajustar_bitrate(&mut self, bps: u32) {
        if bps == self.config.bitrate_bps {
            return;
        }
        self.config.bitrate_bps = bps;
        let api = self.api;
        let Some(s) = self.sessao.as_mut() else {
            return;
        };
        let mut rc = s.config.rcParams;
        let fps = self.config.fps_max.max(1.0);
        rc.averageBitRate = bps.max(100_000);
        rc.maxBitRate = rc.averageBitRate;
        rc.vbvBufferSize = (rc.averageBitRate as f32 / fps * QUADROS_VBV) as u32;
        rc.vbvInitialDelay = rc.vbvBufferSize;
        s.config.rcParams = rc;
        let ok = self.ctx.com(|| {
            // SAFETY: reconfiguração com os mesmos parâmetros de init (que apontam para a
            // config desta sessão, já com o RC novo), sem reset nem IDR.
            unsafe {
                let mut re: NV_ENC_RECONFIGURE_PARAMS = std::mem::zeroed();
                re.version = RECONFIGURE_PARAMS_VER;
                re.reInitEncodeParams = s.init;
                re.reInitEncodeParams.encodeConfig = &mut *s.config;
                api.f
                    .nvEncReconfigureEncoder
                    .is_some_and(|f| f(s.enc, &mut re) == NV_ENC_SUCCESS)
            }
        });
        if !matches!(ok, Ok(true)) {
            // Sem reconfigurar, a próxima sessão já abre com a taxa nova.
            self.fechar();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DecoderOpenH264, VideoDecoder};

    /// Codifica cores sólidas no NVENC e confere no openh264 que saem em BT.709 limitado.
    /// Sem GPU NVIDIA o teste não tem o que testar e passa.
    #[test]
    fn cores_saem_em_bt709_no_openh264() {
        let Some((dev, nome, _)) = gpus().into_iter().next() else {
            eprintln!("sem NVENC: pulando");
            return;
        };
        let mut enc = EncoderNvenc::novo(
            dev,
            nome,
            CodecVideo::H264,
            ConfigEncoder {
                bitrate_bps: 8_000_000,
                fps_max: 60.0,
            },
        )
        .unwrap();
        let mut dec = DecoderOpenH264::novo().unwrap();
        let (l, a) = (320u32, 240u32);
        // (R, G, B) → (Y, U, V) em BT.709 limitado.
        let casos = [
            ((255, 255, 255), (235, 128, 128)),
            ((255, 0, 0), (63, 102, 240)),
            ((0, 255, 0), (173, 42, 26)),
            ((0, 0, 255), (32, 240, 118)),
        ];
        let mut au = Vec::new();
        for (i, ((r, g, b), esperado)) in casos.into_iter().enumerate() {
            let img: Vec<u8> = [b, g, r, 0]
                .into_iter()
                .cycle()
                .take((l * a * 4) as usize)
                .collect();
            let q = QuadroRgb {
                largura: l,
                altura: a,
                stride: l as usize * 4,
                ordem: OrdemRgb::Bgrx,
                dados: &img,
            };
            let tipo = enc.codificar(&q, true, &mut au).unwrap();
            assert_eq!(tipo, Some(TipoQuadro::Chave), "caso {i}");
            let mut visto = None;
            dec.decodificar(&au, &mut |y| {
                let c = (l as usize / 2) / 2 + (a as usize / 4) * y.planos[1].passo as usize;
                visto = Some((
                    y.planos[0].dados
                        [(a as usize / 2) * y.planos[0].passo as usize + l as usize / 2],
                    y.planos[1].dados[c],
                    y.planos[2].dados[c],
                ));
            })
            .unwrap();
            let (vy, vu, vv) = visto.expect("decodificou");
            let (ey, eu, ev): (u8, u8, u8) = esperado;
            assert!(
                vy.abs_diff(ey) <= 3 && vu.abs_diff(eu) <= 3 && vv.abs_diff(ev) <= 3,
                "caso {i}: YUV ({vy}, {vu}, {vv}), esperado ({ey}, {eu}, {ev})"
            );
        }
    }

    /// HEVC (e AV1, se a GPU tiver) do NVENC decodificado no NVDEC.
    #[test]
    fn hevc_e_av1_ida_e_volta_no_nvdec() {
        use crate::linux::nvdec::{self, DecoderNvdec};
        let Some((dev, nome, _)) = gpus().into_iter().next() else {
            eprintln!("sem NVENC: pulando");
            return;
        };
        for codec in [CodecVideo::Hevc, CodecVideo::Av1] {
            if !codecs(dev).contains(&codec) || !nvdec::codecs(dev).contains(&codec) {
                eprintln!("{codec:?} sem encode ou decode nesta GPU: pulando");
                continue;
            }
            let mut enc = EncoderNvenc::novo(
                dev,
                nome.clone(),
                codec,
                ConfigEncoder {
                    bitrate_bps: 8_000_000,
                    fps_max: 60.0,
                },
            )
            .unwrap();
            let mut dec = DecoderNvdec::novo(dev, nome.clone(), codec).unwrap();
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
                enc.codificar(&q, i == 0, &mut au).unwrap();
                dec.decodificar(&au, &mut |y| {
                    assert_eq!((y.largura, y.altura), (l, a));
                    let esperado = 16 + i32::from(v) * 219 / 255;
                    let visto = i32::from(y.planos[0].dados[(a as usize / 2) * l as usize + 7]);
                    assert!(
                        (visto - esperado).abs() <= 3,
                        "{codec:?} {i}: {visto} vs {esperado}"
                    );
                    vistos += 1;
                })
                .unwrap();
            }
            assert!(vistos >= 9, "{codec:?}: {vistos}");
        }
    }
}
