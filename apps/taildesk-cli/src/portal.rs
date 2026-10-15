//! `taildesk-cli portal [--so-tela]`: abre a sessão do portal (RemoteDesktop + ScreenCast) como o
//! host vai abrir, lista os monitores liberados, mede 3 s de captura de cada um e passeia o ponteiro
//! pelo centro deles. Serve para conferir o portal de cada compositor (KDE, GNOME, wlroots).

use std::process::ExitCode;
use std::time::Duration;

use taildesk_platform::captura_pipewire::{CapturaPipeWire, Conexao};
use taildesk_platform::wayland::{ErroPortal, SessaoPortal};

use crate::medicao::Medidor;

/// Captura `segundos` de cada monitor e imprime quadros por segundo, tamanho e formato.
fn medir(conexao: Conexao, monitores: &[(u32, u32)], segundos: u64) -> std::io::Result<()> {
    let ids: Vec<u32> = monitores.iter().map(|m| m.0).collect();
    let medidor = Medidor::novo(&ids);
    let cb = medidor.clone();
    let captura = CapturaPipeWire::iniciar(conexao, monitores, move |q| cb.registrar(q))?;
    std::thread::sleep(Duration::from_secs(segundos));
    drop(captura);
    medidor.imprimir();
    Ok(())
}

/// `taildesk-cli captura-node <node>`: lê um node do PipeWire do usuário, sem portal.
pub fn captura_node(args: &[String]) -> ExitCode {
    let Some(node) = args.first().and_then(|a| a.parse().ok()) else {
        eprintln!("uso: taildesk-cli captura-node <node-id>");
        return ExitCode::from(2);
    };
    match medir(Conexao::Padrao, &[(0, node)], 3) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("erro: {e}");
            ExitCode::FAILURE
        }
    }
}

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
    let fd = sessao.abrir_pipewire().await?;
    let nodes: Vec<(u32, u32)> = sessao
        .monitores()
        .iter()
        .map(|m| (m.display_id, m.node_id))
        .collect();
    println!("Capturando 3 s de cada monitor…");
    let medicao = tokio::task::spawn_blocking(move || medir(Conexao::Portal(fd), &nodes, 3));
    match medicao.await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => println!("captura: {e}"),
        Err(e) => println!("captura: {e}"),
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
