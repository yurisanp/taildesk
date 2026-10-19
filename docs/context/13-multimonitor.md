# 13 — Multi-monitor no TailDesk

> Pesquisa pedida pelo Yuri em 15/10/2026: o **controlador decide quais monitores do host quer ver**
> e **como**: cada um numa aba/janela própria (simulando o multimonitor) ou um por vez, escolhendo qual.
> Escopo: Windows x64 e Linux x64 (Wayland principal, X11 secundário), como em `10-plano-windows-linux.md`.
> Este arquivo **não altera** `07`, `10` nem `12`; a §9 lista o que precisaria mudar neles.

## 0. Resumo

1. **Três modos de visualização, escolhidos pelo controlador** (por sessão, com padrão nas configurações):
   - **Um por vez** (padrão): uma janela, barra com uma **aba por monitor**; clicar troca a tela. Opção "seguir o mouse".
   - **Janelas separadas**: cada monitor escolhido abre na **sua própria janela** do controlador, que o usuário arrasta para um monitor local e põe em tela cheia. É a simulação de multimonitor. Uma aba pode ser **destacada** para virar janela, e a janela pode ser **recolocada** como aba.
   - **Todos numa janela** (mosaico, pós-MVP): uma só imagem com o layout real do host.
2. O pedido "abas separadas" fica coberto assim: **abas = um por vez**, **destacar a aba = janela separada**. As duas coisas convivem na mesma sessão (TeamViewer faz parecido com "Monitors as individual tabs" e "Show sessions in tabs").
3. **Protocolo: um stream de vídeo por monitor**, cada um com seu encoder, keyframes e `stream_id` (o header de 16 bytes já comporta). `StartVideo`/`StopVideo` passam a valer por `display_id` e podem ser vários ao mesmo tempo; entra `DisplaysChanged` para plugar/desplugar.
4. **Banda e CPU**: só a janela/aba com foco recebe qualidade cheia; os outros monitores visíveis caem para **5–15 fps e bitrate menor**, e monitores em abas escondidas **param de ser codificados**. Com openh264, limitar a **2 monitores simultâneos** por padrão até o encode por hardware (fase 4).
5. **Wayland muda o fluxo**: quem escolhe os monitores compartilhados é a **pessoa no host**, no diálogo do portal. O TailDesk pede `multiple=true` uma vez, e o controlador só escolhe entre os monitores que o host liberou. Trocar a seleção exige novo diálogo.
6. **Input**: coordenadas do mouse vão **por monitor** (`display_id` + posição normalizada); o host converte para o espaço do SO. Isso casa direto com o portal (`NotifyPointerMotionAbsolute` recebe o stream) e evita problemas de DPI misto e coordenadas negativas no Windows.
7. Cabe na **fase 4** do plano (multi-monitor já está lá), mas o protocolo deve nascer **por display já na fase 1** para não quebrar depois. "Um por vez" pode chegar na fase 2; "janelas separadas" na fase 4.

## 1. Como os concorrentes fazem

| Produto | Um por vez | Todos numa janela | Separado (janela/aba) | Observações |
|---|---|---|---|---|
| **TeamViewer Classic** | Ícone de monitores na barra | Sim ("show all monitors") | **"Monitors as individual tabs"**: cada monitor e a visão combinada em abas; desmarcar "Show sessions in tabs" vira janelas | Separar em janelas só quando o controlador tem tantos monitores quanto o host ([TeamViewer KB](https://www.teamviewer.com/en-in/global/support/knowledge-base/teamviewer-classic/remote-control/in-session-features/use-multi-monitor-support)) |
| **AnyDesk** | Um retângulo numerado por monitor na barra | Não documentado | Só abrindo **várias conexões** ao mesmo host, uma por janela ([AnyDesk](https://support.anydesk.com/docs/how-do-i-work-with-multiple-displays.md)) | Simples, mas custa uma sessão por monitor |
| **RustDesk** | Seletor de monitor | Sim | Alternou entre janelas e abas entre versões; usuários pedem abas destacáveis e reclamam de janelas que não podem ir para outro monitor local ([discussão #2889](https://github.com/rustdesk/rustdesk/discussions/2889), [issue #7454](https://polar.sh/rustdesk/rustdesk/issues/7454)) | Lição: arrastar uma janela para outro monitor local não pode ser tratado como "borda da tela" |
| **RemotePC** | Sim | "All Monitors (One Window)" e "(Scrollable)" | Sim ([AnyViewer](https://www.anyviewer.com/how-to/access-multiple-monitors-with-remotepc-2578-gc.amp.html)) | — |
| **Chrome Remote Desktop / Windows App** | Seletor só com 2+ monitores (já citado em `12` §3) | CRD: "mostrar todos" | RDP: "usar todos os meus monitores" cria monitores virtuais na sessão | RDP é outro modelo (sessão nova), não espelha monitores físicos |

Conclusões para o TailDesk:
- Todos oferecem **trocar de monitor** com um clique; é o mínimo.
- A forma mais pedida é **aba destacável**: começa simples e vira multimonitor quando o usuário quer.
- **Não exigir** o mesmo número de monitores dos dois lados (limite do TeamViewer): janelas separadas são janelas comuns; o usuário põe onde quiser, inclusive duas no mesmo monitor local.
- Evitar o modelo do AnyDesk (uma conexão por monitor): duplica consentimento, input e cursor.

## 2. Modos de visualização

### 2.1 Um por vez (padrão)
- Janela da sessão como em `12` §5.6, com a **faixa de abas de monitores** na barra (só aparece com 2+ monitores compartilhados). Rótulo: número + nome + resolução, ex. "1 · DELL U2720Q · 2560×1440", com estrela no principal.
- Miniatura do layout do host no menu do seletor (retângulos na posição real), como no painel de vídeo do Windows. Clicar num retângulo troca.
- Atalhos: `Ctrl+Alt+→/←` próximo/anterior, `Ctrl+Alt+1…9` direto (a confirmar com a decisão pendente de atalhos em `12`).
- **Seguir o mouse** (desligado por padrão): quando o cursor do host cruza para outro monitor, a aba troca sozinha. Útil quando o host usa atalhos que movem janelas (Win+Shift+→).
- No fio: só **um** stream ativo. Trocar = `StopVideo` do anterior + `StartVideo` do novo, com keyframe imediato. Meta: troca visível em < 300 ms em LAN.
- Enquanto o novo keyframe não chega, a janela mostra o último quadro do monitor (se houver) desfocado, em vez de preto.

### 2.2 Janelas separadas (simulação de multimonitor)
- Ao conectar (ou pelo menu do seletor) o usuário marca quais monitores quer ver; **cada um abre numa janela própria**, título "notebook-yuri · Monitor 2".
- Cada janela tem barra própria (tela cheia, ajustar/tamanho real); arquivos, teclado e "⋯" ficam na janela do monitor principal, ou em qualquer uma com o mesmo estado (decidir no protótipo).
- **Destacar/recolocar**: arrastar uma aba para fora (ou botão "abrir em janela") cria a janela; fechar a janela de um monitor devolve a aba, não encerra a sessão. Encerrar a sessão fecha todas.
- **Posicionamento automático** (opcional, "Usar todos os meus monitores"): se o controlador tem N monitores e o usuário escolheu até N, o TailDesk distribui as janelas em tela cheia nos monitores locais seguindo a ordem esquerda→direita do host. Lembrar a distribuição por host.
  - Implementado: botão "🖥 Todos os monitores" na barra da sessão (com 2+ monitores no host). Os N primeiros monitores liberados do host, na ordem das abas, vão para os N monitores locais ordenados por posição; a janela da sessão fica com o que já mostrava (nenhum vídeo reinicia) e os outros viram destacados em tela cheia (`Fullscreen::Borderless` no monitor local certo). Sair da tela cheia em qualquer janela devolve tudo às abas. Com um monitor só num dos lados, nada muda.
- Teclado vai para a janela com foco; mouse vale na janela onde está. O cursor remoto só é desenhado na janela do monitor onde ele está no host.
- No fio: um stream por janela aberta e visível.

### 2.3 Todos numa janela (mosaico) — pós-MVP
- Uma superfície com todos os monitores no layout do host, com rolagem ou ajuste. O plano atual já deixa mosaico para depois; aqui fica como 3º modo.
- Duas formas de implementar: (a) compor no **controlador** os N streams numa textura wgpu (reaproveita tudo da §2.2, sem mudar o host); (b) capturar a área virtual inteira num stream só (mais barato em encoders, mas resolução enorme e não funciona em Wayland, onde cada monitor é um stream). **Recomendação: (a)**.

### 2.4 O que o usuário escolhe, e onde
| Onde | O quê |
|---|---|
| Configurações → Sessão | "Ao conectar, mostrar os monitores em": Abas / Janelas separadas / Todos os meus monitores, e "Seguir o mouse". O que o usuário usa numa sessão (entrar/sair de "Todos os monitores", botão "🖱 Seguir o mouse") fica lembrado para aquele host (`monitores_host` no config, pelo `node_stable_id`) e vale por cima do padrão |
| Diálogo de conexão / detalhe do dispositivo | Quais monitores abrir (lembrado por host; padrão = principal) |
| Barra da sessão | Abas de monitores, menu com layout do host, destacar/recolocar, trocar modo a qualquer momento |

Textos (pt-BR / en): "Um por vez" / "One at a time"; "Janelas separadas" / "Separate windows"; "Todos numa janela" / "All in one window"; "Seguir o mouse" / "Follow mouse"; "Usar todos os meus monitores" / "Use all my monitors"; "Abrir em janela" / "Open in window"; "Voltar para abas" / "Back to tabs".

## 3. Protocolo

Hoje (`07` §2–3) o handshake tem `displays: [DisplayInfo]` no `HelloAck`, um `StartVideo{display_id}` e um `SwitchDisplay`. Proposta:

```protobuf
message DisplayInfo {
  uint32 id = 1;            // estável na sessão; não reutilizar após remover
  string name = 2;          // "DELL U2720Q" (EDID/portal); vazio se desconhecido
  int32  x = 3; int32 y = 4; // posição no layout do host, em pixels lógicos (pode ser negativa)
  uint32 width = 5; uint32 height = 6;        // pixels físicos capturados
  uint32 logical_width = 7; uint32 logical_height = 8;
  float  scale = 9;         // 1.0, 1.25, 1.5, 2.0…
  uint32 refresh_mhz = 10;
  bool   primary = 11;
  Rotation rotation = 12;
  bool   shared = 13;       // false = existe mas o host não liberou (Wayland: fora da seleção do portal)
}

message DisplaysChanged { repeated DisplayInfo displays = 1; }  // H→C: plugou, desplugou, mudou resolução/escala

message StartVideo { uint32 display_id = 1; QualityPrefs wanted = 2; Priority priority = 3; }
message StopVideo  { uint32 display_id = 1; }
message SetPriority { uint32 display_id = 1; Priority priority = 2; } // FOCUSED | VISIBLE | (HIDDEN = StopVideo)
message VideoStopped { uint32 display_id = 1; Reason reason = 2; }     // H→C: monitor removido, portal fechou o stream…
```

- **Vários `StartVideo` ativos ao mesmo tempo**, cada um respondido por um `CodecSelected{display_id, stream_id, …}` próprio. `SwitchDisplay` some (vira Stop + Start, mais simples de testar).
- `RequestKeyframe`, `PictureLoss`, `Nack`, `ReceiverReport` já levam `display`: manter.
- `MouseMove` ganha **`display_id` + `x,y` normalizados (u16, 0..65535) dentro daquele monitor**; `rel` continua igual. `CursorPos` do host também passa a trazer `display_id`.
- Header de mídia: nada muda; `stream_id` (u8) já identifica o stream de cada monitor.
- Limites do parser: no máximo 16 `DisplayInfo` por mensagem, `name` ≤ 128 bytes; host recusa mais `StartVideo` simultâneos que o seu limite (`Error{TOO_MANY_STREAMS}`).
- Testes de round-trip para as mensagens novas (regra do `AGENTS.md`).

## 4. Host: captura por monitor

### 4.1 Windows
- **Enumerar**: `EnumDisplayMonitors` + `GetMonitorInfoW` (retângulo no desktop virtual, que pode ter coordenadas negativas) e `QueryDisplayConfig`/`DisplayConfigGetDeviceInfo` para o nome amigável. Processo precisa ser **per-monitor DPI aware v2** para receber pixels reais.
- **DXGI Desktop Duplication**: um `IDXGIOutputDuplication` **por output**, criado no `ID3D11Device` do **adaptador dono daquele output**; o WebRTC agrupa por adaptador da mesma forma ([dxgi_adapter_duplicator.h](https://webrtc.googlesource.com/src/+/main/modules/desktop_capture/win/dxgi_adapter_duplicator.h), [Chromium](https://codereview.chromium.org/2099123002)). Em notebook híbrido, `DuplicateOutput` falha com `DXGI_ERROR_UNSUPPORTED` quando o processo roda na GPU errada ([freemancw](https://freemancw.com/tag/desktop-duplication-api)): cair para **WGC** (`GraphicsCaptureItem` por monitor) só naquele monitor, como o plano já prevê.
- Monitores em GPUs diferentes ⇒ texturas em devices diferentes: o encoder HW de cada stream deve usar o device do seu adaptador (ou copiar via CPU no caminho openh264, que já faz readback).
- Plug/desplug e mudança de modo: `WM_DISPLAYCHANGE` + `DXGI_ERROR_ACCESS_LOST` ⇒ re-enumerar, recriar o duplicador e mandar `DisplaysChanged`.
- **Input**: `SendInput` com `MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK`, coordenadas normalizadas sobre o desktop virtual. O host converte `(display_id, x, y)` → retângulo do monitor → desktop virtual.

### 4.2 Linux Wayland (portal + PipeWire)
- `ScreenCast.SelectSources` com `types=MONITOR` e **`multiple=true`**; o `Start` devolve **um stream PipeWire por monitor**, cada um com `position`, `size` e `logical_size` ([ScreenCast](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html), [feaneron](https://feaneron.com/?p=6840)).
- **A seleção é feita pela pessoa no host**, no diálogo do compositor. Consequências:
  - O TailDesk não consegue "pedir o monitor 2" depois: monitores fora da seleção aparecem como `shared=false` e o controlador mostra "não liberado pelo host".
  - Para mudar a seleção: novo `CreateSession` → novo diálogo no host (com consentimento), e streams novos. O `restore_token` (fase 5) guarda a seleção.
  - Recomendação: no Wayland, o diálogo de consentimento do TailDesk explica "escolha **todos** os monitores que a pessoa pode ver"; o controlador decide depois quais abrir. Streams que ninguém está vendo são **pausados no host** (não consumir o PipeWire, sem encode), então liberar todos custa pouco.
- **Input**: `RemoteDesktop.NotifyPointerMotionAbsolute(stream, x, y)` recebe o **node do stream** e coordenadas no espaço lógico dele ([RemoteDesktop](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.RemoteDesktop.html)), o que casa exatamente com `(display_id, x, y)`. Atenção: o portal do KDE ignorava o stream e usava o deslocamento do último stream iniciado, quebrando multimonitor; a correção está no [MR 210 do xdg-desktop-portal-kde](https://invent.kde.org/plasma/xdg-desktop-portal-kde/-/merge_requests/210). Testar multimonitor no Fedora KDE e no Arch KDE com a versão das distros.
- libei (`ConnectToEIS`): os dispositivos de ponteiro absoluto expõem **regiões**; é preciso casar cada região com o stream do monitor (a confirmar na fase 1 como cada compositor relaciona região e stream).
- Plug/desplug: monitor removido ⇒ o stream PipeWire fecha (`VideoStopped`); monitor novo **não** entra sozinho na sessão do portal (precisa de nova seleção). A UI do controlador mostra "Novo monitor no host · pedir acesso".
- wlroots (Sway) / Hyprland: ScreenCast com vários monitores existe, mas o `xdg-desktop-portal-wlr` historicamente escolhe **um** output por stream e sem RemoteDesktop (plano §2.6); validar `multiple=true` nesses portais na fase 4 (inferência, não testado).
- GNOME oferece também **monitor virtual** (`types=VIRTUAL`): útil no futuro para "estender a tela" (fora do escopo; ver `10` §1).

### 4.3 Linux X11
- Uma raiz só; monitores via **RandR** (`RRGetMonitors`, `x11rb`) com retângulos dentro da raiz.
- Captura por monitor = `XShmGetImage` do **sub-retângulo** da raiz (um segmento SHM por monitor) + XDamage filtrando o dano por retângulo.
- Input: XTest em coordenadas da raiz = offset do monitor + posição.
- Plug: evento `RRScreenChangeNotify` ⇒ re-enumerar e `DisplaysChanged`.

### 4.4 Resumo
| | Enumeração | Captura | Quem escolhe | Input absoluto | Hotplug |
|---|---|---|---|---|---|
| Windows | EnumDisplayMonitors + DisplayConfig | DXGI DD por output (WGC fallback) | Controlador | SendInput no desktop virtual | Automático |
| Wayland | Streams do portal | 1 node PipeWire por monitor | **Pessoa no host** (diálogo) | Portal por stream / regiões libei | Remoção automática; adição pede nova seleção |
| X11 | RandR | XShm por sub-retângulo | Controlador | XTest na raiz | Automático |

Trait em `taildesk-platform` (já esboçada em `06` §3): `displays()` + `start(display, opts, sink)` por display; acrescentar um stream de eventos `DisplayEvent::{Added, Removed, Changed}` e `pause/resume(display)` para não capturar o que ninguém vê.

## 5. Mídia, banda e CPU

- **Um pipeline por monitor** (captura → encoder → pacer), como `06` §4 já indica ("encoder pool por display"). Um pacer único divide a banda entre os streams.
- **Prioridades**: `FOCUSED` = qualidade e FPS cheios; `VISIBLE` = teto de 5–15 fps e ~25% do bitrate, sobe quando há dano grande (ex.: vídeo tocando); aba escondida = `StopVideo` (zero custo). O controlador manda `SetPriority` ao mudar o foco de janela.
  - Implementado: o host corta o `VISIBLE` para no máximo 15 fps e 30 % do teto de taxa, só enquanto outro monitor `FOCUSED` estiver saindo (um só `VISIBLE` não perde nada); a troca vale em até 100 ms (laço de taxa). O controlador manda `SetPriority` quando a janela da sessão ou a de um destacado ganha foco, e depois de trocar, destacar ou recolocar.
- Divisão de banda sugerida: 70% para o stream com foco, 30% dividido entre os visíveis, ajustada pelo laço de QoS de `07` §5.
- **Limite padrão de streams simultâneos**: 2 com openh264 (1440p em software já é pesado, ver riscos de `10` §7); até 4 com encoder HW, conforme o probe. Acima disso a UI avisa "Mais monitores podem deixar a sessão lenta".
- Resolução de cada stream independe das outras (monitor 4K e 1080p na mesma sessão). Monitor HiDPI com openh264: oferecer escala para baixo no host (ex. 4K → 1440p) por padrão em `VISIBLE`.
- Keyframes independentes por stream; perda num stream não pede keyframe nos outros.

## 6. Controlador: janelas e render

- Slint 1.18 cria várias janelas mostrando várias instâncias de componente ([docs Slint](https://docs.slint.dev/latest/docs/rust/slint/)). O vídeo entra por `set_rendering_notifier` **por janela**, e todas precisam usar o **mesmo `wgpu::Device`/`Queue`** do Slint ([wgpu_texture](https://git.joshthomas.dev/language-servers/slint/src/branch/master/examples/wgpu_texture/README.md), [módulo wgpu_30](https://docs.slint.dev/latest/docs/rust/slint/wgpu_30/)). O Slint 1.18.1 tem o recurso `unstable-wgpu-30`, compatível com o `wgpu` 30.0.1 do plano.
- Não achei exemplo oficial de várias janelas com vídeo wgpu ao mesmo tempo: **fazer um spike na fase 2** (2 janelas, 2 texturas NV12, 60 fps) antes de prometer o modo "janelas separadas".
- Um decoder por stream. Decode HW com vários streams: D3D11VA/VA-API aguentam vários contextos, mas é preciso medir em GPU integrada.
- Abas: a faixa de abas é um componente Slint dentro da barra (não abas do SO). Destacar = criar nova janela com a mesma textura do stream; o stream não reinicia.
- Captura de teclado (hook Win/Alt+Tab, `11-teclado.md`) vale para a janela com foco; trocar de janela solta as teclas (`ReleaseAll`).
- Mouse saindo da janela em direção a outra janela do mesmo host: cada janela manda coordenadas do seu monitor; não simular "atravessar a borda" (o RustDesk erra aqui ao tratar o movimento como borda da tela).

## 7. Casos de borda

| Caso | Comportamento |
|---|---|
| Host com 1 monitor | Sem abas nem seletor (como `12` §5.6) |
| Monitor desplugado com janela aberta | Janela mostra "Monitor desconectado no host" e oferece fechar; em "um por vez", troca para o principal |
| Resolução/escala muda no host | `DisplaysChanged` + novo `CodecSelected`; janela reajusta sem fechar |
| Monitores espelhados (clone) no host | Mostrar uma vez (mesmo retângulo) |
| Monitores com DPI diferente | Coordenadas sempre por monitor; host converte |
| Wayland, monitor fora da seleção | Aba cinza "Não liberado pelo host" com botão "Pedir acesso" (novo diálogo no host) |
| Controlador com menos monitores que o host | Janelas separadas continuam possíveis (lado a lado ou sobrepostas); só "usar todos os meus monitores" fica limitado ao que cabe |
| Reconexão (`12` F7) | Restaura os mesmos monitores e o mesmo modo |
| Privacidade | Faixa "sendo controlado" do host (`12` §5.5) diz quais monitores estão sendo vistos, ex. "vendo monitores 1 e 2" |

## 8. Fases sugeridas

| Fase do plano | Entrega de multi-monitor |
|---|---|
| 1 — Ver | Protocolo já por display (`DisplayInfo` completo, `StartVideo/StopVideo` por id, `display_id` no mouse). CLI: `taildesk-cli connect <host> --display 2`. Wayland pede `multiple=true` |
| 2 — Controlar | Modo **um por vez** com abas e atalhos; input por monitor; spike de 2 janelas Slint + wgpu |
| 4 — Qualidade | Modo **janelas separadas** com destacar/recolocar, prioridades e divisão de banda, hotplug completo, "seguir o mouse", "usar todos os meus monitores" |
| Pós-MVP | **Todos numa janela** (mosaico composto no controlador); monitor virtual |

Testes manuais: Windows com 2 monitores de DPI diferente; notebook híbrido Intel+NVIDIA; Fedora GNOME e KDE com 2 monitores; Arch Hyprland; X11 (Ubuntu com sessão Xorg ou Arch + i3).

## 9. O que precisaria mudar nos outros documentos

Não editei estes arquivos; ficam para as threads donas.

**`10-plano-windows-linux.md`** (thread "Arquitetura inicial do TailDesk")
- §1: trocar "Multi-monitor (troca de tela, não mosaico)" por "Multi-monitor: um por vez e janelas separadas, escolhidos pelo controlador; mosaico depois".
- §5: fase 1 com protocolo por display; fase 2 com "um por vez"; fase 4 com "janelas separadas" (hoje a fase 4 só diz "multi-monitor").
- §2.2: registrar que no Wayland a seleção de monitores é do host (diálogo do portal, `multiple=true`) e que novos monitores exigem nova seleção.
- §7: dois riscos novos: várias janelas Slint com vídeo wgpu (spike na fase 2) e input multimonitor no portal KDE (MR 210).

**`07-protocolo.md`** (mesma thread; também `docs/context/07` no repositório)
- `DisplayInfo` completo (posição, tamanho lógico, escala, rotação, `shared`), `DisplaysChanged`, `StopVideo{display_id}`, `SetPriority`, `VideoStopped`; remover `SwitchDisplay`; vários `StartVideo` simultâneos.
- `MouseMove` e `CursorPos` com `display_id` e coordenadas normalizadas por monitor.

**`06-arquitetura.md`** §3–4: trait de captura com `DisplayEvent` e `pause/resume`; pacer único dividindo banda por prioridade.

**`design/12-levantamento-ux.md`** (thread "Levantamento de design da interface")
- §5.6: o "seletor de monitor" vira **faixa de abas de monitores** com menu de layout, "abrir em janela" e "seguir o mouse".
- §8 item 1 ("Sessão em janela própria, sem abas no MVP"): esclarecer que a regra vale para **sessões** (uma janela por host); abas de **monitores dentro da sessão** são outra coisa, e cada monitor pode virar janela própria.
- §5.9 Configurações → Sessão: modo padrão de monitores, "seguir o mouse", "usar todos os meus monitores".
- §5.5 faixa do host: mostrar quais monitores estão sendo vistos.
- Protótipo Slint: incluir a sessão com 2 monitores (abas) e uma janela destacada.

## 10. Fontes
- TeamViewer, multi-monitor: <https://www.teamviewer.com/en-in/global/support/knowledge-base/teamviewer-classic/remote-control/in-session-features/use-multi-monitor-support>
- AnyDesk, múltiplos monitores: <https://support.anydesk.com/docs/how-do-i-work-with-multiple-displays.md>
- RustDesk, janelas vs abas: <https://github.com/rustdesk/rustdesk/discussions/2889>; janelas em vários monitores: <https://polar.sh/rustdesk/rustdesk/issues/7454> (só a discussão pública; nada de código, regra 9 do `AGENTS.md`)
- RemotePC: <https://www.anyviewer.com/how-to/access-multiple-monitors-with-remotepc-2578-gc.amp.html>
- Portal ScreenCast: <https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html>
- Portal RemoteDesktop: <https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.RemoteDesktop.html>
- KDE, input por stream: <https://invent.kde.org/plasma/xdg-desktop-portal-kde/-/merge_requests/210>
- Portais e streams (feaneron): <https://feaneron.com/?p=6840>
- WebRTC, DXGI por adaptador: <https://webrtc.googlesource.com/src/+/main/modules/desktop_capture/win/dxgi_adapter_duplicator.h>; Chromium: <https://codereview.chromium.org/2099123002>
- DXGI em GPU híbrida: <https://freemancw.com/tag/desktop-duplication-api>
- Slint (várias janelas, wgpu_30): <https://docs.slint.dev/latest/docs/rust/slint/>; exemplo wgpu_texture: <https://git.joshthomas.dev/language-servers/slint/src/branch/master/examples/wgpu_texture/README.md>
