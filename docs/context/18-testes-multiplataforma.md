# 18 — Testes multiplataforma sem as máquinas do Yuri

Levantamento de 08/11/2026 (pedido do Yuri: "testar de forma abrangente, em diversos SOs, sem
precisar das minhas máquinas; se preciso criamos VMs"). Diz o que dá para testar em CI e em VMs,
o que custa, o que precisa de ação do Yuri e por onde começar.

**Resumo.** Quase tudo cabe nos runners grátis do GitHub (o repositório é público: minutos
ilimitados nos runners padrão Linux, Windows e macOS). O que muda o jogo:

1. **Wayland de verdade sem tela** num container: `kwin_wayland --virtual` (N monitores) +
   PipeWire + `xdg-desktop-portal-kde` com a **pré-autorização `kde-authorized`** (Plasma ≥ 6.3)
   abre RemoteDesktop + ScreenCast + libei sem diálogo. Provado neste levantamento (§4.2) e
   virou o primeiro PR.
2. **Tailnet de teste real** com nós efêmeros (OAuth client do Tailscale + `tag:taildesk-ci`):
   um job Linux e um job Windows entram na mesma tailnet e fazem uma sessão completa pelo
   `tailscaled` de verdade. Precisa de um secret do Yuri.
3. **VMs de desktop completo** (GNOME, KDE, X11, Fedora, Ubuntu, Arch) com QEMU/KVM *dentro* do
   runner Linux do GitHub, que tem `/dev/kvm` (o job do emulador Android já usa). Grátis e lento:
   bom para rodar toda noite, não em todo PR.

Só **aceleração de vídeo por hardware** (VA-API Intel/AMD, NVENC) não tem caminho grátis. Para
isso há três saídas pagas ou com hardware (§7), e o teste manual no notebook continua sendo o
de aceitação final.

## 1. O que já existe

| Onde | O que roda |
|---|---|
| `ci.yml` / `ubuntu-latest`, `windows-latest` | fmt, clippy, `cargo test --workspace` |
| `ci.yml` / containers `fedora:latest`, `archlinux:latest` | clippy + testes |
| `ci.yml` / `cargo-deny`, metadados de pacote | licenças, RustSec, AppStream, .desktop |
| `android.yml` | APK, Robolectric, lint, emulador API 30 e 34 (com KVM), capturas da Play |
| Testes de integração | sessão completa em loopback QUIC (`taildesk-session/tests/conexao_quic.rs`), arquivos com retomada, vídeo em loopback, matriz de teclado |

Falta: nada roda **com compositor, portal, PipeWire, GPU, tailscaled ou SO real**. Captura,
injeção, render, codecs de hardware, identidade pela tailnet e caminho DERP só foram testados
no notebook e no Windows do Yuri.

## 2. Matriz-alvo e onde cada célula pode rodar

| Alvo | Captura/entrada | Grátis no CI | Precisa de VM | Precisa de hardware |
|---|---|---|---|---|
| Linux KDE Wayland | portal KDE + PipeWire + libei | **sim** (container, §4.2) | para sessão de usuário completa | — |
| Linux GNOME Wayland | portal GNOME + PipeWire + libei | parcial: mutter `--headless` sobe; o diálogo do portal GNOME não tem pré-autorização (§4.3) | sim (VM, §6) | — |
| Linux wlroots (Sway/Hyprland) | só ScreenCast | parcial: sway headless + xdpw falhou no screencopy (memória da fase 1); tentar com `WLR_RENDERER=pixman` | sim | — |
| Linux X11 (i3, sessão X do KDE) | XShm/XTest | **sim** (Xvfb, §4.1) | — | — |
| Fedora, Arch, Ubuntu (pacotes, libs) | — | **sim** (containers; instalação dos .deb/.rpm/AppImage em container limpo) | para "instala e abre" com sessão gráfica | — |
| Windows 11 / Server | DXGI + SendInput | **a provar**: o runner `windows-latest` tem sessão interativa com adaptador básico; DXGI Desktop Duplication precisa de sessão ≠ 0 (spike §5) | Windows 10 ABNT2 (mãe da Ana Paula) | — |
| Windows tela de entrada/UAC (agente SYSTEM) | serviço + SendSAS | não (runner não deixa trocar de desktop seguro com confiança) | **sim** (VM Windows de avaliação, §6.3) | — |
| Rede: identidade, whois, app caps | `tailscaled` | **sim** com secret (§3) | — | — |
| Rede: direto vs DERP, perda, latência | `tc netem` | **sim** (netns no runner, §3.3) | — | — |
| Render wgpu | Vulkan | **sim** (lavapipe/llvmpipe, já instalado no CI) | — | GPU real para desempenho |
| openh264 | CPU | **sim** | — | — |
| VA-API (Iris Xe, AMD) | GPU | não | não ajuda (sem GPU na VM) | **sim** (§7) |
| NVENC/NVDEC | GPU NVIDIA | não | — | runner GPU do GitHub (pago, exige org) ou nuvem (§7) |
| Media Foundation | Windows | encoder de software da Microsoft no runner (spike §5) | — | GPU para o caminho HW |
| Áudio (PipeWire, WASAPI) | — | **sim**: null sink do PipeWire; WASAPI loopback no runner Windows é incerto | — | — |
| Android controlador | emulador | **sim** (já roda) | — | aparelho real para MediaCodec/AAudio (Firebase Test Lab, §8) |
| Android ↔ desktop (E2E) | — | **sim** no mesmo runner (emulador + host Linux, §8) | — | — |

## 3. Tailnet de teste

### 3.1 Tailscale de verdade, nós efêmeros (recomendado)

- `tailscale/github-action` com **OAuth client** do Tailscale e tag `tag:taildesk-ci`. Cada job
  vira um nó **efêmero** que some da tailnet quando o job acaba.
- Cabe no plano Personal grátis (até 100 dispositivos; efêmeros não acumulam).
- Testa o que só o `tailscaled` real dá: `Overlay::identify()` por `whois`, app capability do
  TailDesk (grant no ACL para `tag:taildesk-ci`), `ping` disco, região DERP, MagicDNS.
- Dois jobs em runners diferentes (ex.: host em `windows-latest`, controlador em
  `ubuntu-latest`) = **sessão Windows ↔ Linux real**, pela internet, sem as máquinas do Yuri.
  Os runners ficam atrás de NAT da Azure: costuma sair caminho direto, às vezes DERP. Bom: os
  dois casos aparecem.
- Segurança: PRs de fork não recebem secrets, então o job só roda em `push` na main, em
  `workflow_dispatch` e em PR do próprio repositório. O ACL deve isolar `tag:taildesk-ci`
  (só fala com `tag:taildesk-ci`), para um runner nunca alcançar o notebook ou o Windows.

### 3.2 Headscale só para teste (opcional, offline)

- Um container do Headscale + dois `tailscaled` no mesmo runner, sem secret e sem internet.
  Serve para PR de fork e para rodar localmente.
- Não fere a ADR-009: o produto continua usando só o servidor do Tailscale; o Headscale é
  ferramenta de teste. Mesmo assim, app capabilities (grants com `app`) só chegaram no Headscale
  0.29 (beta em 11/2026), então o teste de permissão por app cap fica no §3.1.

### 3.3 Dois nós no mesmo runner e rede ruim de propósito

- Cada `tailscaled` num **network namespace** próprio (`ip netns`, `--tun=tailscale0`,
  `--statedir` separado). Assim um runner só tem host e controlador.
- `tc qdisc ... netem delay 120ms loss 2%` no veth de um namespace simula 4G ruim; bloquear UDP
  direto entre os namespaces força **DERP**. Isso testa a seleção adaptativa
  (`14-selecao-adaptativa.md`) e o critério da fase 4 "degrada sem congelar via DERP".
- `tailscaled --tun=userspace-networking` **não serve**: o TailDesk abre o UDP do SO no IP da
  tailnet, que precisa do TUN de verdade.

### 3.4 O que falta no código para o E2E

O `taildesk-cli host` e `connect` ainda dizem "não implementado"; o host real só existe no app
gráfico. Para o E2E é preciso um modo sem janela: `taildesk-cli host --aceitar <login|tag>` e
`taildesk-cli connect <peer> --roteiro <arquivo>` (conecta, pede vídeo, conta quadros, digita
uma sequência de teclas, manda e recebe arquivo, mede RTT, sai com código ≠ 0 se algo falhar).
Reaproveita `taildesk_session::conexao::{host, controlador}`, que já têm os testes em loopback.

## 4. Testes de desktop sem tela (grátis, em todo PR que mexe em plataforma)

### 4.1 X11 (Xvfb)

- `xvfb-run` + `setxkbmap br abnt2` / `us intl`: captura XShm de um padrão conhecido e injeção
  XTest conferida por uma janelinha de teste que registra o que recebeu.
- Mesma técnica da memória da fase 1 (python-xlib, `import -window root`).

### 4.2 KDE Wayland (provado)

Receita (`.github/scripts/wayland-kde.sh`):

1. `dbus-run-session`, `XDG_RUNTIME_DIR` próprio, `pipewire` + `wireplumber`.
2. `kwin_wayland --virtual --output-count 2 --width 1920 --height 1080 --no-lockscreen`.
   No container é preciso tirar o `cap_sys_nice` do binário (`setcap -r`), senão o exec dá EPERM.
3. `flatpak permission-set kde-authorized remote-desktop "" yes`: pré-autoriza app sem app id
   (processo do host). Com app id registrado (`org.freedesktop.host.portal.Registry`) o id
   seria `io.github.yurisanp.TailDesk`.
4. `xdg-desktop-portal-kde` e `xdg-desktop-portal`.
5. `taildesk-cli portal`: sessão RemoteDesktop + ScreenCast, conta quadros de cada monitor por
   3 s, liga o libei (ConnectToEIS) e move o ponteiro.

Resultado no container Arch (KWin 6.8.2, xdg-desktop-portal 1.22.1, xdp-kde 6.8.2, PipeWire
1.6.10): ver §10.

Próximos passos sobre a mesma base: conferir a injeção (uma janela Wayland de teste que grava
teclas e cliques recebidos, com layouts ABNT2/US-Intl pelo `kxkbrc`), restore_token e
PersistMode (pendência da fase 5), cursor como metadado, troca de resolução.

### 4.3 GNOME Wayland

- `mutter --headless --wayland --virtual-monitor 1920x1080` sobe sem GPU e é o que o próprio
  GNOME usa no CI. O ScreenCast **direto** do mutter funciona.
- O problema é o **diálogo** do `xdg-desktop-portal-gnome`: não há tabela de pré-autorização
  como a do KDE. Opções: (a) VM com GNOME e clique no diálogo por QMP/ydotool (§6); (b) um
  backend de portal de teste (`org.freedesktop.impl.portal.RemoteDesktop`) que aprova sozinho e
  repassa ao mutter (dá trabalho e deixa de testar o backend real); (c) testar o
  `restore_token`: aprovar uma vez na VM, guardar o token e reusar. Recomendo (a).

### 4.4 PipeWire virtual

- Vídeo: `gst-launch-1.0 videotestsrc ! pipewiresink` cria um node; `taildesk-cli
  captura-node <id>` mede a captura sem portal. Bom para formatos (BGRx, NV12, DMA-BUF não).
- Áudio: `pw-cli create-node adapter '{ factory.name=support.null-audio-sink ... }'` como saída
  padrão + um tom de teste; confere captura e Opus sem placa de som.

### 4.5 Render e codecs

- wgpu em lavapipe (`mesa-vulkan-drivers` já está no CI): render de um quadro para textura e
  comparação com referência.
- openh264: ida e volta com PSNR mínimo; VA-API e NVENC só com hardware (§7). O *probe* e o
  *fallback* podem ser testados sem GPU (o probe falha, o fallback precisa escolher openh264).

## 5. Windows sem o PC do Yuri

- **Runner `windows-latest` (Server 2025)**: grátis. Spike a fazer: o runner roda numa sessão
  interativa (navegadores com janela funcionam lá), então DXGI Desktop Duplication **deve**
  funcionar no adaptador básico. Se funcionar: captura + SendInput + `taildesk-cli
  captura-dxgi` no CI, e o host Windows do E2E (§3) roda nele.
- Layout de teclado: `Set-WinUserLanguageList` liga ABNT2 e US-Intl no runner para a matriz de
  teclado com o injetor real.
- Media Foundation: o encoder H.264 de software da Microsoft existe no Server com o recurso
  "Media Foundation" (`Install-WindowsFeature Server-Media-Foundation` se faltar).
- Windows 10/11 desktop de verdade, tela de entrada, UAC, agente SYSTEM: VM (§6.3).
- macOS: fora do escopo do produto. Não vale a pena; serviria só como mais um controlador.

## 6. VMs

### 6.1 Onde

| Opção | Custo | Prós | Contras |
|---|---|---|---|
| **QEMU/KVM dentro do `ubuntu-latest`** | grátis (repositório público) | 4 vCPU, 16 GB, KVM liberado; mesma técnica do emulador Android | 14 GB de disco livre; imagem tem de ser baixada a cada job (cache do Actions é 10 GB); 6 h por job |
| Runner maior do GitHub (Linux 8–16 núcleos) | ~US$ 0,02–0,04/min | mais disco e CPU | exige conta de organização no plano Team |
| VM na nuvem ligada pelo workflow (Hetzner, AWS, GCP) | Hetzner CX/CPX: €0,01–0,03/h; AWS spot | VM fica pronta (imagem salva), dá para usar de dia a dia | precisa de conta e token do Yuri; cuidar de desligar |
| Servidor próprio com Proxmox/libvirt + runner self-hosted | hardware | mais rápido, guarda imagens | é "uma máquina do Yuri", só que dedicada |

Recomendação: **QEMU/KVM no runner grátis**, toda noite e por `workflow_dispatch`. Se ficar
lento demais ou o disco não der, passar para uma VM da Hetzner ligada pelo workflow.

### 6.2 Linux

- Imagens cloud oficiais (Ubuntu 24.04/26.04, Fedora 43, Arch) + cloud-init instalando o
  desktop (GNOME ou Plasma), login automático e o pacote do TailDesk (`.deb`/`.rpm`/AppImage do
  próprio build).
- Controle pela QMP do QEMU: `screendump` para capturas, `send-key`/`input-send-event` para o
  teclado **físico** (com layout ABNT2 no convidado: é exatamente o caminho do teclado do
  usuário), SSH para rodar o roteiro e coletar logs.
- Dois convidados na mesma bridge + tailnet de teste (§3) = **E2E entre duas VMs**: Fedora KDE
  ↔ Ubuntu GNOME, com diálogo do GNOME clicado pela QMP.
- Para caber em 14 GB: qcow2 com overlay, imagem base comprimida guardada como asset de uma
  release `ci-imagens` ou no GHCR (ORAS), refeita uma vez por semana.

### 6.3 Windows

- ISO de avaliação do Windows 11 Enterprise (90 dias, grátis da Microsoft) com instalação
  desassistida (`autounattend.xml`), VirtIO e OpenSSH. Instalar a cada job leva ~40 min, então
  o caminho bom é uma imagem pronta guardada fora do Actions (nuvem ou servidor próprio): a
  imagem tem 15–20 GB e não cabe no cache.
- Serve para o que o runner não dá: tela de entrada, UAC, Ctrl+Alt+Del, agente SYSTEM,
  Windows 10 ABNT2.
- Licença: a avaliação vale para teste; renovar a imagem a cada 90 dias.

## 7. Vídeo por hardware

| Caminho | Opção | Custo aproximado | Ação do Yuri |
|---|---|---|---|
| NVENC/NVDEC | Runner GPU do GitHub (Linux 4 núcleos, GPU NVIDIA) | US$ 0,052/min (Linux), US$ 0,102/min (Windows) | criar organização no plano Team (US$ 4/usuário/mês) e mover o repo |
| NVENC/NVDEC | VM com T4 na nuvem (AWS g4dn.xlarge spot, GCP T4) | ~US$ 0,15–0,55/h | conta + credencial no secret |
| VA-API AMD | AWS g4ad (Radeon Pro V520) | ~US$ 0,40/h | conta; **não confirmado** que o `radeonsi` exponha encode H.264 lá |
| VA-API Intel | sem oferta de nuvem confiável | — | mini-PC Intel (N100/N150, ~R$ 900) como runner self-hosted dedicado, ou continuar no notebook |
| Media Foundation HW | runner Windows GPU do GitHub | US$ 0,102/min | idem NVENC |

Recomendação: deixar HW para o fim. Um job semanal de 10 min no runner GPU do GitHub custaria
~US$ 2/mês (Linux + Windows), mas exige a organização. Até lá, o probe/fallback roda no CI
grátis e a aceitação do HW fica no notebook (Iris Xe + RTX 3060M já cobrem VA-API e NVENC).

## 8. Android

- Já roda: build, Robolectric, lint, emulador API 30/34.
- **E2E no mesmo runner**: emulador + host Linux (Xvfb ou KWin virtual) no `ubuntu-latest`. O
  emulador vê o host em `10.0.2.2`. O app usa `MobileVpnOverlay` (precisa do app do Tailscale
  logado), então o teste precisa de um build de debug com overlay de LAN, só em `debug`. O
  Tailscale do celular não dá para logar sozinho no emulador.
- Aparelhos reais: **Firebase Test Lab** (plano grátis Spark: 5 testes/dia em aparelho físico,
  10 em virtual). Cobre MediaCodec, AAudio e telas reais. Precisa de projeto Firebase e chave de
  conta de serviço no secret.

## 9. Recomendação e ordem

| # | Passo | Custo | Precisa do Yuri | Pega o quê |
|---|---|---|---|---|
| 1 | **Job "Wayland KDE" no CI** (este PR) | grátis | não | regressão no portal, PipeWire, libei, multimonitor |
| 2 | Janela de teste Wayland/X11 que confere a injeção + matriz ABNT2/US-Intl | grátis | não | teclado e mouse reais no Linux |
| 3 | Spike Windows runner: DXGI + SendInput | grátis | não | captura e injeção no Windows |
| 4 | `taildesk-cli host/connect --roteiro` (modo sem janela) | grátis | não | base do E2E |
| 5 | E2E com dois nós em netns + netem (Headscale ou tailnet real) | grátis | não (Headscale) / secret (real) | sessão completa, DERP, rede ruim |
| 6 | **E2E Windows ↔ Linux pela tailnet real** | grátis | **OAuth client + ACL** | identidade, app caps, caminho real |
| 7 | VMs QEMU noturnas: GNOME, Fedora KDE, X11, pacotes instalados | grátis | não | portal GNOME, "instala e abre" |
| 8 | VM Windows de avaliação (tela de entrada, UAC, Windows 10) | Hetzner/servidor | conta ou servidor | agente SYSTEM |
| 9 | Android E2E com emulador + Firebase Test Lab | grátis | projeto Firebase (opcional) | celular ↔ desktop |
| 10 | HW: runner GPU semanal | ~US$ 2–5/mês | organização Team | NVENC, MF HW |

Os passos 1–5 e 7 não dependem do Yuri. Com eles, o teste no notebook vira só a aceitação final
de cada versão.

## 10. O que o Yuri precisa fazer (quando quiser os passos 6, 8, 9 e 10)

1. **Tailscale** (passo 6): no admin do Tailscale, criar `tag:taildesk-ci` em `tagOwners`, um
   ACL que deixa `tag:taildesk-ci` falar só com `tag:taildesk-ci` (porta UDP do TailDesk), o
   grant de app capability do TailDesk para a tag, e um **OAuth client** com escopo `auth_keys`
   e a tag. Guardar `TS_OAUTH_CLIENT_ID` e `TS_OAUTH_SECRET` em Settings → Secrets → Actions.
2. **GPU** (passo 10): criar uma organização no GitHub (plano Team), transferir o repo e criar
   os runners GPU Linux e Windows.
3. **Firebase** (passo 9, opcional): projeto Firebase e `FIREBASE_SA_JSON`.
4. **VM Windows** (passo 8): escolher entre conta na nuvem (Hetzner/AWS, token no secret) ou um
   servidor próprio com runner self-hosted.

## 11. Resultado da prova (08/11/2026)

Preenchido em §4.2 abaixo do PR (ver resultado do job `wayland-kde` no CI).

## Fontes

- Pré-autorização do portal KDE: <https://develop.kde.org/docs/administration/portal-permissions/>
- Discussão sobre id vazio para apps do host: <https://discuss.kde.org/t/how-to-permanently-give-an-application-access-to-remote-desktop-on-wayland/29886>
- Mutter headless e monitores virtuais (GNOME 40): <https://www.phoronix.com/news/GNOME-40-Headless-Virtual>
- Ação do Tailscale (OAuth, nós efêmeros, tags): <https://github.com/tailscale/github-action>
- Preços dos runners do GitHub: <https://docs.github.com/en/enterprise-cloud@latest/billing/reference/actions-runner-pricing>
- Grants com `app` no Headscale 0.29 beta: <https://newreleases.io/project/github/juanfont/headscale/release/v0.29.0-beta.1>
- AWS g4ad: <https://aws.amazon.com/blogs/compute/deep-dive-on-the-new-amazon-ec2-g4ad-instances>
