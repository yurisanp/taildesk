# 09 — Fontes e versões verificadas (15/10/2026)

## Tailscale
- Documentação: https://tailscale.com/docs
- How Tailscale works: https://tailscale.com/blog/how-tailscale-works
- How NAT traversal works: https://tailscale.com/blog/how-nat-traversal-works
- Anúncio do tailscale-rs (15/04/2026): https://tailscale.com/blog/tailscale-rs-rust-tsnet-library-preview
- Repositório tailscale-rs: https://github.com/tailscale/tailscale-rs (README, CHANGELOG.md, ARCHITECTURE.md, `src/lib.rs`, `crates/control_plane/ts_control/src/node.rs`) — commit de 07/10/2026, crate `tailscale` 0.6.1
- App capabilities (grants): https://tailscale.com/docs/features/access-control/grants/grants-app-capabilities
- Policy file syntax: https://tailscale.com/docs/reference/syntax/policy-file
- Auth keys: https://tailscale.com/docs/features/access-control/auth-keys
- RustDesk sobre Tailscale: https://tailscale.com/docs/solutions/access-remote-desktops-with-rustdesk
- Headscale: https://github.com/juanfont/headscale
- Cliente Go / LocalAPI (referência de endpoints): https://github.com/tailscale/tailscale (`client/local`, `ipn/localapi`)

## RustDesk
- Repositório: https://github.com/rustdesk/rustdesk (AGPL-3.0) — `libs/base/protos/message.proto`, `libs/scrap/src/common/codec.rs`, `src/server/video_qos.rs`, `src/rendezvous_mediator.rs`, `src/common.rs`
- hbb_common: https://github.com/rustdesk/hbb_common (`protos/rendezvous.proto`, `src/config.rs` — portas 21116–21119)
- hwcodec: https://github.com/rustdesk-org/hwcodec

## PyroWave
- Repositório: https://github.com/Themaister/pyrowave (MIT; `pyrowave.h`, `metal/pyrowave_metal.h`) — commit de 13/10/2026
- Post de design: https://themaister.net/blog/2025/06/16/i-designed-my-own-ridiculously-fast-game-streaming-video-codec-pyrowave/
- Steam Remote Play (beta): https://www.gamingonlinux.com/2026/10/steam-client-beta-for-october-1st-brings-many-remote-play-pyrowave-improvements/

## UI
- Slint 1.12 (WGPU + iOS tech preview): https://slint.dev/blog/slint-1.12-released
- Projeto NLnet Slint iOS: https://nlnet.nl/project/SlintiOS/
- gpu-video (ex-vk-video): https://docs.rs/crate/gpu-video/latest

## Versões de crates (crates.io, 15/10/2026)
| Crate | Versão | | Crate | Versão |
|---|---|---|---|---|
| tailscale | 0.6.1 | | wgpu | 30.0.1 |
| quinn | 0.11.12 | | slint | 1.18.1 |
| tokio | 1.53.3 | | egui / eframe | 0.36.2 |
| prost | 0.14.4 | | gpui | 0.2.2 |
| rustls | 0.23.46 | | iced | 0.14.0 |
| str0m | 0.24.1 | | dioxus | 0.7.10 |
| webrtc | 0.21.1 | | makepad-widgets | 1.0.0 |
| iroh | 1.3.0 | | freya | 0.4.3 |
| gpu-video | 0.4.0 | | xilem | 0.4.0 |
| nvidia-video-codec-sdk | 0.4.0 | | tauri | 2.12.4 |
| libva | 0.1.4 | | windows | 0.62.2 |
| cros-codecs | 0.0.6 | | objc2-* (VT, SCK, ReplayKit) | 0.3.2 |
| openh264 | 0.9.8 | | ndk | 0.9.0 |
| dav1d | 0.11.1 | | jni | 0.22.4 |
| rav1e | 0.8.1 | | android-activity | 0.6.1 |
| vpx-rs | 0.2.1 | | windows-capture | 2.0.1 |
| ffmpeg-the-third | 6.0.0 | | ashpd | 0.13.13 |
| ffmpeg-next | 9.0.0 | | pipewire | 0.10.1 |
| yuv | 0.8.19 | | reis | 0.7.1 |
| fast_image_resize | 6.1.0 | | evdev | 0.13.2 |
| cpal | 0.18.2 | | xkbcommon | 0.9.0 |
| opus | 0.4.0 | | arboard | 3.7.0 |
| keyring | 4.2.0 | | xcap / enigo | 0.9.8 / 0.6.1 |
| uniffi | 0.32.2 | | cargo-ndk | 4.1.2 |
