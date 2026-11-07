# TailDesk — Plano de execução (o que falta fazer)

> Publicado em 03/11/2026 a pedido do Yuri. Base: issues abertas em `yurisanp/taildesk`,
> `10-plano-windows-linux.md` (fases 0–5), `mobile/` e a memória do projeto.
> O app Android está detalhado em [`mobile/android-detalhado.md`](mobile/android-detalhado.md).
> Este arquivo é o ponto de partida de toda thread nova: pegue a próxima linha da etapa atual,
> abra uma branch, faça PRs pequenos com `Closes #N` e mescle com o CI verde.

## 1. Onde estamos (03/11/2026)

- **Fases 0 a 5 do desktop estão implementadas na `main`.** Último commit em 19/10/2026
  (agente SYSTEM do Windows, Flatpak, host X11, hotplug, multimonitor, laço lento de adaptação, AUR
  pelo botão Run workflow).
- **Última versão publicada: `v0.2.0-beta.6` (17/10/2026).** Tudo o que entrou em 19/10 ainda não
  saiu em nenhuma versão.
- **13 issues abertas** (#84–#86, #93–#96, #98–#102). Duas delas já têm código na `main` mas
  continuam abertas porque falta uma parte:
  - #84 agente SYSTEM: a tela de entrada (ninguém logado) funciona; **faltam o UAC e a tela de
    bloqueio com usuário logado**.
  - #85 Flatpak: app_id `io.github.yurisanp.TailDesk`, manifesto e job de CI prontos; **falta a
    submissão ao Flathub** (conta do Yuri).
- Testes em dispositivo: só o notebook CachyOS (KDE Wayland). O Windows está desligado.
- Mobile: só documentação (`mobile/`); nada no repositório.

Decisões do Yuri em 03/11/2026 para o Android: pacote `com.santech.taildesk`, APK para baixar no
GitHub em toda versão, host Android na Play só depois da aprovação do Google.

## 2. Ordem de execução

Quatro etapas. A etapa 1 fecha o MVP desktop; a etapa 3 (Android) pode começar em paralelo assim que
a beta.7 sair, porque o protocolo do desktop já está estável.

### Etapa 1 — Lançar a v0.2.0 (fecha o MVP)

| # | O que | Issue | Depende de | Quem | Pronto quando |
|---|---|---|---|---|---|
| 1.1 | Publicar `v0.2.0-beta.7` com o que entrou em 19/10 | — | nada | Yuri clica Actions → Release → Run workflow (sessões levam 403 em tag) | Release com AppImage, deb, rpm, tar.gz, MSI, zip; beta.6 se atualiza sozinha |
| 1.2 | Teste manual no notebook CachyOS | #98 | 1.1 | Yuri (com uma thread acompanhando pelo Remote Control) | Checklist da issue marcado; cada falha vira issue `bug` |
| 1.3 | Medir latência 1080p60 em LAN (meta < 40 ms) | #93 | 1.1 | Thread + notebook; Windows quando ligar | Números publicados nos 3 sentidos ou uma issue por gargalo |
| 1.4 | Degradação via DERP sem congelar | #94 | 1.1 (o laço lento #88 já entrou) | Thread + notebook | 5 min via DERP sem congelar > 1 s e volta à qualidade normal |
| 1.5 | Assinatura do Windows pelo SignPath | #86 | cadastro na SignPath Foundation (Yuri) | Yuri cadastra; thread troca o passo do `release.yml` | `signtool verify /pa` passa no `.exe` e no `.msi` |
| 1.6 | Secrets e publicação da `v0.2.0` | #101 | 1.2–1.5 | Yuri: `AUR_SSH_KEY`, projeto COPR + `COPR_TOKEN`; depois Run workflow com `v0.2.0` | Release estável com todos os pacotes, AUR e COPR atualizados, beta se atualiza |
| 1.7 | Teste manual no Windows 11 | #99 | Windows ligado | Yuri | Checklist marcado (pode ficar para depois da v0.2.0 se o Windows continuar desligado) |

Atalho se o Windows continuar desligado: a v0.2.0 pode sair com #99 e #86 pendentes, avisando nas
notas que o MSI ainda não é assinado. Decisão do Yuri.

### Etapa 2 — Pendências de produção do desktop

| # | O que | Issue | Quem | Pronto quando |
|---|---|---|---|---|
| 2.1 | UAC (área segura) e tela de bloqueio com usuário logado | #84 (resto) | Thread; teste no Windows | Prompt de UAC aparece no controlador e aceita clique; tela de bloqueio desbloqueia pela sessão |
| 2.2 | Submissão ao Flathub | #85 (resto) | Yuri cria a conta e abre o PR no `flathub/flathub`; thread prepara | App instalável pelo Flathub |
| 2.3 | Testar nas distros alvo (Ubuntu LTS, Fedora GNOME/KDE, Arch KDE/Hyprland) | #100 | Thread em VMs, Yuri onde precisar de GPU real | Tabela de resultados no plano; falhas viram issues |

### Etapa 3 — App Android (controlador) e sua publicação

Detalhe completo, telas, CI e publicação na Play em [`mobile/android-detalhado.md`](mobile/android-detalhado.md).
Issue guarda-chuva: #96; trabalho do controlador e da publicação: #104. Sugestão: abrir uma issue por linha abaixo quando a etapa começar.

| # | O que | Onde | Pronto quando |
|---|---|---|---|
| 3.1 ✅ 03/11 | Esqueleto `apps/android` + `crates/taildesk-mobile` + CI Android (APK de debug nos artefatos) | repo taildesk | CI verde em PR, APK instala e abre uma tela Slint vazia |
| 3.2 🟡 03/11 (código pronto, medições pendentes) | Spikes do M0 Android (S1 Slint+wgpu, S3 QUIC pela VPN, S4 detecção do Tailscale, S5 MagicDNS reverso, S6 MediaCodec, S7 inbound) | branch própria | `mobile/m0-resultados.md` com os gates; ADR-M1..M4 no `08-roadmap.md` |
| 3.3 ✅ 03/11 | Protocolo mobile: `Hello.client_kind`/`viewport`, `PathInfo`, `PeerList*`, `Pair*`, `KeyboardInfo.source` | `taildesk-proto` + `07-protocolo.md` | Round-trip de todas; host desktop respeita `viewport` |
| 3.4 ✅ 03/11 | Desktop: "Configurações → Celulares → Conectar um celular" (QR) e resposta ao `PeerList` | app desktop | QR lido pelo celular pareia; lista só vai para o mesmo usuário da tailnet |
| 3.5 🟡 03/11 (implementado; falta o roteiro em aparelho) | M1 controlador Android (onboarding, dispositivos, pareamento, sessão touchpad/toque direto, faixa de teclas, monitores, HUD) | `apps/android` | Roteiros R-mobile-1 (Rafael) e A-mobile-1 (Ana Paula) passam num aparelho real |
| 3.6 🟡 03/11 (workflows e ficha prontos; falta a conta, §3 item 5) | Publicação no modelo do babylock: workflow "Versão para a Play", ficha da loja, política de privacidade, capturas pelo CI, guia `PLAY_STORE.md` | `.github/workflows/android*.yml`, `apps/android/` | `.aab` assinado sobe para o teste interno; APK assinado para baixar no GitHub Releases em toda versão |
| 3.7 | Teste fechado (12 testadores por 14 dias) e pedido de produção | Play Console (Yuri) | App publicado na Play |

**Estado da etapa 3 em 03/11/2026 (#104).** Tudo o que não precisa de aparelho nem da conta da
Play está no repositório: protocolo mobile com round-trip, pareamento por QR e `PeerList` no
desktop, `MobileVpnOverlay`, o app (`crates/taildesk-mobile` + `apps/android`), CI Android,
"Versão para a Play", Pages, ficha da loja e `apps/android/PLAY_STORE.md`. Falta: medir os gates
do M0 (`mobile/m0-resultados.md`), o zero-copy do S6, os roteiros R-mobile-1 e A-mobile-1 num
celular e os passos da conta (§3 item 5).

### Etapa 4 — Pós-MVP

| # | O que | Issue | Observação |
|---|---|---|---|
| 4.1 🟡 07/11 (código pronto: #107, #108; falta o teste com duas máquinas na mesma LAN) | Wake on LAN pela tailnet | #95 | `17-wake-on-lan.md`; protocolo em `07-protocolo.md` §3.5; botão "Ligar" no dispositivo desligado |
| 4.2 | Zero-copy captura → encode → decode → render | #102 | Depois de #93 medido, para comparar |
| 4.3 🟡 07/11 (arquivos #109, "Compartilhar com TailDesk" #110, área de transferência de texto #111, teclado físico #112; som em andamento) | M3 paridade no Android: arquivos, "Compartilhar com TailDesk", áudio, teclado/mouse físicos, tablet com 2 monitores, Teste de conexão completo, clipboard | #96 | Ver `mobile/android-detalhado.md` §3 |
| 4.4 | M4 host Android (MediaProjection + AccessibilityService) | #96 | Na Play só depois que o Google aprovar a declaração de Accessibility (decisão do Yuri, 03/11); antes, só no APK do GitHub |
| 4.5 | M2 controlador iOS | #96 | Bloqueado até o Yuri decidir a conta Apple Developer |
| 4.6 | M5 iOS "só ver" | #96 | Opcional, só com pedido real |

## 3. O que depende do Yuri

Em ordem de quando trava algo:

1. **Clicar Run workflow** para a `v0.2.0-beta.7` (1.1) e depois para a `v0.2.0` (1.6).
2. **Testes manuais** no notebook (#98) e, quando ligar, no Windows (#99).
3. **Secrets**: `AUR_SSH_KEY`, projeto COPR + `COPR_TOKEN` (expira em 180 dias), cadastro na
   SignPath. Passo a passo em `configuracao/passo-a-passo.md` (pasta do projeto no Claude).
4. **Flathub**: conta e PR de submissão (#85).
5. **Android** (lista completa em `mobile/android-detalhado.md` §13): keystore de envio + 4 secrets, conta de serviço da Play (opcional), ligar o
   GitHub Pages, criar o app no Play Console, primeiro envio manual, 12 testadores por 14 dias.
6. **Conta Apple Developer** (só para o iOS, etapa 4).

## 4. Regras para quem executar

- Uma thread por issue (ou por linha da etapa 3); PRs pequenos com `Closes #N` (ou `Refs #N` quando
  a issue só fecha com teste manual); merge próprio com CI verde em todas as plataformas (pedido do
  Yuri, 15/10/2026).
- Testar só no notebook CachyOS (`/home/yurisp/Desenvolvimento`), com build local para depender
  menos do CI lento. Windows só quando o Yuri disser que ligou.
- `cargo xtask check` antes de cada push. Os jobs Android **não** entram no `xtask check` padrão;
  ganham `cargo xtask mobile-check`.
- Mudança de protocolo com round-trip e `07-protocolo.md` atualizado; mudança de teclado com caso
  novo nos fixtures de `taildesk-keyboard`.
- Ao terminar uma linha, atualizar este arquivo (marcar ✅ e a data) e a issue.
