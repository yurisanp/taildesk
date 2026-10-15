# 05 — Stack Rust: bibliotecas escolhidas (elegância + performance)

Versões verificadas no crates.io em **15/10/2026**. Pre-1.0 → fixar com `=` e atualizar de forma deliberada.

## 1. UI — escolha da framework

Requisitos: visual premium e coeso, animações suaves, **GPU**, 5 plataformas (incluindo mobile),
uma **superfície de vídeo de altíssima taxa** embutida (texturas wgpu/nativas, sem passar pela CPU),
acessibilidade razoável, i18n (pt-BR/en).

| Framework | Versão | Plataformas | Modelo | Vídeo/wgpu embutido | Prós | Contras |
|---|---|---|---|---|---|---|
| **Slint** | **1.18.1** | Win, mac, Linux, **Android**, **iOS (tech preview desde 1.12)**, embarcado | Declarativo (`.slint` DSL) + Rust; renderers femtovg/Skia/software | ✅ integração **WGPU** (1.12+) e import de texturas (exemplo `wgpu_texture`) | DSL ótima para design system, *live preview*, tooling (LSP), Material/Fluent/Cupertino styles, animações declarativas, licenças flexíveis (GPLv3 / royalty-free para desktop+mobile / comercial) | iOS ainda *preview*; licença royalty-free exige atribuição — confira termos |
| **Makepad** | 1.0.0 (widgets) | Win, mac, Linux, Android, iOS, Web | Shaders como estilo (DSL própria), live design | ✅ nativo (tudo é shader) | Visual muito sofisticado, 5 plataformas reais, performance excelente | Ecossistema pequeno, docs escassas, curva alta |
| **GPUI** (Zed) | 0.2.2 | **Só desktop** (mac, Linux, Windows) | Híbrido imediato/retido, Tailwind-like | ✅ (é GPU-first) | O mais "bonito" e rápido do ecossistema desktop (o Zed é a prova) | Sem mobile; API muda com o Zed; poucos widgets prontos (usar `gpui-component`) |
| **iced** | 0.14.0 | Desktop + Web | Elm | ✅ (`wgpu` shader widget) | Arquitetura limpa | Sem mobile oficial; estilização trabalhosa |
| **egui/eframe** | 0.36.2 | Desktop, Web, Android (iOS parcial) | Imediato | ✅ (callback wgpu) | Rapidíssimo para ferramentas/HUD | Estética "ferramenta", não produto premium |
| **Dioxus** | 0.7.10 | Desktop/mobile via **WebView** (+ renderer nativo Blitz em evolução) | React-like | ⚠️ vídeo em WebView = cópias e latência | Produtividade web | Ruim para a superfície de vídeo |
| **Freya** | 0.4.3 | Desktop (Skia) | Dioxus-like | ⚠️ | Bonito | Sem mobile |
| **Xilem** | 0.4.0 | Desktop | Reativo (Linebender, Vello) | ⚠️ | Futuro promissor | Imaturo |
| **Tauri** | 2.12.4 | 5 plataformas via WebView | HTML/CSS + Rust | ⚠️ idem Dioxus | Ecossistema web | Mesmo problema de vídeo |

### Recomendação
- **Slint** para toda a UI (casca do app, lista de dispositivos, configurações, onboarding, barra de sessão) nas 5 plataformas, com **a superfície de vídeo renderizada por `wgpu`** e composta pela integração WGPU do Slint (ou, em último caso, uma janela/layer nativa filha só para o vídeo).
- **egui** apenas como **HUD de diagnóstico** em builds dev (FPS, bitrate, RTT, perda, codec, tempo de encode/decode, caminho direto/DERP) desenhado sobre o vídeo.
- **Plano B** se Slint/iOS travar: Makepad (mantém 5 plataformas). **Plano C desktop-only**: GPUI.
- Isolar UI do core: o core expõe um `SessionController` assíncrono (canais), a UI só consome estado. Trocar de framework não toca no núcleo.

### Diretrizes de design ("elegante")
- Design tokens num único módulo `.slint` (`Tokens { radius, spacing, palette.light/dark, typography }`); tema claro/escuro seguindo o SO; cor de acento única.
- Tipografia: **Inter** (UI) + **JetBrains Mono** (IDs, IPs, estatísticas); tamanhos em escala modular (12/14/16/20/28).
- Janela de sessão **sem cromo**: vídeo de ponta a ponta; barra flutuante auto-ocultável no topo (monitor, qualidade, codec, chat, arquivos, fim de sessão); indicador discreto do caminho (● direto / ◐ relay).
- Microinterações de 120–200 ms com *easing* `cubic-bezier(.2,.8,.2,1)`; nunca animar sobre o vídeo durante interação.
- Lista de dispositivos = peers da tailnet (hostname, SO, online, tags) com busca instantânea, avatar do SO, último acesso.
- Estados explícitos: conectando → negociando → ao vivo → degradado (relay/banda baixa) → reconectando.
- Acessibilidade: navegação por teclado completa, contraste AA, textos do leitor de tela (Slint expõe AccessKit).
- i18n: `.slint` com `@tr()` + gettext; pt-BR e en desde o dia 1.

## 2. Núcleo assíncrono, serialização, observabilidade

| Área | Crate | Versão | Motivo |
|---|---|---|---|
| Runtime async | `tokio` | 1.53.3 | Exigido pelo `tailscale` crate e `quinn`. |
| Bytes | `bytes` | 1.12.1 | Buffers zero-copy no pipeline. |
| Protocolo (controle) | `prost` | 0.14.4 | Protobuf: evolução de schema, multi-linguagem (útil p/ ferramentas). Alternativa compacta: `postcard` 1.1.3 (serde) — **escolhemos protobuf** para versionamento robusto. |
| Cabeçalhos de mídia | manual (`zerocopy`/byte layout fixo) | — | Hot path: sem alocação, sem varint. |
| Erros | `thiserror` (libs) + `anyhow`/`eyre` (bins) | — | Padrão. |
| Logs/tracing | `tracing` 0.1.44 + `tracing-subscriber` + `tracing-tracy` (perf) | — | Spans por frame (capture→encode→send→recv→decode→present). |
| Métricas | `metrics` + exportador local | — | HUD e telemetria opcional. |
| Config | `serde` + `toml` + `directories` | — | |
| CLI | `clap` | — | `halyard host|connect|probe|bench`. |
| Mobile bindings | `uniffi` 0.32.2 (Kotlin/Swift) quando precisar de glue nativo | — | Activity de consentimento Android, broadcast extension iOS. |
| Android glue | `android-activity` 0.6.1, `jni` 0.22.4, `ndk` 0.9.0 | — | |
| Apple glue | `objc2` + `objc2-*` 0.3.2 (`-app-kit`, `-screen-capture-kit`, `-video-toolbox`, `-replay-kit`, `-core-video`, `-metal`) | — | |
| Windows glue | `windows` 0.62.2 / `windows-sys` 0.61.2 | — | |

## 3. Rede

| Área | Crate | Versão | Motivo |
|---|---|---|---|
| Tailnet embutida | **`tailscale`** (tailscale-rs) | **=0.6.1** | Ver `01-tailscale.md`. |
| LocalAPI (modo sistema) | `hyper` 1.x + `hyper-util` + `hyperlocal` (unix) / named pipe via `tokio::net::windows::named_pipe` | — | Cliente fino escrito por nós (`halyard-tsapi`). |
| API REST Tailscale (provisionamento) | `reqwest` (rustls) | — | Criação de auth keys, tags (admin tool). |
| Transporte | **`quinn`** | 0.11.12 | QUIC: streams confiáveis + **datagramas** (RFC 9221), controle de congestionamento (BBR/Cubic), 0-RTT, sem HOL entre streams. Roda sobre o `UdpSocket` da tailnet implementando `quinn::AsyncUdpSocket` (ou sobre socket do SO no modo sistema). TLS via `rustls` 0.23.46 com certificados autoassinados *pinned* pela identidade da tailnet. |
| Alternativas avaliadas | `str0m` 0.24.1 (WebRTC sans-IO), `webrtc` 0.21.1, `iroh` 1.3.0 | — | WebRTC só se houver cliente web no futuro (str0m é a melhor opção). `iroh` faz o mesmo papel do Tailscale (conexão por chave pública) — redundante. |

## 4. Mídia

| Área | Crate | Versão |
|---|---|---|
| GPU/render | `wgpu` | 30.0.1 |
| Vulkan Video (HW enc/dec) | `gpu-video` (ex-`vk-video`) | 0.4.0 |
| NVENC/NVDEC | `nvidia-video-codec-sdk` | 0.4.0 |
| VA-API | `libva` 0.1.4 / `cros-codecs` 0.0.6 | — |
| VideoToolbox | `objc2-video-toolbox` | 0.3.2 |
| Media Foundation | `windows` (features `Win32_Media_MediaFoundation`, `Win32_Graphics_Direct3D11`) | 0.62.2 |
| MediaCodec | `ndk` (`media`) | 0.9.0 |
| H.264 SW | `openh264` | 0.9.8 |
| AV1 decode SW | `dav1d` | 0.11.1 |
| AV1 encode SW | `rav1e` 0.8.1 (ou SVT-AV1 via FFmpeg) | — |
| VP8/VP9 SW | `vpx-rs` 0.2.1 / `env-libvpx-sys` 5.1.3 | — |
| FFmpeg (opcional) | `ffmpeg-the-third` 6.0.0 | — |
| PyroWave | `pyrowave-sys` + `pyrowave` (**a criar**, bindgen + CMake) | — |
| Cor/escala CPU | `yuv` 0.8.19, `fast_image_resize` 6.1.0 | — |
| Áudio | `cpal` 0.18.2 + `opus` 0.4.0 | — |
| Jitter buffer / pacing | implementação própria (`halyard-media::jitter`) | — |

## 5. Sistema

| Área | Crate | Versão |
|---|---|---|
| Captura Windows | `windows-capture` 2.0.1 / `windows` (DXGI) | — |
| Captura macOS | `objc2-screen-capture-kit` 0.3.2 | — |
| Captura Wayland | `ashpd` 0.13.13 + `pipewire` 0.10.1 | — |
| Captura X11 | `x11rb` | — |
| Input Wayland | `reis` 0.7.1 (libei) / `evdev` 0.13.2 (uinput) | — |
| Teclado | `xkbcommon` 0.9.0 | — |
| Clipboard | `arboard` 3.7.0, `clipboard-rs` 0.3.5, `wl-clipboard-rs` 0.9.4 | — |
| Segredos | `keyring` 4.2.0 | — |
| Protótipo cross-platform rápido | `xcap` 0.9.8, `enigo` 0.6.1 | (substituir por nativos) |

## 6. Qualidade e segurança da cadeia de dependências
- `cargo-deny` (licenças: bloquear GPL no binário se o produto for proprietário; permitir MIT/Apache/BSD/ISC/Zlib/MPL), `cargo-audit`, `cargo-vet` (o tailscale-rs já usa).
- `clippy -D warnings`, `rustfmt`, `cargo-nextest`, `criterion` para benches do pipeline, `cargo-fuzz` nos parsers do protocolo.
- Perf: `tracy` (spans por frame), `cargo-flamegraph`, *frame-time histograms* no HUD.
