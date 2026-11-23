//! "Iniciar com o sistema": o TailDesk em segundo plano (só na bandeja, recebendo conexões)
//! desde que a pessoa entra no computador, para o acesso com senha funcionar sem ninguém abrir
//! o app.
//!
//! - Linux: serviço do usuário no systemd (`~/.config/systemd/user/`[`UNIDADE`]), preso ao
//!   `graphical-session.target`, para ter o Wayland, o D-Bus e o portal da sessão gráfica.
//! - Flatpak: o sandbox não tem `systemctl`; o portal Background cria a entrada de início
//!   automático (`taildesk --oculto`), e um marcador na pasta de configuração do app lembra que
//!   está ligado (o portal não diz).
//! - Windows: serviço do Windows que abre o app na sessão de quem entra
//!   ([`crate::servico_windows`]); ligar e desligar pedem administrador.

use std::io;
use std::path::{Path, PathBuf};

/// Nome da unidade do systemd, no padrão `app-<id>-autostart.service` das entradas de início
/// automático: o portal tira dele o id do app ([`crate::APP_ID`]), o mesmo de quando o app abre
/// pelo menu. Com outro nome o id virava `""` e a permissão de tela lembrada não valia.
pub const UNIDADE: &str = "app-io.github.yurisanp.TailDesk-autostart.service";

/// Nome até a v0.2.0-beta: migrada para [`UNIDADE`] no próximo [`ligar`].
#[cfg(target_os = "linux")]
const UNIDADE_ANTIGA: &str = "taildesk.service";

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
fn caminho_unidade_antiga() -> io::Result<PathBuf> {
    Ok(pasta_config()?.join("systemd/user").join(UNIDADE_ANTIGA))
}

/// A unidade em disco precisa ser regravada: ainda há a de nome antigo, ou o texto não é o que
/// [`unidade`] gera hoje (app movido ou atualizado).
#[cfg(any(target_os = "linux", test))]
fn precisa_regravar(tem_antiga: bool, atual: Option<&str>, esperado: &str) -> bool {
    tem_antiga || atual.is_some_and(|t| t != esperado)
}

/// Desabilita e apaga a unidade de nome antigo, se houver.
#[cfg(target_os = "linux")]
fn apagar_antiga() -> io::Result<()> {
    let p = caminho_unidade_antiga()?;
    if !p.exists() {
        return Ok(());
    }
    let _ = systemctl(&["disable", UNIDADE_ANTIGA]);
    match std::fs::remove_file(p) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
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
    apagar_antiga()?;
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
    apagar_antiga()?;
    let _ = systemctl(&["daemon-reload"]);
    Ok(())
}

/// Se está ligado (a unidade existe e está habilitada).
#[cfg(target_os = "linux")]
pub fn ligado() -> bool {
    if em_flatpak() {
        return marcador_flatpak().is_ok_and(|m| m.exists());
    }
    let habilitada = |caminho: io::Result<PathBuf>, nome: &str| {
        caminho.is_ok_and(|p| p.exists())
            && std::process::Command::new("systemctl")
                .args(["--user", "is-enabled", "--quiet", nome])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
    };
    habilitada(caminho_unidade(), UNIDADE) || habilitada(caminho_unidade_antiga(), UNIDADE_ANTIGA)
}

/// Se a unidade aponta para outro executável (o app foi movido ou atualizado para outro
/// caminho) ou ainda tem o nome antigo: ligue de novo para corrigir.
#[cfg(target_os = "linux")]
pub fn desatualizado() -> bool {
    // No Flatpak o comando é sempre `taildesk` dentro do sandbox.
    if em_flatpak() {
        return false;
    }
    let (Ok(p), Ok(exe)) = (caminho_unidade(), executavel()) else {
        return false;
    };
    let antiga = caminho_unidade_antiga().is_ok_and(|p| p.exists());
    let atual = std::fs::read_to_string(p).ok();
    precisa_regravar(antiga, atual.as_deref(), &unidade(&exe))
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

    /// O portal tira o id do app do nome da unidade (`app-<id>-autostart.service`, regex de
    /// `xdp-app-info-host.c`): tem que dar o mesmo id da janela e do `.desktop`.
    #[test]
    fn unidade_leva_o_id_do_app() {
        let id = UNIDADE
            .strip_prefix("app-")
            .and_then(|u| u.strip_suffix(".service"))
            .and_then(|u| u.strip_suffix("-autostart"));
        assert_eq!(id, Some(crate::APP_ID));
    }

    #[test]
    fn regrava_unidade_antiga_ou_velha() {
        let novo = unidade(Path::new("/opt/taildesk"));
        // Só a antiga (`taildesk.service`): migra.
        assert!(precisa_regravar(true, None, &novo));
        // As duas: migra e apaga a antiga.
        assert!(precisa_regravar(true, Some(&novo), &novo));
        // Nova apontando para outro executável.
        assert!(precisa_regravar(
            false,
            Some(&unidade(Path::new("/velho"))),
            &novo
        ));
        // Em dia, ou desligado.
        assert!(!precisa_regravar(false, Some(&novo), &novo));
        assert!(!precisa_regravar(false, None, &novo));
    }
}
