# 00 — Relatório executivo

**Objeto**: estudo do Tailscale e do RustDesk como base para uma ferramenta própria de acesso remoto em Rust, multiplataforma, usando o Tailscale como camada de rede, com AV1/H.265/H.264/VP9/VP8/PyroWave.
**Data**: 15/10/2026. **Fontes**: documentação oficial do Tailscale, código-fonte de `tailscale/tailscale-rs` (main, 0.6.1), `rustdesk/rustdesk` (master) e `Themaister/pyrowave` (master), crates.io. Ver `09-fontes.md`.

## 1. Conclusões principais

### 1.1 O Tailscale resolve a parte mais cara de um produto de acesso remoto
O RustDesk precisa de dois servidores (rendezvous `hbbs` na 21116 e relay `hbbr` na 21117), de um sistema de IDs, de senhas e de hole-punching próprio. O Tailscale já entrega tudo isso com qualidade de produção: coordenação de chaves, NAT traversal, relays DERP, criptografia WireGuard ponta a ponta, identidade via SSO e **policies centralizadas aplicadas em cada nó**. O próprio Tailscale documenta o uso do RustDesk em "Direct IP access" pela tailnet — o produto proposto é a versão nativa e integrada dessa ideia.

### 1.2 Existe agora um Tailscale oficial em Rust — e ele amadureceu rápido
O `tailscale-rs` (crate `tailscale`) foi anunciado em abril/2026 como preview experimental. Em seis meses:
- 0.4.0 (jul): STUN, monitoramento de rede, SSH experimental.
- 0.5.0 (ago): nós não-efêmeros por padrão, correções de segurança.
- **0.6.0 (set)**: **auditoria de criptografia de terceiros concluída**, remoção dos avisos de "inseguro" e da variável de ambiente obrigatória, macOS com conexões diretas, buffers TCP maiores.
- `main` (out): conexões diretas "em muitos casos", lookup de peers por nome/IP.

Ainda é pre-1.0, **sem Android/iOS**, sem MagicDNS/DERP privado/app capabilities de alto nível, e o throughput do netstack userspace não está documentado. Por isso a recomendação é **encapsular** atrás de uma trait com dois backends (embutido e sistema) e medir cedo (gate G0).

### 1.3 O RustDesk é um excelente mapa de armadilhas — mas não uma base de código
AGPL-3.0 impede reaproveitar código num produto não-AGPL. O que vale estudar:
- **Negociação de codec por capacidade** (`SupportedDecoding`/`SupportedEncoding`, codec comum a todos os viewers do display, troca dinâmica).
- **4:4:4 negociado** separadamente para texto nítido.
- **Fallback de encoder HW → VP9** e teste de sanidade de drivers.
- Caminho **"vram"** (textura D3D11 direto no encoder) como otimização chave no Windows.
- **Serviço SYSTEM + agent por sessão** no Windows para capturar UAC/login; TCC no macOS; portal no Wayland; MediaProjection + Accessibility no Android; iOS só controlador.
O que fazer diferente: separar controle de mídia (QUIC streams + datagramas em vez de um único TCP), backends HW nativos em vez de wrapper FFmpeg, QoS dirigido por estimativa de banda, UI 100% Rust.

### 1.4 Codecs: dois "modos" de operação
- **Modo internet** (inter-frame, eficiente): AV1 HW > H.265 HW > H.264 HW > VP9 SW > H.264 SW > VP8 SW. Configuração low-latency: sem B-frames, GOP infinito + intra refresh, VBV ≈ 1 frame, LTR para recuperação sem IDR.
- **Modo LAN** (PyroWave): wavelet intra-only na GPU, ~0,1 ms de encode a 1080p segundo o autor, porém 100–200+ Mbit/s. Agora tem backend **Metal** (macOS/iOS) além de Vulkan (Win/Linux/Android), API C v1.0, licença MIT, e foi adotado experimentalmente pela Valve no Steam Remote Play. Só ativar com caminho direto e banda medida suficiente.

### 1.5 Multiplataforma: o núcleo é portável, as bordas não
Captura, input, cursor, clipboard, áudio do sistema, codecs HW, zero-copy de GPU, serviço/privilégios e empacotamento são **específicos por SO** (matriz completa em `04-plataformas.md`). Restrições duras:
- **iOS não pode ser controlado** (sem API de injeção) — só controlador ou host view-only via ReplayKit.
- **Android** controla via AccessibilityService e exige consentimento de captura por sessão (Android 14+).
- **Wayland** exige portal/diálogo; tela de login em geral inacessível.
- **Mobile usa o app oficial do Tailscale** (tailscale-rs não suporta Android/iOS ainda).

### 1.6 UI elegante e performática em Rust
Para 5 plataformas com uma superfície de vídeo GPU de alta taxa, **Slint 1.18** (DSL declarativa, estilos nativos, integração WGPU, Android estável, iOS em tech preview) + **wgpu 30** para o vídeo é a combinação mais equilibrada. GPUI (Zed) é a mais bonita no desktop, mas não tem mobile; Makepad cobre as 5 plataformas com visual excelente, porém ecossistema menor (plano B). WebView (Tauri/Dioxus) é inadequado para o caminho de vídeo.

## 2. Arquitetura recomendada (resumo)
`Overlay (tailscale-rs | tailscaled)` → `QUIC (quinn)` → controle protobuf + mídia em datagramas → `CodecRegistry` (backends nativos + PyroWave) → `wgpu` → `Slint`. Autorização: tags + grants + app capability `aglets.com.br/cap/halyard` + consentimento local. Detalhes em `06-arquitetura.md` e `07-protocolo.md`.

## 3. Próximos passos sugeridos
1. Decidir Q4 (modelo de distribuição/licença) e Q5 (Tailscale SaaS vs Headscale) — `08-roadmap.md`.
2. Fase 0 com o **gate G0**: benchmark de throughput do `tailscale` crate (direto e DERP) vs `tailscaled`.
3. MVP desktop com H.264 HW, QUIC e Slint; depois AV1/HEVC/VP9; depois PyroWave; depois mobile.

## 4. Riscos de maior impacto
Throughput do overlay embutido (mitigado por modo sistema), identidade de usuário no modo embutido (Q1), maturidade do Slint no iOS (plano B Makepad), patentes HEVC/H.264 (parecer jurídico; preferir AV1/VP9 e HW do SO) e estabilidade de drivers de encoder (probe + fallback).
