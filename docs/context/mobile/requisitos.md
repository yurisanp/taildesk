# TailDesk Mobile — Levantamento de requisitos (Android e iOS)

> Pedido do Yuri em 15/10/2026: "levantamento de requisitos para aplicativo Android/iOS, já crie
> modelos de telas e deixe pronto para ser executado posteriormente, não atrapalhe a publicação atual".
> Este pacote é **só documentação e mockups**: nada foi mudado no repositório. Ele complementa
> `10-plano-windows-linux.md` (que deixa mobile para depois) e reaproveita `11`–`15`.
> Arquivos irmãos (em `docs/context/mobile/` no repositório): `arquitetura.md` (como construir), `plano.md` (fases prontas para executar),
> `telas/` (modelos de telas em HTML + capturas PNG).
> O que é inferência ou precisa ser medido está marcado **(validar)**.

## 1. Resumo em uma página

1. **O celular é primeiro controlador.** Os dois apps (Android e iOS) acessam os computadores
   Windows/Linux da tailnet: ver, controlar por toque, teclado com tradução, trocar arquivos,
   ouvir o som. Isso é o MVP mobile.
2. **O Android também pode ser host** (fase posterior): tela via MediaProjection e controle via
   AccessibilityService. **O iOS nunca pode ser controlado** (não existe API de injeção de input);
   no máximo transmite a tela, só para ver, e isso fica fora do escopo inicial.
3. **Rede: exige o app oficial do Tailscale instalado e conectado**, como no desktop (decisão do
   Yuri). No celular isso tem três consequências que mudam o desenho:
   - O tráfego do TailDesk passa normalmente pela VPN do Tailscale (sockets UDP comuns para
     endereços 100.x funcionam), então **QUIC/quinn funciona sem nada especial**.
   - **Não há LocalAPI** acessível a outro app no Android nem no iOS: o app não consegue listar a
     tailnet, fazer `whois` nem saber se o caminho é direto ou relay pelos meios do desktop.
     Solução: lista de dispositivos vinda de um **desktop TailDesk pareado** (que tem LocalAPI),
     nomes conferidos pelo **MagicDNS**, e o **host informa o caminho** que ele mede (§4).
   - **Só uma VPN ativa por vez** nos dois sistemas: quem usa outra VPN no celular não consegue
     usar o TailDesk ao mesmo tempo. O app detecta e explica.
4. **Mesmo núcleo Rust do desktop** (`taildesk-proto`, `-transport`, `-session`, `-keyboard`,
   `-files`, adaptador de rede) com UI Slint e vídeo wgpu. O Slint no Android é estável; **no iOS
   ainda é tech preview** desde a 1.12, por isso a primeira fase é um spike com gate (plano B:
   casca nativa SwiftUI/Compose com o núcleo Rust via `uniffi`).
5. **Decode por hardware do sistema** (MediaCodec / VideoToolbox) em vez de openh264: economiza
   bateria e, no iOS, evita a questão de patentes do binário openh264 (que não pode ser baixado
   em runtime numa app da App Store). openh264 fica só como fallback no Android.
6. **Ordem sugerida**: M0 spikes com gates → M1 controlador Android → M2 controlador iOS →
   M3 arquivos/áudio/multimonitor/tablet → M4 host Android → (M5 opcional) iOS "só ver".
   Começa só depois que o protocolo do desktop estabilizar (fases 1–3 do `10-plano`).

## 2. Quem vai usar e para quê (personas de `testes/personas.md`)

| Persona | Situação mobile | Trabalho a fazer | Impacto no requisito |
|---|---|---|---|
| **Rafael** (dev/homelab, Fedora KDE com 3 monitores, Arch Hyprland) | No 4G, fora de casa, precisa olhar um job ou reiniciar um serviço na GUI do servidor | "Quero entrar rápido, achar o monitor certo e digitar um comando sem brigar com o teclado" | Barra de teclas especiais (Ctrl, Esc, Tab, setas, F1–F12), seletor de monitor, economia de dados, reconexão rápida |
| **Ana Paula** (analista híbrida, Win US-Intl → Ubuntu ABNT2; ajuda a mãe no Win10 ABNT2) | Mãe liga pedindo ajuda quando Ana está sem notebook | "Quero ver a tela da minha mãe pelo celular, apontar e clicar onde ela não acha" | Consentimento claro no host, modo toque direto, zoom com pinça, ponteiro visível no host |
| **Yuri** (primeiro tester) | Tablet ou celular no sofá controlando o desktop de casa | "Usar o PC de casa como se estivesse na frente, inclusive com teclado Bluetooth" | Teclado físico no tablet/iPad com tradução de layout, áudio, modo paisagem |
| **Celular como host** (Android) | Ajudar alguém cujo celular Android está com problema; ou acessar o próprio Android do PC | "Quero ver e tocar na tela do meu Android pelo computador" | MediaProjection + Accessibility, consentimento por sessão, aviso permanente |

## 3. Escopo por plataforma

| Funcionalidade | Android controlador | iOS/iPadOS controlador | Android host | iOS host |
|---|---|---|---|---|
| Ver tela remota (H.264) | ✅ M1 | ✅ M2 | — | — |
| Controle por toque (touchpad e toque direto) | ✅ M1 | ✅ M2 | — | — |
| Teclado virtual com tradução (`11-teclado.md`) | ✅ M1 | ✅ M2 | — | — |
| Teclado físico (Bluetooth/USB) com tradução | ✅ M3 | ✅ M3 (iPadOS dá HID usage) | — | — |
| Mouse físico (Bluetooth/USB) | ✅ M3 | ✅ M3 (iPadOS 13.4+ pointer) | — | — |
| Multimonitor (um por vez, troca por gesto/lista) | ✅ M1 básico, M3 completo | ✅ M2 básico, M3 completo | — | — |
| Troca de arquivos + navegador remoto | ✅ M3 | ✅ M3 | ✅ M4 | — |
| "Compartilhar com TailDesk" (enviar sem abrir sessão) | ✅ M3 | ✅ M3 (Share Extension) | — | — |
| Áudio do host (Opus) | ✅ M3 | ✅ M3 | ✅ M4 (Android 10+ AudioPlaybackCapture) | — |
| Perfis adaptativos + Teste de conexão (`14`) | ✅ M1 (mínimo), M3 aba completa | ✅ M2/M3 | ✅ M4 | — |
| Ser controlado (ver + controlar) | — | — | ✅ M4 | ❌ impossível |
| Ser visto (só tela) | — | — | ✅ M4 | ⚠️ M5 opcional (ReplayKit) |
| Clipboard texto | ✅ M3 | ✅ M3 | ✅ M4 (só com app em foco) | — |
| Sessão em segundo plano | ⚠️ áudio continua com Foreground Service; vídeo pausa | ⚠️ app suspenso: sessão pausa e reconecta ao voltar | ✅ Foreground Service obrigatório | — |

Fora do escopo mobile inicial: microfone do celular → host, câmera, gamepad, várias sessões ao
mesmo tempo, janelas destacadas por monitor (só iPad em Stage Manager, pós-MVP), modo mosaico,
acesso não-assistido ao Android.

## 4. O Tailscale no celular: o que a decisão "Tailscale instalado" significa

### 4.1 Como o tráfego passa
- **Android**: o app oficial (`com.tailscale.ipn`) é um `VpnService`. Sockets de qualquer app
  para 100.64.0.0/10 (e fd7a:115c:a1e0::/48) entram no túnel. O quinn usa um `UdpSocket` comum.
- **iOS**: o app oficial é um *Packet Tunnel Provider* (NetworkExtension). Rotas da tailnet são
  instaladas no sistema; sockets de qualquer app seguem essas rotas.
- **MagicDNS**: o Tailscale instala o DNS 100.100.100.100 enquanto a VPN está ativa, então
  `desktop-casa` ou `desktop-casa.tail4f2a.ts.net` resolvem normalmente.
- **Entrada (host Android)**: conexões vindas da tailnet para o IP 100.x do celular chegam a
  sockets que escutam em `0.0.0.0` (é como o `sshd` do Termux funciona pelo Tailscale)
  **(validar com UDP/QUIC no spike M0)**.

### 4.2 O que não existe no celular
| Recurso do desktop | Desktop (tailscaled + LocalAPI) | Android | iOS | Substituto no mobile |
|---|---|---|---|---|
| Lista de peers (`/status`) | ✅ | ❌ LocalAPI é interna ao app do Tailscale | ❌ | Lista vinda do **desktop pareado** + favoritos locais + nome digitado + (opcional) token da API do Tailscale |
| `whois` do peer | ✅ | ❌ | ❌ | Nome via **MagicDNS reverso (PTR em 100.100.100.100)** **(validar)**; quem decide a autorização é sempre o **host desktop**, que tem `whois` |
| Direto ou relay (`ping` disco) | ✅ | ❌ | ❌ | O **host mede e informa** o caminho (mensagem `PathInfo`, `arquitetura.md` §4); sem host desktop, "Desconhecido" |
| Versão do Tailscale ≥ 1.102 | ✅ | ⚠️ `PackageManager` lê `versionName` | ❌ não dá para ler | Android: avisa se < 1.102; iOS: não verifica, mostra dica no Diagnóstico |
| Ligar/desligar a VPN | CLI | ❌ sem intent pública documentada | ❌ | Botão "Abrir o Tailscale" (Android: `getLaunchIntentForPackage`; iOS: **(validar)** URL scheme do app; senão, texto explicando) |

### 4.3 Detecção de estado (tela de onboarding e erros)
O app classifica o estado ao abrir e a cada mudança de rede:

| Estado | Como detectar (Android) | Como detectar (iOS) | Mensagem |
|---|---|---|---|
| Tailscale não instalado | `PackageManager.getPackageInfo("com.tailscale.ipn")` (declarar `<queries>`) | Não dá para saber com certeza; tratado como "Tailscale desconectado ou ausente" | "Instale o Tailscale para usar o TailDesk" + botão para a loja |
| Instalado, VPN desligada | Nenhuma rede com `TRANSPORT_VPN` e nenhuma interface com IP 100.x | `getifaddrs`: nenhuma `utun*` com IP 100.x | "Ligue o Tailscale" + botão "Abrir o Tailscale" |
| Outra VPN ativa | `TRANSPORT_VPN` presente, mas sem interface 100.x | `utun` sem 100.x e sem DNS 100.100.100.100 | "Outra VPN está ativa. O Android/iOS só permite uma VPN por vez." |
| TailDesk excluído do túnel (Android, "split tunneling por app" do Tailscale) | Interface 100.x existe, mas UDP para 100.100.100.100:53 do app falha | — | "O Tailscale está deixando o TailDesk de fora. Em Tailscale → Configurações → Split tunneling, inclua o TailDesk." **(validar nome do menu)** |
| Conectado | Interface 100.x + DNS 100.100.100.100 responde | Idem | Lista de dispositivos |

### 4.4 Identidade e autorização (regra 3 do `AGENTS.md`)
- **No host (desktop)** nada muda: `Overlay::identify()` faz `whois` do IP 100.x do celular e vê
  usuário, máquina e SO (Android/iOS). O diálogo de consentimento mostra "celular-yuri (Android)".
- **No controlador (celular)** o "overlay" é o `MobileVpnOverlay` (`arquitetura.md` §3): a
  identidade do host é o **IP 100.x** da conexão QUIC (autenticado pelo WireGuard: só aquele nó
  manda pacotes daquele IP) + o **nome resolvido pelo MagicDNS do próprio cliente Tailscale**.
  O nome que o host diz de si mesmo no `Hello` é só exibição, nunca decisão.
- Fixação de chave: na primeira conexão o certificado QUIC do host é guardado por IP+nome (TOFU);
  troca de certificado mostra aviso. O pareamento por QR (§5.3) já traz a impressão digital.

### 4.5 Limitações para documentar na loja e na ajuda
- Uma VPN por vez (Android e iOS).
- Celular fora da tailnet não alcança nada: não existe "ID + senha".
- iOS suspende apps em segundo plano: trocar de app pausa a sessão (reconexão automática ao voltar).
- Android com otimização de bateria agressiva (Xiaomi, Samsung) pode matar o Tailscale ou o
  TailDesk em segundo plano: tela de ajuda pedindo exceção de bateria para os dois.

## 5. Requisitos funcionais

IDs `MF-*` para usar nos testes e no plano.

### 5.1 Primeira execução e conta
- **MF-01** Onboarding em 3 passos: (1) checagem do Tailscale (§4.3); (2) "Encontrar seus
  computadores" (parear com desktop ou digitar nome); (3) permissões opcionais (notificações,
  e no Android exceção de bateria). Nenhuma conta própria do TailDesk.
- **MF-02** Sem login próprio: a identidade é a da tailnet em que o Tailscale do celular está.
- **MF-03** Idioma pt-BR e en seguindo o sistema.

### 5.2 Dispositivos (tela inicial)
- **MF-10** Lista pesquisável de dispositivos com estado: online com TailDesk, online sem
  TailDesk, offline, desconhecido. Favoritos no topo, recentes em seguida (igual a `12` §5.2).
- **MF-11** Fonte da lista, em ordem: (a) **desktop pareado** responde `PeerList` com os peers da
  tailnet e quais têm TailDesk; (b) favoritos e recentes salvos no celular; (c) entrada manual
  por nome MagicDNS ou IP 100.x; (d) opcional avançado: token da API do Tailscale guardado no
  Keystore/Keychain.
- **MF-12** Estado online de cada favorito por um **probe leve** na porta do TailDesk
  (`udp:7420`), só para dispositivos conhecidos, no máximo 1 por dispositivo a cada 30 s com a
  tela aberta. Nunca varrer a faixa 100.x.
- **MF-13** Tocar no dispositivo abre um cartão de detalhe: Conectar, Testar conexão, Enviar
  arquivo, Copiar endereço, Favoritar, Esquecer.
- **MF-14** Puxar para atualizar.

### 5.3 Pareamento com um desktop
- **MF-20** O desktop ganha (fase própria, fora deste pacote) "Configurações → Celulares →
  Conectar um celular", que mostra um **QR** com: nome MagicDNS, IP 100.x, porta, impressão
  digital do certificado e um código de uso único válido por 5 minutos.
- **MF-21** O celular lê o QR (câmera; permissão pedida só aqui) ou recebe o código digitado
  (6 caracteres). Depois disso o desktop vira "fonte da lista" e favorito.
- **MF-22** O desktop só responde `PeerList` a celulares do **mesmo usuário da tailnet**
  (conferido por `whois`) ou explicitamente autorizados. A lista não inclui IPs públicos nem
  chaves.

### 5.4 Conectar e consentimento
- **MF-30** Etapas com texto ("Encontrando desktop-casa…" → "Esperando desktop-casa
  aceitar…" → sessão), botão Cancelar, timeout de 60 s explicado (recusou vs não respondeu).
- **MF-31** O consentimento acontece **no host**, igual ao desktop (`12` §5.4). O celular não
  recebe pedidos no M1–M3.
- **MF-32** No host Android (M4), o pedido aparece como notificação de alta prioridade + tela
  cheia, foco em **Recusar**, e depois o diálogo do sistema do MediaProjection.

### 5.5 Sessão (controlador)
- **MF-40** Tela cheia em paisagem por padrão (retrato permitido); a tela remota se ajusta à
  largura e pode ser ampliada com pinça e arrastada com dois dedos.
- **MF-41** Dois modos de toque, trocáveis na barra:
  - **Touchpad** (padrão no celular): cursor relativo; toque = clique; dois dedos tocando =
    botão direito; dois dedos arrastando = rolagem; tocar-segurar-arrastar = arrastar; três dedos
    = botão do meio. Cursor sempre visível e ampliado.
  - **Toque direto** (padrão no tablet): o ponto tocado é o clique; tocar-segurar = botão direito.
- **MF-42** Barra da sessão recolhível (alça visível no topo), com: monitor, teclado, arquivos,
  som, modo de toque, qualidade/HUD e Encerrar. Sempre há uma forma visível de reabrir.
- **MF-43** Teclado virtual do sistema + **faixa de teclas especiais** acima dele: Esc, Tab, Ctrl,
  Alt, Super/Win, Shift, setas, Home/End, PgUp/PgDn, Del, F1–F12 (rolável), Ctrl+Alt+Del. Ctrl/Alt
  /Super são "travas" de um toque (acendem; dois toques fixam).
- **MF-44** Texto do teclado virtual vai como `KeyChar` (modo caractere do `11-teclado.md`),
  incluindo acentos, emojis fora do layout com aviso no HUD, e IME (CJK) com texto composto.
  Atalhos montados na faixa (Ctrl + letra) vão como `KeyShortcut`.
- **MF-45** Teclado físico: detecta layout do celular quando o SO informa (Android: layout do
  `InputDevice`, API 34+ **(validar)**; iPadOS: `UIKey` com HID usage + idioma do teclado) e usa o
  modo Auto do `11-teclado.md`. Sem layout conhecido, usa modo caractere para teclas imprimíveis.
- **MF-46** Indicador de teclado "Toque → ABNT2 · Auto" ou "US-Intl (Bluetooth) → ABNT2 · Auto".
- **MF-47** Multimonitor: lista de monitores na barra; deslizar com três dedos (ou botões ◀ ▶)
  troca de monitor; só **um monitor recebido por vez** no celular (economia de decode e banda);
  tablet pode ter 2 em tela dividida (M3, conforme probe).
- **MF-48** HUD opcional: latência, caminho (direto/relay, informado pelo host), classe de rede,
  FPS. Aparece sozinho quando a classe cai.
- **MF-49** Rotação, ir para segundo plano e voltar: pausa vídeo, mantém controle, reconecta em
  até 3 s ao voltar (meta) sem perder a posição/zoom.
- **MF-50** Ao sair da sessão ou perder foco, envia `ReleaseAll` (sem teclas presas no host).

### 5.6 Arquivos
- **MF-60** Painel de arquivos na sessão (folha que sobe de baixo no celular; painel lateral no
  tablet): navegar raízes permitidas do host, baixar, enviar.
- **MF-61** Enviar: seletor do sistema (Android SAF `ACTION_OPEN_DOCUMENT`, iOS
  `UIDocumentPickerViewController`/Fotos). Receber: pasta "TailDesk" do app (Android:
  `Download/TailDesk` via MediaStore; iOS: pasta do app visível no app Arquivos).
- **MF-62** Mesmas regras do desktop: `.part` + rename, BLAKE3, retomada, nunca sobrescrever,
  caminhos do peer sempre via `resolve_in_root`.
- **MF-63** "Compartilhar com TailDesk" no menu de compartilhar do sistema: escolhe um computador
  e envia sem abrir sessão (o host aplica a regra de arquivos, podendo pedir aceite).
- **MF-64** Em dados móveis, arquivos > 50 MB pedem confirmação (configurável).

### 5.7 Áudio
- **MF-70** Botão de som na barra; segue a regra do `15-audio.md`: desligado = nenhum `kind=1`.
- **MF-71** Respeita o modo silencioso do iOS (categoria `playback` só quando o usuário ligar o
  som) e o foco de áudio do Android; para quando entra ligação.
- **MF-72** Em segundo plano no Android o áudio pode continuar (Foreground Service
  `mediaPlayback`), com notificação "Ouvindo desktop-casa · Encerrar" **(validar política)**.

### 5.8 Rede e Teste de conexão
- **MF-80** Mesmo adaptador e mesmas 5 classes do `14-selecao-adaptativa.md`. Rede tarifada
  (Android `isActiveNetworkMetered`, iOS `NWPath.isExpensive`/`isConstrained`) = teto na Média,
  como já previsto em `14` §2.
- **MF-81** Preferência "Economizar dados" ligada por padrão em dados móveis; mostra consumo da
  sessão (MB) no HUD e no fim da sessão.
- **MF-82** O controlador pede ao host a resolução máxima da tela do celular (em pixels físicos),
  para o host não codificar 4K para uma tela de 2400×1080.
- **MF-83** Aba "Teste de conexão" (no detalhe do dispositivo): latência, perda, banda, caminho
  informado pelo host, decode HW do celular, perfil escolhido e porquê. Demonstração e
  simulação de rede como no desktop (M3).

### 5.9 Host Android (M4)
- **MF-90** "Permitir acesso a este celular" desligado por padrão; ao ligar, explica as duas
  permissões: **Gravação de tela** (pedida a cada sessão no Android 14+) e **Acessibilidade**
  (uma vez, nas Configurações do sistema).
- **MF-91** Sessão ativa = notificação permanente "desktop-casa está vendo/controlando este
  celular · Encerrar" e um ponto colorido na tela; encerrar com um toque.
- **MF-92** Sem Acessibilidade ligada: modo "só ver" (ainda útil para ajudar alguém).
- **MF-93** Controle remoto: toque/arrasto do mouse do desktop vira gesto
  (`dispatchGesture`), botões Voltar/Início/Recentes na barra do controlador, texto via
  `ACTION_SET_TEXT`/colar no campo focado **(validar teclados)**.
- **MF-94** Áudio do celular (Android 10+ `AudioPlaybackCapture`, só apps que permitem captura).
- **MF-95** O desktop controlador mostra a tela do celular em retrato, com moldura e botões de
  navegação; teclado do desktop digita no campo focado.

### 5.10 Configurações
- **MF-100** Tema claro/escuro do sistema; idioma; modo de toque padrão; faixa de teclas
  (ordem e quais); qualidade (Automática, Nitidez, Fluidez, Economizar dados); pasta de
  recebimento; computadores pareados (remover); confirmar arquivos grandes em dados móveis;
  diagnóstico (copiar relatório sem chaves); sobre (licenças, atribuição do Slint).

## 6. Requisitos não funcionais

| ID | Requisito | Meta |
|---|---|---|
| MNF-01 | Latência de toque até o quadro na tela, LAN/Wi-Fi bom | ≤ 80 ms p50 (o desktop mira menos; celular tem pipeline de toque e display mais lento) |
| MNF-02 | Primeiro quadro após aceitar | ≤ 1,5 s em Wi-Fi |
| MNF-03 | Bateria | Sessão de 30 min a 1080p30 gasta ≤ 10% num celular médio de 2024 **(validar)**; decode HW obrigatório quando houver |
| MNF-04 | Dados | Classe Lenta ≤ 3 Mbit/s; "Economizar dados" ≤ 1 Mbit/s com texto legível |
| MNF-05 | Aparelhos mínimos | Android 10+ (API 29) arm64; iOS/iPadOS 17+. Android 14+ para o host com consentimento por sessão (já é a regra do sistema) |
| MNF-06 | Tamanho | APK ≤ 40 MB por ABI; IPA ≤ 50 MB **(validar com Slint + wgpu)** |
| MNF-07 | Acessibilidade | Alvos de toque ≥ 44 pt / 48 dp; TalkBack/VoiceOver em toda a UI fora da área de vídeo; contraste AA |
| MNF-08 | Segurança | Segredos (token API opcional, chaves de fixação) só em Keystore/Keychain (`keyring` onde der, senão `security-framework`/JNI); nunca logar chaves |
| MNF-09 | Licença | MIT OR Apache-2.0; nada GPL/AGPL; Slint pela licença royalty-free (desktop e mobile) com atribuição no "Sobre" |
| MNF-10 | Privacidade | Nenhuma telemetria; câmera só no pareamento por QR |
| MNF-11 | Hot path | Mesmas regras do `AGENTS.md`: sem alocação/lock/await em decode; canais bounded latest-wins; datagramas ≤ `max_datagram_size()` (o túnel do Tailscale no celular tem MTU 1280: medir) |

## 7. Lojas e distribuição

| | Android | iOS |
|---|---|---|
| Canal | Google Play (AAB) + APK no GitHub Releases | App Store + TestFlight |
| Conta | Play Console (taxa única) | Apple Developer Program (anual) — decisão do Yuri |
| Pontos de política | **AccessibilityService**: o TailDesk não é ferramenta para pessoas com deficiência, então precisa de divulgação proeminente + consentimento no app, e declaração no Play Console ([política do Google Play](https://support.google.com/googleplay/android-developer/answer/10964491)). Foreground Service tipos `mediaProjection`/`mediaPlayback`/`connectedDevice` declarados. Risco de rejeição do host: mitigação = **build Play só controlador** e host no APK do GitHub até a revisão aprovar. | Remote desktop é aceito na App Store; explicar no review que exige Tailscale e uma conta de teste (fornecer tailnet de demonstração com um host ligado). Câmera (QR) e rede local com textos de uso. |
| F-Droid | Possível depois; precisa build reproduzível e avaliar a licença do Slint (royalty-free não é licença livre para o F-Droid) **(decisão futura)** | — |

## 8. Decisões que ficam com o Yuri (antes de executar)

**Decidido pelo Yuri em 15/10/2026:** (1) **pareamento por QR** com desktop como fonte da lista;
(2) **Android antes do iOS** (M1 Android, M2 iOS); (4) **conta Apple Developer fica para depois**:
a M2 só começa quando ela for decidida (até lá, o spike S2 de iOS roda no simulador e num aparelho
com conta gratuita do Xcode). Itens 3 e 5 continuam abertos.

1. **Pareamento por QR com desktop como fonte da lista** (recomendado) vs exigir token da API do
   Tailscale (lista completa, mas o usuário cria e renova um token).
2. **Ordem Android → iOS** (recomendado: Android primeiro, Slint estável lá) vs os dois juntos.
3. **Host Android no Play** só depois de aprovado (recomendado) vs APK fora da loja desde o início.
4. **Conta Apple Developer** (custo anual) e quem é o titular das lojas.
5. **Gate do Slint no iOS** (M0): se falhar, aceitar casca nativa SwiftUI com núcleo Rust
   (recomendado como plano B, mantém o núcleo único) ou tentar Makepad (plano B original do `05`).

## Fontes
- Pacote original: `00` (mobile usa o app oficial do Tailscale; iOS só controlador), `01` §2.1,
  `04` §3 (matriz Android/iOS), `05` (Slint/uniffi/android-activity), `08` fase 4.
- Slint 1.12, iOS tech preview: <https://slint.dev/blog/slint-1.12-released>; guia iOS:
  <https://docs.slint.dev/latest/docs/slint/guide/platforms/mobile/ios/>.
- Política de Accessibility do Google Play: <https://support.google.com/googleplay/android-developer/answer/10964491>.
- Automação do Tailscale no Android sem intents públicas (fórum):
  <https://forum.tailscale.com/t/automating-android-vpn-connect-disconnect/4029>;
  pedido de recurso: <https://github.com/tailscale/tailscale/issues/3302>.
