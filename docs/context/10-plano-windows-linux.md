# 10 — Plano TailDesk: Windows + Linux, desktop remoto + troca de arquivos

> Recorte do pacote `00`–`09` para o escopo decidido em **15/10/2026**: começar do zero,
> **Windows e Linux**, **desktop remoto** (ver + controlar) e **troca de arquivos**.
> Onde este arquivo diverge dos anteriores, **este vale para o MVP**; o restante do pacote
> continua como referência para fases futuras (macOS, mobile, PyroWave, áudio).
> Versões conferidas no crates.io hoje: `tailscale` 0.6.1, `quinn` 0.11.12, `slint` 1.18.1,
> `wgpu` 30.0.1, `windows-capture` 2.0.1, `openh264` 0.9.8, `ashpd` 0.13.13, `reis` 0.7.1, `x11rb` 0.14.0.

Codinome: **`taildesk`** no lugar de `halyard` (crates `taildesk-*`).

## 1. O que entra e o que fica para depois

| Entra no MVP | Fica para depois |
|---|---|
| Host e controlador em Windows 10/11 x64 e Linux x64 (Ubuntu/Debian, **Fedora**, **Arch**) | macOS, Android, iOS, Windows ARM64 |
| Ver e controlar (teclado físico + texto, mouse, scroll, cursor separado) | Microfone do controlador → host, chat, gamepad, toque |
| Áudio do sistema do host (Opus, fase 4) com liga/desliga que corta captura e tráfego (`15-audio.md`) | |
| Troca de arquivos nos dois sentidos + navegador de arquivos remoto | Clipboard de arquivos (fase 4), sync automático |
| H.264 (SW primeiro, HW em seguida) | AV1/HEVC/VP9 (fase 4), PyroWave (pós-MVP) |
| Multi-monitor escolhido pelo controlador: **um por vez** (abas) e **janelas separadas** (`13-multimonitor.md`) | Mosaico (todos numa janela), monitores virtuais |
| Modo assistido (consentimento na tela do host) | Não-assistido, tela de login/UAC (fase 5) |

## 2. Decisões ajustadas para Windows + Linux

### 2.1 Rede (Tailscale)
- **Overlay embutido (`tailscale` =0.6.1) é o padrão** nas duas plataformas: ambas são suportadas pelo tailscale-rs (Linux x86_64/ARM64, Windows x86_64). O usuário não precisa instalar o Tailscale.
- **Versões de referência (15/10/2026)**: cliente Tailscale / `tailscaled` **mínimo 1.102** (definido pelo Yuri; estável atual 1.104.1) e crate `tailscale` **0.6.1** (ainda a mais nova no crates.io, conferido hoje). São projetos separados: a crate Rust tem numeração própria e não acompanha a 1.102. O `SystemOverlay` exige `tailscaled` ≥ 1.102 e os testes de contrato da LocalAPI rodam contra a 1.102 e a estável atual; ao surgir uma crate nova, reavaliar o pin (em especial conexão direta, `14-selecao-adaptativa.md`).
- **Overlay sistema (`tailscaled` + LocalAPI)** entra logo depois como alternativa: dá `whois` com **usuário + app capabilities**, que o modo embutido não expõe (Q1 em `08`). LocalAPI: unix socket no Linux, named pipe no Windows — só esses dois clientes, sem o caso macOS.
- **Autorização no MVP**, enquanto Q1 não se resolve: ACL/grants da tailnet decidem quem alcança `udp:7420`; o host aplica **tags do peer** + **lista local de nós confiáveis** (`node_stable_id`) + **consentimento na tela**. App capabilities por usuário só no modo sistema.
- **Risco técnico nº 1**: o quinn precisa de um `AsyncUdpSocket` sobre o socket UDP do tailscale-rs. É o primeiro spike (fase 0); se ficar ruim, plano B é QUIC no modo sistema (socket do SO) enquanto se contribui upstream.

### 2.2 Linux: Wayland primeiro, não por último
O pacote original deixava Wayland para a fase 5. Para Linux isso não serve mais: as distros GNOME
atuais (Fedora 43, Ubuntu 25.10+) só oferecem sessão Wayland e o KDE caminha para o mesmo lugar
(inferência do estado do ecossistema; validar nas distros-alvo). Então:
- **Host Linux Wayland** = portal `RemoteDesktop` + `ScreenCast` (`ashpd`) → PipeWire (`pipewire`) para vídeo; input pelo próprio portal (`NotifyPointerMotion`/`NotifyKeyboardKeycode`) e libei (`reis`) quando o portal oferecer `ConnectToEIS`. Guardar `restore_token` para não perguntar a cada sessão.
- **Host Linux X11** = XShm + XDamage + XFixes + XTest (`x11rb`), como caminho secundário.
- O **controlador** Linux funciona igual em X11 e Wayland (Slint/winit + wgpu).
- Sem tela de login em Wayland no MVP (limitação do SO, não do produto).
- **Multi-monitor no Wayland**: quem escolhe os monitores é a pessoa no host, no diálogo do portal (`SelectSources` com `multiple=true`, um stream PipeWire por monitor). O controlador só escolhe entre os liberados; monitor novo ou fora da seleção exige novo diálogo. Input absoluto vai por stream (`NotifyPointerMotionAbsolute`). Detalhes em `13-multimonitor.md` §4.2.

### 2.6 Distros Linux-alvo: Ubuntu, Fedora e Arch
Pedido do Yuri (15/10/2026): **Fedora e Arch Linux** são alvos de primeira classe, ao lado do Ubuntu/Debian.

| | Fedora (Workstation / KDE) | Arch Linux |
|---|---|---|
| Sessão gráfica | Só Wayland (GNOME e KDE) | Escolha do usuário: GNOME, KDE, e muitos usam compositores wlroots (Sway, Hyprland) |
| Portal para o host | `xdg-desktop-portal-gnome` / `-kde`: ScreenCast + RemoteDesktop | GNOME/KDE iguais ao Fedora. **wlroots**: `xdg-desktop-portal-wlr` só tem ScreenCast, sem RemoteDesktop; Hyprland tem portal próprio com suporte parcial. Input nesses casos: protocolos `virtual-keyboard`/`wlr-virtual-pointer` ou uinput (fase 4, com aviso na UI) |
| H.264 em software | `openh264` vem do repositório da Cisco, habilitado por padrão | `openh264` no repositório `extra` |
| H.264/HEVC por hardware (VA-API) | **O Mesa do Fedora vem sem H.264/HEVC** por patentes: AMD e Intel antigas precisam do `mesa-va-drivers-freeworld` (RPM Fusion). O probe deve detectar isso e a UI deve explicar | Mesa completo; Intel usa `intel-media-driver` |
| NVIDIA | Driver proprietário via RPM Fusion (NVENC) | `nvidia` / `nvidia-open` (NVENC) |
| Segurança | SELinux enforcing: uinput e serviço de sistema precisam de política (fase 5) | Sem MAC por padrão |
| Pacote | `.rpm` + repositório COPR | `PKGBUILD` no AUR (`taildesk-bin` e `taildesk-git`) |
| Comum | **AppImage** é o pacote universal publicado em toda versão (roda em qualquer distro com glibc ≥ 2.35; build no Ubuntu 22.04). Flatpak (Flathub) fica para depois: só portal, sem uinput, e precisa de `--socket=pulseaudio` para o áudio |

Dependências de build que entram nas próximas fases (nomes de pacote):

| Para quê | Fedora | Arch | Ubuntu/Debian |
|---|---|---|---|
| Captura Wayland | `pipewire-devel` | `pipewire` | `libpipewire-0.3-dev` |
| bindgen | `clang-devel` | `clang` | `libclang-dev` |
| Teclado | `libxkbcommon-devel` | `libxkbcommon` | `libxkbcommon-dev` |
| X11 | `libxcb-devel` | `libxcb` | `libxcb1-dev` |
| VA-API | `libva-devel` | `libva` | `libva-dev` |
| Portal/segredos | `dbus-devel` | `dbus` | `libdbus-1-dev` |
| Áudio (Opus, ou `cmake` para compilar o embutido) | `opus-devel` | `opus` | `libopus-dev` |

CI: além de `ubuntu-latest` e `windows-latest`, jobs em container `fedora:latest` e `archlinux:latest` rodam clippy e testes.
Testes manuais de cada fase: Fedora Workstation (GNOME), Fedora KDE, Arch + KDE, Arch + Hyprland, Ubuntu LTS.

### 2.3 Windows
- **Captura**: DXGI Desktop Duplication (dirty rects, cursor, e é o que funciona no desktop seguro na fase 5); WGC (`windows-capture`) como fallback em notebooks híbridos onde o DD falha.
- **Input**: `SendInput`; low-level keyboard hook no controlador para capturar Win/Alt+Tab.
- **Processo**: no MVP o host roda como app na sessão do usuário. Serviço SYSTEM + agent por sessão (UAC, tela de login, Ctrl+Alt+Del) é a fase 5.

### 2.4 Vídeo: caminho mais curto até ver a tela
1. **Primeiro de ponta a ponta com `openh264`** nos dois lados + upload NV12 para o wgpu. Funciona em qualquer máquina e destrava o resto do produto.
2. Depois **HW**: Windows = Media Foundation (entrada D3D11, zero-copy); Linux = VA-API (`cros-codecs`/`libva`), com NVENC opcional. Decode HW: D3D11VA/MF e VA-API. Sempre com probe + fallback para openh264.
3. Configuração low-latency de `03-codecs.md` §2 (sem B-frames, intra refresh, VBV ≈ 1 frame, codificar só quando há dano).

### 2.5 UI
Slint + wgpu como no pacote. Como o MVP é só desktop, o risco do Slint no iOS sai da mesa; GPUI
vira alternativa real se o Slint incomodar, mas não há motivo para trocar agora.

## 3. Troca de arquivos (detalhe novo)

O pacote só tinha `FileOffer/FileAccept/FileCancel/FileProgress`. Proposta completa:

**Dois usos**
1. **Enviar/receber** (arrastar para a janela da sessão, ou botão): o arquivo cai na pasta de entrada do outro lado (`~/Downloads/TailDesk` por padrão).
2. **Navegador remoto** (painel lateral tipo RustDesk): listar, baixar e enviar dentro das **raízes permitidas** pelo host.

**Fio**
- Metadados e comandos no stream de controle (protobuf):
  `FsList{path}` → `FsListing{entries[name, kind, size, mtime]}`,
  `FileOffer{xfer_id, rel_path, size, mtime, blake3?}`, `FileRequest{xfer_id, path, offset}`,
  `FileAccept{xfer_id, offset}` (offset > 0 = retomada), `FileCancel`, `FileDone{xfer_id, blake3}`, `FileError{code}`.
- Dados: **um stream QUIC unidirecional por arquivo**, cabeçalho curto (`xfer_id`, `offset`) e bytes crus em blocos de 256 KiB. O controle de fluxo do QUIC faz o resto; vídeo continua em datagramas, então arquivo grande não trava a tela (no máximo disputa banda, e o pacer dá prioridade ao vídeo).
- Pastas = lista de `FileOffer` com `rel_path`; até 4 arquivos em paralelo.
- Integridade: **BLAKE3** calculado em streaming, conferido no `FileDone`. Escrita em `nome.part` e rename atômico no fim; retomada pelo tamanho do `.part`.

**Segurança**
- Permissão `files` vem da policy (tags/caps) e do consentimento; pode ser só-envio ou só-recebimento.
- Caminhos sempre **relativos a uma raiz permitida**, normalizados no host; rejeitar `..`, links simbólicos que saem da raiz, nomes reservados do Windows (`CON`, `NUL`…) e caminhos absolutos.
- Nunca sobrescrever sem confirmação; conflito = renomear (`arquivo (1).txt`).
- Registro de auditoria local de cada transferência (quem, o quê, tamanho, hash).

Por que não Taildrop: o tailscale-rs não o suporta, e ele não dá navegação nem integração com a sessão.

## 4. Workspace inicial

```
taildesk/
├─ Cargo.toml              # workspace, edition 2024, resolver 3
├─ crates/
│  ├─ taildesk-proto/      # .proto (prost) + header binário de mídia
│  ├─ taildesk-overlay/    # trait Overlay + Embedded(tailscale) + System(LocalAPI) + Lan(dev)
│  ├─ taildesk-tsapi/      # cliente LocalAPI: unix socket / named pipe
│  ├─ taildesk-transport/  # quinn sobre Overlay, certificados pinned, canais tipados
│  ├─ taildesk-codec/      # traits + openh264 → MF / VA-API
│  ├─ taildesk-media/      # pipelines host e cliente, pacing, QoS
│  ├─ taildesk-platform/   # capture/input/cursor/secrets: windows/, linux_wayland/, linux_x11/
│  ├─ taildesk-render/     # wgpu: NV12→RGB, escala, cursor
│  ├─ taildesk-files/      # transferência, navegação, sandbox de caminhos
│  ├─ taildesk-session/    # estados, autorização, consentimento
│  └─ taildesk-ui/         # Slint
├─ apps/
│  ├─ taildesk/            # app GUI (host + controlador)
│  └─ taildesk-cli/        # host/connect headless, probe, bench-net
└─ xtask/                  # check, empacotamento
```
CI: GitHub Actions com `windows-latest` e `ubuntu-latest` (fmt, clippy, nextest, cargo-deny). Testes de GPU marcados `#[ignore]`.

## 5. Fases

| Fase | Entrega | Pronto quando |
|---|---|---|
| **0 — Fundações** | Workspace, CI, `taildesk-proto`, `EmbeddedOverlay`, spike quinn-sobre-tailscale-rs, `taildesk-cli bench-net` | **Gate G0**: QUIC sobre o overlay embutido entre um Windows e um Linux, conexão direta e via DERP, com números de throughput/RTT/CPU. Meta mínima: 30 Mbit/s estáveis direto. O README do tailscale-rs diz que a conexão direta funciona "in many cases", mas cai para relay mais que o cliente oficial e a biblioteca não informa o caminho: o G0 **mede com que frequência o embutido fica direto** (mesma LAN e NAT diferentes) e compara com o `tailscaled`. Decisão do Yuri (`14-selecao-adaptativa.md`): o **modo sistema garante a classe LAN** desde já. |
| **1 — Ver** | Captura (DXGI no Windows, portal+PipeWire no Wayland, XShm no X11), openh264, datagramas, janela wgpu. **Protocolo por monitor desde já**: um stream de vídeo por monitor, `DisplayInfo` completo, `StartVideo`/`StopVideo` por `display_id`, mouse com `display_id`; Wayland pede `multiple=true`. **Protocolo de áudio reservado** (mensagens de `15-audio.md` §6.5 e `kind=1` com `stream_id` ≥ 0xF0), sem implementação | `taildesk-cli connect <host> --display N` mostra qualquer monitor do host em 1080p30 nos 4 sentidos (Win↔Win, Win↔Linux, Linux↔Linux). |
| **2 — Controlar** | Input com tradução automática de layouts (`11-teclado.md`: US-Intl ↔ ABNT2 etc.), cursor separado, consentimento, UI Slint com lista de peers da tailnet; multi-monitor **um por vez** (abas e atalhos, input por monitor); **spike de 2 janelas Slint com vídeo wgpu** (2 texturas NV12 a 60 fps) | Usar o host remoto para trabalho normal de escritório; matriz de teclado de `11-teclado.md` §5 passando; troca de monitor < 300 ms em LAN; latência < 60 ms em LAN. |
| **3 — Arquivos** | §3 completo: enviar/receber, navegador remoto, retomada, hash | 10 GB em ambos os sentidos sem travar o vídeo; retomada após queda de rede. |
| **4 — Qualidade** | Encode/decode HW, QoS (BBR + ReceiverReport), multi-monitor em **janelas separadas** (destacar/recolocar aba, prioridades, hotplug, "seguir o mouse", "usar todos os meus monitores"), clipboard texto/imagem, **áudio do sistema** (Opus via `cpal`: WASAPI loopback no Windows, PipeWire/Pulse no Linux) com liga/desliga que corta o tráfego, `SystemOverlay` + `whois` | 1080p60 em LAN < 40 ms; degrada sem congelar via DERP. |
| **5 — Produção** | Serviço Windows (UAC/login), não-assistido, `restore_token` Wayland, instalador MSI assinado, repositório COPR e AUR (os pacotes básicos já saem no Releases, §8), auditoria | Instalação limpa em máquina nova e acesso sem ninguém na frente (Windows). |

## 6. Decisões do Yuri (15/10/2026)

1. **Repositório**: `github.com/yurisanp/taildesk`.
2. **Licença (Q4)**: **open-source, MIT OR Apache-2.0**. Slint entra pela Royalty-free License (atribuição na tela "Sobre"); OpenH264 pelo binário da Cisco baixado em runtime; dependências GPL/AGPL só com ADR. Código do RustDesk continua proibido.
3. **Servidor de controle (Q5)**: **Tailscale (SaaS)**. Headscale fica fora do MVP; o código não deve impedi-lo (URL de controle configurável), mas não será testado nem documentado agora.
4. **Nome**: "TailDesk" lembra a marca Tailscale; como o projeto é open-source e público, vale checar as diretrizes de marca do Tailscale antes de divulgar.

## 7. Riscos específicos deste recorte

| Risco | Mitigação |
|---|---|
| quinn sobre o socket do tailscale-rs dá trabalho ou rende pouco | Spike e G0 na fase 0; plano B = modo sistema |
| Portal Wayland varia entre GNOME, KDE e wlroots (libei, restore_token, RemoteDesktop ausente no wlroots) | Testar Fedora GNOME/KDE e Arch KDE/Hyprland desde a fase 1 (§2.6) |
| Fedora sem H.264/HEVC por hardware no Mesa padrão | openh264 da Cisco como padrão; probe detecta e a UI sugere o `mesa-va-drivers-freeworld` |
| Identidade de usuário ausente no modo embutido | Tags + nós confiáveis + consentimento; `whois` no modo sistema |
| DXGI DD falha em notebook híbrido | Fallback WGC |
| openh264 pesado em 1440p/4K | É só ponte; HW na fase 4 |
| Vários monitores ao mesmo tempo multiplicam encode e banda | Qualidade cheia só no monitor em foco; os outros visíveis a 5–15 fps; abas escondidas param de codificar; **máximo de 2 monitores simultâneos com openh264** (até 4 com HW, conforme o probe) |
| Várias janelas Slint com vídeo wgpu ao mesmo tempo sem exemplo oficial | Spike na fase 2 antes de prometer "janelas separadas"; mesmo `wgpu::Device` em todas as janelas |
| Portal do KDE ignorava o stream no input absoluto (multimonitor quebrado) | Correção no MR 210 do xdg-desktop-portal-kde; testar Fedora KDE e Arch KDE com a versão das distros |
| Caractere fora do layout do host em Wayland não tem injeção padrão | Portal `NotifyKeyboardKeysym`; senão aviso no HUD (`11-teclado.md` §2.4) |

## 8. Releases (desde 15/10/2026)

Cada tag `v*` gera, pelo workflow `release.yml`, uma versão na aba Releases do GitHub com:
AppImage (Linux universal), `.deb` (Ubuntu/Debian), `.rpm` (Fedora), `.tar.gz` (outros Linux e base do
`taildesk-bin` do AUR) e `.zip` (Windows x64), mais `SHA256SUMS`. Os pacotes Linux são gerados por
`packaging/linux/build-packages.sh`. Em PRs que mexem no empacotamento o workflow só gera e testa os pacotes, sem publicar.
Pendente para a fase 5: instalador MSI assinado, repositório COPR, publicação no AUR e assinatura dos binários.
