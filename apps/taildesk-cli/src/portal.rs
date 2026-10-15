//! `taildesk-cli portal [--so-tela]`: abre a sessão do portal (RemoteDesktop + ScreenCast) como o
//! host vai abrir, lista os monitores liberados e passeia o ponteiro pelo centro de cada um.
//! Serve para conferir o portal de cada compositor (KDE, GNOME, wlroots) antes da captura.

use std::process::ExitCode;
use std::time::Duration;

use taildesk_platform::wayland::{ErroPortal, SessaoPortal};

pub fn main(args: &[String]) -> ExitCode {
    let com_input = !args.iter().any(|a| a == "--so-tela");
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    match rt.block_on(rodar(com_input)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("erro: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn rodar(com_input: bool) -> Result<(), ErroPortal> {
    println!("Escolha os monitores no diálogo do sistema…");
    let sessao = SessaoPortal::abrir(com_input).await?;
    for m in sessao.monitores() {
        println!(
            "monitor {}: node PipeWire {}, posição {:?}, tamanho lógico {}x{}{}",
            m.display_id,
            m.node_id,
            m.posicao,
            m.tamanho_logico.0,
            m.tamanho_logico.1,
            if m.mapping_id.is_empty() {
                String::new()
            } else {
                format!(", região libei {}", m.mapping_id)
            }
        );
    }
    let d = sessao.dispositivos();
    println!("teclado: {}, mouse: {}", sim(d.teclado), sim(d.ponteiro));
    if let Some(motivo) = sessao.sem_input() {
        println!("sem controle: {}", ErroPortal::SemInput(motivo));
    }
    match sessao.abrir_pipewire().await {
        Ok(_) => println!("PipeWire: conexão da sessão aberta"),
        Err(e) => println!("PipeWire: {e}"),
    }
    if d.ponteiro {
        match sessao.conectar_eis().await {
            Ok(Some(_)) => println!("libei: disponível (ConnectToEIS)"),
            Ok(None) => println!("libei: o portal não oferece ConnectToEIS"),
            Err(e) => println!("libei: {e}"),
        }
        for m in sessao.monitores() {
            println!(
                "movendo o ponteiro para o centro do monitor {}",
                m.display_id
            );
            sessao
                .ponteiro_absoluto(m.display_id, u16::MAX / 2, u16::MAX / 2)
                .await?;
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
    println!("Sessão aberta por 5 s; o compositor deve mostrar o indicador de compartilhamento.");
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_secs(5)) => {}
        r = sessao.esperar_fechamento() => {
            r?;
            println!("o host encerrou a sessão");
            return Ok(());
        }
    }
    sessao.fechar().await
}

fn sim(v: bool) -> &'static str {
    if v { "sim" } else { "não" }
}
