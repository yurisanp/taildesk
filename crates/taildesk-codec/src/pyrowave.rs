//! PyroWave: codec wavelet intra-only do Themaister (MIT), todo em compute Vulkan. Codifica e
//! decodifica 1080p em décimos de milissegundo, ao custo de taxas altas (centenas de Mbit/s):
//! serve para rede local, onde a latência importa mais que a banda.
//!
//! A biblioteca (`libpyrowave-shared.so.1` / `libpyrowave-shared-1.dll`) é aberta em tempo de
//! execução, de junto do executável, do AppImage ou do sistema; sem ela, ou sem Vulkan 1.3, o
//! codec some da negociação. Uso pelo caminho de CPU da API C: a imagem entra e sai em YUV 4:2:0
//! planar (o mesmo I420 BT.709 limitado dos outros codecs) e a GPU faz o resto.
//!
//! Access unit do TailDesk: `"PW"`, largura e altura (u16 LE cada) e os pacotes do PyroWave,
//! cada um com o tamanho em u32 LE na frente. Todo quadro é independente (sempre "chave").

use std::ffi::{c_int, c_void};
use std::path::PathBuf;
use std::sync::OnceLock;

use crate::cor::I420;
use crate::{
    CodecVideo, ConfigEncoder, ErroCodec, Faixa, FormatoYuv, Matriz, PlanoYuv, QuadroRgb,
    QuadroYuv, TipoQuadro, VideoDecoder, VideoEncoder,
};

type Resultado = c_int;
const SUCESSO: Resultado = 0;
const CHROMA_420: c_int = 0;
const CPU_YUV420P: c_int = 1;
/// Tamanho de cada pacote do PyroWave dentro do access unit.
const PACOTE: usize = 16 * 1024;
const MAGICO: [u8; 2] = *b"PW";

#[repr(C)]
struct InfoEncoder {
    device: *mut c_void,
    width: c_int,
    height: c_int,
    chroma: c_int,
}

#[repr(C)]
struct InfoDecoder {
    device: *mut c_void,
    width: c_int,
    height: c_int,
    chroma: c_int,
    fragment_path: bool,
}

#[repr(C)]
struct BufferCpu {
    data: [*mut c_void; 3],
    row_stride_in_bytes: [usize; 3],
    plane_size_in_bytes: [usize; 3],
    width: c_int,
    height: c_int,
    format: c_int,
}

#[repr(C)]
struct ControleTaxa {
    maximum_bitstream_size: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Pacote {
    offset: usize,
    size: usize,
}

/// Funções da API C usadas aqui (pyrowave.h, API 1.x).
struct Api {
    _lib: libloading::Library,
    criar_device: unsafe extern "C" fn(*mut *mut c_void) -> Resultado,
    destruir_device: unsafe extern "C" fn(*mut c_void),
    criar_encoder: unsafe extern "C" fn(*const InfoEncoder, *mut *mut c_void) -> Resultado,
    codificar_cpu:
        unsafe extern "C" fn(*mut c_void, *const BufferCpu, *const ControleTaxa) -> Resultado,
    contar_pacotes: unsafe extern "C" fn(*mut c_void, usize, *mut usize) -> Resultado,
    empacotar: unsafe extern "C" fn(
        *mut c_void,
        *mut Pacote,
        usize,
        *mut usize,
        *mut c_void,
        usize,
    ) -> Resultado,
    destruir_encoder: unsafe extern "C" fn(*mut c_void),
    criar_decoder: unsafe extern "C" fn(*const InfoDecoder, *mut *mut c_void) -> Resultado,
    empurrar: unsafe extern "C" fn(*mut c_void, *const c_void, usize) -> Resultado,
    pronto: unsafe extern "C" fn(*mut c_void, bool) -> bool,
    decodificar_cpu: unsafe extern "C" fn(*mut c_void, *const BufferCpu) -> Resultado,
    destruir_decoder: unsafe extern "C" fn(*mut c_void),
}

/// Nomes do arquivo da biblioteca (o CMake do PyroWave põe a versão no nome da DLL).
const BIBLIOTECAS: &[&str] = if cfg!(windows) {
    &["libpyrowave-shared-1.dll", "libpyrowave-shared.dll"]
} else {
    &["libpyrowave-shared.so.1"]
};

/// Onde procurar a biblioteca: `TAILDESK_PYROWAVE_DIR`, `../lib` a partir do executável
/// (tar.gz e AppImage), junto do executável (zip do Windows), `$APPDIR/usr/lib` e o sistema.
fn caminhos() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(dir) = std::env::var_os("TAILDESK_PYROWAVE_DIR") {
        dirs.push(PathBuf::from(dir));
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(PathBuf::from))
    {
        dirs.push(dir.join("../lib"));
        dirs.push(dir);
    }
    if let Some(app) = std::env::var_os("APPDIR") {
        dirs.push(PathBuf::from(app).join("usr/lib"));
    }
    let mut c: Vec<PathBuf> = dirs
        .iter()
        .flat_map(|d| BIBLIOTECAS.iter().map(move |b| d.join(b)))
        .collect();
    c.extend(BIBLIOTECAS.iter().map(PathBuf::from));
    c
}

fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(|| {
        // SAFETY: a biblioteca do PyroWave só roda construtores ao carregar; cada símbolo é lido
        // com a assinatura do pyrowave.h, e a versão da API é conferida antes de usar.
        unsafe {
            let lib = caminhos()
                .into_iter()
                .find_map(|p| libloading::Library::new(p).ok())?;
            let versao: libloading::Symbol<unsafe extern "C" fn(*mut u32, *mut u32, *mut u32)> =
                lib.get(b"pyrowave_get_api_version\0").ok()?;
            let (mut maior, mut menor, mut patch) = (0, 0, 0);
            versao(&mut maior, &mut menor, &mut patch);
            if maior != 1 {
                return None;
            }
            Some(Api {
                criar_device: *lib.get(b"pyrowave_create_default_device\0").ok()?,
                destruir_device: *lib.get(b"pyrowave_device_destroy\0").ok()?,
                criar_encoder: *lib.get(b"pyrowave_encoder_create\0").ok()?,
                codificar_cpu: *lib.get(b"pyrowave_encoder_encode_cpu\0").ok()?,
                contar_pacotes: *lib.get(b"pyrowave_encoder_compute_num_packets\0").ok()?,
                empacotar: *lib.get(b"pyrowave_encoder_packetize\0").ok()?,
                destruir_encoder: *lib.get(b"pyrowave_encoder_destroy\0").ok()?,
                criar_decoder: *lib.get(b"pyrowave_decoder_create\0").ok()?,
                empurrar: *lib.get(b"pyrowave_decoder_push_packet\0").ok()?,
                pronto: *lib.get(b"pyrowave_decoder_decode_is_ready\0").ok()?,
                decodificar_cpu: *lib
                    .get(b"pyrowave_decoder_decode_cpu_buffer_synchronous\0")
                    .ok()?,
                destruir_decoder: *lib.get(b"pyrowave_decoder_destroy\0").ok()?,
                _lib: lib,
            })
        }
    })
    .as_ref()
}

fn erro(o_que: &str, r: Resultado) -> ErroCodec {
    ErroCodec::Backend(format!("pyrowave: {o_que}: {r}"))
}

/// `true` se a biblioteca carrega e cria um device Vulkan (testado uma vez).
pub fn disponivel() -> bool {
    static OK: OnceLock<bool> = OnceLock::new();
    *OK.get_or_init(|| Device::novo().is_ok())
}

/// Device Vulkan do PyroWave (o padrão dele: a primeira GPU com Vulkan 1.3 e os recursos que ele
/// pede).
struct Device {
    api: &'static Api,
    d: *mut c_void,
}

impl Device {
    fn novo() -> Result<Self, ErroCodec> {
        let api = api().ok_or_else(|| ErroCodec::Backend("pyrowave: sem biblioteca".into()))?;
        let mut d = std::ptr::null_mut();
        // SAFETY: `d` recebe o device criado.
        let r = unsafe { (api.criar_device)(&mut d) };
        if r != SUCESSO || d.is_null() {
            return Err(erro("create_default_device", r));
        }
        Ok(Device { api, d })
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        // SAFETY: device criado em `novo`; encoder e decoder que o usam já foram destruídos
        // (são soltos antes, ver a ordem dos campos).
        unsafe { (self.api.destruir_device)(self.d) };
    }
}

fn buffer_i420(y: &mut I420) -> BufferCpu {
    let (l, a) = (y.largura as usize, y.altura as usize);
    BufferCpu {
        data: [
            y.y.as_mut_ptr().cast(),
            y.u.as_mut_ptr().cast(),
            y.v.as_mut_ptr().cast(),
        ],
        row_stride_in_bytes: [l, l / 2, l / 2],
        plane_size_in_bytes: [l * a, l / 2 * (a / 2), l / 2 * (a / 2)],
        width: l as c_int,
        height: a as c_int,
        format: CPU_YUV420P,
    }
}

/// Encoder PyroWave.
pub struct EncoderPyroWave {
    // Ordem dos campos = ordem de drop: o encoder antes do device.
    encoder: *mut c_void,
    device: Device,
    config: ConfigEncoder,
    tamanho: (u32, u32),
    yuv: I420,
    pacotes: Vec<Pacote>,
    bits: Vec<u8>,
}

// SAFETY: encoder e device são usados por uma thread de cada vez (`&mut self`); a API só pede
// que as chamadas não sejam simultâneas.
unsafe impl Send for EncoderPyroWave {}

impl std::fmt::Debug for EncoderPyroWave {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EncoderPyroWave")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl EncoderPyroWave {
    pub fn novo(config: ConfigEncoder) -> Result<Self, ErroCodec> {
        Ok(EncoderPyroWave {
            encoder: std::ptr::null_mut(),
            device: Device::novo()?,
            config,
            tamanho: (0, 0),
            yuv: I420::default(),
            pacotes: Vec::new(),
            bits: Vec::new(),
        })
    }

    fn fechar(&mut self) {
        if !self.encoder.is_null() {
            // SAFETY: encoder criado em `abrir` e ainda não destruído.
            unsafe { (self.device.api.destruir_encoder)(self.encoder) };
            self.encoder = std::ptr::null_mut();
        }
    }

    fn abrir(&mut self, l: u32, a: u32) -> Result<(), ErroCodec> {
        self.fechar();
        let info = InfoEncoder {
            device: self.device.d,
            width: l as c_int,
            height: a as c_int,
            chroma: CHROMA_420,
        };
        let mut e = std::ptr::null_mut();
        // SAFETY: `info` válido durante a chamada; `e` recebe o encoder.
        let r = unsafe { (self.device.api.criar_encoder)(&info, &mut e) };
        if r != SUCESSO {
            return Err(erro("encoder_create", r));
        }
        self.encoder = e;
        self.tamanho = (l, a);
        Ok(())
    }
}

impl Drop for EncoderPyroWave {
    fn drop(&mut self) {
        self.fechar();
    }
}

impl VideoEncoder for EncoderPyroWave {
    fn nome(&self) -> &'static str {
        "pyrowave"
    }

    fn gpu(&self) -> Option<&str> {
        Some("Vulkan")
    }

    fn codec(&self) -> CodecVideo {
        CodecVideo::PyroWave
    }

    fn codificar(
        &mut self,
        quadro: &QuadroRgb<'_>,
        _forcar_chave: bool,
        saida: &mut Vec<u8>,
    ) -> Result<Option<TipoQuadro>, ErroCodec> {
        saida.clear();
        if !quadro.valido() {
            return Err(ErroCodec::QuadroInvalido);
        }
        let (l, a) = (quadro.largura & !1, quadro.altura & !1);
        if l > u32::from(u16::MAX) || a > u32::from(u16::MAX) {
            return Err(ErroCodec::QuadroInvalido);
        }
        if self.encoder.is_null() || self.tamanho != (l, a) {
            self.abrir(l, a)?;
        }
        self.yuv.preparar(l, a);
        self.yuv.converter(quadro);
        let buffer = buffer_i420(&mut self.yuv);
        let taxa = ControleTaxa {
            maximum_bitstream_size: (f64::from(self.config.bitrate_bps)
                / 8.0
                / f64::from(self.config.fps_max.max(1.0)))
                as usize,
        };
        let api = self.device.api;
        let mut n = 0usize;
        // SAFETY: o buffer aponta para os planos de `self.yuv` (vivos durante a chamada, com os
        // tamanhos declarados); a saída do empacotamento cabe em `pacotes`/`bits`, dimensionados
        // pelo `compute_num_packets` e pelo teto de taxa (mais a folga de cabeçalhos).
        unsafe {
            let r = (api.codificar_cpu)(self.encoder, &buffer, &taxa);
            if r != SUCESSO {
                return Err(erro("encode_cpu", r));
            }
            let r = (api.contar_pacotes)(self.encoder, PACOTE, &mut n);
            if r != SUCESSO {
                return Err(erro("compute_num_packets", r));
            }
            self.pacotes.resize(n.max(1), Pacote::default());
            self.bits
                .resize(taxa.maximum_bitstream_size + n * PACOTE + 4096, 0);
            let r = (api.empacotar)(
                self.encoder,
                self.pacotes.as_mut_ptr(),
                PACOTE,
                &mut n,
                self.bits.as_mut_ptr().cast(),
                self.bits.len(),
            );
            if r != SUCESSO {
                return Err(erro("packetize", r));
            }
        }
        saida.extend_from_slice(&MAGICO);
        saida.extend_from_slice(&(l as u16).to_le_bytes());
        saida.extend_from_slice(&(a as u16).to_le_bytes());
        for p in &self.pacotes[..n.min(self.pacotes.len())] {
            let Some(dados) = self.bits.get(p.offset..p.offset + p.size) else {
                return Err(ErroCodec::Backend(
                    "pyrowave: pacote fora do bitstream".into(),
                ));
            };
            saida.extend_from_slice(&(p.size as u32).to_le_bytes());
            saida.extend_from_slice(dados);
        }
        Ok(Some(TipoQuadro::Chave))
    }

    fn ajustar_bitrate(&mut self, bps: u32) {
        self.config.bitrate_bps = bps;
    }
}

/// Decoder PyroWave.
pub struct DecoderPyroWave {
    // Ordem dos campos = ordem de drop: o decoder antes do device.
    decoder: *mut c_void,
    device: Device,
    tamanho: (u32, u32),
    yuv: I420,
}

// SAFETY: ver `EncoderPyroWave`.
unsafe impl Send for DecoderPyroWave {}

impl std::fmt::Debug for DecoderPyroWave {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecoderPyroWave")
            .field("tamanho", &self.tamanho)
            .finish_non_exhaustive()
    }
}

impl DecoderPyroWave {
    pub fn novo() -> Result<Self, ErroCodec> {
        Ok(DecoderPyroWave {
            decoder: std::ptr::null_mut(),
            device: Device::novo()?,
            tamanho: (0, 0),
            yuv: I420::default(),
        })
    }

    fn fechar(&mut self) {
        if !self.decoder.is_null() {
            // SAFETY: decoder criado em `abrir` e ainda não destruído.
            unsafe { (self.device.api.destruir_decoder)(self.decoder) };
            self.decoder = std::ptr::null_mut();
        }
    }

    fn abrir(&mut self, l: u32, a: u32) -> Result<(), ErroCodec> {
        self.fechar();
        let info = InfoDecoder {
            device: self.device.d,
            width: l as c_int,
            height: a as c_int,
            chroma: CHROMA_420,
            fragment_path: false,
        };
        let mut d = std::ptr::null_mut();
        // SAFETY: `info` válido durante a chamada; `d` recebe o decoder.
        let r = unsafe { (self.device.api.criar_decoder)(&info, &mut d) };
        if r != SUCESSO {
            return Err(erro("decoder_create", r));
        }
        self.decoder = d;
        self.tamanho = (l, a);
        self.yuv.preparar(l, a);
        Ok(())
    }
}

impl Drop for DecoderPyroWave {
    fn drop(&mut self) {
        self.fechar();
    }
}

impl VideoDecoder for DecoderPyroWave {
    fn nome(&self) -> &'static str {
        "pyrowave"
    }

    fn gpu(&self) -> Option<&str> {
        Some("Vulkan")
    }

    fn decodificar(
        &mut self,
        access_unit: &[u8],
        ao_quadro: &mut dyn FnMut(&QuadroYuv<'_>),
    ) -> Result<bool, ErroCodec> {
        let invalido = || ErroCodec::Backend("pyrowave: access unit inválido".into());
        if access_unit.len() < 6 || access_unit[..2] != MAGICO {
            return Err(invalido());
        }
        let l = u32::from(u16::from_le_bytes([access_unit[2], access_unit[3]]));
        let a = u32::from(u16::from_le_bytes([access_unit[4], access_unit[5]]));
        if l < 2 || a < 2 || l % 2 != 0 || a % 2 != 0 {
            return Err(invalido());
        }
        if self.decoder.is_null() || self.tamanho != (l, a) {
            self.abrir(l, a)?;
        }
        let api = self.device.api;
        let mut resto = &access_unit[6..];
        while !resto.is_empty() {
            let tam = resto
                .get(..4)
                .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
                .ok_or_else(invalido)?;
            let pacote = resto.get(4..4 + tam).ok_or_else(invalido)?;
            // SAFETY: decoder criado acima; o pacote é lido só durante a chamada.
            let r = unsafe { (api.empurrar)(self.decoder, pacote.as_ptr().cast(), pacote.len()) };
            if r != SUCESSO {
                return Err(erro("push_packet", r));
            }
            resto = &resto[4 + tam..];
        }
        // SAFETY: decoder criado acima.
        if !unsafe { (api.pronto)(self.decoder, false) } {
            return Ok(false);
        }
        let buffer = buffer_i420(&mut self.yuv);
        // SAFETY: os planos de `self.yuv` têm o tamanho do decoder (preparados em `abrir`) e
        // ficam vivos durante a chamada síncrona.
        let r = unsafe { (api.decodificar_cpu)(self.decoder, &buffer) };
        if r != SUCESSO {
            return Err(erro("decode_cpu", r));
        }
        let y = &self.yuv;
        ao_quadro(&QuadroYuv {
            largura: l,
            altura: a,
            formato: FormatoYuv::I420,
            planos: [
                PlanoYuv {
                    dados: &y.y,
                    passo: l,
                },
                PlanoYuv {
                    dados: &y.u,
                    passo: l / 2,
                },
                PlanoYuv {
                    dados: &y.v,
                    passo: l / 2,
                },
            ],
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

    /// Ida e volta pelo PyroWave. Sem a biblioteca (CI) o teste não tem o que testar e passa.
    #[test]
    fn ida_e_volta() {
        if !disponivel() {
            eprintln!("sem PyroWave: pulando");
            return;
        }
        let (l, a) = (640u32, 360u32);
        let mut enc = EncoderPyroWave::novo(ConfigEncoder {
            bitrate_bps: 200_000_000,
            fps_max: 60.0,
        })
        .unwrap();
        let mut dec = DecoderPyroWave::novo().unwrap();
        let mut au = Vec::new();
        for i in 0..5u32 {
            let v = (i * 40 + 30) as u8;
            let img = vec![v; (l * a * 4) as usize];
            let q = QuadroRgb {
                largura: l,
                altura: a,
                stride: l as usize * 4,
                ordem: OrdemRgb::Bgrx,
                dados: &img,
            };
            assert_eq!(
                enc.codificar(&q, false, &mut au).unwrap(),
                Some(TipoQuadro::Chave)
            );
            let mut visto = None;
            assert!(
                dec.decodificar(&au, &mut |y| {
                    visto = Some(y.planos[0].dados[(a as usize / 2) * l as usize + 9]);
                })
                .unwrap()
            );
            let esperado = 16 + i32::from(v) * 219 / 255;
            let visto = i32::from(visto.unwrap());
            assert!((visto - esperado).abs() <= 3, "{i}: {visto} vs {esperado}");
        }
    }
}
