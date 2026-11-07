//! Tarefas do repositório:
//! - `cargo xtask check`: fmt, clippy e testes como o CI do desktop;
//! - `cargo xtask mobile-check`: o mesmo para o app Android (`crates/taildesk-mobile`), mais o
//!   clippy dos alvos Android pelo `cargo ndk` (fica fora do `check` padrão para não deixar o
//!   desktop lento; `docs/context/plano-execucao.md` §4).

use std::process::{Command, ExitCode};

fn cargo(args: &[&str]) -> bool {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    println!("$ cargo {}", args.join(" "));
    Command::new(cargo)
        .args(args)
        .status()
        .is_ok_and(|s| s.success())
}

fn tem_cargo_ndk() -> bool {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    Command::new(cargo)
        .args(["ndk", "--version"])
        .output()
        .is_ok_and(|o| o.status.success())
}

fn check() -> bool {
    cargo(&["fmt", "--all", "--check"])
        && cargo(&[
            "clippy",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ])
        && cargo(&["test", "--workspace"])
}

const PACOTES_MOBILE: [&str; 6] = [
    "-p",
    "taildesk-mobile",
    "-p",
    "taildesk-overlay",
    "-p",
    "taildesk-codec",
];

fn mobile_check() -> bool {
    let mut host = vec!["clippy"];
    host.extend(PACOTES_MOBILE);
    host.extend(["--all-targets", "--", "-D", "warnings"]);
    let mut testes = vec!["test"];
    testes.extend(PACOTES_MOBILE);
    let ok = cargo(&["fmt", "--all", "--check"]) && cargo(&host) && cargo(&testes);
    if !ok {
        return false;
    }
    if !tem_cargo_ndk() {
        let exigir = std::env::var_os("TAILDESK_EXIGIR_ANDROID").is_some();
        eprintln!(
            "mobile-check: sem `cargo ndk` (cargo install cargo-ndk) ou sem o NDK \
             (ANDROID_NDK_HOME): {} o clippy dos alvos Android",
            if exigir { "falta" } else { "pulando" }
        );
        return !exigir;
    }
    cargo(&[
        "ndk",
        "-t",
        "arm64-v8a",
        "-t",
        "x86_64",
        "--platform",
        "29",
        "clippy",
        "-p",
        "taildesk-mobile",
        "--",
        "-D",
        "warnings",
    ])
}

fn main() -> ExitCode {
    let ok = match std::env::args().nth(1).as_deref() {
        Some("check") => check(),
        Some("mobile-check") => mobile_check(),
        _ => {
            eprintln!("uso: cargo xtask check | cargo xtask mobile-check");
            return ExitCode::from(2);
        }
    };
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
