//! O openh264 compilado da fonte só usa SIMD (SSE2/AVX2) se o `nasm` estiver no PATH; sem ele o
//! encode de tela fica cerca de 4x mais lento. Aqui o build avisa, e com `TAILDESK_EXIGIR_NASM=1`
//! (releases) falha.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=TAILDESK_EXIGIR_NASM");
    println!("cargo:rerun-if-env-changed=PATH");
    let tem_nasm = Command::new("nasm")
        .arg("-v")
        .output()
        .is_ok_and(|s| s.status.success());
    if tem_nasm {
        return;
    }
    let msg = "nasm não encontrado: o openh264 sai sem SIMD e o encode fica ~4x mais lento \
               (instale o pacote `nasm`)";
    if std::env::var_os("TAILDESK_EXIGIR_NASM").is_some_and(|v| v == "1") {
        panic!("{msg}");
    }
    println!("cargo:warning={msg}");
}
