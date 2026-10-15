//! `taildesk-cli captura-dxgi [segundos] [--video]`: lista os monitores do Windows como o host vai ver
//! (`display_id`, retângulo físico, rotação) e mede a Desktop Duplication de todos ao mesmo
//! tempo. Mexa o mouse durante a medição para ver as atualizações de cursor. `--video` também
//! mede o encode H.264 de cada monitor (`medicao`).

use std::process::ExitCode;
use std::time::Duration;

use taildesk_platform::captura_dxgi::{CapturaDxgi, listar_monitores};

use crate::medicao::Medidor;

pub fn main(args: &[String]) -> ExitCode {
    let segundos = args.iter().find_map(|a| a.parse().ok()).unwrap_or(5);
    let video = args.iter().any(|a| a == "--video");
    match rodar(segundos, video) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("erro: {e}");
            ExitCode::FAILURE
        }
    }
}

fn rodar(segundos: u64, video: bool) -> std::io::Result<()> {
    let monitores = listar_monitores()?;
    for m in &monitores {
        println!(
            "monitor {}: {} em ({}, {}) {}x{} px físicos, {:?}{}",
            m.display_id,
            m.nome,
            m.area.x,
            m.area.y,
            m.area.largura,
            m.area.altura,
            m.rotacao,
            if m.principal { ", principal" } else { "" }
        );
    }
    let ids: Vec<u32> = monitores.iter().map(|m| m.display_id).collect();
    let medidor = Medidor::novo(&ids, video);
    let cb = medidor.clone();
    println!("Capturando {segundos} s de todos os monitores…");
    let captura = CapturaDxgi::iniciar(&monitores, move |q| cb.registrar(q))?;
    std::thread::sleep(Duration::from_secs(segundos));
    drop(captura);
    medidor.imprimir();
    Ok(())
}
