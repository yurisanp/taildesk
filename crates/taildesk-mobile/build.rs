//! Compila as telas Slint do celular com o estilo Material (o do Android) e as traduções.

fn main() {
    let config = slint_build::CompilerConfiguration::new()
        .with_style("material".into())
        .with_bundled_translations(concat!(env!("CARGO_MANIFEST_DIR"), "/traducoes"));
    slint_build::compile_with_config("ui/app.slint", config).expect("falha ao compilar a UI Slint");
    println!("cargo:rerun-if-changed=traducoes");
}
