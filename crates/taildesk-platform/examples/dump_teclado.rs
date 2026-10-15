//! Grava o layout no formato de fixture de `taildesk-keyboard` (`11-teclado.md` §5).
//!
//! Linux: `cargo run -p taildesk-platform --example dump_teclado -- br abnt2 pt_BR.UTF-8`
//! (layout, variante e locale da tabela Compose).

fn main() {
    #[cfg(target_os = "linux")]
    {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let layout = args.first().map(String::as_str).unwrap_or("us");
        let variante = args.get(1).map(String::as_str).unwrap_or("");
        let locale = args
            .get(2)
            .cloned()
            .unwrap_or_else(taildesk_platform::teclado::xkb::locale_da_sessao);
        match taildesk_platform::teclado::xkb::ler_keymap_nomes(layout, variante, &locale) {
            Ok(km) => print!("{}", km.to_text()),
            Err(e) => {
                eprintln!("erro: {e}");
                std::process::exit(1);
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    eprintln!("só no Linux por enquanto");
}
