//! GPUs do sistema pelo sysfs: endereço PCI, fabricante, nome (do `pci.ids`, se houver) e se é a
//! GPU que está ligada às telas.

use std::path::{Path, PathBuf};

/// Uma GPU com nó de render.
#[derive(Clone, Debug)]
pub struct GpuSys {
    /// `/dev/dri/renderD128`.
    pub no: PathBuf,
    /// `0000:00:02.0`.
    pub pci: String,
    pub fabricante: u16,
    pub dispositivo: u16,
    pub nome: String,
    /// Tem tela conectada ou é a GPU de boot: é onde o compositor desenha.
    pub principal: bool,
    /// Ordem de preferência: tela conectada, depois GPU de boot.
    ordem: (bool, bool),
}

pub const INTEL: u16 = 0x8086;
pub const NVIDIA: u16 = 0x10de;
pub const AMD: u16 = 0x1002;

fn hex(p: &Path) -> Option<u16> {
    let s = std::fs::read_to_string(p).ok()?;
    u16::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok()
}

/// Nome curto do `pci.ids` ("Iris Xe Graphics", "GeForce RTX 3060 Mobile / Max-Q").
fn nome_pci(fabricante: u16, dispositivo: u16) -> Option<String> {
    let ids = ["/usr/share/hwdata/pci.ids", "/usr/share/misc/pci.ids"]
        .iter()
        .find_map(|p| std::fs::read_to_string(p).ok())?;
    let f = format!("{fabricante:04x}  ");
    let d = format!("\t{dispositivo:04x}  ");
    let mut no_fabricante = false;
    for linha in ids.lines() {
        if !linha.starts_with('\t') && !linha.starts_with('#') {
            if no_fabricante {
                return None;
            }
            no_fabricante = linha.starts_with(&f);
        } else if no_fabricante && let Some(nome) = linha.strip_prefix(&d) {
            // "TU117M [GeForce GTX 1650 Mobile]": o que está entre colchetes é o nome comercial.
            let nome = match (nome.find('['), nome.rfind(']')) {
                (Some(a), Some(b)) if a < b => &nome[a + 1..b],
                _ => nome,
            };
            return Some(nome.to_string());
        }
    }
    None
}

fn nome_fabricante(f: u16) -> &'static str {
    match f {
        INTEL => "Intel",
        NVIDIA => "NVIDIA",
        AMD => "AMD",
        _ => "GPU",
    }
}

/// `true` se algum conector desta GPU tem tela.
fn tem_tela(dispositivo: &Path) -> bool {
    let Ok(drm) = std::fs::read_dir(dispositivo.join("drm")) else {
        return false;
    };
    drm.flatten()
        .filter(|card| card.file_name().to_string_lossy().starts_with("card"))
        .filter_map(|card| std::fs::read_dir(card.path()).ok())
        .flatten()
        .flatten()
        .any(|c| {
            std::fs::read_to_string(c.path().join("status")).is_ok_and(|s| s.trim() == "connected")
        })
}

/// GPUs com nó de render, a principal primeiro.
pub fn gpus() -> Vec<GpuSys> {
    let mut gpus: Vec<GpuSys> = super::va::nos_de_render()
        .into_iter()
        .filter_map(|no| {
            let nome_no = no.file_name()?.to_str()?.to_string();
            let dispositivo =
                std::fs::canonicalize(format!("/sys/class/drm/{nome_no}/device")).ok()?;
            let pci = dispositivo.file_name()?.to_str()?.to_ascii_lowercase();
            let fabricante = hex(&dispositivo.join("vendor"))?;
            let id = hex(&dispositivo.join("device")).unwrap_or(0);
            let boot = std::fs::read_to_string(dispositivo.join("boot_vga"))
                .is_ok_and(|s| s.trim() == "1");
            let nome = match nome_pci(fabricante, id) {
                Some(n) if n.starts_with(nome_fabricante(fabricante)) => n,
                Some(n) => format!("{} {n}", nome_fabricante(fabricante)),
                None => format!("{} {id:04x}", nome_fabricante(fabricante)),
            };
            let tela = tem_tela(&dispositivo);
            Some(GpuSys {
                principal: tela || boot,
                ordem: (!tela, !boot),
                no,
                pci,
                fabricante,
                dispositivo: id,
                nome,
            })
        })
        .collect();
    gpus.sort_by_key(|g| g.ordem);
    gpus
}
