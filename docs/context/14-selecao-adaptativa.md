# 14 — Seleção adaptativa de perfil de rede e aba "Teste de conexão"

> Pedido do Yuri em 15/10/2026: uma **suíte inteligente que escolha o melhor protocolo conforme a rede**
> (mesmo roteador e muita banda → qualidade alta; longe → mais compressão) e uma **aba de teste e
> demonstração da conexão**. Escopo do MVP (`10-plano-windows-linux.md`): Windows x64 e Linux x64,
> overlay Tailscale, QUIC (`quinn`), openh264 primeiro e encoder de hardware depois.
> Este arquivo **não altera** `06`, `07`, `10`, `12` nem `13`; a §10 lista o que precisaria mudar neles.

## 0. Resumo

1. "Protocolo" aqui são **três alavancas** que o TailDesk controla: o **caminho** (direto na LAN,
   direto pela internet ou relay DERP), o **perfil de mídia** (codec, resolução, FPS, bitrate, chroma)
   e as **regras de transporte** (FEC, NACK, jitter buffer, quanto da banda os arquivos podem usar).
   O QUIC continua o mesmo em todos os casos; o que muda é como ele é usado.
2. A decisão usa **cinco classes de rede**: **LAN**, **Rápida**, **Média**, **Lenta** e **Mínima**.
   Cada classe tem um perfil pronto (§3). Dentro do perfil, o laço de bitrate de `07` §5 ajusta
   a cada 100 ms; a **troca de perfil** é mais lenta, com histerese, para não ficar oscilando.
3. A classe vem de **medição própria**, não só do que o Tailscale diz: RTT do handshake, uma
   **rajada de sondagem** de ~300 ms antes do primeiro quadro, as estatísticas do `quinn` e o
   `ReceiverReport`. Os sinais do Tailscale (caminho direto/DERP, endereços do peer) servem de pista.
4. **Achado importante**: no `tailscale` **0.6.1** embutido (versão mais nova da crate, que é
   versionada à parte do cliente Tailscale 1.x) a conexão direta **existe, mas ainda é trabalho
   em andamento**: o README do `main` diz "direct connections in many cases" e avisa que pode
   cair para DERP "em mais casos" que o cliente oficial, e a crate **não expõe por qual caminho**
   os pacotes vão (§1.1). Então o G0 precisa medir com que frequência o embutido fica direto.
   Se ficar muito no DERP, o perfil LAN depende do **Tailscale do sistema** (`tailscaled`, cliente
   1.x, com conexão direta madura) ou de um **atalho LAN** que precisaria de ADR (§6).
5. A **aba "Teste de conexão"** é a interface gráfica do mesmo motor do `taildesk-cli bench-net`:
   mostra caminho, RTT, jitter, perda, banda nos dois sentidos, codecs das duas máquinas, a
   classe escolhida **e o porquê**, e tem uma **demonstração** com um padrão de teste animado
   codificado ao vivo, onde dá para forçar cada perfil e **simular rede ruim** para ver a adaptação
   acontecer (§7).
6. Multimonitor (`13`): a classe define o **orçamento total** da sessão; o pacer divide entre os
   monitores por prioridade, e a classe também limita **quantos monitores** ficam ativos (§5).
7. Entra em fatias: medição e protocolo nas fases 0–1, aba de teste na fase 2, troca completa de
   perfil com encoder HW e simulador na fase 4 (§9).

## 1. Sinais disponíveis

### 1.1 Do overlay Tailscale

| Sinal | Embutido (`tailscale` =0.6.1) | Sistema (`tailscaled` + LocalAPI) |
|---|---|---|
| Caminho direto ou DERP | **Não exposto**. `NodeInfo` traz `derp_region` e `underlay_addresses` do peer, mas não diz por onde os pacotes passam agora | `GET /localapi/v0/status`: `CurAddr` (vazio = relay) e `Relay` por peer |
| Latência ao peer pelo Tailscale | Não há API; medir no QUIC | `POST /localapi/v0/ping?ip=…&type=disco`: latência, endpoint usado, região DERP e `IsLocalIP` |
| Endereços do peer (underlay) | `NodeInfo::underlay_addresses` (os "Endpoints" do Go) | `status` → `Addrs` |
| Mudança de rede local | `ts_netmon` interno, sem evento público | `watch-ipn-bus` |
| Região DERP e latências | `ts_netcheck` (interno) | `status` / `netcheck` |

Conferido em 15/10/2026. A crate `tailscale` mais nova no crates.io é a **0.6.1** (18/09/2026);
ela é versionada à parte do cliente Tailscale (o cliente do sistema está na série 1.10x: o Yuri
citou 1.102 e a página de pacotes estáveis mostrava 1.104.1). As fontes da crate se contradizem:
- o README publicado na 0.6.1 listava "Direct connections (NAT traversal, STUN, and Disco)" em
  **Coming Soon**, e a documentação de `lib.rs` (igual no `main`) ainda diz "We currently rely on
  DERP relays for all communication";
- o CHANGELOG da 0.6.0 fala em paridade de conexão direta entre Linux, Windows e macOS, e o
  README do `main` (sem release nova) já move para **Implemented** "Communicate with peers via
  direct connections in many cases", mantendo o aviso de que pode cair para DERP "em mais casos".

Conclusão: o embutido **faz conexão direta em parte dos casos**, mas sem API para saber qual
caminho está em uso. Tratar o caminho do modo embutido como **desconhecido até medir**, e o G0
deve registrar com que frequência ele fica direto. O modo sistema usa o `tailscaled` 1.x, que
tem conexão direta madura e informa o caminho pela LocalAPI.

**Pista de "mesmo roteador"** (os dois modos): se algum `underlay_address` do peer é IPv4 privado
na mesma sub-rede de uma interface local, ou o IP público do peer é igual ao nosso, a chance de
LAN é alta. É só pista: decide o perfil inicial, nunca substitui a medição.

### 1.2 Do QUIC (`quinn::Connection::stats()` e `rtt()`)
- `path.rtt` (suavizado), `path.cwnd`, `path.congestion_events`, `path.lost_packets`,
  `path.sent_packets`, `path.current_mtu`; `max_datagram_size()` para fragmentar.
- `min_rtt` não é exposto diretamente: manter o mínimo de uma janela de 10 s no próprio
  adaptador. `queue_delay = rtt − min_rtt` é o principal sinal de congestionamento (`07` §5).

### 1.3 Medições do TailDesk
| Medição | Como | Quando |
|---|---|---|
| RTT e relógio | 5 × `Ping`/`Pong` no stream de controle durante o handshake | Sempre |
| Banda disponível | **Rajada de sondagem**: datagramas `kind=3` (já reservado no header) em degraus de taxa, o receptor devolve taxa recebida, perda e crescimento do atraso de ida (§3.2) | Antes do 1º quadro e ao tentar subir de classe |
| Qualidade contínua | `ReceiverReport` a cada 250 ms (`07` §3) com dois campos novos: `owd_trend_us` (tendência do atraso de ida) e `probe_*` | Durante a sessão |
| Capacidade das máquinas | `probe_codecs()` de `03` §6 (encode/decode HW e SW, ms por quadro) | Startup, com cache |
| Rede local | Tipo de link (Ethernet/Wi-Fi) e **conexão tarifada** (Windows: `ConnectionCost`; Linux: propriedade `Metered` do NetworkManager) via `taildesk-platform` | Ao conectar e ao mudar de rede |

## 2. Classes de rede

| Classe | Critério (medido) | Exemplo típico |
|---|---|---|
| **LAN** | Caminho direto, RTT < 5 ms, banda ≥ 100 Mbit/s, perda < 0,5% | Mesmo roteador, cabo ou Wi-Fi 6 forte |
| **Rápida** | Banda ≥ 25 Mbit/s, RTT < 40 ms | Fibra entre casas na mesma cidade |
| **Média** | Banda ≥ 8 Mbit/s | Internet comum, ou DERP perto e folgado |
| **Lenta** | Banda ≥ 2 Mbit/s | 4G, Wi-Fi ruim, DERP distante |
| **Mínima** | Banda < 2 Mbit/s ou perda > 5% | Rede muito ruim |

Modificadores, independentes da classe:
- **Relay DERP**: teto na classe **Média** enquanto o caminho for DERP, e a sondagem nunca passa
  de 20 Mbit/s nele (os relays são compartilhados e limitados; não abusar da infraestrutura do Tailscale).
- **Perda > 1%**: liga FEC (XOR por grupo) e prefere intra-refresh a keyframe.
- **RTT > 80 ms**: desliga NACK (a retransmissão chegaria tarde) e usa só `PictureLoss` + intra-refresh.
- **Conexão tarifada**: teto na **Média** e arquivos não disputam banda com o vídeo, salvo se o usuário mudar.

Os números são o ponto de partida; o simulador da §8 e as medições do G0 vão calibrá-los.

## 3. Perfis e decisão

### 3.1 Perfil por classe (monitor em foco)

| Classe | Codec (MVP → depois) | Resolução | FPS | Bitrate | Transporte |
|---|---|---|---|---|---|
| **LAN** | H.264 HW, QP baixo → depois HEVC/AV1 4:4:4 e PyroWave (`03`) | Nativa | 60 | 20–80 Mbit/s | Sem FEC, NACK ligado, jitter buffer 0 |
| **Rápida** | H.264 HW (openh264 se não houver) → AV1/HEVC | Nativa até 1440p | 60 | 8–25 Mbit/s | NACK, jitter ~p95 |
| **Média** | H.264 → AV1/HEVC quando houver (mais compressão por bit) | Até 1080p | 30–60 | 3–8 Mbit/s | NACK se RTT < 30 ms |
| **Lenta** | Idem, modo *Text* de `07` §5 | Até 1080p, cai para 720p | 15–30 | 1–3 Mbit/s | FEC conforme perda |
| **Mínima** | Idem | 720p → 540p | 5–15 | 0,3–1 Mbit/s | FEC, sem NACK, só monitor em foco |

- No MVP só existe H.264; então, até a fase 4, a diferença entre perfis está em **bitrate, FPS,
  resolução e regras de transporte**, não em codec. O desenho já deixa o codec como campo do
  perfil para quando HEVC/AV1/PyroWave chegarem.
- A preferência do usuário desloca a escolha: **Automática** (padrão), **Nitidez** (corta FPS antes
  de resolução, modo *Text*), **Fluidez** (corta resolução antes de FPS, modo *Motion*) e
  **Economizar dados** (teto na Lenta). As três primeiras já estão em `12` §5.9; a quarta é nova.
- Antes de subir de perfil, o host confere se o encoder aguenta: `probe_codecs()` diz quantos ms
  o quadro leva; perfil que estoura o orçamento de encode não é escolhido.

### 3.2 Início da sessão (sem atrasar o primeiro quadro)
1. **Pista**: modo sistema diz direto/DERP; os dois modos olham "mesmo roteador" (§1.1).
2. **RTT**: 5 pings no handshake (já previstos para o relógio).
3. O vídeo **começa na classe Média** (ou Rápida se a pista for LAN), sem esperar.
4. **Sondagem em paralelo**: degraus de 100 ms em 5 → 20 → 50 → 100 → 200 Mbit/s
   (parando em 20 no DERP), usando datagramas `kind=3` de enchimento. Para no primeiro degrau
   em que o atraso de ida cresce > 5 ms ou a perda passa de 1%. Custo máximo ≈ 4 MB.
5. Com o resultado (em ~0,5–1 s), o host fixa a classe e manda `ProfileChanged`. Se a classe
   mudar o codec ou a resolução, vai junto um keyframe.

### 3.3 Durante a sessão
- **Laço rápido (100 ms)**, dentro do perfil: o de `07` §5 (BBR × 0,85, corte de 20% se
  `queue_delay` > 15 ms, recuperação +5%/s).
- **Laço lento (1 s)**, troca de classe:
  - **Descer**: bitrate preso no piso do perfil por 2 s, ou perda > 3% por 1 s, ou RTT dobrou.
    Desce uma classe por vez, imediatamente.
  - **Subir**: bitrate no teto do perfil por 5 s com `queue_delay` < 5 ms **e** uma sondagem curta
    (enchimento de FEC até a taxa da classe de cima) passou. Sobe uma classe por vez.
  - **Histerese**: depois de descer, só tenta subir após 15 s; no máximo uma troca de codec ou
    resolução a cada 10 s (cada uma custa um keyframe).
- **Eventos que forçam reavaliação na hora**: mudança de caminho (modo sistema: `CurAddr`
  mudou; embutido: degrau no `min_rtt` > 2× ou endereço do peer mudou), troca de rede local
  (Wi-Fi ↔ cabo), migração de conexão QUIC.
- Toda decisão gera uma **razão legível** ("perda de 4% por 1 s → Lenta"), que vai para o
  log, para o HUD e para a aba de teste.

### 3.4 Onde mora o código
- **`taildesk-media::adapt`**: o adaptador é uma **função pura**,
  `Adaptador::atualizar(&mut self, agora, Sinais) -> Option<Decisao>`, sem I/O nem relógio
  próprio. Assim é testável com traces gravados e roda no host (que manda no encoder) com os
  relatórios do controlador.
- **`taildesk-overlay`**: `path()` passa a devolver `PathInfo { kind, rtt, derp_region,
  same_lan_hint, peer_endpoints }` (hoje é `PathKind { Direct, Relayed, Unknown }`).
- **`taildesk-transport`**: sondagem (`kind=3`) e, atrás da feature `impair`, um embrulho do
  `DatagramSocket` que injeta atraso, perda e teto de banda para a demonstração e os testes (§7, §8).
- **`taildesk-platform`**: tipo de link e conexão tarifada.

## 4. Protocolo (proposta)

```protobuf
// H→C, logo após o HelloAck e a cada mudança de caminho
message LinkInfo {
  PathKind path = 1;            // DIRECT_LAN | DIRECT_WAN | RELAYED | UNKNOWN
  string derp_region = 2;       // "sao", "mia"… vazio se direto/desconhecido
  bool same_lan_hint = 3;
  OverlayMode overlay = 4;      // EMBEDDED | SYSTEM
}

// C→H ou H→C: pede uma rajada de sondagem; o lado que recebe os datagramas responde
message ProbeRequest { uint32 probe_id = 1; repeated uint32 steps_kbps = 2; uint32 step_ms = 3; }
message ProbeResult {
  uint32 probe_id = 1;
  repeated ProbeStep steps = 2; // por degrau: kbps_recebido, perda_ppm, owd_delta_us
}

// H→C: o host mudou de classe/perfil
message ProfileChanged {
  NetClass class = 1;           // LAN | FAST | MEDIUM | SLOW | MINIMAL
  uint32 display_id = 2;        // perfil vale por stream (multimonitor)
  string reason_code = 3;       // "loss_high", "probe_ok", "path_relayed"… (texto vem do i18n)
}

// Aba de teste: teste sem vídeo da tela real
message BenchRequest { BenchKind kind = 1; uint32 duration_ms = 2; uint32 max_kbps = 3; } // RTT | UP | DOWN | DEMO
message BenchReport  { /* rtt min/med/p95, jitter, perda, kbps por sentido, mtu, datagrama máx. */ }
```
- `QualityPrefs.mode` ganha `SAVE_DATA`; `ReceiverReport` ganha `owd_trend_us`.
- Datagrama `kind=3` (probe): payload de enchimento com `probe_id` e número de sequência; o
  receptor só conta, nunca decodifica.
- Limites do parser: `steps_kbps` ≤ 16 entradas, `step_ms` ≤ 500, `duration_ms` ≤ 30 000,
  `max_kbps` ≤ 1 000 000; o host recusa `BenchRequest` mais de uma vez a cada 10 s por peer.
- Testes de round-trip para todas as mensagens novas (regra do `AGENTS.md`).

## 5. Multimonitor (`13-multimonitor.md`)
- A classe vale para a **conexão**; o pacer divide a banda como em `13` §5 (70% para o
  monitor em foco, 30% para os visíveis). O monitor em foco recebe o perfil da classe; os
  visíveis recebem o perfil **duas classes abaixo** (5–15 fps).
- **Monitores ativos por classe**: LAN e Rápida = até o limite do encoder (2 com openh264, 4 com
  HW); Média = 2; Lenta e Mínima = **só o monitor em foco**. Os demais congelam no último quadro
  com o aviso "Rede lenta: só o monitor em foco é atualizado" / "Slow network: only the focused
  monitor is updated".
- `ProfileChanged` leva `display_id`, porque cada stream pode estar num perfil diferente.

## 6. O caminho LAN (decisão para o Yuri)

O pedido "mesmo roteador → protocolo melhor" depende de os pacotes **realmente** irem direto.
Opções:

| Opção | Como | Prós | Contras |
|---|---|---|---|
| **A. Confiar no tailscale-rs** | Usar o embutido como está; quando ele ficar no DERP, a classe máxima é Média | Zero código novo; respeita a decisão 1 do `AGENTS.md` | LAN só quando o embutido conseguir caminho direto, o que ainda é incompleto e invisível para o app |
| **B. Tailscale do sistema** | Quem tem `tailscaled` instalado usa o modo sistema (já planejado logo após o embutido); ele faz conexão direta madura | Já está no plano; dá `ping`/`CurAddr` | Exige instalar o Tailscale; o embutido continua limitado |
| **C. Atalho LAN** | Com a sessão já autenticada pela tailnet, os dois lados trocam endereços LAN e um token de uso único pelo stream de controle e abrem um **segundo caminho QUIC na LAN** (mesmo certificado *pinned*), migrando a mídia para ele; se cair, volta para a tailnet | LAN de verdade mesmo no modo embutido; abre caminho para PyroWave | Sai da trait `Overlay` (decisão 1): **precisa de ADR**; o tráfego LAN fica protegido só pelo TLS do QUIC, não pelo WireGuard; mais superfície de ataque na rede local |

**Recomendação** (aprovada pelo Yuri em 15/10/2026): **B agora**, medir o embutido no G0, e
escrever o ADR da **C** só se o G0 mostrar que o embutido fica muito no DERP. A identidade continuaria vindo de `Overlay::identify()` na
conexão da tailnet (decisão 3); o atalho só carregaria mídia e arquivos de uma sessão já autorizada.
O `01-tailscale.md` já previa um `DirectLanOverlay` "só para testes"; a C seria a versão de produção dele.

## 7. Aba "Teste de conexão"

### 7.1 Onde fica
- **Tela inicial** (`12` §5.2): no painel de detalhe do dispositivo, botão **"Testar conexão"**
  ao lado de "Conectar". Abre a aba de teste já com aquele peer.
- **Na sessão**: "⋯ → Diagnóstico da conexão" abre a mesma aba numa janela à parte, com os
  números da sessão ao vivo (sem sondagem ativa, para não atrapalhar).
- **Configurações → Diagnóstico** (`12` §5.9): atalho para a aba e "Copiar relatório".
- O teste **não mostra a tela do outro lado e não controla nada**, então não pede o diálogo de
  consentimento. O host precisa estar com o TailDesk aberto, aceita por padrão pedidos de peers que
  a ACL já deixa chegar, limita a 1 teste a cada 10 s por peer, registra no log de auditoria e pode
  desligar isso em Configurações ("Permitir testes de conexão" / "Allow connection tests").

### 7.2 O que mostra

```
┌ Teste de conexão · desktop-casa ──────────────────────────────── [Testar de novo] ┐
│ Caminho      ● Direto · mesma rede local       Tailscale: embutido                 │
│ Latência     2 ms  (mín 1 · p95 4)   Variação 0,4 ms   Perda 0,0 %                 │
│ Banda        ↑ 420 Mbit/s   ↓ 390 Mbit/s       Datagrama máx. 1 182 B              │
│ Máquinas     Esta: H.264 HW (Intel) · desktop-casa: H.264 HW (NVIDIA)              │
│ ──────────────────────────────────────────────────────────────────────────────── │
│ Perfil escolhido: LAN — H.264 por hardware, 1440p 60 fps, até 50 Mbit/s            │
│ Por quê: caminho direto, 2 ms, banda medida 390 Mbit/s, encoder HW nos dois lados. │
│ ──────────────────────────────────────────────────────────────────────────────── │
│ [Demonstração ▶]  Perfil: (•) Automático ( ) LAN ( ) Rápida ( ) Média ( ) Lenta ( ) Mínima │
│ Simular rede: [Desligado ▾]  (4G · Wi-Fi ruim · Outro país · Personalizado)        │
│ [Copiar relatório]                                                                 │
└────────────────────────────────────────────────────────────────────────────────────┘
```

1. **Caminho**: "Direto · mesma rede local", "Direto pela internet", "Via relay (São Paulo)" ou
   "Desconhecido"; modo do Tailscale (embutido/sistema). No embutido sem certeza, diz
   "provavelmente via relay" e explica que o modo do sistema pode melhorar.
2. **Medições**: latência mín/mediana/p95, variação (jitter), perda, banda nos dois sentidos,
   datagrama máximo, tempo de conexão. Cada número com uma faixa de cor (bom/ok/ruim).
3. **Máquinas**: codecs de encode/decode das duas pontas, HW ou SW, e ms por quadro 1080p.
   Se o Fedora estiver sem H.264 por hardware, a dica do `mesa-va-drivers-freeworld` aparece aqui
   (`10` §2.6).
4. **Perfil escolhido e o porquê**: a classe, o perfil e as razões do adaptador, em frases.
5. **Demonstração**: o host codifica um **padrão de teste sintético** (texto rolando em tamanhos
   pequenos, gradiente, um objeto em movimento e um relógio em código de barras) com o perfil
   escolhido, e a aba mostra o vídeo ao vivo com um gráfico de bitrate, FPS, RTT e trocas de
   perfil. O relógio em código de barras permite medir **latência de ponta a ponta** (o
   controlador lê o quadro decodificado e compara com o relógio alinhado no handshake).
   Dá para **forçar cada perfil** e ver lado a lado nitidez de texto e fluidez.
6. **Simular rede**: liga o embrulho `impair` do transporte (§3.4) **só na conexão de teste**,
   com predefinições (4G: 60 ms, 1% de perda, 10 Mbit/s; Wi-Fi ruim: jitter 30 ms, 3% de perda;
   Outro país: 180 ms; Personalizado) para mostrar o adaptador descendo e subindo de classe.
   Nunca se aplica a sessões reais.
7. **Copiar relatório**: texto + JSON (sem chaves, sem auth keys, sem IPs públicos completos)
   para colar em issue.

Mesmo motor no terminal: `taildesk-cli bench-net <peer> [--json] [--impair 4g]` (já previsto
na fase 0 do plano), o que deixa a aba fina: ela só desenha o resultado.

### 7.3 HUD da sessão
O HUD de `12` §5.6 ganha a classe atual ("12 ms · direto · LAN") e, ao clicar, expande o gráfico
da §7.2 item 5. Quando o adaptador desce de classe por causa da rede, o HUD aparece sozinho por
alguns segundos, como já previsto para o relay.

### 7.4 Textos (pt-BR / en)
"Testar conexão" / "Test connection"; "Diagnóstico da conexão" / "Connection diagnostics";
"Direto · mesma rede local" / "Direct · same local network"; "Direto pela internet" / "Direct over
the internet"; "Via relay (%s)" / "Via relay (%s)"; "Perfil escolhido" / "Chosen profile";
"Por quê" / "Why"; "Demonstração" / "Demo"; "Simular rede" / "Simulate network"; "Copiar relatório"
/ "Copy report"; classes "LAN · Rápida · Média · Lenta · Mínima" / "LAN · Fast · Medium · Slow ·
Minimal"; "Economizar dados" / "Save data".

## 8. Como testar a própria suíte
- **Testes unitários do adaptador** com traces sintéticos e gravados: cada trace é uma sequência
  de `Sinais` com tempo, e o teste confere a sequência de decisões (ex.: "perda de 5% por 2 s
  desce para Lenta em ≤ 1,2 s"; "rede estável não troca de classe em 10 min"). Sem I/O, roda
  no `cargo xtask check` nos dois SOs.
- **Gravação de traces**: `bench-net --record` e sessões reais com a opção de diagnóstico ligada
  salvam os sinais (sem conteúdo de tela) para virar caso de teste.
- **Integração no Linux do CI**: dois processos em *network namespaces* ligados por `veth` com
  `tc netem` (atraso, perda, banda) rodando o `bench-net`; marcar como teste lento. No Windows,
  o embrulho `impair` cobre o mesmo papel.
- **Matriz manual** (junto com a de `10` §2.6): mesmo roteador por cabo, mesmo roteador por Wi-Fi,
  casas diferentes na mesma cidade, 4G roteado, e um peer forçado a DERP.

## 9. Fases sugeridas

| Fase do plano | Entrega |
|---|---|
| **0 — Fundações** | Motor do `bench-net` (RTT, banda, perda, caminho) e gravação de traces. **G0 registra o caminho real do embutido** (direto ou DERP) |
| **1 — Ver** | Mensagens do §4 no protocolo; sondagem inicial; duas classes efetivas (rápida/lenta) só com bitrate e FPS do openh264; `ReceiverReport` com `owd_trend_us` |
| **2 — Controlar** | Aba "Teste de conexão" com medições, caminho, máquinas e "por quê"; HUD com a classe; preferência Automática/Nitidez/Fluidez/Economizar dados |
| **3 — Arquivos** | Fatia de banda dos arquivos por classe; conexão tarifada |
| **4 — Qualidade** | As cinco classes com encoder HW e escada de resolução; orçamento multimonitor por classe; demonstração com padrão de teste; simulador `impair`; sinais do modo sistema; ADR do atalho LAN se o G0 pedir |
| **Pós-MVP** | Perfil LAN com HEVC/AV1 4:4:4 e PyroWave |

## 10. O que precisaria mudar nos outros documentos

Não editei estes arquivos; ficam para as threads donas.

**`10-plano-windows-linux.md`** (thread "Arquitetura inicial do TailDesk")
- §2.1 e §7: novo risco — no `tailscale` 0.6.1 a conexão direta ainda é trabalho em andamento
  e a crate não diz qual caminho está em uso; o G0 deve medir e registrar com que frequência o
  embutido fica direto, e a meta "30 Mbit/s estáveis direto" pode depender do modo sistema.
- §4: `taildesk-media::adapt`, feature `impair` em `taildesk-transport`, tipo de link e conexão
  tarifada em `taildesk-platform`; `PathInfo` no lugar de `PathKind` em `taildesk-overlay`.
- §5: incluir as entregas da §9 deste arquivo em cada fase.

**`07-protocolo.md`** (mesma thread; também `docs/context/07` no repositório)
- §3: `LinkInfo`, `ProbeRequest`/`ProbeResult`, `ProfileChanged`, `BenchRequest`/`BenchReport`;
  `QualityPrefs.mode` com `SAVE_DATA`; `ReceiverReport.owd_trend_us`.
- §4: definir o payload do datagrama `kind=3` (probe).
- §5: apontar para este arquivo para classes, histerese e sondagem.

**`06-arquitetura.md`** §3: `Overlay::path()` devolvendo `PathInfo`; módulo `adapt` puro em `media`.

**`08-roadmap.md`**: reservar o ADR do atalho LAN (§6, opção C) caso o Yuri aprove.

**`design/12-levantamento-ux.md`** (thread "Levantamento de design da interface")
- §5.2: botão "Testar conexão" no detalhe do dispositivo.
- §5.6: HUD com a classe atual e gráfico expansível; "⋯ → Diagnóstico da conexão".
- §5.9: "Economizar dados" nas opções de qualidade; "Permitir testes de conexão"; Diagnóstico
  abre a aba.
- Nova tela "Teste de conexão" (§7.2 daqui) no protótipo Slint.

**`13-multimonitor.md`** §5: monitores ativos e perfil dos visíveis por classe (§5 daqui).

## 11. Fontes
- `tailscale` 0.6.1 (crates.io, código publicado): README "Coming Soon: Direct connections",
  `src/lib.rs` "We currently rely on DERP relays for all communication", `CHANGELOG.md` 0.6.0;
  README do `main` (15/10/2026): "direct connections in many cases" em Implemented
  <https://github.com/tailscale/tailscale-rs/blob/main/README.md>; pacotes estáveis do cliente:
  <https://pkgs.tailscale.com/stable/>;
  `ts_control` 0.6.1 `Node { underlay_addresses, derp_region }`. <https://crates.io/crates/tailscale/0.6.1>
- LocalAPI do Tailscale (`status`, `ping`, `watch-ipn-bus`): <https://github.com/tailscale/tailscale/tree/main/ipn/localapi>
- `quinn` `ConnectionStats`/`PathStats`: <https://docs.rs/quinn/0.11.12/quinn/struct.ConnectionStats.html>
- `03-codecs.md` §2, §6, §7 e `07-protocolo.md` §5 (laço de bitrate e ordem de degradação).
