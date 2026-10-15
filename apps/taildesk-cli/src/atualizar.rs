//! `taildesk-cli atualizar [--pre] [--instalar]`: procura a release mais nova e, num AppImage,
//! troca o arquivo pela versão nova.

use std::process::ExitCode;

use taildesk_update::{Canal, Instalacao, instalar_appimage, procurar, versao_atual};

pub fn main(args: &[String]) -> ExitCode {
    let canal = if args.iter().any(|a| a == "--pre") {
        Canal::PreLancamento
    } else {
        Canal::Estavel
    };
    let instalar = args.iter().any(|a| a == "--instalar");
    let atual = versao_atual();
    let nova = match procurar(canal, &atual) {
        Ok(Some(n)) => n,
        Ok(None) => {
            println!("TailDesk {atual} é a versão mais nova ({canal:?}).");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("não deu para procurar atualizações: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!(
        "TailDesk {} disponível (atual {atual}): {}",
        nova.versao, nova.pagina
    );
    let instalacao = Instalacao::detectar();
    if !instalar {
        if instalacao.automatica() {
            println!("Para instalar: taildesk-cli atualizar --instalar");
        }
        return ExitCode::SUCCESS;
    }
    let Instalacao::AppImage { caminho } = &instalacao else {
        eprintln!("só o AppImage se atualiza sozinho; baixe a versão nova na página acima");
        return ExitCode::from(2);
    };
    if !instalacao.automatica() {
        eprintln!("sem permissão de escrita em {}", caminho.display());
        return ExitCode::from(2);
    }
    let mut ultimo = 0;
    let r = instalar_appimage(&nova, caminho, |b, t| {
        let mb = b >> 20;
        if mb != ultimo {
            ultimo = mb;
            eprint!("\r{mb} de {} MiB", t >> 20);
        }
    });
    eprintln!();
    match r {
        Ok(()) => {
            println!(
                "Instalado em {}. Reinicie o TailDesk para usar a versão nova.",
                caminho.display()
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("atualização falhou, a versão atual continua: {e}");
            ExitCode::FAILURE
        }
    }
}
