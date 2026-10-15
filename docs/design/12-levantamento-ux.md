# 12 — Levantamento de design (UX/UI) do TailDesk

> Pedido do Yuri em 15/10/2026: "levantamento para o design da nossa ferramenta".
> Escopo: MVP Windows + Linux de `10-plano-windows-linux.md` (desktop remoto assistido + troca
> de arquivos + tradução de teclado de `11-teclado.md`), UI em Slint.
> Método: análise competitiva e heurística (produtos de referência, documentação, relatos de
> usuários em issues e fóruns) mais um plano de pesquisa com usuários para validar (§10).
> **Ainda não houve entrevistas nem testes com usuários**: tudo aqui é hipótese embasada, e o
> que é inferência está marcado como tal.

## 1. Resumo em uma página

1. **O TailDesk não é um TeamViewer.** Não existe "ID + senha" nem servidor de rendezvous: só
   máquinas da mesma tailnet se enxergam. A tela inicial, portanto, é a **lista de dispositivos
   da tailnet**, como no app do Tailscale, e conectar é **um clique** num dispositivo online.
2. **Segurança visível, sem cerimônia.** A identidade vem da tailnet; o diálogo de consentimento
   no host mostra **quem** (usuário/máquina da tailnet), **o que** pede (ver, controlar,
   arquivos) e um botão de recusar óbvio. Durante a sessão o host sempre sabe que está sendo
   controlado e encerra com um clique.
3. **A sessão é a tela.** A janela remota ocupa tudo; os controles ficam numa barra recolhível no
   topo (padrão do Windows App/RDP e do Chrome Remote Desktop) e num HUD discreto de qualidade
   (padrão do Parsec).
4. **Teclado "simplesmente funciona" é o diferencial.** O RustDesk tem três modos de teclado
   (Legacy, Map, Translate) e ainda assim recebe relatos de layouts que não digitam certo
   ([issue #7530](https://github.com/rustdesk/rustdesk/issues/7530),
   [technibble](https://www.technibble.com/forums/threads/best-remote-software.91836/page-4)).
   O TailDesk mostra **um** indicador ("US-Intl → ABNT2 · Auto") e esconde os modos manuais num
   submenu.
5. **Arquivos dentro da sessão**, sem abrir outra conexão (reclamação comum ao RustDesk no mesmo
   fórum): arrastar para a janela envia; um painel lateral navega o host.
6. **MVP com 8 telas** (§5) e **7 fluxos** (§6). Recomendação: prototipar primeiro a lista de
   dispositivos, o consentimento e a barra da sessão, que são os três pontos onde o produto
   ganha ou perde confiança.

## 2. Quem vai usar (hipóteses de público)

| Perfil | Trabalho a fazer (JTBD) | O que importa | Prioridade no MVP |
|---|---|---|---|
| **Dono das próprias máquinas** (o Yuri: desktop Windows em casa, notebook Linux, servidor) | "Quando estou longe, quero usar meu outro computador como se estivesse na frente dele" | Conectar rápido, teclado certo, nitidez de texto, trazer/levar arquivos | **Principal** |
| **Quem ajuda família/amigos** que estão na mesma tailnet (ou foram convidados) | "Quando alguém pede ajuda, quero ver e mexer na tela dele sem ensinar a instalar coisas" | Consentimento claro do outro lado, instalação simples, segurança contra golpes | Secundário (o modo assistido do MVP serve) |
| **Dev / homelab** | "Quero abrir a GUI de uma VM ou servidor Linux sem configurar VNC" | Wayland funcionando, CLI (`taildesk-cli connect`), multi-monitor | Secundário |

Fora do MVP: suporte técnico comercial (filas, tickets, acesso não-assistido em massa) e jogos.

## 3. Referências: o que aproveitar e o que evitar

| Produto | Aproveitar | Evitar |
|---|---|---|
| **Tailscale (app windowed)** | Barra lateral + **lista pesquisável** de dispositivos, detalhe num painel à direita com ações de um clique (copiar IP/MagicDNS, ping, enviar arquivo); status ao lado de cada dispositivo; erros visíveis no rodapé da barra lateral e janela de diagnóstico ([blog Tailscale](https://tailscale.com/blog/windowed-macos-ui-beta)). Motivo declarado: o menu da bandeja escondia recursos e não comportava busca. | Depender só do ícone da bandeja. |
| **RustDesk** | Painel de transferência de arquivos lado a lado; endereço/ID favoritos; abas por sessão. | Três modos de teclado expostos ao usuário e ainda com falhas; arquivos exigindo outra conexão; UI Flutter densa de opções. Licença AGPL: só referência visual, nada de código. |
| **AnyDesk / TeamViewer** | Diálogo de aceite no host com permissões por checkbox; confirmação extra para transferência de arquivos; listas de bloqueio/permissão ([TeamViewer](https://www.teamviewer.com/en-ca/global/support/knowledge-base/teamviewer-remote/security/teamviewer-and-scamming/)). Modos de teclado "Remoto" (posição física) e "Local" (layout local) da [AnyDesk](https://support.anydesk.com/docs/keyboard-hotkeys). | O modelo "me passa seu ID e senha", base dos golpes de falso suporte técnico ([pcrisk](https://www.pcrisk.com/removal-guides/15592-teamviewer-tech-support-scam), [isthisascam](https://isthisascam.to/scam-types/tech_support)). Um diálogo de aceite não diz **quem** está do outro lado; o nosso precisa dizer. |
| **Parsec** | Overlay/HUD com avisos de rede e hardware que podem ser desligados; escolha de janela ou tela cheia ao conectar ([Parsec Windows](https://support.parsec.app/hc/en-us/articles/32381199341716-Parsec-App-for-Windows), [Parsec Linux](https://support.parsec.app/hc/en-us/articles/32381494397332-Parsec-App-for-Linux)). | Foco em jogos: interface escura e "gamer" não serve ao público de escritório. |
| **Chrome Remote Desktop / Windows App (RDP)** | Barra de sessão no topo em tela cheia, revelada ao passar o mouse, fixável com alfinete; controles condicionais (seletor de monitor só com mais de um monitor); "ajustar à janela" vs "redimensionar o remoto" ([guia CRD](https://www.airdroid.com/remote-support/how-do-i-use-chrome-remote-desktop), [toolbar overview](https://docs.fixme.it/help/features/remote-desktop-toolbar-overview)). | Barra que some e o usuário não acha de volta (reclamação recorrente, ver [AnyViewer](https://www.anyviewer.com/how-to/remote-desktop-taskbar-not-showing-in-full-screen-1796.html)): ter sempre um atalho de teclado e uma "alça" visível. |

## 4. Princípios de design

1. **Um clique para conectar** um dispositivo online da tailnet. Configuração vem depois, nunca antes.
2. **Mostrar identidade, não códigos.** Nome da máquina, usuário da tailnet, SO e IP 100.x em
   todo lugar onde se decide algo (lista, consentimento, barra da sessão).
3. **O host está no controle.** Indicador permanente enquanto alguém controla, botão
   "Encerrar" e atalho global para encerrar (`Ctrl+Alt+Pause` sugerido; a validar).
4. **Padrões inteligentes, ajustes escondidos.** Teclado Auto, qualidade automática, pasta de
   entrada padrão. Manuais existem, mas num nível abaixo.
5. **Estado sempre explicado.** Conectando, direto vs relay (DERP), qualidade caindo, layout
   traduzido, caractere que não pôde ser digitado: cada um tem uma frase curta e humana.
6. **Nunca perder trabalho.** Queda de rede reconecta sozinha e preserva a janela; transferência
   retoma; teclas "presas" são soltas ao perder foco (já previsto em `11-teclado.md` §2.3).
7. **Parecer nativo e sóbrio** nos dois SOs: claro/escuro seguindo o sistema, sem estética de jogo.

## 5. Telas do MVP (arquitetura de informação)

Navegação: janela principal com barra lateral (Dispositivos, Transferências, Configurações) e,
por sessão, uma janela própria (ou aba, a decidir no protótipo).

### 5.1 Primeira execução
- Passo 1: "Entrar na sua tailnet" (login do Tailscale embutido; abre o navegador). Texto
  explica em uma frase: "O TailDesk só conecta máquinas da sua rede Tailscale."
- Passo 2: "Este computador pode ser acessado?" com duas escolhas: **Sim, pedindo minha
  permissão a cada vez** (padrão) / **Não, só vou acessar outros**.
- Passo 3 (Linux Wayland): explicar antes que o sistema vai pedir permissão de tela e de
  controle, e por quê. O diálogo do portal é do SO e assusta se aparecer sem aviso. Pedir que a
  pessoa libere **todos** os monitores que o controlador pode ver: no Wayland quem escolhe os
  monitores é o host, e o controlador só alterna entre os liberados (`13-multimonitor.md` §4.2).

### 5.2 Dispositivos (tela inicial)
```
┌ TailDesk ───────────────────────────────────────────────────────────────┐
│ ▸ Dispositivos │ 🔍 Buscar dispositivos                     [tailnet ▾]│
│   Transferências│ ─────────────────────────────────────────────────────── │
│   Configurações │ ★ desktop-casa   Windows 11   ● online · direto  [Conectar]│
│                 │   notebook-yuri  Fedora 43    ● online · relay   [Conectar]│
│                 │   servidor       Ubuntu       ○ sem TailDesk   [Como instalar]│
│                 │   celular        Android      — não suportado            │
│ ─────────────── │                                                          │
│ ⚠ 1 aviso       │ Detalhe (painel à direita ao selecionar): IP 100.x,      │
│ Este PC: aceita │ MagicDNS [copiar], último acesso, teclado, [Enviar arquivo]│
└─────────────────┴──────────────────────────────────────────────────────────┘
```
- Estados por dispositivo: online com TailDesk, online sem TailDesk, offline, não suportado.
  O modo embutido do tailscale-rs pode não expor todos os dados (SO, usuário); mostrar o que houver.
- Favoritos (★) no topo; recentes logo abaixo.
- Ações rápidas no detalhe: Conectar, Enviar arquivo (sem abrir sessão), Copiar endereço.
- Rodapé da barra lateral: status do próprio PC ("Aceitando conexões" / "Desativado") e avisos,
  como no Tailscale.

### 5.3 Conectando
Etapas com texto curto, substituindo o spinner mudo: "Encontrando desktop-casa…" →
"Esperando desktop-casa aceitar…" (com botão Cancelar) → abre a sessão. Se o host recusar ou
não responder em 60 s, dizer qual dos dois.

### 5.4 Pedido de acesso (no host)
```
┌──────────────────────────────────────────────┐
│  notebook-yuri quer acessar este computador  │
│  Usuário: yuri@  ·  Fedora 43  ·  100.64.1.7 │
│  ✓ ver a tela   ✓ controlar   ✓ arquivos     │
│  [ ] Lembrar deste dispositivo               │
│        [Recusar]          [Permitir]         │
└──────────────────────────────────────────────┘
```
- Sempre em primeiro plano, foco inicial em **Recusar** (proteção contra Enter acidental).
- Permissões desmarcáveis pelo host antes de aceitar.
- "Lembrar" grava na lista local de nós confiáveis (`node_stable_id`, `10-plano` §2.1); a lista
  é editável em Configurações. Mesmo lembrado, o host vê o aviso de sessão ativa (§5.5).
- Inferência a validar: um alerta extra "Nunca aceite pedidos de quem ligou dizendo ser
  suporte técnico" na primeira vez reduz o risco do golpe clássico.

### 5.5 Indicador de sessão ativa (no host)
Faixa pequena e arrastável no topo da tela do host: "notebook-yuri controla este PC ·
vendo monitores 1 e 2 · arquivos ✓ · [Pausar controle] [Encerrar]", mais ícone colorido na bandeja. Em Wayland, o próprio
GNOME/KDE também mostra seu indicador de compartilhamento; não brigar com ele.

### 5.6 Janela da sessão (no controlador)
```
  ┌─ desktop-casa │ [1 · DELL ★ ⧉] 2 · LG ▾ │ ⌨ ABNT2→US-Intl │ 📁 │ ⤢ │ ⋯ │ ✕ ─┐  ← barra (recolhe)
            │                                                                │
            │                  tela remota                                   │
            │                                                     ┌────────┐ │
            │                                                     │ 12 ms  │ │ ← HUD opcional
            │                                                     │ direto │ │
            │                                                     └────────┘ │
```
- **Barra**: nome do host (menu com detalhes), abas de monitores (§5.6.1, só com 2+ monitores),
  indicador de teclado (clique abre Auto/Físico/Texto), arquivos (abre painel §5.7), tela cheia,
  "⋯" (Ctrl+Alt+Del para o host Windows, ajustar à janela / tamanho real, qualidade,
  mostrar HUD), desconectar.
- Barra recolhe em tela cheia; reaparece com o mouse no topo, com uma alça sempre visível e
  com atalho (`Ctrl+Alt+Home` sugerido; a validar contra conflitos nos dois SOs).
- **Captura de teclado**: em foco, Win/Alt+Tab vão para o remoto (`10-plano` §2.3); deixar claro
  como "sair" (atalho mostrado ao entrar em tela cheia, como faz o navegador).
- **HUD** desligado por padrão; aparece sozinho por alguns segundos quando a conexão vira relay
  ou a qualidade cai ("Conexão via relay, qualidade reduzida").
- **Toast de teclado** (de `11-teclado.md` §2.4): "Não foi possível digitar ‘ß’ neste host"
  com no máximo um aviso a cada poucos segundos para não poluir.

#### 5.6.1 Vários monitores no host (aprovado em 15/10/2026, detalhe em `13-multimonitor.md`)
- **Um por vez é o padrão**: uma aba por monitor na barra, rótulo "1 · DELL U2720Q ★" (estrela no
  principal). Clicar troca o monitor; enquanto o novo quadro não chega, mostrar o último quadro
  daquele monitor desfocado, nunca preto.
- **▾ ao lado das abas**: miniatura do layout do host (clicar num retângulo troca), "Um por vez",
  "Abrir monitor N em janela" e **Seguir o mouse** (desligado por padrão: troca de aba quando o
  cursor muda de monitor no host).
- **Arrastar a aba para fora vira janela própria** (há também o botão ⧉ na aba ativa). A janela
  se chama "desktop-casa · Monitor 2", tem barra própria com "Voltar para abas" e o teclado; o
  painel de arquivos fica só na janela principal da sessão. A aba original fica em itálico com
  "em janela".
- **Fechar a janela de um monitor devolve a aba** e não encerra a sessão. "Desconectar" em qualquer
  janela encerra a sessão e fecha todas. A última aba não pode ser destacada.
- **Wayland**: só aparecem os monitores que a pessoa liberou no diálogo do sistema; os outros ficam
  como aba cinza "não liberado" e pedir acesso abre o diálogo de novo no host. O menu explica isso.
- **Atalhos sugeridos** (entram na decisão de atalhos ainda aberta): `Ctrl+Alt+1…9` vai direto ao
  monitor, `Ctrl+Alt+←/→` anterior/próximo.
- **Casos de borda** (`13-multimonitor.md` §7): host com 1 monitor não mostra abas; monitor
  desplugado no host fecha a aba (ou mostra "Monitor desconectado no host" na janela destacada) e
  volta para o principal; reconexão restaura os mesmos monitores e o mesmo modo.
- **Mosaico** (todos numa janela) fica fora do MVP.
- Esta regra é sobre **monitores dentro de uma sessão**; cada sessão (host) continua em janela própria (§11).

### 5.7 Painel de arquivos (dentro da sessão)
- Painel lateral recolhível, dois níveis: **Enviar/receber** (arrastar para a janela; lista de
  transferências com progresso, pausa, cancelar) e **Navegar no host** (raízes permitidas,
  lista nome/tamanho/data, baixar, enviar para cá).
- Conflito de nome: renomear automático (`arquivo (1).txt`, `10-plano` §3) e dizer isso na linha
  da transferência; nada de diálogo modal por arquivo.
- Ao concluir: "Salvo em Downloads/TailDesk · [Abrir pasta]".
- Raízes não permitidas aparecem como "Sem permissão", não somem silenciosamente.

### 5.8 Transferências (janela principal)
Histórico de envios e recebimentos de todas as sessões, com hash verificado (✓), retomar e
abrir pasta. Serve também como registro de auditoria legível (`10-plano` §3).

### 5.9 Configurações
- **Este computador**: aceitar conexões (sim/não), pedir permissão sempre ou lembrar
  dispositivos, dispositivos confiáveis (lista com revogar), raízes de arquivos permitidas,
  pasta de entrada.
- **Sessão**: qualidade (Automática / Nitidez / Fluidez), teclado padrão (Auto), tela cheia ao conectar,
  modo de monitores (Um por vez / Janelas separadas), "Seguir o mouse" e "Usar todos os meus
  monitores" (distribui as janelas de monitores em tela cheia nos monitores deste PC).
- **Rede**: modo embutido vs Tailscale do sistema (quando existir), tailnet atual, sair.
- **Sobre / diagnóstico**: versão, copiar relatório de diagnóstico (inspirado na janela de
  debug do Tailscale).

## 6. Fluxos principais

| # | Fluxo | Caminho feliz | Pontos de falha e como a UI responde |
|---|---|---|---|
| F1 | Primeiro uso | Instalar → login na tailnet → "aceitar conexões?" → lista | Login cancelado: voltar ao passo com "Tentar de novo". Wayland: avisar antes do diálogo do portal. |
| F2 | Conectar a um PC próprio | Lista → Conectar → host aceita (ou já confiável) → sessão | Host offline; host sem TailDesk ("Como instalar"); recusado; sem resposta; ACL da tailnet bloqueia (`udp:7420`): mensagem dizendo que a política da tailnet não permite e link para o admin. |
| F3 | Receber pedido (host) | Diálogo §5.4 → Permitir → faixa §5.5 | Ninguém na frente: expira em 60 s e o controlador vê "sem resposta". |
| F4 | Digitar com layouts diferentes | Entrar na sessão → toast "Teclado: US-Intl → ABNT2 (automático)" uma vez → digita | Caractere impossível: toast §5.6. Layout trocado no meio: indicador da barra atualiza. |
| F5 | Enviar arquivo | Arrastar para a janela → linha de progresso → "Salvo em…" no host também | Host sem permissão de arquivos; disco cheio; queda → "Retomando…". |
| F6 | Baixar do host | 📁 → Navegar → pasta → Baixar → Downloads/TailDesk | Raiz não permitida; arquivo em uso. |
| F7 | Rede piora | Direto → relay: HUD aparece "Via relay" | Queda: tela congela com overlay "Reconectando…" e volta sozinha; após N tentativas, "Conexão perdida · [Reconectar]". |

## 7. Textos-chave (UX copy, rascunho)

| Situação | Texto |
|---|---|
| Esperando aceite | "Esperando **desktop-casa** permitir o acesso…" |
| Recusado | "**desktop-casa** recusou o acesso." |
| Sem resposta | "Ninguém respondeu em **desktop-casa**. Tente de novo quando houver alguém lá." |
| ACL bloqueia | "A política da sua tailnet não permite conectar a **servidor**. Fale com quem administra a tailnet." |
| Sem TailDesk | "**servidor** está na sua tailnet, mas não tem o TailDesk aberto." |
| Relay | "Conexão via relay do Tailscale. A imagem pode ficar mais lenta." |
| Reconectando | "Conexão perdida. Reconectando…" |
| Caractere impossível | "Não foi possível digitar ‘ß’ em **notebook-yuri**." |
| Arquivo salvo | "**relatorio.pdf** salvo em Downloads/TailDesk." |

Português como idioma principal, inglês desde o início pela estrutura de i18n do Slint (`@tr`).

## 8. Acessibilidade, tema e plataforma

- Contraste AA nos dois temas; tamanho de clique ≥ 32 px nos controles da barra; tudo operável
  por teclado na janela principal (na sessão o teclado vai para o remoto, então a barra precisa
  de atalho próprio).
- Leitor de tela: o Slint expõe acessibilidade via AccessKit; validar NVDA (Windows) e Orca
  (Linux) na janela principal e no diálogo de consentimento, que é o mais crítico.
  (Inferência: o suporte do Slint a leitores de tela é recente; testar cedo.)
- Escala de DPI diferente entre controlador e host (ex.: 150% vs 100%): "tamanho real" vs
  "ajustar à janela" na barra.
- Bandeja/área de notificação: Windows sim; Linux GNOME não tem bandeja por padrão, então o
  indicador do host (§5.5) não pode depender só do ícone da bandeja.

## 9. Implicações para o Slint e para o plano

- Componentes a criar: lista virtualizada de dispositivos com busca, painel de detalhe, diálogo
  modal de consentimento, barra de sessão sobreposta ao `wgpu`, toast, HUD, painel de arquivos
  com drag-and-drop de arquivos do SO.
- Pontos a validar tecnicamente cedo (sugestão para a fase 2, não alteram o plano):
  sobrepor widgets Slint à textura `wgpu` da sessão; receber drag-and-drop de arquivos do
  Explorer/Nautilus no Slint/winit; janela "sempre no topo" para o consentimento e a faixa do
  host nos dois SOs (em Wayland o compositor pode ignorar isso).
- Design tokens (`06-arquitetura.md` já cita `taildesk-ui` com tokens): cores semânticas para
  online/relay/offline/erro, espaçamento em múltiplos de 4, tipografia do sistema.

## 10. Plano de pesquisa com usuários (para validar este levantamento)

**Objetivos**
1. Confirmar que a lista de dispositivos basta para achar e conectar (sem ID/senha).
2. Ver se o diálogo de consentimento é entendido (quem, o quê) e se recusar é fácil.
3. Medir se a tradução de teclado passa despercebida (sucesso = ninguém percebe).
4. Testar descoberta de arquivos (arrastar e painel) e da barra recolhida em tela cheia.

**Métodos**
| Momento | Método | Participantes |
|---|---|---|
| Agora (antes do código de UI) | Teste de protótipo clicável (Figma ou Slint estático) das telas 5.2, 5.4, 5.6 | 5 pessoas: 2 "dono das máquinas", 2 "ajuda a família", 1 dev |
| Fim da fase 2 | Teste de usabilidade com o app real, Windows US-Intl ↔ Linux ABNT2 | Os mesmos 5, mais quem for receber ajuda (do lado host) |
| Fase 3 | Tarefa de arquivos (enviar pasta grande, baixar do host, retomar) | 5 |

**Roteiro do teste (45 min)**
1. Aquecimento (5 min): como acessa outro computador hoje; já usou AnyDesk/RustDesk/TeamViewer?
2. Contexto (10 min): última vez que precisou de acesso remoto; o que deu errado.
3. Tarefas (20 min): (a) conecte ao "desktop-casa"; (b) do outro lado, decida se aceita um
   pedido de "pc-desconhecido"; (c) digite "Ação, café e pão" num editor remoto; (d) traga um
   arquivo do host; (e) saia da tela cheia e desconecte.
4. Reação (5 min): o que deu confiança ou medo; o que faltou.
5. Encerramento (5 min).

**Métricas**: sucesso por tarefa, tempo até conectar, erros de digitação na tarefa (c),
quantos acharam o botão de encerrar no host sem ajuda.

## 11. Decisões do Yuri (15/10/2026)

1. **Sessão em janela própria**: uma janela por host, sem abas de sessões no MVP. Isso não impede as
   **abas de monitores** dentro da sessão (§5.6.1), e cada monitor pode virar janela própria.
2. **Protótipo em Slint estático** (não Figma).
3. **Primeiro teste com usuário: o próprio Yuri**; os 5 participantes da §10 ficam para depois.
4. Ainda aberto: atalhos para revelar a barra e para "encerrar tudo" no host (sugestões na §5).

## Fontes
- Tailscale, app windowed para macOS: <https://tailscale.com/blog/windowed-macos-ui-beta>
- RustDesk, issue de layout de teclado: <https://github.com/rustdesk/rustdesk/issues/7530>
- Fórum Technibble (relatos sobre RustDesk): <https://www.technibble.com/forums/threads/best-remote-software.91836/page-4>
- RustDesk 1.3.0 e modos de teclado: <https://www.elevenforum.com/t/connecting-to-remote-windows-11-machine-with-rustdesk-1-3-0.27884>
- AnyDesk, teclado e atalhos: <https://support.anydesk.com/docs/keyboard-hotkeys>
- TeamViewer e golpes: <https://www.teamviewer.com/en-ca/global/support/knowledge-base/teamviewer-remote/security/teamviewer-and-scamming/>
- Golpe de suporte técnico: <https://www.pcrisk.com/removal-guides/15592-teamviewer-tech-support-scam>, <https://isthisascam.to/scam-types/tech_support>
- Parsec (Windows/Linux, overlay): <https://support.parsec.app/hc/en-us/articles/32381199341716-Parsec-App-for-Windows>, <https://support.parsec.app/hc/en-us/articles/32381494397332-Parsec-App-for-Linux>
- Chrome Remote Desktop (barra lateral, ajuste): <https://www.airdroid.com/remote-support/how-do-i-use-chrome-remote-desktop>
- Barra de sessão remota (padrão RDP): <https://docs.fixme.it/help/features/remote-desktop-toolbar-overview>
- Barra que some em tela cheia: <https://www.anyviewer.com/how-to/remote-desktop-taskbar-not-showing-in-full-screen-1796.html>
