# 06 — Arquitetura proposta

## 1. Visão de alto nível

```
┌──────────────────────── CONTROLADOR ────────────────────────┐      ┌────────────────────────── HOST ──────────────────────────┐
│ UI (Slint) ── SessionController ── InputCapture (teclado/   │      │ ServiceHost (serviço/agent) ── PolicyEngine (grants+     │
│   │              │                 mouse/touch/gamepad)     │      │   │                            consentimento local)       │
│   ▼              ▼                                           │      │   ▼                                                      │
│ Renderer(wgpu) ◄─ Decoder(HW/SW) ◄─ JitterBuffer ◄─┐        │      │ Capturer ─► Converter(GPU) ─► Encoder(HW/SW) ─► Packetizer│
│                                                    │        │      │ CursorSource ─────────────────────────────────────┐      │
│                       Transport (QUIC: streams + datagrams) ◄═══════► Transport                                         ▼      │
│                       Overlay (tailscale-rs | tailscaled)   │ tailnet│ Overlay           InputInjector ◄── control stream      │
└─────────────────────────────────────────────────────────────┘ (WG) └──────────────────────────────────────────────────────────┘
```

## 2. Workspace Cargo

```
halyard/
├─ Cargo.toml                 # [workspace], edition 2024, resolver 3, profiles (release: lto="fat", codegen-units=1, panic="abort" p/ bins)
├─ crates/
│  ├─ halyard-proto/          # .proto (prost) + cabeçalhos binários de mídia; sem dependências de SO
│  ├─ halyard-overlay/        # trait Overlay + EmbeddedOverlay(tailscale) + SystemOverlay(LocalAPI) + LanOverlay(dev)
│  ├─ halyard-tsapi/          # cliente LocalAPI (status, whois) — unix socket / named pipe / macOS
│  ├─ halyard-transport/      # quinn sobre Overlay (AsyncUdpSocket adapter), pinning de certificados, canais tipados
│  ├─ halyard-codec/          # traits VideoEncoder/VideoDecoder, CodecRegistry, probe, negociação, rate control
│  │  ├─ backends/mf.rs  vt.rs  vaapi.rs  vulkan.rs  nvenc.rs  mediacodec.rs  openh264.rs  dav1d.rs  vpx.rs  ffmpeg.rs  pyrowave.rs
│  ├─ pyrowave-sys/  pyrowave/ # FFI + wrapper seguro
│  ├─ halyard-media/          # pipeline host (capture→encode→send) e cliente (recv→jitter→decode), pacing, QoS, áudio Opus
│  ├─ halyard-platform/       # traits de SO + impls cfg (capture, input, cursor, clipboard, audio, secrets, service)
│  ├─ halyard-render/         # wgpu: YUV→RGB, escala, cursor overlay, import zero-copy por SO
│  ├─ halyard-session/        # máquina de estados da sessão, autorização, consentimento, transferência de arquivos, chat
│  ├─ halyard-ui/             # Slint (.slint + bindings), design tokens, i18n
│  └─ halyard-hud/            # egui debug overlay (feature "hud")
├─ apps/
│  ├─ halyard-desktop/        # binário GUI (controlador + host)
│  ├─ halyard-service/        # serviço/daemon do host (Windows service, launchd, systemd)
│  ├─ halyard-cli/            # probe, bench, connect headless, provisionamento
│  ├─ android/                # Gradle + cargo-ndk; Kotlin mínimo (consentimento MediaProjection, AccessibilityService)
│  └─ ios/                    # Xcode: app + Broadcast Upload Extension; Swift mínimo
└─ xtask/                     # build de pyrowave, empacotamento, geração de bindings
```

Regra de dependência (camadas): `proto` ← `overlay`/`tsapi` ← `transport` ← `codec`/`platform` ← `media`/`render` ← `session` ← `ui` ← `apps`. Nenhum crate de baixo conhece Slint, e nenhum conhece SO concreto fora de `platform`, `codec/backends`, `render/interop` e `overlay`.

## 3. Traits centrais (esboço)

```rust
// halyard-overlay
#[async_trait::async_trait]
pub trait Overlay: Send + Sync + 'static {
    async fn local_addr(&self) -> Result<IpAddr>;
    async fn udp_bind(&self, port: u16) -> Result<Box<dyn DatagramSocket>>;
    async fn tcp_listen(&self, port: u16) -> Result<Box<dyn Listener>>;   // opcional (fallback)
    async fn peers(&self) -> Result<Vec<PeerInfo>>;                       // hostname, ip, os, online, tags
    async fn identify(&self, remote: SocketAddr) -> Result<PeerIdentity>; // whois: user?, node, tags, caps
    async fn path(&self, remote: IpAddr) -> PathKind;                     // Direct{rtt} | Relayed{derp_region} | Unknown
}
pub struct PeerIdentity {
    pub node_stable_id: String,
    pub hostname: String,
    pub user_login: Option<String>,            // None no modo embutido (ver Q1)
    pub tags: Vec<String>,
    pub app_caps: Vec<serde_json::Value>,      // conteúdo de "aglets.com.br/cap/halyard"
}

// halyard-codec
pub trait VideoEncoder: Send {
    fn info(&self) -> &EncoderInfo;                       // codec, backend, hw, input kinds, 444, ltr
    fn encode(&mut self, frame: InputFrame<'_>, ctl: &EncodeControl) -> Result<Option<EncodedFrame>>;
    fn reconfigure(&mut self, p: &RateParams) -> Result<()>; // bitrate/fps sem recriar
    fn request_keyframe(&mut self);
    fn invalidate_refs(&mut self, last_good: FrameSeq) -> bool; // LTR/RPSI; false = não suporta
}
pub enum InputFrame<'a> {
    Cpu { planes: [&'a [u8]; 3], strides: [usize; 3], fmt: PixFmt, w: u32, h: u32 },
    Gpu(GpuFrame),            // D3D11Texture | IOSurface/CVPixelBuffer | DmaBuf | AHardwareBuffer | VkImage
}
pub trait VideoDecoder: Send {
    fn decode(&mut self, au: &EncodedFrame) -> Result<Option<DecodedFrame>>; // DecodedFrame = GPU quando possível
    fn flush(&mut self);
}

// halyard-platform
pub trait ScreenCapturer: Send {
    fn displays(&self) -> Result<Vec<DisplayInfo>>;
    fn start(&mut self, display: DisplayId, opts: CaptureOpts, sink: FrameSink) -> Result<()>; // push model, um por monitor
    fn pause(&mut self, display: DisplayId);   // ninguém vendo: não capturar nem codificar
    fn resume(&mut self, display: DisplayId);
    fn stop(&mut self, display: DisplayId);
    fn events(&mut self) -> DisplayEvents;     // DisplayEvent::{Added, Removed, Changed} -> DisplaysChanged/VideoStopped
}
pub trait InputInjector: Send {
    fn inject(&mut self, ev: InputEvent) -> Result<()>;   // KeyPhysical{hid,down} | Text(String) | Mouse{..} | Touch{..} | Scroll{..}
}
```

## 4. Pipeline do host (por display)

```
[SO callback thread]  frame (GPU texture) + dirty rects + timestamp
        │  (descarta se o encoder ainda está ocupado: "latest-frame-wins", nunca enfileira)
        ▼
[encode thread dedicada, prioridade alta]
   converter GPU (BGRA→NV12/I444) → encoder.encode() → EncodedFrame
        │
        ▼
[tokio: packetizer + pacer]
   fragmenta em datagramas ≤ 1200 B com header de mídia → pacing (espalha o frame em ~1/2 intervalo)
   FEC opcional (XOR/RS sobre grupos) p/ links com perda; NACK cache curto (último ~100 ms)
        ▼
   quinn::Connection::send_datagram
```
- **Um pipeline por monitor** (captura → encoder → pacer) e **um pacer único** que divide a banda por prioridade: `FOCUSED` ≈ 70% com FPS cheio, `VISIBLE` divide o resto a 5–15 fps, aba escondida = `StopVideo` (zero custo). Limite padrão de 2 streams simultâneos com openh264 (`13-multimonitor.md` §5).
- Encoder pool **por display**; múltiplos viewers no mesmo display compartilham o encoder (codec = interseção, como no RustDesk), ou encoders separados se as capacidades divergirem muito (config).
- Sem frames novos (tela estática) → envia nada; a cada 1 s um `Heartbeat` de mídia.
- **Orçamento de latência** (alvo LAN 1080p60): captura ≤ 2 ms, conversão ≤ 1 ms, encode HW ≤ 4 ms (PyroWave < 0,5), rede ≤ 2 ms, decode ≤ 3 ms, render + vsync ≤ 8 ms → **< 20 ms glass-to-glass**. Internet: rede domina; meta < 80 ms com RTT 40 ms.

## 5. Pipeline do controlador

```
datagramas → reassembler (por frame_seq) → jitter buffer adaptativo (alvo 0 em LAN; ~p95 jitter na internet)
          → decoder (HW, saída GPU) → renderer wgpu (YUV→RGB, escala com filtro p/ texto, cursor local)
          → present (mailbox/immediate se disponível; evitar FIFO longo)
input local → InputEvent → stream de controle QUIC (prioridade máxima) + coalescência de mouse move (≤ 1 por ms)
```
- Cursor desenhado localmente com a forma recebida → movimento do mouse parece instantâneo mesmo com vídeo atrasado.
- Medir e exibir `present_time - capture_time` usando timestamps do host + offset de relógio estimado (tipo NTP no handshake).

## 6. Segurança

Camadas:
1. **Rede**: WireGuard (tailnet) — E2E, e o pacote nem chega ao app se a policy não permitir (`ip` em grants).
2. **Transporte**: QUIC/TLS 1.3 (`rustls`). Certificado autoassinado gerado por instalação; *fingerprint* trocado/pinned no primeiro contato (TOFU) **e** vinculado à identidade da tailnet (`node_stable_id`). Isso protege contra um nó comprometido na mesma tailnet se passando por outro IP após reconfiguração. (Opcional desligar TLS em LAN de laboratório para medir overhead — nunca em produção.)
3. **Autorização**: `PolicyEngine` no host decide com `PeerIdentity` (tags + app caps + usuário) + config local (lista de permitidos, horários) → `Permissions { view, control, clipboard, files, audio, terminal }`. Negado → fecha com motivo.
4. **Consentimento**: modo assistido mostra diálogo com nome/usuário/dispositivo do técnico e permissões pedidas; timeout = nega. Modo não-assistido só se a config local e a capability permitirem (`unattended: true`).
5. **Transparência**: indicador permanente de "sessão ativa" no host com botão de encerrar; log de auditoria local (JSON lines) + opcional envio a um coletor.
6. **Hardening**: parsers de protocolo com fuzzing; limites de tamanho em todas as mensagens; transferência de arquivos confinada a diretórios escolhidos; o serviço privilegiado (SYSTEM/root) expõe IPC mínimo e autenticado ao agent; nada de comandos arbitrários.
7. **Segredos**: estado do nó Tailscale, chave TLS e config sensível no `SecretStore` do SO.

## 7. Máquina de estados da sessão

```
Idle → Dialing(overlay connect) → Handshake(Hello/caps/auth) → AwaitConsent → Negotiating(codec)
     → Live ⇄ Degraded(relay/low bw) ⇄ Reconnecting(≤ 30 s, preserva sessão) → Closed(reason)
```
Reconexão: QUIC suporta *connection migration*; se o caminho mudar (direto→DERP) a sessão continua e o QoS reajusta o codec/bitrate.

## 8. Concurrency model
- `tokio` multi-thread para rede/sessão.
- Threads dedicadas (não-tokio) para: callback de captura, encode, decode, áudio realtime. Comunicação por canais *bounded* de capacidade 1 (latest-wins) ou `triple buffer`.
- Nunca bloquear o runtime tokio com encode/decode; nunca alocar no hot path (pools de buffers/texturas).
