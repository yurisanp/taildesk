//! Capturas das telas do app Android sem emulador (software renderer do Slint), para a ficha da
//! loja e para conferir o visual: `cargo run -p taildesk-mobile --example capturas -- <pasta>
//! [pt-BR|en-US]`. Saem em 1080×1920 (retrato) e 1920×1080 (paisagem), como pede a Play.

#[cfg(target_os = "android")]
fn main() {}

#[cfg(not(target_os = "android"))]
fn main() {
    capturas::main();
}

#[cfg(not(target_os = "android"))]
mod capturas {
    use std::path::{Path, PathBuf};

    use slint::ComponentHandle;
    use taildesk_mobile::app::{App, SemPlataforma};
    use taildesk_mobile::captura;
    use taildesk_mobile::controle::Aparelho;
    use taildesk_mobile::ui::{AppCelular, EstadoTs, Tela, Tema};
    use taildesk_overlay::mobile::SinaisRede;

    fn gravar(caminho: &Path, (w, h, rgba): (u32, u32, Vec<u8>)) {
        let arquivo = std::fs::File::create(caminho).expect("criar png");
        let mut enc = png::Encoder::new(std::io::BufWriter::new(arquivo), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()
            .and_then(|mut e| e.write_image_data(&rgba))
            .expect("gravar png");
        println!("{}", caminho.display());
    }

    pub fn main() {
        let mut args = std::env::args().skip(1);
        let pasta = PathBuf::from(args.next().unwrap_or_else(|| "capturas".into()));
        let idioma = args.next().unwrap_or_else(|| "pt-BR".into());
        std::fs::create_dir_all(&pasta).expect("pasta das capturas");

        let janela = captura::iniciar();
        let ui = AppCelular::new().expect("janela");
        // O contexto das traduções nasce com a primeira janela.
        if idioma.starts_with("en") {
            slint::select_bundled_translation("en").expect("tradução en embutida");
        }
        let app = App::new(
            &ui,
            Box::new(SemPlataforma(Aparelho {
                modelo: "Pixel 8".into(),
                arch: "aarch64".into(),
                tablet: false,
                largura_px: 2400,
                altura_px: 1080,
                dpi: 420,
                decode_hw: true,
                idioma_teclado: idioma.clone(),
                versao_app: env!("CARGO_PKG_VERSION").into(),
            })),
        );
        ui.show().expect("mostrar");
        let retrato = |n: &str| {
            captura::dimensionar(&janela, 1080, 1920, 2.5);
            gravar(
                &pasta.join(format!("{n}.png")),
                captura::renderizar(&janela),
            );
        };
        let paisagem = |n: &str| {
            captura::dimensionar(&janela, 1920, 1080, 2.5);
            gravar(
                &pasta.join(format!("{n}.png")),
                captura::renderizar(&janela),
            );
        };

        // 1. Onboarding com o Tailscale desligado (mockup 01).
        app.sinais(SinaisRede {
            instalado: true,
            versao: Some("1.104.1".into()),
            ..SinaisRede::default()
        });
        ui.set_tela(Tela::Onboarding);
        retrato("01-onboarding");

        // 2. Dispositivos (mockup 04).
        app.preencher_exemplo();
        ui.set_estado_ts(EstadoTs::Conectado);
        ui.set_tarifada(true);
        ui.set_tela(Tela::Dispositivos);
        retrato("02-dispositivos");

        // 3. Encontrar computadores / parear (mockup 02).
        ui.set_tela(Tela::Encontrar);
        retrato("03-parear");

        // 4. Sessão em paisagem, touchpad, com o HUD (mockup 07).
        captura::dimensionar(&janela, 1920, 1080, 2.5);
        app.demonstracao();
        ui.invoke_alternar_hud();
        paisagem("04-sessao");

        // 5. Sessão com a faixa de teclas (mockup 08).
        ui.set_teclado_aberto(true);
        ui.invoke_texto("  — ação".into());
        ui.invoke_modificador("ctrl".into());
        paisagem("05-teclado");
        ui.set_teclado_aberto(false);

        // 6. Configurações.
        ui.invoke_encerrar();
        ui.set_tela(Tela::Configuracoes);
        retrato("06-configuracoes");

        // 7. Outra VPN no tema escuro (mockup 18).
        ui.global::<Tema>().set_escuro(true);
        ui.set_estado_ts(EstadoTs::OutraVpn);
        ui.set_tela(Tela::Onboarding);
        retrato("07-outra-vpn-escuro");
    }
}
