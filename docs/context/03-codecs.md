# 03 — Codecs: AV1, H.265, H.264, VP9, VP8 e PyroWave

## 1. Visão geral comparativa (para *desktop streaming* em tempo real)

| Codec | Eficiência (bits p/ mesma qualidade) | Latência típica de encode | HW encode disponível | HW decode disponível | Patentes/royalties | Papel no produto |
|---|---|---|---|---|---|---|
| **AV1** | Melhor (≈30% < HEVC em conteúdo de tela, ferramentas SCC: *palette*, *IntraBC*) | Baixa em HW; SW realtime é pesado | NVIDIA RTX 40+, AMD RDNA3+, Intel Arc/Core Ultra+, alguns Android recentes. **Apple: não** | Amplo em GPUs ≥2020; Apple M3/A17 Pro+; Android ≥2022 em muitos SoCs | Royalty-free (AOMedia) | **Preferido em internet** quando ambos têm HW |
| **H.265/HEVC** | Muito boa; suporta 4:4:4 (RExt) em parte do HW | Baixa em HW | Amplo (NVENC, AMF, QSV, VideoToolbox, MediaCodec) | Amplo; Windows: via D3D11VA/DXVA direto (MF pode exigir extensão HEVC) | **Pool de patentes complexo** — avaliar jurídico se distribuir encoder/decoder SW | Preferido quando não há AV1 HW (especialmente Apple) |
| **H.264/AVC** | Base de referência | Muito baixa | Universal | Universal | Patenteado (MPEG LA/Via LA); OpenH264 binário da Cisco cobre royalties | **Piso de compatibilidade universal** |
| **VP9** | ≈ HEVC; perfil 1 suporta 4:4:4 | SW realtime ok (libvpx `cpu-used` alto); HW encode raro (Intel) | Raro | Amplo | Royalty-free | Fallback SW de qualidade (default do RustDesk) |
| **VP8** | Pior que H.264 | SW leve | Quase nenhum | Parcial | Royalty-free | Último recurso SW / dispositivos muito fracos |
| **PyroWave** | **Ruim por bit** (intra-only, sem entropia): 100–200+ Mbit/s para 1080p60 | **~0,1 ms 1080p / ~0,25 ms 4K na GPU** (dados do autor) | É um codec de *compute shader*: qualquer GPU Vulkan (Win/Linux/Android) e Metal (Apple7+ macOS/iOS) | Idem | MIT, sem patentes conhecidas | **Modo LAN / ultra-baixa latência** (gaming, CAD, edição de vídeo) |

Regras práticas:
- **Internet / Wi-Fi ruim / DERP** → AV1 HW > H.265 HW > H.264 HW > VP9 SW > VP8 SW.
- **LAN gigabit ou Wi-Fi 6/7 forte, conexão direta na tailnet** → PyroWave (se GPU dos dois lados) com fallback automático para o modo internet se a banda medida cair.
- **Texto nítido (IDE, planilhas, ERP)** → negociar **4:4:4** (HEVC RExt, VP9 profile 1, AV1 High, PyroWave 444) ou ativar *chroma upsampling* esperto; 4:2:0 borra texto colorido.

## 2. Configuração de baixa latência (válida para AV1/HEVC/H.264/VP9)

- **Sem B-frames**; perfil *low-delay P* (`tune=ull`/`ultra-low-latency` no NVENC, `usage=realtime` no libaom/SVT, `deadline=realtime` no libvpx).
- **GOP "infinito"** + **intra refresh** (faixas intra rotativas) em vez de keyframes periódicos → evita picos de bitrate. Keyframe só sob pedido (perda irreparável, novo viewer, troca de resolução).
- **Rate control CBR/VBR com VBV ≈ 1 frame** (buffer = bitrate/fps). Picos > 1 frame viram latência.
- **LTR (Long-Term Reference) / reference invalidation** onde existir (NVENC, MF, VT parcial) → ao perder frame N, o receptor avisa e o encoder referencia um LTR confirmado em vez de mandar IDR.
- **Slices / tiles**: dividir o frame em fatias independentes alinhadas ao MTU ajuda a recuperar perdas parciais e paraleliza decode.
- **Codificar só quando há dano** (dirty rects/frame change). Tela parada = 0 fps + *keepalive*. Isso é a maior economia de banda em desktop.
- **Cursor separado** do vídeo (forma + posição como mensagens próprias) — latência percebida cai muito.
- Resolução/FPS adaptativos: preferir baixar **FPS** antes de baixar **resolução** em desktop (texto legível > fluidez); o inverso em jogos.

## 3. Formato de bitstream no fio

| Codec | Unidade enviada | Observações |
|---|---|---|
| H.264 | Access unit em **Annex B** (start codes) com SPS/PPS repetidos em todo IDR | Decoders HW de Apple (VT) querem **AVCC** (length-prefixed) → converter no receptor. |
| H.265 | Annex B com VPS/SPS/PPS no IDR | Idem (HVCC p/ VT). |
| AV1 | **Low-overhead OBU stream** (temporal unit com OBU_TEMPORAL_DELIMITER), Sequence Header no keyframe | dav1d e decoders HW aceitam OBUs. |
| VP8/VP9 | Frame "cru" do libvpx (IVF sem cabeçalho) | Superframes VP9 já vêm agrupados. |
| PyroWave | **Pacotes já fragmentados pelo próprio encoder** (`pyrowave_encoder_packetize*`, tamanho de pacote configurável) | Cada bloco 32×32 é independente: perda = borrão local, sem dependência temporal. Mapeia 1:1 em datagramas. |

Cabeçalho comum de frame (nosso protocolo, ver `07-protocolo.md`): `codec`, `display_id`, `frame_seq`, `pts_us`, `keyframe`, `chroma`, `width/height`, `color` (BT.709 limited por padrão; sinalizar full range / BT.2020 / HDR quando suportado).

## 4. Implementações e crates (verificados em 15/10/2026)

### 4.1 Software
| Codec | Encode | Decode | Notas |
|---|---|---|---|
| H.264 | `openh264` **0.9.8** (bindings; pode baixar o binário oficial da Cisco em runtime — é isso que dá cobertura de royalties) | `openh264` | Baseline/Constrained Baseline apenas; ótimo piso universal. |
| AV1 | `rav1e` **0.8.1** (Rust puro, mas pesado p/ realtime); SVT-AV1 ou libaom via FFmpeg (`ffmpeg-the-third`) | **`dav1d` 0.11.1** (bindings do libdav1d — o decoder SW mais rápido) | Para encode SW realtime preferir SVT-AV1 preset 10–12 `--rtc` via FFmpeg. |
| VP8/VP9 | `vpx-rs` **0.2.1** ou `env-libvpx-sys` 5.1.3 (libvpx) | idem | `cpu-used=8..10`, `row-mt`, `tile-columns`. |
| H.265 | x265 via FFmpeg (licença GPL!) | FFmpeg `hevc` / libde265 | Evitar SW HEVC em distribuição comercial sem análise jurídica. |
| Todos | **`ffmpeg-the-third` 6.0.0** (FFmpeg 5+; tag "+ffmpeg-9.0") ou `ffmpeg-next` 9.0.0 | idem | FFmpeg completo traz GPL/LGPL + patentes; buildar **LGPL**, linkar dinamicamente, só com os codecs que quiser. |

### 4.2 Hardware (por API nativa — preferido)
| Plataforma | API | Crate Rust | Codecs (enc/dec típicos) |
|---|---|---|---|
| Windows | **Media Foundation** (MFT HW) + D3D11/D3D12 | `windows` **0.62** | H.264/H.265/AV1 (conforme GPU); entrada por textura D3D11 → zero-copy |
| Windows/Linux NVIDIA | **NVENC/NVDEC** (Video Codec SDK) | `nvidia-video-codec-sdk` **0.4.0** | H.264/H.265/AV1 (RTX 40+), LTR, intra refresh, ULL |
| Windows/Linux AMD | **AMF** | (sem crate maduro — bindgen ou FFmpeg `*_amf`) | H.264/H.265/AV1 (RDNA3+) |
| Intel | **oneVPL/QSV** | (bindgen ou FFmpeg `*_qsv`) | H.264/H.265/AV1/VP9 |
| Linux | **VA-API** | `libva` **0.1.4**, `cros-codecs` **0.0.6** (ChromeOS; encoders/decoders stateless VA-API e V4L2) | H.264/H.265/AV1/VP9 conforme driver (Mesa/intel-media) |
| Win/Linux/Android (Vulkan) | **Vulkan Video** | **`gpu-video` 0.4.0** (ex-`vk-video`, Software Mansion; integra com `wgpu`) | Hoje: H.264 enc+dec, H.265 **só enc**, AV1 em progresso. Linux (NVIDIA/AMD Mesa recente) e Windows. |
| macOS/iOS | **VideoToolbox** | `objc2-video-toolbox` **0.3.2** (+ `objc2-core-video`, `objc2-core-media`) | H.264/H.265 enc+dec (HEVC 4:2:2/4:4:4 em parte do Apple Silicon); AV1 **só decode** (M3/A17 Pro+); entrada `CVPixelBuffer`/`IOSurface` zero-copy |
| Android | **MediaCodec** (NDK `AMediaCodec`) | `ndk` **0.9.0** (`ndk::media::media_codec`) | H.264/H.265 universais; AV1/VP9 conforme SoC; entrada por `Surface`/`AHardwareBuffer` zero-copy |

**Estratégia de backends**: implementar a trait `VideoEncoder` por backend nativo; FFmpeg como backend "coringa" opcional atrás de feature flag (`--features ffmpeg`). Ordem de preferência resolvida em runtime por *probe* (ver §6).

### 4.3 PyroWave
- Repositório: <https://github.com/Themaister/pyrowave> (Hans-Kristian Arntzen / Themaister). **MIT.**
- Codec **intra-only** baseado em **DWT (wavelet)** — README cita CDF 9/7 (o post de design cita CDF 5/3 com 5 níveis); quantização mais agressiva em bandas de alta frequência; **sem entropy coding** (bit-planes crus) → rate control exato de passe único, CBR com teto rígido (fica ~10–20 bytes abaixo do alvo).
- Implementado em **compute shaders Vulkan**; **backend Metal** (`metal/pyrowave_metal.h`, Apple7+ — macOS e iOS) adicionado em 2026; build Android arm64 via NDK; build "Steam Runtime" (sniper).
- API C estável v1.0 (`pyrowave.h`): `pyrowave_create_device` / `pyrowave_create_default_device`, `pyrowave_image_create` (memória externa), `pyrowave_encoder_create`, `pyrowave_encoder_encode_gpu[_scaled]`, `pyrowave_encoder_encode_cpu`, `pyrowave_encoder_packetize[_with_padding]`, `pyrowave_encoder_compute_num_packets`, `pyrowave_decoder_create`, `pyrowave_decoder_push_packet`, `pyrowave_decoder_decode_gpu_buffer`, `pyrowave_decoder_decode_cpu_buffer_{synchronous,async,complete}`, `pyrowave_decoder_decode_is_ready`, `pyrowave_sync_object_*` (semáforos exportáveis p/ interop com outro device Vulkan — ex.: o do `wgpu`).
- Chroma: **4:2:0 e 4:4:4**.
- Números do autor: 1080p 4:2:0 ≈ **0,13 ms** de encode numa RX 9070 XT, decode < 100 µs; 4K ≈ 0,25 ms. Bitrate: **~100+ Mbit/s** mínimo prático; ~200 Mbit/s a 60 fps ≈ 1,5 bpp com artefatos difíceis de ver. Raw 1080p60 4:2:0 ≈ 1,5 Gbit/s.
- **Limitações**: inviável na internet comum (só P2P em fibra), cap de PSNR por usar FP16 na DWT, comparação limitada pelo autor. Valve adotou o PyroWave experimentalmente no **Steam Remote Play** (beta do cliente Steam em 2026).
- Integração Rust: **criar `pyrowave-sys`** (bindgen sobre `pyrowave.h` + build CMake vendorizado com o checkout parcial do Granite) e um wrapper seguro `pyrowave` no workspace. Usar o **mesmo `VkDevice` do wgpu** (via `wgpu-hal` + `pyrowave_create_device` com handles externos) ou dois devices com `sync_object` exportado para zero-copy.
- Política no produto: habilitar só se (a) ambos têm GPU compatível, (b) caminho **direto** na tailnet (não DERP), (c) banda medida ≥ alvo + 30% de folga, (d) RTT < ~10 ms. Senão cair para AV1/H.265/H.264.

## 5. Conversão de cor e escala

- Captura entrega BGRA (Windows/macOS/X11) ou dmabuf (Wayland/PipeWire) ou YUV (Android).
- **Converter na GPU** (compute/fragment shader wgpu, ou VideoProcessor do D3D11, ou VTPixelTransfer) direto para NV12/P010/I444 → encoder HW sem cópia para CPU.
- Caminho CPU (fallback): `yuv` **0.8.19** (Rust puro, SIMD) ou `dcv-color-primitives` 1.0; escala com `fast_image_resize` 6.1.
- Receptor: decoder HW → textura (NV12) → shader YUV→RGB em wgpu (matriz BT.709/BT.601, limited/full) → superfície. Evitar download para CPU.

## 6. Negociação e fallback (algoritmo)

```text
1. Cada lado executa probe_codecs() no startup (com cache em disco, invalidado por driver/OS):
   para cada backend: tenta criar encoder/decoder 1280x720 e (enc) 1 frame de teste em processo
   isolado ou thread com timeout → Capabilities { codec, dir, hw, max_res, chroma444, ltr, intra_refresh, input_texture }
2. Controlador envia Hello{ decode_caps, preferences(user), display_info_wanted }.
3. Host calcula candidatos = encode_caps ∩ decode_caps(de TODOS os viewers do display).
4. Pontua: score = base(codec) + hw_bonus + 444_bonus(se pedido) − custo_rede(path=DERP?) ...
   - Se path direto && banda ≥ PyroWave_target && ambos pyrowave → PyroWave
   - Senão AV1(hw) > HEVC(hw) > H264(hw) > VP9(sw) > H264(sw/openh264) > VP8(sw)
5. Host envia CodecSelected{ codec, profile, chroma, w, h, fps_target, bitrate_target }.
6. Em erro de encoder em runtime → marca backend como quebrado (persistido), recalcula, envia novo
   CodecSelected + keyframe. O receptor recria decoder ao ver mudança de codec/resolução.
```

**Implementado hoje** (`taildesk-codec::selecao`): o controlador anuncia H.264 sempre e HEVC/AV1
só com decoder de hardware (NVDEC no Linux; Media Foundation com D3D11 no Windows). O host
escolhe AV1 > HEVC > H.264 entre os encoders de hardware que tem (NVENC: H.264/HEVC, AV1 a
partir da Ada; VA-API: H.264), com openh264 por último no H.264. Sem probe em disco: cada
backend testa um quadro ao abrir. `TAILDESK_CODEC`, `TAILDESK_ENCODER` e `TAILDESK_DECODER`
forçam a escolha para diagnóstico. PyroWave (`taildesk-codec::pyrowave`, biblioteca MIT aberta em tempo de
execução, empacotada pelo `packaging/pyrowave/build.sh`) é anunciado quando a biblioteca e o
Vulkan abrem, mas só é escolhido quando pedido (escolha manual no controlador ou
`TAILDESK_CODEC=pyrowave`): ele pede centenas de Mbit/s.

## 7. Recuperação de erros

| Evento | Ação |
|---|---|
| Perda de datagrama de mídia (codecs inter) | Receptor espera *jitter* curto (≈ 1 RTT) por retransmissão (NACK seletivo se RTT < 30 ms); se não chegar: descarta frame, envia `PictureLoss{last_good_seq}` → host usa LTR/intra-refresh/IDR. |
| Perda (PyroWave) | Nada: bloco zerado = borrão local que some no próximo frame. Não pedir retransmissão. |
| Congestionamento | Controle por QUIC (BBR) + medida de *queue delay*; reduzir bitrate do encoder **antes** da fila crescer; descartar frames antigos no sender (nunca enfileirar vídeo velho). |
| Novo viewer | Keyframe (ou refresh completo) só para o encoder do display em questão. |

## 8. Áudio (complemento)

- **Opus** (crate `opus` **0.4.0**) a 48 kHz, frames de 10 ms (ou 5 ms em modo LAN), FEC in-band + DTX. Playback e microfone: `cpal` **0.18.2**. Captura do **áudio do sistema** depende de API específica por SO (verifique o que o `cpal` da versão escolhida já cobre; o RustDesk mantém fork próprio justamente por isso): WASAPI loopback no Windows; ScreenCaptureKit no macOS 13+; monitor do PipeWire/Pulse no Linux; `AudioPlaybackCapture` no Android 10+; iOS só via ReplayKit no modo broadcast.
