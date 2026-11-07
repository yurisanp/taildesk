# 07 — Protocolo de fio (proposta v1)

## 1. Camadas

```
Tailnet (WireGuard, UDP)  →  QUIC (quinn, ALPN "halyard/1")
   ├─ Stream bidi 0  : CONTROLE   (protobuf length-delimited) — handshake, negociação, input, cursor, clipboard, QoS
   ├─ Streams uni N  : ARQUIVOS   (um stream por transferência; chunks + hash)
   ├─ Stream uni     : CURSOR SHAPES (bitmaps grandes, cacheáveis)
   └─ DATAGRAMAS     : MÍDIA (vídeo/áudio) — header binário fixo + payload
```

- Porta UDP padrão na tailnet: **7420** (configurável). Um único endpoint QUIC por host.
- Fallback (rede que bloqueia UDP fora do DERP não se aplica — tailnet sempre é UDP/DERP); `tcp_listen` fica reservado para diagnóstico.

## 2. Handshake (stream de controle)

```
C → H  Hello {
          proto_version: 1, app_version, platform {os, arch, device_model},
          decode_caps: [CodecCap{codec, profile, max_w, max_h, chroma444, hw, gpu_output}],
          display_prefs {fit, scale_factor, max_fps}, features: [clipboard, files, audio, multi_display, ...],
          clock: t_client_ns, keyboard, password  (senha de acesso salva; vazia = pedir acesso),
          client_kind: DESKTOP|PHONE|TABLET, viewport {w_px, h_px, dpi}  (celular, §3.4)
       }
H → C  HelloAck {
          proto_version, host_name, platform, displays: [DisplayInfo] (completo em `13-multimonitor.md` §3: posição, tamanho físico e lógico, escala, rotação, `shared`),
          encode_caps: [CodecCap] (codecs que o host tem encoder), permissions: Permissions, consent: Required|NotRequired,
          clock: t_host_ns  (+ eco de t_client_ns p/ estimar offset/RTT), keyboard,
          password_enabled  (o host aceita senha de acesso)
       }
H → C  ConsentResult { granted, by_user?, permissions }        (se Required)
C → H  StartVideo { display_id, wanted: QualityPrefs, priority }   (um por monitor; vários podem estar ativos)
H → C  CodecSelected { display_id, codec, profile, chroma, w, h, fps_target, bitrate_kbps, color: ColorInfo, stream_id }
...    (vídeo começa por datagrama; primeiro frame é keyframe/refresh completo)
```
A identidade do controlador **não** vem do `Hello` — vem do `Overlay::identify(remote_addr)` no host (whois/NodeInfo). O `Hello` só traz preferências e, opcionalmente, a senha.

**Senha de acesso** (fase 4):
- `Hello.password` (tag 9, ≤ 128 bytes, `limits::PASSWORD_LEN`): certa, entra sem o diálogo de consentimento; errada, `Close(BAD_PASSWORD)`; vazia, segue o pedido de acesso normal. Vai só dentro do QUIC com o certificado do host conferido; o host guarda só o hash (argon2). O `Debug` do `Hello` mostra `***` no lugar dela.
- `HelloAck.password_enabled` (tag 11): o host aceita senha, então o controlador pode oferecer o campo enquanto espera o consentimento.
- Senhas erradas demais do mesmo peer: `Close(TOO_MANY_ATTEMPTS)` e o host não confere por um tempo.

## 3. Mensagens de controle (protobuf, `oneof`)

Implementado em `taildesk_proto::control`: cada mensagem vai num `Control` cujo `oneof` tem uma **faixa de tags por grupo** (PR #13). Faixas sem mensagem ficam reservadas para a fase que as traz; números de tag e de enum nunca mudam de significado. No fio, cada `Control` vai precedido do tamanho em varint, com no máximo 64 KiB.

| Tags | Grupo |
|---|---|
| 1–19 | Sessão |
| 20–39 | Vídeo, um stream por monitor (`13-multimonitor.md` §3) |
| 40–49 | Mouse |
| 50–59 | Teclado (`11-teclado.md` §3, fase 2) |
| 60–69 | Cursor |
| 70–79 | Clipboard (fase 4) |
| 80–89 | Arquivos (fase 3; §3.1) |
| 90–99 | QoS e seleção adaptativa (`14-selecao-adaptativa.md` §4) |
| 100–109 | Áudio (`15-audio.md` §6.5; reservado) |
| 110–119 | Chat |
| 120–129 | Celular: caminho, lista de dispositivos e pareamento (§3.4) |
| 130–139 | Energia: Wake on LAN (§3.5, `17-wake-on-lan.md`) |

| Grupo | Mensagens |
|---|---|
| Sessão | `Hello`, `HelloAck`, `ConsentResult`, `Close{reason: CloseReason}` (valores abaixo), `Ping{t}`/`Pong{t,t_echo}` |
| Vídeo | `StartVideo{display_id, wanted, priority}`, `CodecSelected{display_id, stream_id, ...}`, `RequestKeyframe{display}`, `PictureLoss{display, last_good_seq}`, `Nack{display, frame_seq, missing_frag_bitmap}`, `QualityPrefs{fps_max, mode: Auto|Text|Motion|LowBandwidth, chroma444, codec, bitrate_kbps}`, `SetVideoQuality{display_id, wanted: QualityPrefs}` (C→H, tag 30), `StopVideo{display_id}`, `SetPriority{display_id, FOCUSED|VISIBLE}`, `VideoStopped{display_id, reason}`, `VideoStats{display_id, encoder, gpu, fps, bitrate, encode_us, ..., est_kbps}` (H→C, 1×/s, só para o painel de diagnóstico), `DisplaysChanged{displays}` (sem `SwitchDisplay`: trocar = Stop + Start) |
| Input | Teclado: ver `11-teclado.md` §3 (`KeyboardInfo`, `Key`, `KeyShortcut`, `KeyChar`, `KeyboardMode`, `ReleaseAll`), `MouseMove{display_id, x, y (u16 normalizado no monitor) | rel}`, `MouseButton{btn, down}`, `Scroll{dx, dy, precise}` (convenção abaixo), `Touch{id, phase, x, y, pressure}`, `Gamepad{...}` (futuro) |
| Cursor | `CursorPos{display_id, x, y, visible}`, `CursorShapeRef{hash}` (forma completa no stream de cursor, §3.2) |
| Clipboard | `ClipboardOffer`, `ClipboardRequest`, `ClipboardData`, `ClipboardError` (tags 70–73; detalhes em §3.3) |
| Arquivos (80–89) | `FsRootsRequest`, `FsRoots`, `FsList`, `FsListing`, `FileOffer`, `FileRequest`, `FileAccept`, `FileCancel`, `FileDone`, `FileError` (detalhe em §3.1) |
| QoS | `ReceiverReport{display, recv_kbps, loss_pct, jitter_ms, decode_ms, render_ms, queue_frames, audio_loss_pct, audio_jitter_ms, audio_buffer_ms, concealed_ms}` (a cada 250 ms) |
| Áudio (`15-audio.md` §6.5) | `StartAudio{stream_id, prefs: AudioPrefs{profile: AUTO|VOICE|MUSIC, max_kbps, channels}}`, `AudioSelected{stream_id, codec=OPUS, sample_rate, channels, frame_ms_x10, bitrate_kbps, redundancy, inband_fec}`, `AudioConfig{stream_id, bitrate_kbps, frame_ms_x10, redundancy}` (troca a quente), `StopAudio{stream_id}`, `AudioStopped{stream_id, reason: BY_CONTROLLER|BY_HOST|NO_PERMISSION|DEVICE_ERROR}` |
| Chat | `ChatMessage{text}` |
| Celular (120–124) | `PathInfo{kind, relay_region, rtt_ms}` (H→C), `PeerListRequest{start}` / `PeerList{entries, more}`, `PairRequest{code, device_name}` / `PairResult{ok, cert_fingerprint, error}` (detalhe em §3.4) |
| Energia (130–132) | `PowerInfo{ifaces}` (H→C), `WakeRequest{mac, lan}` / `WakeResult{ok, sent, error}` (detalhe em §3.5) |

Multi-monitor (`13-multimonitor.md` §3): **um stream de vídeo por monitor**, cada um com encoder, keyframes e `stream_id` próprios, respondido por um `CodecSelected` por `display_id`. Host recusa `StartVideo` acima do seu limite (`Error{TOO_MANY_STREAMS}`; padrão 2 com openh264). Parser: no máximo 16 `DisplayInfo` por mensagem, `name` ≤ 128 bytes.

Codec e taxa escolhidos pelo usuário (seletor da sessão e Configurações):
- `QualityPrefs.codec` (tag 4): `Unspecified` = automático. O host só atende se o controlador decodifica o codec (`Hello.decode_caps`) e ele tem encoder (`HelloAck.encode_caps`, que lista os codecs com encoder no host); senão fica no automático.
- `QualityPrefs.bitrate_kbps` (tag 5): `0` = automática (o host ajusta pelo tamanho da imagem e pela rede); teto no parser de 1 Gbit/s (`limits::VIDEO_KBPS`).
- Valem no `StartVideo.wanted` e no `SetVideoQuality{display_id, wanted}` (C→H, tag 30), que troca codec ou taxa de um monitor que já está saindo. Codec novo = encoder novo e outro `CodecSelected` (com outro `stream_id`); só a taxa = ajuste a quente, sem `CodecSelected`.
- `VideoStats.est_kbps` (tag 13): banda que o QUIC do host estima para o caminho (janela de congestionamento ÷ RTT), em kbit/s; `0` = sem estimativa.
- PyroWave (`Codec = 6`) é opcional e só entra quando escolhido à mão.

**`CloseReason`** (`Close.reason`): `UNSPECIFIED = 0`, `BY_CONTROLLER = 1`, `BY_HOST = 2`, `CONSENT_DENIED = 3` (a pessoa no host recusou), `VERSION_MISMATCH = 4`, `PROTOCOL_ERROR = 5`, `CONSENT_TIMEOUT = 6` (ninguém respondeu ao pedido de acesso em 60 s), `NOT_ALLOWED = 7` (o host não aceita conexões deste peer, por configuração ou política), `BAD_PASSWORD = 8` (senha de acesso errada, ou o host não tem mais senha), `TOO_MANY_ATTEMPTS = 9` (senhas erradas demais deste peer; o host não confere por um tempo). Com `CONSENT_DENIED`, `CONSENT_TIMEOUT` e `NOT_ALLOWED` a UI distingue "recusou", "não respondeu" e "não aceita conexões" (`12-levantamento-ux.md` §5.3).

**`Scroll`**: valores positivos são **para baixo e para a direita**, como no libinput. Com `precise = false`, `dx`/`dy` contam **dentes da roda**; com `precise = true`, são **pixels lógicos** (touchpad). `dx = dy = 0` encerra o gesto do touchpad.

Regras: campos novos sempre opcionais; `proto_version` maior só quando quebrar; mensagens desconhecidas no `oneof` são ignoradas e logadas.

### 3.1 Arquivos (tags 80–89, fase 3)

Caminhos são relativos a uma raiz (`root`), com `/` como separador, e quem recebe sempre os passa por `taildesk_files::resolve_in_root`. A **raiz 0** (`files::INBOX_ROOT`) é a pasta de entrada de quem recebe (`~/Downloads/TailDesk`). A raiz **`files::CLIPBOARD_ROOT` = `u32::MAX`** (só leitura, nunca aparece em `FsRoots`) são os arquivos copiados na área de transferência de quem envia, aceita em `FileRequest` e `FsList` com caminho `"<índice em ClipboardOffer.files>/<nome>[/...]"` da oferta em vigor (§3.3).

| Tag | Mensagem | Sentido | Campos |
|---|---|---|---|
| 80 | `FsRootsRequest` | C→H | — |
| 81 | `FsRoots` | H→C | `roots[{id ≠ 0, name, access: READ\|WRITE\|READ_WRITE}]` (vazio = navegador remoto desligado; até 64 raízes, nome ≤ 128 bytes) |
| 82 | `FsList` | C→H | `req_id, root, path ("" = a raiz), start` |
| 83 | `FsListing` | H→C | `req_id, entries[{name, kind: DIR\|FILE\|OTHER, size, mtime_ms}], more, error` |
| 84 | `FileOffer` | quem envia | `xfer_id, root, rel_path, size, mtime_ms` |
| 85 | `FileRequest` | quem baixa | `xfer_id, root, path` (sem offset) |
| 86 | `FileAccept` | quem recebe | `xfer_id, offset, prefix_blake3` |
| 87 | `FileCancel` | ambos | `xfer_id` |
| 88 | `FileDone` | quem envia | `xfer_id, blake3` (arquivo inteiro) |
| 89 | `FileError` | ambos | `xfer_id, code` |

- **Envio**: `FileOffer` → `FileAccept{offset, prefix_blake3}` → stream de dados → `FileDone`.
- **Download**: `FileRequest` → o peer responde com `FileOffer` → daí em diante igual ao envio. O offset nunca vai no `FileRequest`: quem recebe decide pelo `.part` que tem.
- **Retomada**: `offset` = tamanho do `.part`; `prefix_blake3` = BLAKE3 desses bytes (32 bytes; vazio quando `offset` = 0). Se o prefixo não bate com o arquivo de quem envia, ele responde `FileError(RESUME_MISMATCH)` e quem recebe recomeça do zero.
- **Listagem paginada**: no máximo **512 itens ou 40 KiB de nomes** por `FsListing`, para caber no frame de 64 KiB; `more = true` → pedir de novo com `start += entries.len()`.
- **Stream de dados**: um stream QUIC uni por arquivo, começando pelo cabeçalho de **16 bytes** `files::DataHeader` (little-endian): `"TDF"` + versão `0x01` (4 bytes), `xfer_id` u32, `offset` u64 (posição do primeiro byte que segue); depois bytes crus em blocos de 256 KiB. Cabeçalho errado ou bytes a mais = `FileError(PROTOCOL)`.
- **Limites**: caminho ≤ 4096 bytes, nome ≤ 1024 bytes, até 4 transferências simultâneas por sessão (acima disso, `BUSY`).
- **Códigos de `FileError`**: `NOT_FOUND`, `DENIED` (sem `Permissions.files`, raiz sem o acesso pedido ou recusa da pessoa), `BAD_PATH`, `NO_SPACE`, `HASH_MISMATCH`, `RESUME_MISMATCH`, `IO`, `BUSY`, `CANCELLED`, `CHANGED` (origem mudou durante o envio), `PROTOCOL`.

### 3.2 Stream de cursor (`taildesk_proto::cursor`, fase 2)

Stream QUIC uni do host para o controlador:
- Começa com os 4 bytes `"TDC\x01"` (`TDC`, versão 1).
- Cada forma = cabeçalho de **16 bytes**, little-endian: `hash` u64, `width` u16, `height` u16, `hot_x` u16, `hot_y` u16. `width` e `height` vão de 1 a 256.
- Em seguida, `width × height × 4` bytes de pixels **RGBA8 sRGB, sem pré-multiplicação**.
- A forma é sempre enviada **antes** do `CursorShapeRef{hash}` que a referencia.
- Cache LRU: **64** formas no host e **128** no controlador.

### 3.3 Área de transferência (tags 70–73, fase 4)

Quem copia manda uma `ClipboardOffer` com os formatos (e, para arquivos, a lista). O outro lado põe a oferta na área de transferência do sistema sem o conteúdo e só pede o formato (`ClipboardRequest`) quando alguém cola. O conteúdo volta em pedaços de `ClipboardData`, entre as outras mensagens de controle, para um PNG grande não segurar o mouse. Sem `Permissions.clipboard` nada disso é atendido.

| Tag | Mensagem | Campos |
|---|---|---|
| 70 | `ClipboardOffer` | `seq`, `formats: [ClipFormat]`, `sizes` (na ordem de `formats`, 0 = desconhecido), `files: [ClipFile{name, size, dir}]` (com `FILES`, só o primeiro nível). `formats` vazio = ficou vazia. Só sai quando a área de transferência muda por alguém daquele lado, nunca pela oferta do peer |
| 71 | `ClipboardRequest` | `seq, format`: alguém colou |
| 72 | `ClipboardData` | `seq, format, offset, total, data`: `data` vai em `offset` de `total` bytes; o último pedaço é o que chega a `total`; conteúdo vazio = um pedaço só com `total = 0` |
| 73 | `ClipboardError` | `seq, format, code: ClipErrorCode` |

- **`ClipFormat`**: `UNSPECIFIED = 0`, `TEXT = 1` (UTF-8, quebras `\n`), `HTML = 2` (fragmento UTF-8, sem o cabeçalho "HTML Format" do Windows), `PNG = 3`, `FILES = 4` (lista em `ClipboardOffer.files`; o conteúdo vem pelos streams de arquivos).
- **`ClipErrorCode`**: `UNSPECIFIED = 0`, `GONE = 1` (a oferta `seq` já foi trocada, ou nunca existiu), `DENIED = 2` (sem `Permissions.clipboard`), `TOO_BIG = 3` (passa de `CLIP_TOTAL`), `UNAVAILABLE = 4` (o sistema não entregou o formato, por exemplo porque o app que copiou fechou).
- **`seq`**: cada lado numera as próprias ofertas; pedido de uma oferta que já foi trocada recebe `ClipboardError(GONE)`.
- **Arquivos colados**: vêm pelos streams de arquivos (§3.1) com `FileRequest`/`FsList` na raiz `CLIPBOARD_ROOT` (`u32::MAX`), caminho `"<índice>/<nome>[/...]"`, onde o índice é a posição em `ClipboardOffer.files` da oferta em vigor.
- **Limites** (`taildesk_proto::control::limits`): `CLIP_FORMATS = 8` formatos (e `sizes`) por oferta; `CLIP_FILES = 256` itens em `files` (o resto não vai); `CLIP_CHUNK = 32 KiB` por `ClipboardData`; `CLIP_TOTAL = 64 MiB` por formato.

### 3.4 Celular (tags 120–124, `mobile/android-detalhado.md` §7)

O celular não tem LocalAPI (o app do Tailscale não a expõe), então o host desktop informa o
caminho e a lista de dispositivos, e o pareamento por QR fixa o certificado do host. Nada muda no
header de 16 bytes nem nos streams.

- **`Hello.client_kind`** (tag 10): `UNSPECIFIED = 0` (desktop antigo), `DESKTOP = 1`, `PHONE = 2`,
  `TABLET = 3`. Só para exibir ("Celular" no diálogo do host); a identidade continua vindo de
  `Overlay::identify()`.
- **`Hello.viewport`** (tag 11): `{w_px, h_px, dpi}` da área útil da tela, cada lado ≤ 8192. O host
  nunca codifica acima disso, em qualquer degrau do laço lento: `Viewport::fit` mantém a proporção,
  compara lado maior com maior (a tela gira) e arredonda para lados pares (MF-82).
- **`KeyboardInfo.source`** (tag 4): `UNSPECIFIED = 0` (= físico), `VIRTUAL = 1`, `HARDWARE = 2`. O
  teclado virtual não tem layout físico: tudo que é imprimível vai como `KeyChar`; atalhos com as
  travas da faixa de teclas vão como `KeyShortcut`.
- **`PathInfo`** (tag 120, H→C, na sessão): `kind` é o `PathKind` do fio (`0` desconhecido,
  `1` direto LAN, `2` direto internet, `3` relay), `relay_region` (≤ 128 bytes, vazio = nenhuma),
  `rtt_ms` (0 = sem medida). O host mede pelo `ping` disco do `tailscaled` e manda no início da
  sessão de um celular e quando muda. É pista para o HUD e o adaptador, nunca a classe de rede.

**Conexões de serviço.** `PeerListRequest` e `PairRequest` vão como **primeira e única mensagem**
de uma conexão QUIC curta, no lugar do `Hello`: o host responde e fecha (`Close{BY_HOST}`), sem
diálogo e sem mídia. No meio de uma sessão são ignoradas.

- **`PeerListRequest{start}`** (tag 121, `start` ≤ 256) → **`PeerList{entries, more}`** (tag 122):
  até 64 `PeerEntry{name ≤ 128 B, dns_name ≤ 253 B, ip, os, online, taildesk: UNKNOWN|YES|NO}` por
  página e 256 no total; `more` = há mais a partir de `start + entries.len()`. O host só responde
  ao **mesmo usuário da tailnet** (login do `whois` igual ao do próprio host) ou a um celular
  **pareado**; para os outros, `Close{NOT_ALLOWED}`. A lista é sugestão: quem conecta é
  identificado de novo pela tailnet.
- **`PairRequest{code ≤ 16 B, device_name ≤ 128 B}`** (tag 123) → **`PairResult{ok,
  cert_fingerprint, error}`** (tag 124). Com `ok`, `cert_fingerprint` tem os 32 bytes do SHA-256 do
  certificado do host; sem `ok`, vem vazio e `error` diz por quê: `BAD_CODE = 1` (errado ou já
  usado), `EXPIRED = 2` (passou dos 5 minutos), `TOO_MANY_ATTEMPTS = 3`, `DISABLED = 4` (o host
  não aceita celulares). O celular pareado fica guardado pelo `node_stable_id` do `whois`, nunca
  pelo `device_name`.

**QR de pareamento** (`taildesk_proto::pair`): URI
`taildesk://pair?v=1&n=<nome>&a=<IP da tailnet>&p=<porta>&f=<SHA-256 em hex>&c=<código>`
(≤ 512 bytes, campos desconhecidos ignorados, `v` maior = atualizar o app). O código tem 6
símbolos do Base32 de Crockford (`0-9A-Z` sem `I L O U`), vale uma vez por 5 minutos e aparece em
dois grupos (`K7P-2QX`); ao digitar, `O` vira `0` e `I`/`L` viram `1`. O celular confere o
certificado contra `f` antes de mandar o código.

### 3.5 Energia: Wake on LAN (tags 130–132, `17-wake-on-lan.md` §4)

O pacote mágico é broadcast de camada 2 e não atravessa a tailnet: quem manda é um nó acordado na
mesma LAN do alvo (o **repetidor**). O host informa as próprias placas enquanto está ligado, e o
controlador guarda para quando ele estiver desligado.

- **`PowerInfo{ifaces}`** (tag 130, H→C, logo depois do `HelloAck`): até 8 `LanIface{mac (6 B),
  ipv4 (fixed32, ordem de rede; 0 = sem endereço), prefix_len (0–32), gateway_mac (vazio ou 6 B),
  wired, wol: UNKNOWN|ARMED|OFF}`. Só IPv4 e sem a placa da tailnet. No `PowerInfo` o `mac` é
  obrigatório.
- **`WakeRequest{mac (6 B), lan?}`** (tag 131) → **`WakeResult{ok, sent, error}`** (tag 132), numa
  **conexão de serviço** como as do §3.4. `lan` é a `LanIface` do alvo (o `mac` dela é opcional e
  não é usado). O repetidor só atende o **mesmo usuário da tailnet** ou um celular **pareado**
  (como a `PeerList` e a PeerAPI `/v0/wol` do Tailscale); para os outros, `Close{NOT_ALLOWED}`.
  Duas placas estão na mesma LAN quando a sub-rede bate (pelo prefixo do alvo) e, se os dois lados
  sabem, o MAC do gateway também. Sem placa na LAN: `NOT_SAME_LAN = 1`, sem mandar nada (o
  controlador tenta outro repetidor). O mesmo MAC em menos de 10 s: `RATE_LIMITED = 2`. Nenhuma
  placa conseguiu: `SEND_FAILED = 3`. Com `ok`, `sent` = por quantas placas saiu. O pacote (102
  bytes: `FF`×6 + MAC×16) vai em UDP para `255.255.255.255:9` e para o broadcast dirigido da
  sub-rede, 3 vezes com 100 ms entre elas. `WakeResult` não diz se o alvo acordou: isso se vê pelo
  peer voltar `Online` no `status`.

## 4. Header de datagrama de mídia (binário, little-endian, 16 bytes)

| Offset | Tamanho | Campo | Descrição |
|---|---|---|---|
| 0 | 1 | `ver:2 \| kind:2 \| flags:4` | kind: 0=vídeo 1=áudio 2=FEC 3=probe · flags: `KEY`, `LAST`, `DISCARDABLE`, `CODEC_CHANGE` |
| 1 | 1 | `stream_id` | id do stream: vídeo (< 0xF0) anunciado em `CodecSelected`, áudio (≥ 0xF0) em `AudioSelected` |
| 2 | 2 | `frame_seq` | sequência do frame (wrap-around, comparação serial) |
| 4 | 2 | `frag_idx` | índice do fragmento |
| 6 | 2 | `frag_count` | total de fragmentos do frame |
| 8 | 4 | `pts_us` | timestamp do host (µs, u32 com wrap; relógio alinhado no handshake) |
| 12 | 1 | `fec_group` | grupo FEC (0 = sem FEC) |
| 13 | 1 | `reserved` | — |
| 14 | 2 | `codec_aux` | uso por codec (ex.: PyroWave: id de bloco inicial; AV1: tile) |

Payload ≤ `quinn::Connection::max_datagram_size()` − 16 (≈ 1150–1180 B na tailnet). Consultar em runtime, nunca fixar.
- **Vídeo inter (AV1/HEVC/H.264/VP8/VP9)**: o frame só é entregue ao decoder completo; fragmentos faltantes → NACK (se RTT baixo) ou `PictureLoss`.
- **PyroWave**: cada datagrama = 1 pacote do `pyrowave_encoder_packetize` (blocos independentes); o decoder recebe por `pyrowave_decoder_push_packet` à medida que chegam e decodifica no prazo do frame mesmo com faltas.
- **Áudio Opus** (`15-audio.md` §6.2–6.3): `kind=1`, mesmo header de 16 bytes, `stream_id` na faixa reservada **≥ 0xF0** (displays ficam abaixo), `frag_idx/frag_count` sempre 0/1 (Opus não fragmenta), `KEY` = início de trecho, `LAST` = fim de trecho (silêncio = zero pacotes, sem PLC), `DISCARDABLE` = quadro de redundância embutido, `codec_aux` = quadros no datagrama. Perda: < 1% só PLC; 1–10% redundância do quadro anterior no mesmo datagrama; > 10% SILK mono com FEC in-band (e DRED se disponível). O pacer envia áudio na hora e reserva a banda dele antes de calcular o vídeo.
- **Regra de áudio desligado**: o host **não envia nenhum `kind=1`** sem `Permissions.audio` e um `StartAudio` aceito; para de enviar **antes** de responder `AudioStopped`, e depois dele qualquer `kind=1` daquele `stream_id` é bug. Desligado = sem captura e sem pacotes (não é mudo). `Hello.features` sem `audio` = o controlador não oferece o botão.

## 5. Controle de congestionamento e QoS

- QUIC (quinn) com **BBR** para estimar banda; o *media pacer* consulta `connection.stats()` (cwnd, rtt, lost) + `ReceiverReport`.
- Laço de controle a cada ~100 ms: `target_bitrate = min(estimativa_bbr × 0.85, teto_do_modo)`; se `queue_delay` (rtt − min_rtt) > 15 ms → corta 20% imediatamente; recuperação +5%/s.
- Ordem de degradação em modo *Text* (desktop/ERP): bitrate → FPS (60→30→15) → resolução. Modo *Motion*: bitrate → resolução → FPS.
- Mudança de caminho (direto ↔ DERP) detectada → renegociar codec (sai do PyroWave).
- Probe de banda no início (rajada curta de `kind=3`) para escolher o perfil inicial sem esperar convergência.

## 6. Versionamento e compatibilidade
- ALPN `halyard/1`; host aceita `halyard/1` até `halyard/N` na lista do cliente.
- Arquivos `.proto` em `halyard-proto/proto/halyard/v1/*.proto`; checagem de compatibilidade com `buf breaking` no CI.
