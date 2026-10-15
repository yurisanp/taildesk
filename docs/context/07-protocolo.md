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
          clock: t_client_ns
       }
H → C  HelloAck {
          proto_version, host_name, platform, displays: [DisplayInfo] (completo em `13-multimonitor.md` §3: posição, tamanho físico e lógico, escala, rotação, `shared`),
          encode_caps: [...], permissions: Permissions, consent: Required|NotRequired,
          clock: t_host_ns  (+ eco de t_client_ns p/ estimar offset/RTT)
       }
H → C  ConsentResult { granted, by_user?, permissions }        (se Required)
C → H  StartVideo { display_id, wanted: QualityPrefs, priority }   (um por monitor; vários podem estar ativos)
H → C  CodecSelected { display_id, codec, profile, chroma, w, h, fps_target, bitrate_kbps, color: ColorInfo, stream_id }
...    (vídeo começa por datagrama; primeiro frame é keyframe/refresh completo)
```
A identidade do controlador **não** vem do `Hello` — vem do `Overlay::identify(remote_addr)` no host (whois/NodeInfo). O `Hello` só traz preferências.

## 3. Mensagens de controle (protobuf, `oneof`)

| Grupo | Mensagens |
|---|---|
| Sessão | `Hello`, `HelloAck`, `ConsentResult`, `Close{reason}`, `Ping{t}`/`Pong{t,t_echo}` |
| Vídeo | `StartVideo{display_id, wanted, priority}`, `CodecSelected{display_id, stream_id, ...}`, `RequestKeyframe{display}`, `PictureLoss{display, last_good_seq}`, `Nack{display, frame_seq, missing_frag_bitmap}`, `QualityPrefs{fps_max, mode: Auto|Text|Motion|LowBandwidth, chroma444}`, `StopVideo{display_id}`, `SetPriority{display_id, FOCUSED|VISIBLE}`, `VideoStopped{display_id, reason}`, `DisplaysChanged{displays}` (sem `SwitchDisplay`: trocar = Stop + Start) |
| Input | Teclado: ver `11-teclado.md` §3 (`KeyboardInfo`, `Key`, `KeyShortcut`, `KeyChar`, `KeyboardMode`, `ReleaseAll`), `MouseMove{display_id, x, y (u16 normalizado no monitor) | rel}`, `MouseButton{btn, down}`, `Scroll{dx,dy,precise}`, `Touch{id, phase, x, y, pressure}`, `Gamepad{...}` (futuro) |
| Cursor | `CursorPos{display_id, x, y, visible}`, `CursorShapeRef{hash}` (shape completo no stream de cursor) |
| Clipboard | `ClipboardOffer{formats}`, `ClipboardData{format, bytes}` (texto/HTML/PNG; arquivos → transferência explícita) |
| Arquivos | `FileOffer{id, name, size, hash}`, `FileAccept`, `FileCancel`, `FileProgress` |
| QoS | `ReceiverReport{display, recv_kbps, loss_pct, jitter_ms, decode_ms, render_ms, queue_frames}` (a cada 250 ms) |
| Áudio | `StartAudio{codec=opus, rate, channels}`, `StopAudio` |
| Chat | `ChatMessage{text}` |

Multi-monitor (`13-multimonitor.md` §3): **um stream de vídeo por monitor**, cada um com encoder, keyframes e `stream_id` próprios, respondido por um `CodecSelected` por `display_id`. Host recusa `StartVideo` acima do seu limite (`Error{TOO_MANY_STREAMS}`; padrão 2 com openh264). Parser: no máximo 16 `DisplayInfo` por mensagem, `name` ≤ 128 bytes.

Regras: campos novos sempre opcionais; `proto_version` maior só quando quebrar; mensagens desconhecidas no `oneof` são ignoradas e logadas.

## 4. Header de datagrama de mídia (binário, little-endian, 16 bytes)

| Offset | Tamanho | Campo | Descrição |
|---|---|---|---|
| 0 | 1 | `ver:2 \| kind:2 \| flags:4` | kind: 0=vídeo 1=áudio 2=FEC 3=probe · flags: `KEY`, `LAST`, `DISCARDABLE`, `CODEC_CHANGE` |
| 1 | 1 | `stream_id` | id do stream (display/áudio) anunciado em `CodecSelected` |
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
- **Áudio Opus**: 1 frame Opus por datagrama, `kind=1`, com FEC in-band do Opus.

## 5. Controle de congestionamento e QoS

- QUIC (quinn) com **BBR** para estimar banda; o *media pacer* consulta `connection.stats()` (cwnd, rtt, lost) + `ReceiverReport`.
- Laço de controle a cada ~100 ms: `target_bitrate = min(estimativa_bbr × 0.85, teto_do_modo)`; se `queue_delay` (rtt − min_rtt) > 15 ms → corta 20% imediatamente; recuperação +5%/s.
- Ordem de degradação em modo *Text* (desktop/ERP): bitrate → FPS (60→30→15) → resolução. Modo *Motion*: bitrate → resolução → FPS.
- Mudança de caminho (direto ↔ DERP) detectada → renegociar codec (sai do PyroWave).
- Probe de banda no início (rajada curta de `kind=3`) para escolher o perfil inicial sem esperar convergência.

## 6. Versionamento e compatibilidade
- ALPN `halyard/1`; host aceita `halyard/1` até `halyard/N` na lista do cliente.
- Arquivos `.proto` em `halyard-proto/proto/halyard/v1/*.proto`; checagem de compatibilidade com `buf breaking` no CI.
