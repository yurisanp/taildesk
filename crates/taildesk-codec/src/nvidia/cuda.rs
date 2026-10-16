//! O pedaço da API de driver do CUDA (`libcuda.so.1`, vem com o driver da NVIDIA) que o NVENC e o
//! NVDEC pedem: um contexto por GPU e cópias entre a memória da GPU e a do processo.
//!
//! Carregada em tempo de execução: sem driver da NVIDIA, [`Cuda::carregar`] devolve `None` e os
//! backends NVIDIA ficam de fora.

use std::ffi::{c_char, c_int, c_uint, c_void};
use std::sync::OnceLock;

pub type CuResult = c_int;
pub type CuDevice = c_int;
pub type CuContext = *mut c_void;
pub type CuDevicePtr = u64;

const CUDA_SUCCESS: CuResult = 0;
const CU_MEMORYTYPE_HOST: c_uint = 1;
const CU_MEMORYTYPE_DEVICE: c_uint = 2;

/// `CUDA_MEMCPY2D` (cuda.h).
#[repr(C)]
#[derive(Debug)]
pub struct Memcpy2d {
    pub src_x_in_bytes: usize,
    pub src_y: usize,
    pub src_memory_type: c_uint,
    pub src_host: *const c_void,
    pub src_device: CuDevicePtr,
    pub src_array: *mut c_void,
    pub src_pitch: usize,
    pub dst_x_in_bytes: usize,
    pub dst_y: usize,
    pub dst_memory_type: c_uint,
    pub dst_host: *mut c_void,
    pub dst_device: CuDevicePtr,
    pub dst_array: *mut c_void,
    pub dst_pitch: usize,
    pub width_in_bytes: usize,
    pub height: usize,
}

/// Funções do `libcuda` usadas aqui.
pub struct Cuda {
    _lib: libloading::Library,
    init: unsafe extern "C" fn(c_uint) -> CuResult,
    device_get_count: unsafe extern "C" fn(*mut c_int) -> CuResult,
    device_get: unsafe extern "C" fn(*mut CuDevice, c_int) -> CuResult,
    device_get_name: unsafe extern "C" fn(*mut c_char, c_int, CuDevice) -> CuResult,
    device_get_pci_bus_id: unsafe extern "C" fn(*mut c_char, c_int, CuDevice) -> CuResult,
    ctx_create: unsafe extern "C" fn(*mut CuContext, c_uint, CuDevice) -> CuResult,
    ctx_destroy: unsafe extern "C" fn(CuContext) -> CuResult,
    ctx_push: unsafe extern "C" fn(CuContext) -> CuResult,
    ctx_pop: unsafe extern "C" fn(*mut CuContext) -> CuResult,
    memcpy_2d: unsafe extern "C" fn(*const Memcpy2d) -> CuResult,
}

impl std::fmt::Debug for Cuda {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cuda").finish_non_exhaustive()
    }
}

fn ok(r: CuResult, o_que: &str) -> Result<(), String> {
    if r == CUDA_SUCCESS {
        Ok(())
    } else {
        Err(format!("{o_que}: CUresult {r}"))
    }
}

impl Cuda {
    /// O `libcuda` do sistema, inicializado uma vez por processo. `None` sem driver da NVIDIA.
    pub fn carregar() -> Option<&'static Cuda> {
        static CUDA: OnceLock<Option<Cuda>> = OnceLock::new();
        CUDA.get_or_init(|| {
            // SAFETY: o `libcuda.so.1` é a biblioteca do driver; carregá-la só roda os
            // construtores dela. Cada símbolo é lido com a assinatura de cuda.h (API v2).
            let cuda = unsafe {
                let lib = libloading::Library::new(super::biblioteca("libcuda.so.1", "nvcuda.dll"))
                    .ok()?;
                Cuda {
                    init: *lib.get(b"cuInit\0").ok()?,
                    device_get_count: *lib.get(b"cuDeviceGetCount\0").ok()?,
                    device_get: *lib.get(b"cuDeviceGet\0").ok()?,
                    device_get_name: *lib.get(b"cuDeviceGetName\0").ok()?,
                    device_get_pci_bus_id: *lib.get(b"cuDeviceGetPCIBusId\0").ok()?,
                    ctx_create: *lib.get(b"cuCtxCreate_v2\0").ok()?,
                    ctx_destroy: *lib.get(b"cuCtxDestroy_v2\0").ok()?,
                    ctx_push: *lib.get(b"cuCtxPushCurrent_v2\0").ok()?,
                    ctx_pop: *lib.get(b"cuCtxPopCurrent_v2\0").ok()?,
                    memcpy_2d: *lib.get(b"cuMemcpy2D_v2\0").ok()?,
                    _lib: lib,
                }
            };
            // SAFETY: `cuInit(0)` é a primeira chamada pedida pela API e pode repetir.
            ok(unsafe { (cuda.init)(0) }, "cuInit").ok()?;
            Some(cuda)
        })
        .as_ref()
    }

    /// GPUs CUDA: índice, nome e endereço PCI ("0000:01:00.0").
    pub fn gpus(&self) -> Vec<(CuDevice, String, String)> {
        let mut n = 0;
        // SAFETY: `n` é um inteiro válido para escrita.
        if unsafe { (self.device_get_count)(&mut n) } != CUDA_SUCCESS {
            return Vec::new();
        }
        (0..n)
            .filter_map(|i| {
                let mut dev = 0;
                let mut nome = [0 as c_char; 128];
                let mut pci = [0 as c_char; 32];
                // SAFETY: saídas válidas, com o tamanho de cada buffer passado junto; o driver
                // termina as strings em NUL dentro do tamanho dado.
                unsafe {
                    ok((self.device_get)(&mut dev, i), "cuDeviceGet").ok()?;
                    ok(
                        (self.device_get_name)(nome.as_mut_ptr(), nome.len() as c_int, dev),
                        "cuDeviceGetName",
                    )
                    .ok()?;
                    let _ = (self.device_get_pci_bus_id)(pci.as_mut_ptr(), pci.len() as c_int, dev);
                }
                Some((dev, texto(&nome), texto(&pci).to_ascii_lowercase()))
            })
            .collect()
    }
}

fn texto(c: &[c_char]) -> String {
    let bytes: Vec<u8> = c
        .iter()
        .take_while(|&&b| b != 0)
        .map(|&b| b as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Contexto CUDA de uma GPU; destruído ao soltar.
#[derive(Debug)]
pub struct Contexto {
    cuda: &'static Cuda,
    pub ctx: CuContext,
}

// SAFETY: um contexto CUDA pode ser usado de qualquer thread (com push/pop); o encoder e o
// decoder que o guardam o usam de uma thread por vez.
unsafe impl Send for Contexto {}

impl Contexto {
    pub fn novo(cuda: &'static Cuda, dev: CuDevice) -> Result<Self, String> {
        let mut ctx = std::ptr::null_mut();
        // SAFETY: `ctx` recebe o contexto criado; `dev` veio de `cuDeviceGet`.
        let r = unsafe { (cuda.ctx_create)(&mut ctx, 0, dev) };
        ok(r, "cuCtxCreate")?;
        // O create deixa o contexto atual nesta thread; quem usa empurra quando precisa.
        let mut atual = std::ptr::null_mut();
        // SAFETY: tira da pilha desta thread o contexto que o create empurrou.
        unsafe { (cuda.ctx_pop)(&mut atual) };
        Ok(Contexto { cuda, ctx })
    }

    /// Roda `f` com este contexto atual na thread.
    pub fn com<T>(&self, f: impl FnOnce() -> T) -> Result<T, String> {
        // SAFETY: `self.ctx` vive até `drop`; push/pop são pareados aqui.
        let r = unsafe { (self.cuda.ctx_push)(self.ctx) };
        ok(r, "cuCtxPushCurrent")?;
        let r = f();
        let mut atual = std::ptr::null_mut();
        // SAFETY: desfaz o push acima.
        unsafe { (self.cuda.ctx_pop)(&mut atual) };
        Ok(r)
    }

    /// Copia `altura` linhas de `largura` bytes da GPU (`src`, passo `src_passo`) para `dst`
    /// (passo `dst_passo`). Precisa rodar dentro de [`Contexto::com`].
    pub fn copiar_para_host(
        &self,
        src: CuDevicePtr,
        src_passo: usize,
        dst: &mut [u8],
        dst_passo: usize,
        largura: usize,
        altura: usize,
    ) -> Result<(), String> {
        if altura == 0 || dst.len() < (altura - 1) * dst_passo + largura {
            return Err("cuMemcpy2D: destino pequeno".into());
        }
        let c = Memcpy2d {
            src_x_in_bytes: 0,
            src_y: 0,
            src_memory_type: CU_MEMORYTYPE_DEVICE,
            src_host: std::ptr::null(),
            src_device: src,
            src_array: std::ptr::null_mut(),
            src_pitch: src_passo,
            dst_x_in_bytes: 0,
            dst_y: 0,
            dst_memory_type: CU_MEMORYTYPE_HOST,
            dst_host: dst.as_mut_ptr().cast(),
            dst_device: 0,
            dst_array: std::ptr::null_mut(),
            dst_pitch: dst_passo,
            width_in_bytes: largura,
            height: altura,
        };
        // SAFETY: o destino cobre `altura` linhas de `largura` bytes com passo `dst_passo`
        // (conferido acima); a origem é uma superfície mapeada pelo chamador.
        ok(unsafe { (self.cuda.memcpy_2d)(&c) }, "cuMemcpy2D")
    }
}

impl Drop for Contexto {
    fn drop(&mut self) {
        // SAFETY: o contexto foi criado em `novo` e ninguém mais o usa.
        unsafe { (self.cuda.ctx_destroy)(self.ctx) };
    }
}
