//! "Iniciar com o sistema": o TailDesk em segundo plano (só na bandeja, recebendo conexões)
//! desde que a pessoa entra no computador, para o acesso com senha funcionar sem ninguém abrir
//! o app.
//!
//! - Linux: serviço do usuário no systemd (`~/.config/systemd/user/taildesk.service`), preso ao
//!   `graphical-session.target`, para ter o Wayland, o D-Bus e o portal da sessão gráfica.
//! - Flatpak: o sandbox não tem `systemctl`; o portal Background cria a entrada de início
//!   automático (`taildesk --oculto`), e um marcador na pasta de configuração do app lembra que
//!   está ligado (o portal não diz).
//! - Windows: serviço do Windows que abre o app na sessão de quem entra
//!   ([`crate::servico_windows`]); ligar e desligar pedem administrador.

use std::io;
use std::path::{Path, PathBuf};

/// Nome da unidade do systemd.
pub const UNIDADE: &str = "taildesk.service";

/// O executável a iniciar: o AppImage inteiro quando o app roda de dentro de um (o caminho do
/// binário montado muda a cada execução).
pub fn executavel() -> io::Result<PathBuf> {
    if let Some(a) = std::env::var_os("APPIMAGE").filter(|a| !a.is_empty()) {
        return Ok(PathBuf::from(a));
    }
    std::env::current_exe()
}

/// Um argumento do `ExecStart=` entre aspas: `\` e `"` escapados, `%` dobrado (especificador
/// do systemd) e `$` dobrado (variável).
fn citar(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '%' => out.push_str("%%"),
            '$' => out.push_str("$$"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Texto da unidade do systemd que abre `exe --oculto`.
pub fn unidade(exe: &Path) -> String {
    format!(
        "[Unit]\n\
         Description=TailDesk (acesso remoto em segundo plano)\n\
         Documentation=https://github.com/yurisanp/taildesk\n\
         PartOf=graphical-session.target\n\
         After=graphical-session.target\n\
         \n\
         [Service]\n\
         ExecStart={} --oculto\n\
         Restart=on-failure\n\
         RestartSec=5\n\
         \n\
         [Install]\n\
         WantedBy=graphical-session.target\n",
        citar(&exe.to_string_lossy())
    )
}

/// O app roda dentro de um Flatpak.
#[cfg(target_os = "linux")]
pub fn em_flatpak() -> bool {
    Path::new("/.flatpak-info").exists()
}

#[cfg(target_os = "linux")]
fn pasta_config() -> io::Result<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .ok_or_else(|| io::Error::other("sem pasta de configuração"))
}

/// Marcador do início automático no Flatpak (na pasta do app dentro do sandbox).
#[cfg(target_os = "linux")]
fn marcador_flatpak() -> io::Result<PathBuf> {
    Ok(pasta_config()?
        .join("taildesk")
        .join("iniciar-com-o-sistema"))
}

/// Pede ao portal Background para abrir (ou não) o app sozinho ao entrar. Devolve o que o
/// sistema aceitou.
#[cfg(target_os = "linux")]
fn portal_background(ligar: bool) -> io::Result<bool> {
    use ashpd::desktop::background::Background;
    pollster::block_on(async {
        let r = Background::request()
            .reason("Receber conexões na bandeja desde que você entra no computador")
            .auto_start(ligar)
            .command(["taildesk", "--oculto"])
            .dbus_activatable(false)
            .send()
            .await
            .map_err(io::Error::other)?
            .response()
            .map_err(io::Error::other)?;
        Ok(r.auto_start())
    })
}

#[cfg(target_os = "linux")]
fn caminho_unidade() -> io::Result<PathBuf> {
    Ok(pasta_config()?.join("systemd/user").join(UNIDADE))
}

#[cfg(target_os = "linux")]
fn systemctl(args: &[&str]) -> io::Result<()> {
    let s = std::process::Command::new("systemctl")
        .arg("--user")
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .status()?;
    if s.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "systemctl --user {} falhou ({s})",
            args.join(" ")
        )))
    }
}

/// Liga: grava a unidade e a habilita para as próximas entradas (não abre outra cópia agora).
#[cfg(target_os = "linux")]
pub fn ligar() -> io::Result<()> {
    if em_flatpak() {
        if !portal_background(true)? {
            return Err(io::Error::other(
                "o sistema não deixou o TailDesk abrir sozinho (veja as permissões do app)",
            ));
        }
        let m = marcador_flatpak()?;
        if let Some(d) = m.parent() {
            std::fs::create_dir_all(d)?;
        }
        return std::fs::write(m, b"1\n");
    }
    let p = caminho_unidade()?;
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, unidade(&executavel()?))?;
    std::fs::rename(&tmp, &p)?;
    systemctl(&["daemon-reload"])?;
    systemctl(&["enable", UNIDADE])
}

/// Desliga e apaga a unidade.
#[cfg(target_os = "linux")]
pub fn desligar() -> io::Result<()> {
    if em_flatpak() {
        portal_background(false)?;
        return match std::fs::remove_file(marcador_flatpak()?) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    // Já desabilitada (ou o systemd do usuário não existe): segue para apagar o arquivo.
    let _ = systemctl(&["disable", UNIDADE]);
    match std::fs::remove_file(caminho_unidade()?) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let _ = systemctl(&["daemon-reload"]);
    Ok(())
}

/// Se está ligado (a unidade existe e está habilitada).
#[cfg(target_os = "linux")]
pub fn ligado() -> bool {
    if em_flatpak() {
        return marcador_flatpak().is_ok_and(|m| m.exists());
    }
    caminho_unidade().is_ok_and(|p| p.exists())
        && std::process::Command::new("systemctl")
            .args(["--user", "is-enabled", "--quiet", UNIDADE])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
}

/// Se a unidade aponta para outro executável (o app foi movido ou atualizado para outro
/// caminho): ligue de novo para corrigir.
#[cfg(target_os = "linux")]
pub fn desatualizado() -> bool {
    // No Flatpak o comando é sempre `taildesk` dentro do sandbox.
    if em_flatpak() {
        return false;
    }
    let (Ok(p), Ok(exe)) = (caminho_unidade(), executavel()) else {
        return false;
    };
    std::fs::read_to_string(p).is_ok_and(|t| t != unidade(&exe))
}

/// Liga: instala o serviço (o Windows pede administrador).
#[cfg(windows)]
pub fn ligar() -> io::Result<()> {
    crate::servico_windows::elevar(&executavel()?, "--instalar-servico")
}

/// Desliga: remove o serviço (o Windows pede administrador).
#[cfg(windows)]
pub fn desligar() -> io::Result<()> {
    if !crate::servico_windows::instalado() {
        return Ok(());
    }
    crate::servico_windows::elevar(&executavel()?, "--remover-servico")
}

#[cfg(windows)]
pub fn ligado() -> bool {
    crate::servico_windows::instalado()
}

/// O serviço guarda o caminho do executável instalado; atualizar troca o arquivo no lugar.
#[cfg(windows)]
pub fn desatualizado() -> bool {
    false
}

#[cfg(not(any(target_os = "linux", windows)))]
pub fn ligar() -> io::Result<()> {
    Err(io::ErrorKind::Unsupported.into())
}

#[cfg(not(any(target_os = "linux", windows)))]
pub fn desligar() -> io::Result<()> {
    Ok(())
}

#[cfg(not(any(target_os = "linux", windows)))]
pub fn ligado() -> bool {
    false
}

#[cfg(not(any(target_os = "linux", windows)))]
pub fn desatualizado() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unidade_cita_o_caminho() {
        let u = unidade(Path::new(
            "/home/ana/Aplicativos/Tail Desk \"x\" 100%$HOME.AppImage",
        ));
        assert!(
            u.contains(
                "ExecStart=\"/home/ana/Aplicativos/Tail Desk \\\"x\\\" 100%%$$HOME.AppImage\" --oculto\n"
            ),
            "{u}"
        );
        assert!(u.contains("WantedBy=graphical-session.target"));
    }
}
