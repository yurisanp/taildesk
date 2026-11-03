# TailDesk Mobile — Plano de fases (pronto para executar)

> Complementa `requisitos.md` e `arquitetura.md`. Cada fase tem entregas, critério de pronto e o
> que precisa estar pronto antes. IDs `MF-*`/`MNF-*` vêm dos requisitos; passos `R*`/`A*` são das
> personas em `testes/personas.md`.

## 0. Quando começar (pré-requisitos)

- Desktop com **fases 1–3 do `10-plano`** feitas (ver, controlar com teclado Auto, arquivos): o
  protocolo de vídeo por monitor, input e arquivos precisa estar estável antes do celular depender dele.
- **Pode adiantar já, sem atrapalhar o desktop** (pequeno, só protocolo, opcional): reservar no
  `Hello` os campos `client_kind` e `viewport`, e `KeyboardInfo.source` (`arquitetura.md` §4), com
  round-trip. Só se a thread de arquitetura achar oportuno; não é bloqueante.
- Decisões do Yuri da `requisitos.md` §8 (pelo menos 1, 2 e 4).

## M0 — Spikes com gate (2–3 semanas)

| Spike | O que provar | Gate (passa/não passa) |
|---|---|---|
| S1 Slint + wgpu no Android | App Slint com textura wgpu atualizada a 60 fps num aparelho médio | ≥ 55 fps, sem travar UI |
| S2 Slint + wgpu no iOS | O mesmo no iPhone (guia iOS do Slint, `xcodegen`) | ≥ 55 fps; teclado do sistema abre e entrega texto/IME; safe areas corretas. **Se falhar**: casca SwiftUI + `uniffi` + `CAMetalLayer` do wgpu (ADR-M4) |
| S3 QUIC pela VPN do Tailscale | `taildesk-cli bench-net` equivalente rodando no celular contra um desktop | Android e iOS conectam por nome MagicDNS; ≥ 20 Mbit/s em Wi-Fi direto; datagrama máx. medido |
| S4 Detecção do Tailscale | Estados da `requisitos.md` §4.3 nos dois SOs, incluindo outra VPN e split tunneling | Todos os estados distinguidos em aparelho real |
| S5 MagicDNS reverso | PTR do IP 100.x em 100.100.100.100 | Responde o nome do nó? Se não, `identify()` usa nome pareado |
| S6 Decode HW | MediaCodec e VideoToolbox decodificando o stream H.264 do host atual, zero-copy até a tela | Latência de decode ≤ 8 ms 1080p; sem cópia CPU no caminho |
| S7 Inbound no Android | Celular escuta UDP no IP 100.x; desktop conecta | Funciona (pré-requisito do M4) |

Saída: relatório `mobile/m0-resultados.md` + ADR-M1..M4 no `08-roadmap.md`.

## M1 — Controlador Android (4–6 semanas)

Entregas:
- `MobileVpnOverlay`, `taildesk-mobile`, `apps/android`, telas Slint: onboarding (MF-01),
  dispositivos (MF-10..14), pareamento QR (MF-20..22, inclui a tela do QR no **desktop**),
  conectar (MF-30), sessão com touchpad/toque direto (MF-40..42), faixa de teclas + teclado virtual
  (MF-43..44, 46), troca de monitor básica (MF-47), HUD (MF-48), ciclo de vida (MF-49/50).
- Protocolo: `client_kind`, `viewport`, `PathInfo`, `PeerList*`, `Pair*`, `KeyboardInfo.source`, com round-trip.
- Decode MediaCodec + fallback openh264.
- Adaptador de rede com rede tarifada (MF-80/81/82).
- CI Android + APK de debug nos artefatos do CI.

Pronto quando:
- Rafael R-mobile-1: no 4G, abre o app, acha `servidor-homelab`, conecta, troca para o monitor 2,
  digita `sudo systemctl restart nginx` com a faixa (Ctrl+C funciona) em < 60 s.
- Ana Paula A-mobile-1: no Wi-Fi, conecta no `pc-mae` (Win10 ABNT2), mãe aceita, Ana toca no botão
  certo em modo toque direto e digita "ação" com acento certo.
- MNF-01/02/04 medidos e anotados.

## M2 — Controlador iOS/iPadOS (3–5 semanas)

> Bloqueada até a decisão da conta Apple Developer (adiada pelo Yuri em 15/10/2026).

- `apps/ios` com o mesmo núcleo; decode VideoToolbox; detecção do Tailscale (iOS);
  `AVAudioSession` preparado; safe areas e teclado iOS.
- TestFlight interno.
- Pronto quando: os mesmos roteiros do M1 passam no iPhone; iPad em paisagem usa toque direto
  por padrão.

## M3 — Paridade de recursos (4–6 semanas)

- Arquivos: painel/folha (MF-60..62), "Compartilhar com TailDesk" (MF-63, Android share target e
  iOS Share Extension), confirmação em dados móveis (MF-64).
- Áudio (MF-70..72), seguindo `15-audio.md`; teste automatizado "desligado = zero `kind=1`".
- Teclado físico e mouse físico (MF-45), com casos novos nos fixtures de `taildesk-keyboard`
  (iPad com teclado US ↔ host ABNT2; Android Bluetooth ABNT2 ↔ host US-Intl).
- Multimonitor completo: lista com miniaturas, tablet com 2 monitores em tela dividida (MF-47).
- Aba Teste de conexão completa (MF-83).
- Clipboard de texto.
- Publicação: Play (controlador), App Store, APK no GitHub Releases.

## M4 — Host Android (5–7 semanas)

- MediaProjection + encoder MediaCodec + Foreground Service; AccessibilityService; áudio
  AudioPlaybackCapture; notificação permanente e encerramento (MF-90..95).
- Desktop controlador ganha a moldura de celular e os botões Voltar/Início/Recentes
  (`NavAction`), e o celular passa de "não suportado" para conectável na lista do desktop.
- Pareamento obrigatório (ADR-M5).
- Play: declaração de Accessibility; se negada, host só no APK do GitHub (ADR-M6).
- Pronto quando: Ana Paula ajuda alguém num Android 14 a partir do Windows: consentimento,
  ver, tocar, Voltar, digitar texto num campo.

## M5 — (opcional) iOS "só ver"

Broadcast Upload Extension (ReplayKit), encoder VideoToolbox, limite de ~50 MB de memória na
extensão, QUIC dentro da extensão. Só se houver pedido real; risco alto e valor baixo.

## Riscos

| Risco | Prob. | Impacto | Mitigação |
|---|---|---|---|
| Slint iOS (tech preview) não segura texto/IME ou performance | Média | Alto | Gate S2; casca SwiftUI + núcleo Rust |
| Usuário com outra VPN no celular | Alta | Médio | Detecção e mensagem clara; FAQ (usar exit node do Tailscale no lugar) |
| Otimização de bateria mata Tailscale/TailDesk | Média | Médio | Tela de ajuda por fabricante; reconexão rápida |
| Play recusa Accessibility para o host | Média | Médio | Build Play só controlador; APK fora da loja |
| Sem LocalAPI: caminho e usuário do host desconhecidos | Certa | Baixo | `PathInfo` do host; autorização no host |
| MTU 1280 no túnel móvel | Alta | Baixo | `max_datagram_size()` sempre; fragmentação já prevista |
| Consumo de dados no 4G | Média | Médio | Economizar dados por padrão em rede tarifada; viewport limita resolução |

## Brief para a thread que for executar
1. Ler `requisitos.md`, `arquitetura.md`, este arquivo e `telas/` (mockups) antes de tudo.
2. Começar pelo M0 em branch própria; não mexer em `release.yml` nem no `cargo xtask check` padrão.
3. Abrir ADR-M1..M6 no `08-roadmap.md` conforme os gates.
4. Toda mudança de protocolo com round-trip; toda mudança de teclado com caso novo nos fixtures.
