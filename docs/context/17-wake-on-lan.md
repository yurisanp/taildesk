# 17 — Wake on LAN no TailDesk

Pesquisa de 17/10/2026. Implementação em andamento (issue #95): protocolo e repetidor em `07-protocolo.md` §3.5. Vale com o plano `10-plano-windows-linux.md` e o ADR-009
(Tailscale instalado, sem nó embutido).

## Resumo

- O pacote mágico é um quadro de camada 2 (broadcast), então **não atravessa a tailnet**: alguém acordado
  **na mesma LAN** do alvo precisa enviar. Isso vale para todos os produtos do mercado.
- **Achado principal:** o próprio `tailscaled` já sabe fazer isso. Desde out/2023 todo cliente Tailscale
  (Windows e Linux inclusos, recurso `wakeonlan` compilado por padrão) expõe na PeerAPI o endpoint
  `POST /v0/wol?mac=…`, que manda o pacote mágico em broadcast por todas as interfaces IPv4 do nó.
  Ou seja: **qualquer nó ligado na LAN do alvo serve de repetidor, mesmo sem o TailDesk instalado**
  (um roteador com Tailscale, um NAS, um Raspberry Pi, outro PC).
- Proposta: o TailDesk guarda o MAC de cada host quando ele está online, e o botão "Ligar" no card de um
  dispositivo offline escolhe um repetidor na mesma LAN e pede o envio. Primeiro pelo TailDesk do
  repetidor (mensagem nossa); se não houver, pela PeerAPI do `tailscaled`.
- Diferencial possível: nenhum concorrente acorda máquinas usando um nó qualquer da VPN como repetidor sem
  software deles instalado. O RustDesk nem tem repetidor.

## 1. Como o WoL funciona e por que a tailnet não leva

- Pacote mágico: 6 bytes `FF` + 16 repetições do MAC (102 bytes), normalmente em UDP para
  `255.255.255.255` porta 9 (ou 7), ou como EtherType `0x0842`.
- A placa de rede, com a máquina em S3 (suspensão) ou S5 (desligada), só olha quadros Ethernet. Não há pilha
  IP, não há `tailscaled` rodando, não há tailnet. Por isso o pacote precisa nascer **no mesmo domínio de
  broadcast** do alvo.
- O Tailscale é camada 3. Nem subnet router repassa broadcast (docs e fóruns do Tailscale confirmam). Dá para
  mirar o broadcast dirigido de uma sub-rede (`192.168.1.255`) via subnet router, mas a maioria dos roteadores
  domésticos descarta isso e exige ARP/regras; não é caminho confiável para o produto.

### Requisitos do lado do alvo (fora do nosso controle)

| Item | Windows | Linux |
|---|---|---|
| Firmware | "Wake on LAN"/"Power on by PCI-E" ligado na BIOS/UEFI; ErP/Deep Sleep desligado | igual |
| Placa | Propriedades do adaptador → Gerenciamento de energia → "Permitir que este dispositivo ative o computador" + "Somente pacote mágico"; avançado "Wake on Magic Packet" | `ethtool -s <if> wol g` (persistir via NetworkManager `802-3-ethernet.wake-on-lan magic` ou systemd-networkd `WakeOnLan=magic`) |
| Desligar de S5 | "Inicialização rápida" do Windows costuma desligar a placa; recomendar desativar | geralmente funciona se o firmware manter a placa energizada |
| Wi-Fi | WoWLAN quase nunca funciona em S5; só alguns notebooks em S3 | idem; `iw phy … wowlan enable magic-packet` raro |
| Notebook na bateria | muitos ignoram WoL fora da tomada | idem |

Resumo prático: **cabo + tomada + firmware configurado**. Notebooks em Wi-Fi (como o CachyOS do Yuri) em
geral não acordam; o PC fixo Windows com cabo é o caso bom.

## 2. O que o Tailscale já oferece (verificado no código, v1.105, `feature/wakeonlan/wakeonlan.go`)

1. **`POST http://<ip-tailnet-do-repetidor>:<porta-peerapi>/v0/wol?mac=aa:bb:cc:dd:ee:ff`**
   - Atendido pelo `tailscaled` do repetidor. Envia via `github.com/kortschak/wol` para `255.255.255.255`
     a partir de cada interface IPv4 não-loopback. Resposta JSON `{ "SentTo": [ifaces], "Errors": [...] }`.
   - Autorização (`canWakeOnLAN`): liberado se chamador e repetidor são **o mesmo usuário e nenhum dos dois tem
     tag**; senão exige a capability `https://tailscale.com/cap/wake-on-lan` concedida por grant na política
     da tailnet. Peers só com "unsigned PeerAPI" são recusados.
   - A identidade vem do IP de origem na tailnet, então um processo comum (o app TailDesk) pode chamar; não
     precisa ser o `tailscaled`. A porta vem de `PeerStatus.PeerAPIURL` no `/localapi/v0/status`.
   - Compilado por padrão (`//go:build !ts_omit_wakeonlan`); builds mínimos podem omitir.
2. **`Hostinfo.WoLMACs`**: o nó pode publicar os próprios MACs para o servidor de controle, mas só se o
   `tailscaled` rodar com `TS_WAKE_MAC=auto` ou `TS_WAKE_MAC=<mac>`. Padrão: vazio. Não dá para contar com isso;
   o TailDesk deve guardar o MAC por conta própria (e usar `WoLMACs` como fonte extra quando existir).
3. **c2n `POST /wol`**: o servidor de controle pode mandar um nó enviar o pacote. Não achamos interface pública
   que exponha isso; ignorar.

Limitações: sem senha SecureOn, só IPv4, só broadcast global; sem confirmação de que o alvo acordou (isso a
gente detecta vendo o peer ficar `Online` no status).

## 3. O que os outros fazem

| Produto | Tem WoL? | Quem envia o pacote | Observações |
|---|---|---|---|
| **TeamViewer** | Sim | (a) IP público/DDNS com port forward UDP do roteador, ou (b) **outro PC da mesma rede com TeamViewer**, ligado e na mesma conta | Precisa associar o alvo à conta; exige cabo e tomada; configurar por SO. |
| **AnyDesk** | Sim | **Outro cliente AnyDesk online na mesma LAN**; modo automático (sorteia clientes da LAN) ou lista fixa de IDs | Acorda de S5, desligamento híbrido e suspensão; opção nas configurações do alvo. |
| **RustDesk** | Sim (desde 1.2) | **Só o próprio controlador**, se estiver na mesma LAN (`src/lan.rs::send_wol`) | MAC aprendido na descoberta de LAN (broadcast UDP); sem repetidor, não funciona de fora. |
| **Parsec** | Não | — | Usuários usam Homebridge/Raspberry Pi para acordar e depois conectam pelo Parsec. |
| **GoTo / Remote Utilities / DeskIn** | Sim | Outro computador acordado na mesma LAN com o software deles | Mesmo padrão "irmão na LAN". |
| Comunidade Tailscale | — | Raspberry Pi/NAS na LAN com UpSnap ou script, acessado pela tailnet | Blog oficial do Tailscale descreve UpSnap. |

O padrão do mercado é "um irmão acordado na LAN com o nosso software". O Tailscale nos dá um passo além:
o irmão pode ser **qualquer nó Tailscale** da LAN.

## 4. Proposta para o TailDesk

### 4.1 Aprender o MAC e a LAN do host (enquanto ele está online)

- Ao ligar, o host TailDesk descobre a interface da rota padrão (não a `tailscale0`/Wintun) e registra:
  MAC, IPv4 da LAN, máscara, MAC do gateway (via tabela ARP/vizinhos) e se a placa é cabeada.
- Envia isso ao controlador numa mensagem de controle nova (faixa **130–139 "energia"**, porque a 120–129 ficou
  com o celular: `PowerInfo { ifaces: [LanIface { mac, ipv4, prefix_len, gateway_mac, wired, wol }] }`), com teste de round-trip como manda o
  AGENTS.md. O controlador grava no catálogo local de dispositivos (chave = node key / nome estável da tailnet).
- `wol_ativo`: Linux lê `ethtool`/`/sys/class/net/<if>/device/power/wakeup` e o NetworkManager; Windows lê
  `MSPower_DeviceWakeEnable` (WMI) / `powercfg /devicequery wake_armed`. Serve para avisar "este PC não vai
  acordar" antes de o usuário tentar. Código de SO em `taildesk-platform`.
- Opcional: aba de diagnóstico com botão "Configurar para acordar pela rede" (Linux: perfil do
  NetworkManager via D-Bus com consentimento; Windows: precisa de admin → só no serviço da fase 5).

### 4.2 Escolher um repetidor

Candidatos = peers `Online` no `/localapi/v0/status` que estejam na mesma LAN do alvo. Como saber:

1. Repetidor com TailDesk: o controlador manda a LAN do alvo no `WakeRequest` e o repetidor responde
   `NOT_SAME_LAN` sem mandar nada se não tiver placa nela (o controlador tenta o próximo).
   Mesma sub-rede **e** mesmo MAC de gateway = mesma LAN (evita confundir duas casas com `192.168.0.0/24`).
2. Repetidor só com Tailscale: o status traz os endpoints/`CurAddr` de conexão direta. Peer cujo endpoint
   direto é um IP privado na `lan_cidr` do alvo é candidato provável. É heurística; o usuário pode fixar
   "acordar pelo dispositivo X" nas configurações do host (como a lista fixa do AnyDesk).
3. O próprio controlador, se ele estiver na LAN do alvo (caso RustDesk), envia direto do socket UDP local.

### 4.3 Enviar

Ordem: (a) controlador na mesma LAN envia direto; (b) repetidor TailDesk recebe `WakeRequest { mac, lan }` pelo
canal de controle QUIC e envia; (c) PeerAPI `/v0/wol` do `tailscaled` do repetidor. Em todos, mandar UDP para
`255.255.255.255:9` e para o broadcast dirigido da sub-rede, por cada interface; 3 tentativas espaçadas.

Depois, aguardar o peer aparecer `Online` no status (Windows costuma levar 20–60 s até o `tailscaled` subir),
mostrando "Ligando…" no card, e conectar sozinho se o usuário pediu "Ligar e conectar".

### 4.4 Segurança

- Repetidor TailDesk só aceita `WakeRequest` do mesmo usuário da tailnet ou de um celular pareado (o mesmo
  critério da PeerAPI do Tailscale; identidade de `Overlay::identify()`, decisão 3). Limite de taxa (ex.: 1 por alvo a cada 10 s).
- O MAC não é segredo, mas não vai para logs públicos nem telemetria.
- A PeerAPI já é restrita pelo próprio Tailscale (mesmo usuário ou grant). Em tailnet de empresa com tags, o
  admin precisa conceder `tailscale.com/cap/wake-on-lan`; a UI deve explicar o 403 nesses termos.
- Pacote mágico em broadcast é inofensivo na LAN, mas não faremos broadcast dirigido via subnet router
  (pode ser abusado como amplificação e quase nunca funciona).

### 4.5 Coisas relacionadas que saem quase de graça

- **Desligar / suspender / reiniciar remoto** (mensagens na mesma faixa 120–129), com consentimento da sessão.
  Combina com WoL: "suspender quando eu sair" e "acordar quando eu voltar".
- **Ligar agendado** (ex.: acordar o PC às 8h) rodando no repetidor TailDesk.
- **Evitar suspensão durante a sessão** (`SetThreadExecutionState` no Windows, inibidor `org.freedesktop.login1`
  / portal Inhibit no Linux), útil hoje e independente de WoL.

## 5. Encaixe nas fases e esforço estimado

| Parte | Onde | Fase sugerida | Tamanho |
|---|---|---|---|
| Mensagens `PowerInfo`/`WakeRequest`/`WakeResult` + round-trip | `taildesk-proto` | 4 ou 5 | P |
| Coleta de MAC/LAN/gateway e estado WoL | `taildesk-platform` | 5 | M |
| Envio do pacote mágico (UDP broadcast, sem dependência nova ou crate `wol-rs`/`wake-on-lan` MIT) | `taildesk-session` | 5 | P |
| Chamada PeerAPI `/v0/wol` + leitura de `PeerAPIURL` | `taildesk-tsapi` | 5 | P |
| UI: botão "Ligar" no card offline, estado "Ligando…", aviso "não vai acordar" (pt-BR + en) | `taildesk-ui` | 5 | M |
| Desligar/suspender remoto e inibir suspensão | platform + ui | 5 | M |

Nada disso mexe nas decisões travadas: continua só o Tailscale instalado, identidade pela tailnet, nenhum
relay nosso.

## 6. Teste com os aparelhos do Yuri

- PC Windows 11 (`yurisp`) com cabo é o alvo ideal; o notebook CachyOS faz o papel de controlador/repetidor
  quando estiver na mesma LAN.
- Teste 1: Windows em suspensão (S3), notebook na mesma rede envia direto.
- Teste 2: controlador fora de casa (ex.: celular no 4G com Tailscale) pede ao notebook (repetidor) via
  PeerAPI `/v0/wol`, sem TailDesk no meio, para validar o caminho do `tailscaled`.
- Teste 3: Windows desligado (S5) com "Inicialização rápida" ligada e desligada.
- Teste manual rápido possível hoje, sem código, de qualquer outro nó do mesmo usuário (o notebook em casa
  serve de repetidor):
  `curl -X POST "$(tailscale status --json | jq -r '.Peer[] | select(.HostName=="yuri-notebook") | .PeerAPIURL[0]')/v0/wol?mac=<MAC-do-PC>"`

## Fontes

- Código do Tailscale: `feature/wakeonlan/wakeonlan.go`, `tailcfg/peercap/peercap.go`
  (`https://tailscale.com/cap/wake-on-lan`), `tailcfg.Hostinfo.WoLMACs`, `ipn/ipnstate.PeerStatus.PeerAPIURL`
  — https://github.com/tailscale/tailscale (versão 1.105.0, commit de 16/10/2026).
- Tailscale blog, Wake-on-LAN com UpSnap: https://tailscale.com/blog/wake-on-lan-tailscale-upsnap
- Tailscale/TrueNAS e WoL: https://forums.truenas.com/t/tailscale-and-wol/63887
- RustDesk `src/lan.rs` (`send_wol`): https://github.com/rustdesk/rustdesk/blob/master/src/lan.rs
- AnyDesk Wake-on-LAN: https://support.anydesk.com/Wake-on-LAN
- TeamViewer Wake-on-LAN: https://www.teamviewer.com/en/info/wake-on-lan/ e manual
  https://download.teamviewer.com/docs/en/TeamViewer-Manual-Wake-on-LAN-en.pdf
- Parsec + WoL por fora (Homebridge): https://matthewc.dev/musings/ode-to-parsec/
- GoTo Assist WoL: https://support.goto.com/gotoassist-remote-support/help/wake-sleeping-computers-wake-on-lan
- Devolutions sobre WoL e VPN: https://docs.devolutions.net/rdm/knowledge-base/knowledge-base-articles/wake-on-lan-wol
