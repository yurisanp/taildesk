//! CLI do TailDesk. Subcomandos previstos: `host`, `connect <peer>`, `probe`, `bench-net <peer>`.

use std::process::ExitCode;

mod bench_net;

const USAGE: &str =
    "uso: taildesk-cli <host | connect <peer> | probe | bench-net <peer> | --version>";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version" | "-V") => {
            println!(
                "taildesk-cli {} (protocolo {}, porta {})",
                env!("CARGO_PKG_VERSION"),
                String::from_utf8_lossy(taildesk_proto::ALPN_V1),
                taildesk_proto::DEFAULT_PORT
            );
            ExitCode::SUCCESS
        }
        Some("bench-net") => bench_net::main(&args[1..]),
        Some("host" | "connect" | "probe") => {
            eprintln!("{}: ainda não implementado (fase 0/1)", args[0]);
            ExitCode::from(2)
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
