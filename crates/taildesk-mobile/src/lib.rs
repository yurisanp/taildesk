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
//! - [`demo`]: modo demonstração (host de mentira); [`qr`]: leitura do QR de pareamento.
//! - [`textos`]: pt-BR e en dos textos montados aqui.
//!
//! Telas: [`ui`] (Slint). No Android, [`android`] tem o `android_main` e a ponte JNI com a casca
//! Kotlin (estado do Tailscale, QR, abrir o app do Tailscale). Código de SO fica no overlay
//! (`MobileVpnOverlay`) e no backend MediaCodec do `taildesk-codec`, como manda a regra 8.

pub mod ciclo;
pub mod controle;
pub mod demo;
pub mod dispositivos;
pub mod gestos;
pub mod qr;
pub mod rede;
pub mod textos;
pub mod zoom;
