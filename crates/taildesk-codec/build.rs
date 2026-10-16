//! O openh264 compilado da fonte só usa SIMD (SSE2/AVX2) se o `nasm` estiver no PATH; sem ele o
//! encode de tela fica cerca de 4x mais lento. Aqui o build avisa, e com `TAILDESK_EXIGIR_NASM=1`
//! (releases) falha. O cargo não recompila o openh264 quando o `nasm` aparece depois, então o build
//! também avisa se o openh264 que está no `target/` saiu sem SIMD.

use std::path::{Path, PathBuf};
use std::process::Command;

/// `output` do build script do `openh264-sys2` mais recente deste `target/<perfil>/build`.
fn saida_openh264() -> Option<PathBuf> {
    let out = PathBuf::from(std::env::var_os("OUT_DIR")?);
    let build = out.parent()?.parent()?;
    std::fs::read_dir(build)
        .ok()?
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("openh264-sys2-")
        })
        .map(|e| e.path().join("output"))
        .filter_map(|p| Some((p.metadata().ok()?.modified().ok()?, p)))
        .max()
        .map(|(_, p)| p)
}

fn sem_simd(saida: &Path) -> bool {
    std::fs::read_to_string(saida).is_ok_and(|s| s.contains("not using any assembly"))
}

fn main() {
    println!("cargo:rerun-if-env-changed=TAILDESK_EXIGIR_NASM");
    println!("cargo:rerun-if-env-changed=PATH");
    let tem_nasm = Command::new("nasm")
        .arg("-v")
        .output()
        .is_ok_and(|s| s.status.success());
    if tem_nasm {
        if let Some(saida) = saida_openh264() {
            println!("cargo:rerun-if-changed={}", saida.display());
            if sem_simd(&saida) {
                println!(
                    "cargo:warning=o openh264 em target/ foi compilado sem o nasm (sem SIMD, ~4x \
                     mais lento); rode `cargo clean -p openh264-sys2` e compile de novo"
                );
            }
        }
        return;
    }
    let msg = "nasm não encontrado: o openh264 sai sem SIMD e o encode fica ~4x mais lento \
               (instale o pacote `nasm`)";
    if std::env::var_os("TAILDESK_EXIGIR_NASM").is_some_and(|v| v == "1") {
        panic!("{msg}");
    }
    println!("cargo:warning={msg}");
}
