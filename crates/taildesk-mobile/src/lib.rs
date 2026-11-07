//! App Android do TailDesk (`docs/context/mobile/android-detalhado.md`): a "casca Rust" que fica
//! entre a casca Kotlin mínima (`apps/android`) e o núcleo compartilhado com o desktop
//! (`taildesk-proto`, `-transport`, `-session`, `-keyboard`, `-overlay`).
//!
//! Lógica pura, testada no Linux:
//! - [`gestos`]: toques → ações (touchpad e toque direto).
//! - [`zoom`]: zoom/pan locais e o mapa tela → coordenada do monitor.
//! - [`controle`]: a sessão vista do celular (ações e teclas → mensagens; um monitor por vez).
//! - [`dispositivos`]: lista com favoritos, recentes, estados e a `PeerList` do desktop pareado.
//! - [`rede`]: rede tarifada e consumo de dados; [`ciclo`]: segundo plano, volta, HUD.
//! - [`area`]: área de transferência de texto da sessão; [`fisico`]: teclado físico ligado ao
//!   celular.
//! - [`arquivos`]: folha de arquivos da sessão (navegar, baixar, enviar, confirmação).
//! - [`compartilhar`]: "Compartilhar com TailDesk" (envio sem tela da sessão).
//! - [`demo`]: modo demonstração (host de mentira); [`qr`]: leitura do QR de pareamento.
//! - [`textos`]: pt-BR e en dos textos montados aqui.
//!
//! Telas: [`ui`] (Slint). No Android, [`android`] tem o `android_main` e a ponte JNI com a casca
//! Kotlin (estado do Tailscale, QR, abrir o app do Tailscale). Código de SO fica no overlay
//! (`MobileVpnOverlay`) e no backend MediaCodec do `taildesk-codec`, como manda a regra 8.

/// Mensagem de diagnóstico: logcat no Android, stderr fora dele. Nunca com chaves ou segredos.
#[macro_export]
macro_rules! registrar {
    ($($t:tt)*) => {{
        #[cfg(target_os = "android")]
        log::warn!($($t)*);
        #[cfg(not(target_os = "android"))]
        eprintln!($($t)*);
    }};
}

#[cfg(target_os = "android")]
pub mod android;
pub mod app;
pub mod area;
pub mod arquivos;
#[cfg(not(target_os = "android"))]
pub mod captura;
pub mod ciclo;
pub mod compartilhar;
pub mod controle;
pub mod demo;
pub mod dispositivos;
pub mod fisico;
pub mod gestos;
pub mod qr;
pub mod rede;
pub mod sessao;
pub mod textos;
pub mod video;
pub mod zoom;

/// Telas Slint geradas de `ui/app.slint`.
#[allow(missing_debug_implementations, clippy::all)]
pub mod ui {
    slint::include_modules!();
}
