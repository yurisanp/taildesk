//! Tarefas do repositório: `cargo xtask check` roda fmt, clippy e testes como o CI.

use std::process::{Command, ExitCode};

fn cargo(args: &[&str]) -> bool {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    println!("$ cargo {}", args.join(" "));
    Command::new(cargo)
        .args(args)
        .status()
        .is_ok_and(|s| s.success())
}

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("check") => {
            let ok = cargo(&["fmt", "--all", "--check"])
                && cargo(&[
                    "clippy",
                    "--workspace",
                    "--all-targets",
                    "--",
                    "-D",
                    "warnings",
                ])
                && cargo(&["test", "--workspace"]);
            if ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        _ => {
            eprintln!("uso: cargo xtask check");
            ExitCode::from(2)
        }
    }
}
