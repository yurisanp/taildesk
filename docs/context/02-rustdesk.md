# 02 — RustDesk: o que aprender (e o que não copiar)

Repositório: <https://github.com/rustdesk/rustdesk> — ~125 mil estrelas, **AGPL-3.0**, núcleo em
Rust, UI em Flutter (Sciter legado/deprecated). Estudado a partir do `master` em 15/10/2026.

> ⚖️ **Licença**: AGPL-3.0 é copyleft forte e cobre uso em rede. **Não copie código do
> RustDesk** para um produto que não seja AGPL. Use-o como referência de arquitetura e de
> armadilhas de plataforma; reimplemente. Crates de terceiros que ele usa (ex.: `enigo`
> upstream, `cpal`, `magnum-opus`) têm licenças próprias — verifique cada uma.

## 1. Estrutura do repositório

| Caminho | Função |
|---|---|
| `libs/hbb_common` (submódulo → `rustdesk/hbb_common`) | Config, wrappers TCP/UDP/KCP/WebSocket, cripto (sodiumoxide), `rendezvous.proto`. Compartilhado com o servidor. |
| `libs/base` | `message.proto` (protocolo peer-to-peer), FS para transferência, teclado/plataforma. |
| `libs/scrap` | **Captura de tela + codecs** (vpx, aom, hwcodec, vram, mediacodec, câmera, gravação). |
| `libs/enigo` | Fork do `enigo` para injeção de teclado/mouse. |
| `libs/clipboard` | Copiar/colar **arquivos** (Windows/Linux/macOS). |
| `libs/virtual_display`, `libs/remote_printer`, `libs/portable` | Display virtual (IDD no Windows), impressora remota, modo portátil. |
| `src/server/*` | Serviços do lado host: `video_service`, `audio_service`, `input_service`, `clipboard_service`, `display_service`, `video_qos`, `connection.rs`, `wayland.rs`, `uinput.rs`, `portable_service.rs`, `terminal_service.rs`, `port_forward_mux.rs`. |
| `src/client.rs` | Lado controlador: estabelece conexão, decodifica, encaminha input. |
| `src/rendezvous_mediator.rs` | Registro no servidor de rendezvous, hole-punching, relay, **direct server**. |
| `src/platform/*` | Código por SO (serviço Windows, launchd, etc.). |
| `flutter/` | UI desktop + mobile (via `flutter_rust_bridge`). |

O repo já traz `AGENTS.md`/`CLAUDE.md` — útil para ver como eles orientam agentes.

## 2. Topologia de rede do RustDesk (o que o Tailscale substitui)

```
            ┌───────────── hbbs (rendezvous, 21116 tcp/udp) ─────────────┐
 controller ┤  registro de ID, troca de endpoints, hole-punching TCP/UDP  ├ host
            └───────────── hbbr (relay, 21117) se o P2P falhar ──────────┘
            portas WebSocket: 21118 (rendezvous ws), 21119 (relay ws)
 "Direct IP access": o host escuta em 21118/tcp (RENDEZVOUS_PORT+2, configurável)
```

- O guia oficial do Tailscale "Access remote desktops with RustDesk" usa justamente o
  **Direct IP access**: ativa no host, conecta pelo `100.x.y.z` → elimina hbbs/hbbr.
  Nosso produto é a versão nativa dessa ideia.
- Handshake de segurança (`secure_tcp`): troca de chaves X25519 (`box_`) assinada por Ed25519
  do servidor → chave simétrica `secretbox` (XSalsa20-Poly1305) para o stream. **No nosso caso,
  WireGuard já cifra E2E**; criptografia de aplicação extra é opcional (ver `06-arquitetura.md` §6).
- Login: `LoginRequest` com senha hasheada (sha256 com salt + challenge do host), 2FA opcional,
  senha de uso único ou permanente. **No nosso caso, identidade vem da tailnet**; senha local
  fica só como fator opcional/de consentimento.

## 3. Protocolo (`libs/base/protos/message.proto`, ~1000 linhas, protobuf)

Mensagem raiz: `oneof` com ~34 variantes. As que importam:

```proto
message Message { oneof union {
  SignedId signed_id = 3; PublicKey public_key = 4; TestDelay test_delay = 5;
  VideoFrame video_frame = 6; LoginRequest login_request = 7; LoginResponse login_response = 8;
  Hash hash = 9; MouseEvent mouse_event = 10; AudioFrame audio_frame = 11;
  CursorData cursor_data = 12; CursorPosition cursor_position = 13; uint64 cursor_id = 14;
  KeyEvent key_event = 15; Clipboard clipboard = 16; FileAction file_action = 17;
  FileResponse file_response = 18; Misc misc = 19; Cliprdr cliprdr = 20; PeerInfo peer_info = 25;
  PointerDeviceEvent pointer_device_event = 26; Auth2FA auth_2fa = 27;
  MultiClipboards multi_clipboards = 28; ScreenshotRequest/Response = 29/30;
  TerminalAction/Response = 31/32; PortForwardChannel = 33; OSLogin os_login = 34; } }

message VideoFrame {
  oneof union {
    EncodedVideoFrames vp9s = 6;  RGB rgb = 7;  YUV yuv = 8;
    EncodedVideoFrames h264s = 10; EncodedVideoFrames h265s = 11;
    EncodedVideoFrames vp8s = 12;  EncodedVideoFrames av1s = 13;
  }
  int32 display = 14;          // multi-monitor
}
message EncodedVideoFrame  { bytes data = 1; bool key = 2; int64 pts = 3; }
message EncodedVideoFrames { repeated EncodedVideoFrame frames = 1; }

message CodecAbility { bool vp8 = 1; bool vp9 = 2; bool av1 = 3; bool h264 = 4; bool h265 = 5; }
message SupportedEncoding { bool h264 = 1; bool h265 = 2; bool vp8 = 3; bool av1 = 4; CodecAbility i444 = 5; }
message SupportedDecoding {
  enum PreferCodec { Auto = 0; VP9 = 1; H264 = 2; H265 = 3; VP8 = 4; AV1 = 5; }
  int32 ability_vp9 = 1; int32 ability_h264 = 2; int32 ability_h265 = 3; PreferCodec prefer = 4;
  int32 ability_vp8 = 5; int32 ability_av1 = 6; CodecAbility i444 = 7; Chroma prefer_chroma = 8;
}
```

Lições:
1. **Codec é negociado por sessão**: cada controlador envia `SupportedDecoding` (dentro de
   `OptionMessage` no login) com habilidades + preferência; o host intersecta com o que sabe
   **codificar** (`SupportedEncoding`) e escolhe um codec **comum a todos os viewers** (o host
   tem um encoder por display, compartilhado entre conexões). Quando um viewer entra/sai,
   `Encoder::update(EncodingUpdate::{Update,Remove,…})` recalcula e pode trocar de codec.
2. **Suporte a 4:4:4 (i444)** é negociado separado (texto nítido em desktop) — ótima ideia.
3. Tudo trafega **num único stream TCP/KCP** com framing de tamanho — simples, mas causa
   *head-of-line blocking* (um pacote de vídeo perdido atrasa o input). **Nós vamos separar
   controle (confiável) de mídia (datagramas)** — ver `07-protocolo.md`.
4. `EncodedVideoFrame.pts` + `key` + `display` é o mínimo necessário por frame.

## 4. Pipeline de vídeo (`libs/scrap`)

```
Captura (por SO) → conversão (RGB→I420/NV12/I444, libyuv) → Encoder (trait EncoderApi) → VideoFrame
                        ↑ zero-copy no caminho "vram" (textura D3D11 → encoder HW)
```

- Backends de captura: `dxgi` (Windows Desktop Duplication), `quartz` (macOS, Core Graphics;
  o fork do `cpal` usa ScreenCaptureKit para capturar o **áudio** do sistema), `x11`, `wayland` (PipeWire via portal), `drm`
  (leitura KMS direta no Linux), `android` (MediaProjection via JNI), câmera (`nokhwa`).
- Encoders:
  - `vpxcodec.rs` → **libvpx** (VP8/VP9) — SW, sempre disponível; **default histórico = VP9**.
  - `aom.rs` → **libaom** (AV1) — SW; desabilitado em 32 bits por ser lento.
  - `hwcodec.rs` → crate `rustdesk-org/hwcodec` (wrapper sobre **FFmpeg**): NVENC, AMF, QSV,
    VideoToolbox, MediaCodec, VA-API… entrada em RAM (NV12). "HWRAM".
  - `vram.rs` → `hwcodec/vram`: **entrada direto de textura** (D3D11) → encoder HW sem cópia
    para RAM (Windows). "VRAM".
  - `mediacodec.rs` → decodificação HW no Android.
- Trait central (resumo de `codec.rs`):
  ```rust
  pub trait EncoderApi {
      fn new(cfg: EncoderCfg, i444: bool) -> ResultType<Self> where Self: Sized;
      fn encode_to_message(&mut self, frame: EncodeInput, ms: i64) -> ResultType<VideoFrame>;
      fn yuvfmt(&self) -> EncodeYuvFormat;
      fn input_texture(&self) -> bool;          // feature vram
      fn set_quality(&mut self, ratio: f32) -> ResultType<()>;
      fn bitrate(&self) -> u32;
      fn support_changing_quality(&self) -> bool;
      fn latency_free(&self) -> bool;
      fn is_hardware(&self) -> bool;
      fn disable(&self);
  }
  ```
- **Fallback**: se criar encoder HW falha, limpa a config de HW e volta para **VP9**.
  Há teste de "sanidade" dos encoders HW na inicialização (processo separado) porque drivers
  quebram de formas criativas.
- **Bitrate base por resolução** (kbps, interpolado por nº de pixels): 720p≈1000, 1080p≈2073,
  1440p≈3000, 4K≈5000, 8K≈12000; multiplicado por um `ratio` de qualidade. Bom ponto de
  partida para *auto*; mas são valores conservadores (pensados p/ VP9 em internet ruim).
- **Threads do codec** calculadas a partir de CPU livre e memória (máx. 64 p/ libvpx/libaom,
  16 p/ FFmpeg).

## 5. QoS (`src/server/video_qos.rs`)

- Mede atraso por usuário (`TestDelay` ida-e-volta), mantém média, ajusta **FPS** e **ratio de
  bitrate** (VBR), com aceleração no início da sessão e recuperação gradual.
- Usuário pode fixar FPS/qualidade customizados; o QoS respeita o maior FPS pedido entre viewers.
- Nós devemos fazer melhor usando sinais de transporte reais (perda, RTT, banda estimada pelo
  QUIC/BBR, *queue delay*) em vez de só RTT de aplicação.

## 6. Input, clipboard, áudio

- Input: `MouseEvent`, `KeyEvent` (modo *map* por keycode/scancode vs. *translate* por caractere,
  importante para layouts diferentes — ABNT2 ↔ US), `PointerDeviceEvent` (touch).
  Linux Wayland: `uinput` (requer privilégio) ou portal RemoteDesktop.
- Clipboard: texto/imagem/HTML (`MultiClipboards`) + **arquivos** via `Cliprdr` (mapeia o
  protocolo de clipboard de arquivos do RDP).
- Áudio: `cpal` (fork) + **Opus** (`magnum-opus`); no Linux usa PulseAudio direto.

## 7. Plataformas especiais (aprendizados)

- **Windows**: roda como **serviço** (SYSTEM) que lança um processo na sessão do usuário para
  capturar a tela de login/UAC (*secure desktop*); "privacy mode" com display virtual; modo portátil
  com elevação sob demanda.
- **macOS**: exige permissões TCC (Screen Recording, Accessibility, Input Monitoring); launchd agent.
- **Linux**: X11 é tranquilo; Wayland exige portal (diálogo de permissão), e tela de login (GDM)
  é limitada.
- **Android**: host via MediaProjection + **AccessibilityService** para input; precisa de
  permissões e de manter serviço em foreground.
- **iOS**: o RustDesk iOS é **apenas controlador** (iOS não permite controlar o aparelho remotamente).

## 8. O que fazer diferente

| RustDesk | Nosso produto |
|---|---|
| Servidor de rendezvous/relay próprio | Tailscale (control + DERP) |
| ID numérico + senha | Identidade da tailnet + grants/app capabilities + consentimento local |
| Um stream TCP para tudo | QUIC: streams confiáveis (controle/arquivos) + datagramas (mídia) |
| FFmpeg via wrapper próprio | Backends nativos por SO (MF/VT/VA-API/Vulkan Video/MediaCodec) + FFmpeg opcional |
| Flutter + flutter_rust_bridge | 100% Rust (Slint + wgpu) |
| VP9 como default | Seleção por capacidade e rede: AV1/H.265 HW > H.264 HW > VP9 SW; PyroWave em LAN |
| QoS baseado em RTT de app | QoS baseado em estimativa de banda/perda do transporte + *frame pacing* |
