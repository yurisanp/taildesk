# 08 — Roadmap, ADRs, riscos e perguntas em aberto

## 1. Fases

### Fase 0 — Fundações (2–3 semanas)
- Workspace, CI (Win/mac/Linux), `cargo-deny`, `halyard-proto`, `halyard-overlay` com `EmbeddedOverlay` e `LanOverlay`.
- `halyard-cli probe` (lista codecs/backends disponíveis) e `halyard-cli bench-net` (iperf-like sobre a tailnet).
- **Gate G0 (decisivo)**: medir throughput/latência do `tailscale` crate 0.6.x entre dois nós em conexão **direta** e via **DERP**: UDP datagramas a 20/50/100/200 Mbit/s, perda, jitter, CPU. Comparar com o `tailscaled` do sistema.
  - Se embutido ≥ 150 Mbit/s com CPU aceitável → PyroWave viável no modo embutido.
  - Se não → PyroWave só no modo sistema; abrir issue/contribuir upstream.

### Fase 1 — MVP desktop "ver e controlar" (6–8 semanas)
- Host Windows + macOS + Linux X11; controlador nas 3 plataformas.
- Captura nativa (DXGI/WGC, ScreenCaptureKit, XShm), **H.264** (MF/VT/VA-API + openh264 fallback), render wgpu NV12.
- QUIC (quinn) controle + datagramas, input (teclado físico + texto, mouse), cursor separado.
- Autorização por tags + consentimento local. UI Slint: lista de peers, conectar, janela de sessão, HUD egui.
- **Aceite**: 1080p60 em LAN com < 40 ms glass-to-glass; reconexão sobrevive a troca de Wi-Fi.

### Fase 2 — Codecs e qualidade (4–6 semanas)
- AV1 (HW onde houver; dav1d decode), HEVC (HW), VP9/VP8 (libvpx), negociação completa + fallback + probe com cache.
- 4:4:4 para modo texto; QoS baseado em BBR + ReceiverReport; intra-refresh/LTR; NACK/PictureLoss.
- Multi-monitor, áudio Opus, clipboard texto/imagem, transferência de arquivos.

### Fase 3 — PyroWave e zero-copy (3–5 semanas)
- `pyrowave-sys`/`pyrowave` (Vulkan + Metal), interop com o device do wgpu, packetização 1:1 em datagramas.
- Zero-copy captura→encode e decode→render em todos os desktops.
- Política automática de entrada/saída do modo LAN.
- **Aceite**: < 16 ms glass-to-glass 1080p60 em gigabit; transição automática p/ AV1/H.264 ao cair a banda.

### Fase 4 — Mobile (6–8 semanas)
- Android: controlador (toque → mouse/teclado virtual, gestos) + host (MediaProjection + MediaCodec + AccessibilityService). Overlay = sistema (app Tailscale).
- iOS: controlador; host view-only via Broadcast Upload Extension. Overlay = sistema.
- Slint em iOS (tech preview) — validar cedo; plano B Makepad.

### Fase 5 — Produção
- Serviço/daemon (Windows SYSTEM + secure desktop/UAC, LaunchDaemon, systemd), Wayland (portal + libei), modo não-assistido, auditoria, auto-update assinado, instaladores, provisionamento via API Tailscale (auth keys com tags), integração Headscale.

## 2. ADRs (decisões registradas)

| # | Decisão | Alternativas | Motivo |
|---|---|---|---|
| ADR-001 | Tailscale como overlay; sem servidor próprio de rendezvous/relay | RustDesk hbbs/hbbr, iroh, WebRTC+TURN | Identidade, ACL, NAT traversal e relay prontos; self-host via Headscale |
| ADR-002 (substituído pelo ADR-009) | Dois overlays (embutido tailscale-rs + sistema/LocalAPI) atrás de trait | Só um dos dois | Embutido = UX sem instalar Tailscale; sistema = mobile + maturidade |
| ADR-003 | QUIC (quinn) sobre a tailnet: streams + datagramas | TCP único (RustDesk), UDP cru, WebRTC | Sem HOL, CC moderno, multiplexação; WebRTC só p/ cliente web futuro |
| ADR-004 | Protobuf (prost) no controle; header binário fixo na mídia | postcard/rkyv/flatbuffers | Evolução de schema + hot path sem alocação |
| ADR-005 | Backends HW nativos por SO; FFmpeg opcional | FFmpeg como base (RustDesk/hwcodec) | Zero-copy, controle fino (LTR, intra refresh), licenças |
| ADR-006 | Slint para UI + wgpu para vídeo; egui só HUD | GPUI, Makepad, iced, Tauri/Dioxus | Único com 5 plataformas + DSL de design + integração wgpu |
| ADR-007 | PyroWave como modo LAN opcional | Só codecs inter | Latência sub-ms de codec quando há banda |
| ADR-008 | Não reutilizar código do RustDesk | Fork AGPL | Licença AGPL-3.0 |
| ADR-009 (15/10/2026, substitui ADR-002) | **Só o Tailscale instalado na máquina** (`tailscaled` ≥ 1.102 + LocalAPI); sem nó embutido, crate `tailscale` fora do projeto. A trait `Overlay` fica, com `SystemOverlay` em produção e `LanOverlay` em dev | Embutido como padrão (ADR-002) | Decisão do Yuri. Ganha `whois` com usuário e caps, caminho informado, conexão direta madura e uma dependência pre-1.0 a menos; custa exigir o Tailscale instalado |

## 3. Riscos

| Risco | Prob. | Impacto | Mitigação |
|---|---|---|---|
| tailscale-rs pre-1.0: API quebra, throughput insuficiente | Média | Alto | Trait `Overlay`, versão fixa, gate G0, fallback modo sistema |
| tailscale-rs sem Android/iOS | Certo (hoje) | Médio | Modo sistema no mobile |
| Identidade de usuário indisponível no modo embutido | Alta | Médio | Autorizar por tags + caps de nó; usuário via modo sistema; acompanhar upstream (Q1) |
| Slint iOS ainda *preview* | Média | Médio | Validar na Fase 0/4; plano B Makepad |
| Drivers de encoder HW instáveis | Alta | Médio | Probe isolado com timeout + cache + blacklist + fallback SW |
| Patentes HEVC/H.264 em distribuição comercial | Média | Alto | OpenH264 binário Cisco; HEVC só via HW do SO; parecer jurídico; AV1/VP9 royalty-free |
| Wayland fragmentado | Alta | Médio | Portal + libei como caminho oficial; uinput como opção avançada |
| Políticas de loja (Accessibility no Android, App Store) | Média | Médio | Declarações de uso; distribuição fora da loja p/ host desktop |
| PyroWave API/ABI mudando | Baixa (C API v1.0) | Baixo | `pyrowave-sys` com versão do submódulo fixada |

## 4. Perguntas em aberto

- **Q1** — *(resolvida pelo ADR-009: o `whois` da LocalAPI dá usuário e caps)* O `tailscale` crate expõe (ou vai expor) *peer capabilities*/perfil de usuário do peer conectado (equivalente ao `whois`)? Existe crate interno `ts_peercapability`; abrir issue upstream se necessário.
- **Q2** — Overhead de QUIC/TLS sobre WireGuard em 200 Mbit/s: aceitável ou usar UDP cru com autenticação leve no modo LAN?
- **Q3** — Usar um único `VkDevice` compartilhado entre wgpu, Vulkan Video (`gpu-video`) e PyroWave? (simplifica zero-copy, mas acopla versões).
- **Q4** — Modelo de distribuição: produto interno (Aglets), comercial, ou open-source? Afeta licença do Slint, escolha de codecs SW e AGPL.
- **Q5** — Control server: tailnet SaaS do Tailscale, Headscale self-hosted, ou ambos? (o tailscale-rs fala com ambos).
- **Q6** — Precisa de cliente **web** (navegador)? Se sim, adicionar gateway WebRTC (`str0m`) — fora da tailnet ou via Tailscale Serve.
