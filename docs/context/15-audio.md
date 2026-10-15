# 15 — Áudio no TailDesk

> Pesquisa pedida pelo Yuri em 15/10/2026: protocolos e codecs de áudio e como encaixá-los no TailDesk
> (Windows x64 + Linux x64), com a regra de que **desligar o áudio corta de fato os pacotes de áudio da rede**.
> Este arquivo **não altera** `06`, `07`, `10`, `12` nem `14`; a §10 lista o que precisaria mudar neles.
> **Decisão do Yuri (15/10/2026): áudio entra na fase 4**, com o protocolo reservado já na fase 1 (§9). Microfone fica pós-MVP.
> Versões e licenças conferidas no crates.io hoje (15/10/2026). Números de banda da §5 são estimativas de cálculo,
> não medições; medir no `bench-net` da fase 0.

## 0. Resumo

1. **Codec: Opus, e só Opus.** É o único codec aberto, sem royalties, de baixa latência que cobre de voz a música
   (6–510 kbit/s, quadros de 2,5 a 60 ms). Os concorrentes que usam codec próprio de áudio usam Opus (RustDesk, Sunshine/Moonlight, WebRTC/CRD).
2. **Transporte: o que já temos.** Um quadro Opus por datagrama QUIC, `kind=1` no header de 16 bytes, comandos no stream de controle.
   RTP/WebRTC não acrescentam nada que o QUIC + header não tenham, e trariam um segundo controle de congestionamento.
3. **Captura do "som do computador"**: `cpal` 0.18.2 cobre os dois SOs: **WASAPI loopback** no Windows e, no Linux,
   **PipeWire** (captura do sink) com **PulseAudio** como alternativa. Não depende do portal Wayland.
4. **Desligar = parar a fonte, não silenciar.** Com o áudio desligado o host **nem abre o dispositivo de captura**, não cria encoder
   e não envia nenhum datagrama `kind=1`; o controlador também não manda ACKs desses pacotes porque eles não existem.
   Isso vale para os dois lados poderem desligar: a **permissão do host** e o **botão do controlador**. Volume local é outra coisa (§4).
5. **Silêncio também não gasta rede**: quando nada toca no host, o envio para (Windows já entrega "nada" no loopback); o controlador sabe
   que é silêncio e não perda.
6. **Custo**: ~120 kbit/s no fio para áudio estéreo bom (64 kbit/s de Opus em quadros de 20 ms); o overhead por pacote (QUIC + WireGuard + IP)
   é quase do tamanho do áudio, então **quadros de 20 ms** são o padrão e 10 ms só em LAN.
7. **Com a seleção adaptativa (`14`)**: áudio tem prioridade sobre vídeo e sai do orçamento antes dele; perfis de 16 a 128 kbit/s conforme a rede.
8. **Fase**: o `10` deixa áudio fora do MVP. Recomendo **reservar o protocolo já na fase 1** (permissão, `features`, mensagens) e
   **implementar na fase 4** junto do pacer/QoS. Microfone do controlador → host fica pós-MVP.

## 1. Codecs comparados

| Codec | Licença / patentes | Latência de algoritmo | Banda típica | Perda de pacote | Veredito |
|---|---|---|---|---|---|
| **Opus** (RFC 6716; libopus 1.5+) | BSD-3, royalty-free | 5–26,5 ms (modo `RESTRICTED_LOWDELAY`: 2,5 ms + quadro) | 16–128 kbit/s para o nosso uso | PLC; FEC in-band (só SILK/híbrido); DRED e PLC neural no 1.5 | **Escolhido** |
| AAC-LD / AAC-ELD | Pool de patentes (Via LA) | ~15–20 ms | 48–128 kbit/s | Sem FEC próprio | Não: licenciamento |
| G.711 / G.722 | Livres | < 1 ms | 64 kbit/s | Fraco | Só voz, banda estreita; não serve para música/vídeo |
| LC3 (Bluetooth LE) | liblc3 Apache-2.0, mas patentes via Bluetooth SIG | 5–10 ms | 32–128 kbit/s | PLC | Não: patentes fora do ecossistema BT |
| FLAC / PCM | Livres | 0 (PCM) / ~20–90 ms (FLAC) | 700–1 500 kbit/s | Nenhum | Talvez um modo "sem perdas em LAN" pós-MVP; não vale agora |
| Codecs neurais (Lyra, EnCodec…) | Variado | Alta | 3–12 kbit/s | — | Só voz; fora |

Por que Opus resolve tudo aqui: um encoder só para voz de chamada, música e áudio de vídeo; troca de bitrate a quente
(o adaptativo pode mexer sem renegociar); tamanho de quadro escolhível; e todo SO-alvo tem `libopus` empacotado.

**Detalhe importante que o `07` hoje não reflete**: o FEC in-band do Opus (LBRR) só existe nas camadas SILK/híbrida, que o encoder usa
para voz e bitrates baixos. Para música em 64+ kbit/s (CELT puro) ele **não protege nada**. Por isso a proteção de perda do TailDesk
é redundância própria (§6.3), com o FEC in-band ligado só nos perfis de banda baixa.

## 2. Protocolos de transporte considerados

| Opção | O que é | Por que não (ou sim) |
|---|---|---|
| **Datagrama QUIC + header TailDesk** | Já decidido para vídeo (`07` §4, AGENTS decisão 2) | **Sim.** Mesmo pacer, mesma criptografia, mesma conexão, mesmo `stream_id` |
| RTP/RTCP (+ SRTP) | Padrão de VoIP | Duplicaria o header (seq, timestamp, SSRC já estão no nosso) e o RTCP duplicaria o `ReceiverReport`. A tailnet já cifra |
| RTP sobre QUIC (draft IETF avtcore) | RTP dentro de datagramas QUIC | Mesmo desenho que o nosso, com header maior; acompanhar, não adotar |
| WebRTC (`webrtc-rs`) | Pilha completa ICE/DTLS/SRTP/GCC | ICE/NAT é papel do Tailscale; controle de congestionamento próprio brigaria com o do quinn; dependência enorme |
| Stream QUIC confiável | Bytes ordenados | Retransmissão = atraso acumulado; áudio prefere perder um quadro a atrasar todos |

Como os outros fazem (documentação e código públicos; não testado aqui): RustDesk manda Opus como mensagem protobuf no mesmo canal da
sessão; Sunshine/Moonlight manda Opus sobre RTP/UDP com FEC Reed-Solomon e quadros curtos; Chrome Remote Desktop usa WebRTC com Opus.

## 3. Captura e reprodução em Rust

### 3.1 Crates

| Para quê | Crate | Versão | Licença | Nota |
|---|---|---|---|---|
| Captura e playback (os dois SOs) | `cpal` | 0.18.2 | Apache-2.0 | Features `pipewire` e `pulseaudio` no Linux; WASAPI no Windows |
| Opus | `opus` | 0.4.0 | MIT/Apache-2.0 | Usa `opusic-sys` 0.7.x (BSD-3, compila a libopus via **cmake** com `bundled`) |
| Alternativa Opus | `opusic-c` | 1.6.1 | BSD-3 | Mesmo `opusic-sys` por baixo, expõe DRED/OSCE como features |
| Opus em Rust puro (acompanhar) | `opus-rs` 0.1.37 (BSD-3), `mousiki` 0.2.1 (MIT) | — | — | Novos; evitariam cmake/C, mas sem histórico. Reavaliar na fase 4 |
| Reamostragem | `rubato` | 5.0.1 | MIT/Apache-2.0 | 44,1 kHz → 48 kHz e correção fina de deriva de relógio |
| Fila callback → encoder | `ringbuf` | 0.5.3 | MIT/Apache-2.0 | SPSC sem lock, como pede o AGENTS para hot path |
| Loopback por processo (Windows, opcional) | `wasapi` | 0.26.0 | MIT | `AudioClient::new_application_loopback_client` |

Todas compatíveis com MIT OR Apache-2.0, sem GPL/AGPL. `libopus` é BSD-3; a `libpulse` não é necessária (o backend
`pulseaudio` do cpal usa o crate `pulseaudio` 0.3, implementação do protocolo em Rust puro, MIT).

### 3.2 Host Windows
- `cpal` com WASAPI: abrir o **dispositivo de saída padrão como entrada** liga o loopback (doc do `cpal::host::wasapi`).
  Captura a mistura final que vai para o alto-falante; não exige permissão do SO nem aparece como "microfone em uso".
- Sem nada tocando, o loopback não entrega quadros (ou entrega com flag de silêncio): o encoder simplesmente não roda (§6.2).
- **Excluir o próprio TailDesk** (bipes de notificação, som do pedido de consentimento) com loopback por processo em modo
  "excluir árvore do processo" (`wasapi` 0.26). A API é do Windows 10 20348+/Windows 11; em versões antigas cai no loopback normal.
  Opcional, fase 4+.
- Troca do dispositivo padrão (fone plugado): reabrir o stream no `DeviceChanged`/erro do cpal.

### 3.3 Host Linux (Ubuntu, Fedora, Arch)
- Quase todas as distros-alvo rodam **PipeWire** com `pipewire-pulse` (Fedora desde 34, Ubuntu desde 22.10, Arch na instalação
  padrão de GNOME/KDE). Ubuntu 22.04 LTS ainda vem com PulseAudio.
- Ordem no runtime: backend `pipewire` do cpal → se não houver daemon, backend `pulseaudio`.
  - PipeWire: o cpal expõe cada `Audio/Sink` como dispositivo de entrada e, ao abrir, liga `stream.capture.sink=true` (captura o que
    toca nele). Conferido no código do `cpal` 0.18.2 (`host/pipewire/device.rs`).
  - PulseAudio: capturar a fonte `<sink>.monitor` do sink padrão.
- **Não depende do portal**: o ScreenCast do portal não entrega áudio do sistema. Fora do Flatpak o app acessa o PipeWire/Pulse
  direto; no Flatpak precisa de `--socket=pulseaudio` (que o PipeWire atende). Funciona igual em Wayland e X11 e em wlroots/Hyprland.
- Build: `pipewire-devel`/`libpipewire-0.3-dev`/`pipewire` já está na lista do `10` §2.6 por causa do vídeo. Entra **cmake** (se usar
  `bundled`) ou `opus-devel`/`libopus-dev`/`opus` (se ligar à libopus do sistema; decidir na spike).
- A captura do sink segue o padrão quando o usuário troca de saída? Para dispositivo específico o cpal 0.18.2 deixou de rerotear;
  para o padrão, **validar na spike** (Fedora GNOME, Arch KDE, Ubuntu 22.04 com Pulse).

### 3.4 Controlador (reprodução, os dois SOs)
- `cpal` saída padrão, 48 kHz estéreo (reamostrar com `rubato` se o dispositivo pedir outra taxa).
- Buffer de jitter adaptativo, alvo 20–40 ms em LAN e até 80 ms via DERP; descarta quadro atrasado em vez de crescer sem limite.
- **Deriva de relógio**: o relógio da placa de som do host e do controlador diferem (tipicamente até ~0,1%). Sem correção, o buffer
  enche ou esvazia em minutos. Corrigir com reamostragem assíncrona leve (`rubato`, razão ±0,2%) guiada pelo nível do buffer.
- Perda: `decode` com FEC do próximo pacote quando houver; senão PLC da libopus (e PLC neural se a build tiver DRED/OSCE).

### 3.5 Microfone do controlador → host (pós-MVP)
Exige um **microfone virtual** no host. Linux: fácil (nó `Audio/Source` virtual no PipeWire, ou `module-null-sink` no Pulse).
Windows: precisa de driver de áudio virtual assinado (kernel), que é trabalho grande e de distribuição. Proposta: fora do MVP;
quando vier, Linux primeiro, sempre desligado por padrão e com permissão separada (`mic`).

## 4. Desligar o áudio: o que significa e como garantir

### 4.1 Duas chaves, uma regra
O áudio só flui quando **as duas** chaves estão ligadas:

| Chave | Quem controla | Onde | Efeito quando desligada |
|---|---|---|---|
| **Permissão `audio` do host** | Pessoa no host (configuração + caixa no pedido de consentimento) e policy da tailnet (`01` §grants) | `Permissions.audio` no `HelloAck`/`ConsentResult` | Host recusa `StartAudio`, nunca abre captura. Pode ser desligada no meio da sessão pelo indicador do host |
| **"Som do computador remoto"** | Controlador (botão na barra da sessão + padrão nas configurações) | `StartAudio` / `StopAudio` | Host fecha captura e encoder; nada é enviado |

Regra de implementação: **o host é quem garante**. O controlador pedir para parar é suficiente, mas o host nunca envia áudio sem
permissão mesmo que receba `StartAudio`, e o controlador descarta qualquer `kind=1` que chegue com o áudio desligado (pacotes em voo,
no máximo ~1 RTT depois do `StopAudio`).

### 4.2 Desligar ≠ mudo
- **Desligar** (o pedido do Yuri): para captura, encoder e envio. Zero pacotes, zero ACKs, CPU do encoder liberada.
- **Volume/mudo local** no controlador: só o volume de reprodução; os pacotes continuam chegando. Para não confundir, a UI **não** tem
  um "mudo" que pareça economizar rede: o ícone de alto-falante da barra é o liga/desliga real; o volume fica num controle deslizante
  separado no menu do mesmo botão, com legenda "continua recebendo áudio".
- O que **não** fazer: mandar quadros vazios/DTX como "desligado", ou manter a captura aberta "para religar rápido". Religar custa só
  abrir o dispositivo e o primeiro quadro (≈ 20–50 ms), não justifica manter nada vivo.

### 4.3 Sequência no fio
```
C → H  StartAudio { stream_id, prefs: AudioPrefs{profile, max_kbps, channels} }
H → C  AudioSelected { stream_id, codec=OPUS, rate=48000, channels, frame_ms, bitrate_kbps, redundancy }
        ... datagramas kind=1 ...
C → H  StopAudio { stream_id }               (botão do controlador)
H → C  AudioStopped { stream_id, reason: BY_CONTROLLER | BY_HOST | NO_PERMISSION | DEVICE_ERROR }
```
- O host para de enviar **antes** de responder `AudioStopped`; depois dele, todo `kind=1` daquele `stream_id` é bug.
- Desligado pelo host: `AudioStopped{reason=BY_HOST}` sem pedido; o controlador mostra "O computador remoto desligou o áudio".
- `Hello.features` sem `audio` = o controlador nem oferece o botão (ex.: build sem áudio, ou "nunca usar áudio" nas configurações).

### 4.4 Como provar que corta
- **Teste de integração** em `taildesk-media` com transporte em memória: depois do `AudioStopped`, contar datagramas `kind=1` enviados = 0;
  o `AudioCapturer` falso tem que estar fechado; sem `StartAudio` aceito, o capturador nunca é aberto.
- **Contadores por `kind`** no pacer (bytes e pacotes enviados/recebidos), expostos no HUD ("Áudio: desligado · 0 kbit/s") e no
  `taildesk-cli connect --stats`. Opção `--no-audio` no CLI.
- **Teste manual**: medir a interface `tailscale0` (modo sistema) ou o contador do overlay embutido com o áudio ligado tocando música
  e desligado; a diferença esperada está na §5.

## 5. Quanto custa na rede (estimativa)

Overhead por datagrama dentro da tailnet, conexão direta IPv4:
header TailDesk 16 B + QUIC (header curto ~11 B, frame DATAGRAM ~3 B, tag AEAD 16 B) + UDP/IP interno 28 B +
WireGuard (16 B header + 16 B tag + padding médio ~8 B) + UDP/IP externo 28 B ≈ **142 B por pacote**. Via DERP o externo é TCP/TLS,
com overhead parecido ou maior.

| Perfil | Opus | Quadro | Payload | Pacotes/s | **No fio (envio)** | ACKs de volta (~) |
|---|---|---|---|---|---|---|
| LAN alta qualidade | 128 kbit/s estéreo | 10 ms | 160 B | 100 | **~240 kbit/s** | ~20 kbit/s |
| Padrão | 64 kbit/s estéreo | 20 ms | 160 B | 50 | **~120 kbit/s** | ~12 kbit/s |
| Padrão + redundância (§6.3) | 64 kbit/s estéreo | 20 ms | ~320 B | 50 | **~185 kbit/s** | ~12 kbit/s |
| Econômico | 32 kbit/s estéreo | 20 ms | 80 B | 50 | **~90 kbit/s** | ~12 kbit/s |
| Muito ruim | 16 kbit/s mono, FEC in-band | 60 ms | 120 B | 17 | **~35 kbit/s** | ~4 kbit/s |
| Desligado ou silêncio | — | — | — | 0 | **0** | 0 |

Leituras:
- O overhead por pacote é do tamanho do próprio áudio: **10 ms dobra o custo** para ganhar 10 ms de latência. 20 ms é o padrão;
  10 ms só no perfil LAN.
- Perto do vídeo (5–20 Mbit/s em 1080p) o áudio é 1–5%. Onde ele pesa é em banda curta (DERP, 4G roteado): em 1 Mbit/s são 12%,
  e é aí que desligar faz diferença visível.
- Redundância custa +50% e não 2×, porque o pacote extra não existe: o quadro anterior vai junto no mesmo datagrama.

## 6. Pipeline e protocolo

### 6.1 Threads (respeitando as regras do AGENTS para hot path)
```
Host:  callback do cpal ──ringbuf SPSC──▶ thread de áudio (sem tokio): reamostra → Opus encode → pacer
Ctrl:  pacer/recv ──jitter buffer──▶ thread de áudio: Opus decode/PLC → reamostra (deriva) ──ringbuf──▶ callback do cpal
```
Buffers pré-alocados, sem `await` na thread de áudio, sem lock contendido, fila bounded. O callback do cpal só copia amostras.

### 6.2 Datagrama `kind=1`
Reaproveita o header de 16 bytes (`taildesk-proto::media`) sem mudar layout:

| Campo | Uso no áudio |
|---|---|
| `stream_id` | Faixa reservada para áudio (proposta: ≥ 0xF0) para nunca colidir com displays do `13` |
| `frame_seq` | Sequência por quadro Opus (u16, comparação serial; 20 ms dá ~22 min por volta) |
| `frag_idx` / `frag_count` | Sempre 0 / 1. Opus não fragmenta: o encoder recebe `max_data_bytes = max_datagram_size() − 16` (com redundância, metade), o que limita o pico a ~460 kbit/s em 20 ms, muito acima do que usamos |
| `pts_us` | Relógio do host alinhado no handshake (mesma base do vídeo) |
| flags `KEY` | **Início de trecho** (primeiro quadro após silêncio, `StartAudio` ou troca de config): o controlador reinicia o jitter buffer |
| flags `LAST` | **Fim de trecho**: depois dele, falta de pacotes é silêncio, não perda (sem PLC, sem contar no `loss_pct`) |
| flags `DISCARDABLE` | Quadro de redundância carregado junto (§6.3) |
| `codec_aux` | Número de quadros no datagrama (1 ou 2) e tamanho do primeiro |

**Silêncio**: o host detecta silêncio digital (pico < −90 dBFS por 100 ms, ou loopback sem dados) e manda o último quadro com `LAST`;
volta com `KEY` quando o som volta. Silêncio = 0 pacotes, mais determinístico que o DTX do Opus (que ainda manda 1 pacote a cada 400 ms
e só age nas camadas de voz).

### 6.3 Perda de pacotes
| Perda medida | Ação |
|---|---|
| < 1% | Nada: PLC da libopus esconde |
| 1–10% | **Redundância**: cada datagrama leva o quadro atual + o anterior (estilo RFC 2198). +50% no fio, zero pacotes extras |
| > 10% ou banda curta | Perfil "Muito ruim": SILK/mono 16 kbit/s com FEC in-band + quadros de 40–60 ms; DRED (libopus 1.5) se a build tiver |

### 6.4 Prioridade frente ao vídeo
- O quinn descarta os **datagramas mais antigos** quando o buffer de envio enche. Um keyframe (dezenas de fragmentos de vídeo) pode
  empurrar áudio para fora. Por isso o **pacer do `taildesk-media` envia áudio na hora** e só cadencia o vídeo; o áudio nunca espera
  atrás de um frame de vídeo.
- O orçamento de banda do pacer reserva o custo do áudio (§5) **antes** de calcular o bitrate do vídeo.
- Sincronia A/V: desktop remoto prioriza latência. Cada mídia toca assim que pode; o áudio não atrasa o vídeo para "lip sync".
  Se a diferença passar de ~80 ms (vídeo em DERP, por exemplo), o controlador atrasa o áudio para alinhar com o vídeo usando o `pts_us`.

### 6.5 Mensagens de controle (proposta para o `07`)
```protobuf
message AudioPrefs   { AudioProfile profile = 1; uint32 max_kbps = 2; uint32 channels = 3; }
enum    AudioProfile { AUTO = 0; VOICE = 1; MUSIC = 2; }
message StartAudio   { uint32 stream_id = 1; AudioPrefs prefs = 2; }
message AudioSelected{ uint32 stream_id = 1; AudioCodec codec = 2; uint32 sample_rate = 3; uint32 channels = 4;
                       uint32 frame_ms_x10 = 5; uint32 bitrate_kbps = 6; bool redundancy = 7; bool inband_fec = 8; }
message AudioConfig  { uint32 stream_id = 1; uint32 bitrate_kbps = 2; uint32 frame_ms_x10 = 3; bool redundancy = 4; } // host → C, troca a quente
message StopAudio    { uint32 stream_id = 1; }
message AudioStopped { uint32 stream_id = 1; AudioStopReason reason = 2; }
// ReceiverReport ganha: audio_loss_pct, audio_jitter_ms, audio_buffer_ms, concealed_ms
```
Todos com testes de round-trip, como pede o AGENTS. `frame_ms_x10` porque o Opus tem quadros de 2,5 ms.

## 7. Áudio na seleção adaptativa (`14`)

Proposta de entrada do áudio no algoritmo que a thread "Seleção adaptativa de protocolo" está desenhando:

| Rede estimada (`14`) | Perfil de áudio | Observação |
|---|---|---|
| LAN / direta com RTT < 10 ms e banda folgada | 128 kbit/s estéreo, 10 ms | Latência mínima |
| Direta WAN | 64 kbit/s estéreo, 20 ms; redundância se perda > 1% | Padrão |
| DERP ou banda < ~2 Mbit/s | 32 kbit/s, 20 ms, redundância | Áudio continua bom; vídeo cede primeiro |
| Banda < ~500 kbit/s | 16 kbit/s mono, 60 ms, FEC in-band | Mostrar no HUD; **sugerir** desligar o áudio, nunca desligar sozinho |

Regras:
1. O áudio sai do orçamento **antes** do vídeo (falha de áudio incomoda mais que perder FPS).
2. Troca de perfil é `AudioConfig` a quente (Opus muda bitrate sem reiniciar); só canais/quadro pedem novo `KEY`.
3. O adaptativo **nunca liga** áudio que o usuário desligou nem desliga sozinho; no máximo sugere.
4. Áudio desligado devolve a banda ao vídeo imediatamente (o pacer para de reservar).
5. Perfil escolhido pelo usuário (Voz/Música) vira teto, o adaptativo só desce.

## 8. Encaixe com multimonitor (`13`) e UX (`12`)

- **Um stream de áudio por sessão**, independente de quantos monitores estão abertos. Trocar de aba ou destacar janela não mexe no áudio.
- Em "janelas separadas", o botão de som fica na janela do monitor principal (onde ficam arquivos e teclado em `13` §2.2).
- **Controlador, barra da sessão**: ícone de alto-falante = liga/desliga o som do computador remoto; menu do botão com volume local e
  perfil (Automático / Voz / Música). Estado lembrado por dispositivo.
- **Host, pedido de consentimento**: caixa "Áudio" junto das outras permissões, ligada por padrão; indicador de sessão no host mostra
  "Compartilhando tela e som" e permite desligar o som sem encerrar.
- **Configurações**: Controlador → "Som do computador remoto: Ligado/Desligado por padrão"; Host → "Permitir que quem se conecta ouça
  o som deste computador".
- Textos (pt-BR / en): "Som do computador remoto" / "Remote computer sound"; "Desligado: nenhum áudio é transmitido" /
  "Off: no audio is sent"; "Volume (continua recebendo áudio)" / "Volume (audio still received)"; "O computador remoto desligou o
  som" / "The remote computer turned sound off"; "Compartilhando tela e som" / "Sharing screen and sound"; "Permitir que quem se
  conecta ouça o som deste computador" / "Let people who connect hear this computer's sound"; "Automático / Voz / Música" /
  "Automatic / Voice / Music".

## 9. Fases e riscos

| Fase | O que entra |
|---|---|
| 1 | Reservar no proto: `features.audio`, `Permissions.audio`, mensagens da §6.5 (sem implementação), faixa de `stream_id` |
| 2 | Caixa "Áudio" no consentimento (desligada e cinza até a fase 4, ou escondida) |
| 4 | Captura + Opus + playback + pacer com prioridade + perfis do `14` + contadores e teste de "zero pacotes" |
| Pós-MVP | Loopback excluindo o próprio processo (Windows), microfone virtual (Linux primeiro), modo sem perdas em LAN, multicanal 5.1 |

| Risco | Mitigação |
|---|---|
| Captura do sink padrão no PipeWire não segue troca de saída | Spike em Fedora GNOME, Arch KDE; reabrir no evento de troca |
| Ubuntu 22.04 com PulseAudio puro | Backend `pulseaudio` do cpal, testado no job manual de Ubuntu LTS |
| cmake na build (libopus `bundled`) nos containers de CI | Ou ligar à libopus do sistema; decidir na spike e atualizar a tabela de deps do `10` |
| Eco/realimentação se host e controlador forem a mesma máquina ou o host tocar o áudio de outra sessão TailDesk | Loopback excluindo o processo do TailDesk (Windows); no Linux, marcar o nó de playback do TailDesk e não capturá-lo |
| Datagrama de áudio descartado em rajada de keyframe | Pacer com prioridade de áudio (§6.4) |
| Deriva de relógio entre placas de som | Reamostragem adaptativa (§3.4) |

## 10. O que outros arquivos precisariam mudar (não editados aqui)

**`07-protocolo.md`** (thread "Arquitetura inicial do TailDesk")
- §3: trocar `StartAudio{codec=opus, rate, channels}`, `StopAudio` pelas mensagens da §6.5 (`AudioPrefs`, `AudioSelected`, `AudioConfig`,
  `AudioStopped` com motivo) e acrescentar os campos de áudio no `ReceiverReport`.
- §4: corrigir "Áudio Opus: … com FEC in-band do Opus" para a política da §6.3 (FEC in-band só em SILK/baixa banda; redundância de
  quadro anterior no resto); documentar `KEY`/`LAST` como início/fim de trecho e `codec_aux` = quadros no datagrama; reservar faixa de
  `stream_id` para áudio.
- Nova regra: host não envia `kind=1` sem `Permissions.audio` e `StartAudio` aceito; depois de `AudioStopped`, nenhum `kind=1`.

**`10-plano-windows-linux.md`** (mesma thread)
- §1: áudio sai de "fica para depois" para a fase 4 (se o Yuri aprovar), microfone continua depois.
- §2.6: deps de build: `cmake` ou `opus-devel`/`opus`/`libopus-dev`; Flatpak precisa de `--socket=pulseaudio`.
- §5: fase 1 reserva o protocolo; fase 4 ganha "áudio do sistema com liga/desliga que corta o tráfego".

**`06-arquitetura.md`** (mesma thread)
- `taildesk-platform`: trait `AudioCapturer` (Windows WASAPI loopback; Linux PipeWire → Pulse) e `AudioPlayer`.
- `taildesk-media`: thread de áudio, jitter buffer com correção de deriva, pacer com prioridade de áudio e contadores por `kind`.

**`12-levantamento-ux.md`** (thread "Levantamento de design da interface")
- Botão de som na barra da sessão (liga/desliga real, volume separado), caixa "Áudio" no pedido de consentimento do host, indicador
  "Compartilhando tela e som", as duas opções de configuração e os textos da §8.

**`14-selecao-adaptativa.md`** (thread "Seleção adaptativa de protocolo")
- Reservar o custo do áudio no orçamento antes do vídeo e usar a tabela de perfis da §7, com as regras 1–5.

## Fontes
- Opus: RFC 6716; opus-codec.org (libopus 1.5: DRED, PLC neural).
- `cpal` 0.18.2: código-fonte do crate (`host/wasapi/mod.rs` loopback; `host/pipewire/device.rs` `STREAM_CAPTURE_SINK`), CHANGELOG.
- `wasapi` 0.26.0: `AudioClient::new_application_loopback_client` (process loopback).
- crates.io (15/10/2026): `opus` 0.4.0, `opusic-sys` 0.7.5, `opusic-c` 1.6.1, `opus-rs` 0.1.37, `mousiki` 0.2.1, `rubato` 5.0.1,
  `ringbuf` 0.5.3, `pipewire` 0.10.1, `pulseaudio` 0.3.1.
- Microsoft Learn: "Loopback Recording" e `PROCESS_LOOPBACK_MODE`.
- Draft IETF "RTP over QUIC" (avtcore); RFC 2198 (redundância de áudio).
