//! Código específico de SO. Hoje: instância única do app ([`instancia`]), detecção da bandeja
//! do sistema e abrir links. Nas próximas etapas da fase 1 entram captura (DXGI/WGC,
//! portal+PipeWire, XShm), injeção de input (SendInput, portal/libei, XTest), cursor e segredos
//! (`docs/context/10-plano-windows-linux.md`).

pub mod area;
pub mod area_formatos;
#[cfg(target_os = "linux")]
pub mod area_wayland;
#[cfg(windows)]
pub mod area_windows;
pub mod arquivos;
pub mod captura;
#[cfg(windows)]
pub mod captura_dxgi;
#[cfg(target_os = "linux")]
pub mod captura_pipewire;
mod forma_cursor_dxgi;
pub mod inicio;
pub mod instancia;
pub mod mouse;
pub mod segredos;
#[cfg(windows)]
pub mod servico_windows;
pub mod soltar;
pub mod som;
pub mod teclado;
#[cfg(target_os = "linux")]
pub mod wayland;

use std::io;
use std::process::{Command, Stdio};

/// Se o sistema mostra ícones na bandeja. Quando não mostra, o app deve fechar ao fechar a
/// janela, em vez de ficar escondido sem ter como voltar.
///
/// - Linux: precisa de um StatusNotifierWatcher no D-Bus da sessão (KDE, Xfce, Cinnamon, LXQt,
///   waybar; GNOME só com a extensão AppIndicator, que o Ubuntu já traz).
/// - Windows: a área de notificação sempre existe.
pub fn bandeja_disponivel() -> bool {
    #[cfg(target_os = "linux")]
    {
        sni_watcher().unwrap_or(false)
    }
    #[cfg(not(target_os = "linux"))]
    {
        cfg!(windows)
    }
}

#[cfg(target_os = "linux")]
fn sni_watcher() -> zbus::Result<bool> {
    let sessao = zbus::blocking::Connection::session()?;
    let dbus = zbus::blocking::fdo::DBusProxy::new(&sessao)?;
    let nome = zbus::names::BusName::try_from("org.kde.StatusNotifierWatcher")?;
    Ok(dbus.name_has_owner(nome)?)
}

/// Lê e apaga do ambiente o token de ativação que o lançador passou ao app: `XDG_ACTIVATION_TOKEN`
/// (xdg-activation, Wayland) ou `DESKTOP_STARTUP_ID` (startup-notification, X11). Com ele a janela
/// ganha o foco do jeito nativo; apagar evita que processos filhos herdem um token já usado.
///
/// # Safety
///
/// Mexe no ambiente do processo: chame antes de criar qualquer outra thread.
pub unsafe fn tomar_token_de_ativacao() -> Option<String> {
    let mut token = None;
    for var in ["XDG_ACTIVATION_TOKEN", "DESKTOP_STARTUP_ID"] {
        if let Ok(valor) = std::env::var(var) {
            if token.is_none() && !valor.is_empty() {
                token = Some(valor);
            }
            // SAFETY: o chamador garante que não há outras threads lendo o ambiente.
            unsafe { std::env::remove_var(var) };
        }
    }
    token
}

/// Onde ficam os registros de auditoria (acessos e arquivos): `%LOCALAPPDATA%\TailDesk\registros`
/// no Windows, `$XDG_STATE_HOME/taildesk` (`~/.local/state/taildesk`) no Linux.
pub fn pasta_registros() -> Option<std::path::PathBuf> {
    use std::path::PathBuf;
    #[cfg(windows)]
    {
        std::env::var_os("LOCALAPPDATA")
            .map(|d| PathBuf::from(d).join("TailDesk").join("registros"))
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
            .map(|d| d.join("taildesk"))
    }
}

/// Pasta de downloads do usuário (`XDG_DOWNLOAD_DIR` no Linux, `%USERPROFILE%\Downloads` no
/// Windows), onde fica a pasta de entrada `TailDesk`.
pub fn pasta_downloads() -> Option<std::path::PathBuf> {
    use std::path::PathBuf;
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join("Downloads"))
    }
    #[cfg(not(windows))]
    {
        let home = PathBuf::from(std::env::var_os("HOME")?);
        // `user-dirs.dirs`: XDG_DOWNLOAD_DIR="$HOME/Baixados"
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        let xdg = std::fs::read_to_string(config.join("user-dirs.dirs"))
            .ok()
            .and_then(|t| {
                t.lines().find_map(|l| {
                    let v = l
                        .trim()
                        .strip_prefix("XDG_DOWNLOAD_DIR=")?
                        .trim_matches('"');
                    Some(match v.strip_prefix("$HOME") {
                        Some(resto) => home.join(resto.trim_start_matches('/')),
                        None => PathBuf::from(v),
                    })
                })
            });
        Some(xdg.unwrap_or_else(|| home.join("Downloads")))
    }
}

/// Abre um link no navegador padrão.
pub fn abrir_link(url: &str) -> io::Result<()> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "só links http(s)",
        ));
    }
    let programa = if cfg!(windows) {
        "explorer.exe"
    } else {
        "xdg-open"
    };
    Command::new(programa)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|mut filho| {
            // Recolhe o processo quando ele terminar, sem segurar quem chamou.
            std::thread::spawn(move || filho.wait());
        })
}

#[cfg(test)]
mod tests {
    #[test]
    fn abrir_link_recusa_o_que_nao_e_web() {
        assert!(super::abrir_link("file:///etc/passwd").is_err());
        assert!(super::abrir_link("-h").is_err());
    }
}
