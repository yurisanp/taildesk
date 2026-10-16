//! VA-API: a `libva` carregada em tempo de execução e um display por nó de render da GPU
//! (`/dev/dri/renderD*`).

use std::ffi::{CStr, c_int};
use std::fs::File;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use super::sys::va::*;
use super::sys::va_drm::LibVaDrm;

/// `libva.so.2` e `libva-drm.so.2`, carregadas uma vez por processo.
pub struct Libs {
    pub va: LibVa,
    pub drm: LibVaDrm,
}

impl std::fmt::Debug for Libs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Libs").finish_non_exhaustive()
    }
}

pub fn libs() -> Option<&'static Libs> {
    static LIBS: OnceLock<Option<Libs>> = OnceLock::new();
    LIBS.get_or_init(|| {
        // SAFETY: as bibliotecas do libva só rodam construtores ao carregar; o bindgen confere
        // que cada símbolo do allowlist existe.
        unsafe {
            Some(Libs {
                va: LibVa::new("libva.so.2").ok()?,
                drm: LibVaDrm::new("libva-drm.so.2").ok()?,
            })
        }
    })
    .as_ref()
}

/// Erro de uma chamada VA.
pub fn checar(libs: &Libs, st: VAStatus, o_que: &str) -> Result<(), String> {
    if st == VA_STATUS_SUCCESS as VAStatus {
        return Ok(());
    }
    // SAFETY: `vaErrorStr` devolve uma string estática para qualquer código.
    let msg = unsafe { CStr::from_ptr(libs.va.vaErrorStr(st)) };
    Err(format!("{o_que}: {}", msg.to_string_lossy()))
}

/// Nós de render das GPUs (`/dev/dri/renderD128`, ...), em ordem.
pub fn nos_de_render() -> Vec<PathBuf> {
    let mut nos: Vec<PathBuf> = std::fs::read_dir("/dev/dri")
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("renderD"))
        })
        .collect();
    nos.sort();
    nos
}

/// Display VA aberto num nó de render; termina ao soltar.
pub struct Display {
    pub libs: &'static Libs,
    pub dpy: VADisplay,
    /// Mantém o fd aberto enquanto o display vive.
    _arquivo: File,
    pub fornecedor: String,
}

// SAFETY: o libva aceita um display usado de várias threads; aqui cada display é de um encoder,
// usado por uma thread de cada vez.
unsafe impl Send for Display {}

impl std::fmt::Debug for Display {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Display")
            .field("fornecedor", &self.fornecedor)
            .finish_non_exhaustive()
    }
}

impl Display {
    pub fn abrir(no: &Path) -> Result<Display, String> {
        let libs = libs().ok_or("sem libva")?;
        let arquivo = File::options()
            .read(true)
            .write(true)
            .open(no)
            .map_err(|e| format!("{}: {e}", no.display()))?;
        // SAFETY: o fd fica aberto (em `_arquivo`) enquanto o display existir.
        let dpy = unsafe { libs.drm.vaGetDisplayDRM(arquivo.as_raw_fd() as c_int) };
        if dpy.is_null() {
            return Err("vaGetDisplayDRM".into());
        }
        // Sem isto o libva escreve "libva info: ..." no stderr a cada display aberto.
        // SAFETY: callbacks nulos desligam as mensagens deste display.
        unsafe {
            libs.va.vaSetInfoCallback(dpy, None, std::ptr::null_mut());
        }
        let (mut maj, mut min) = (0, 0);
        // SAFETY: `dpy` acabou de ser obtido; as saídas são inteiros válidos.
        let st = unsafe { libs.va.vaInitialize(dpy, &mut maj, &mut min) };
        if let Err(e) = checar(libs, st, "vaInitialize") {
            // SAFETY: termina o display que não chegou a inicializar.
            unsafe { libs.va.vaTerminate(dpy) };
            return Err(e);
        }
        // SAFETY: display inicializado; a string vive enquanto ele viver e é copiada já.
        let fornecedor = unsafe {
            let s = libs.va.vaQueryVendorString(dpy);
            if s.is_null() {
                String::new()
            } else {
                CStr::from_ptr(s).to_string_lossy().into_owned()
            }
        };
        Ok(Display {
            libs,
            dpy,
            _arquivo: arquivo,
            fornecedor,
        })
    }

    pub fn checar(&self, st: VAStatus, o_que: &str) -> Result<(), String> {
        checar(self.libs, st, o_que)
    }

    /// Entrypoints do perfil.
    pub fn entrypoints(&self, perfil: VAProfile) -> Vec<VAEntrypoint> {
        // SAFETY: display inicializado.
        let max = unsafe { self.libs.va.vaMaxNumEntrypoints(self.dpy) }.max(0) as usize;
        let mut lista = vec![0 as VAEntrypoint; max];
        let mut n = 0;
        // SAFETY: `lista` tem `vaMaxNumEntrypoints` posições, o máximo que a chamada escreve.
        let st = unsafe {
            self.libs
                .va
                .vaQueryConfigEntrypoints(self.dpy, perfil, lista.as_mut_ptr(), &mut n)
        };
        if st != VA_STATUS_SUCCESS as VAStatus {
            return Vec::new();
        }
        lista.truncate(n.max(0) as usize);
        lista
    }

    /// Valor de um atributo de configuração (`VA_ATTRIB_NOT_SUPPORTED` se não houver).
    pub fn atributo(&self, perfil: VAProfile, ep: VAEntrypoint, tipo: VAConfigAttribType) -> u32 {
        let mut a = VAConfigAttrib {
            type_: tipo,
            value: 0,
        };
        // SAFETY: um atributo, com o tamanho 1 passado junto.
        let st = unsafe {
            self.libs
                .va
                .vaGetConfigAttributes(self.dpy, perfil, ep, &mut a, 1)
        };
        if st == VA_STATUS_SUCCESS as VAStatus {
            a.value
        } else {
            VA_ATTRIB_NOT_SUPPORTED
        }
    }
}

impl Drop for Display {
    fn drop(&mut self) {
        // SAFETY: display inicializado em `abrir`; quem o usa (configs, contextos) já soltou.
        unsafe { self.libs.va.vaTerminate(self.dpy) };
    }
}
