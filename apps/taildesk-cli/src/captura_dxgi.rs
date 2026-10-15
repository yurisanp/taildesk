//! `taildesk-cli captura-dxgi [segundos]`: lista os monitores do Windows como o host vai ver
//! (`display_id`, retângulo físico, rotação) e mede a Desktop Duplication de todos ao mesmo
//! tempo. Mexa o mouse durante a medição para ver as atualizações de cursor.

use std::process::ExitCode;
use std::time::Duration;

use taildesk_platform::captura_dxgi::{CapturaDxgi, listar_monitores};

use crate::medicao::Medidor;

pub fn main(args: &[String]) -> ExitCode {
    let segundos = args.first().and_then(|a| a.parse().ok()).unwrap_or(5);
    match rodar(segundos) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("erro: {e}");
            ExitCode::FAILURE
        }
    }
}

fn rodar(segundos: u64) -> std::io::Result<()> {
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
    let medidor = Medidor::novo(&ids);
    let cb = medidor.clone();
    println!("Capturando {segundos} s de todos os monitores…");
    let captura = CapturaDxgi::iniciar(&monitores, move |q| cb.registrar(q))?;
    std::thread::sleep(Duration::from_secs(segundos));
    drop(captura);
    medidor.imprimir();
    Ok(())
}
