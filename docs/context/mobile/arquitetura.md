# TailDesk Mobile — Arquitetura proposta

> Complementa `requisitos.md`. Proposta para quando o mobile for executado; **nada disto está no
> repositório ainda**. Mudanças em decisões travadas do `AGENTS.md` estão listadas na §8 como
> ADRs a abrir em `08-roadmap.md` na hora de executar.

## 1. Princípio: um núcleo, cascas finas

```
            ┌────────────── app Android (Gradle) ──────────────┐   ┌──────── app iOS (Xcode) ────────┐
            │ Kotlin mínimo: MainActivity (android-activity),  │   │ Swift mínimo: AppDelegate,       │
            │ ForegroundService, MediaProjection (M4),         │   │ Share Extension (M3),            │
            │ AccessibilityService (M4), Share target, SAF     │   │ document picker, AVAudioSession  │
            └───────────────┬──────────────────────────────────┘   └──────────────┬──────────────────┘
                            │ JNI (jni/ndk) ou uniffi                               │ objc2 ou uniffi
            ┌───────────────▼────────────────────────────────────────────────────────▼──────────────┐
            │ taildesk-mobile (crate nova, cdylib/staticlib): ponto de entrada, ciclo de vida,      │
            │ eventos de toque → InputEvent, faixa de teclas, estado de rede do SO                  │
            ├───────────────────────────────────────────────────────────────────────────────────────┤
            │ taildesk-ui (Slint, telas mobile em ui/mobile/*.slint) + taildesk-render (wgpu)       │
            ├───────────────────────────────────────────────────────────────────────────────────────┤
            │ REUSO SEM MUDANÇA: taildesk-proto · taildesk-transport (quinn) · taildesk-session ·    │
            │ taildesk-keyboard (modo caractere/atalho) · taildesk-files · adaptador de rede (14)   │
            ├───────────────────────────────────────────────────────────────────────────────────────┤
            │ taildesk-overlay::MobileVpnOverlay   │ taildesk-codec: MediaCodec / VideoToolbox       │
            │ taildesk-platform: android/, ios/    │ (+ openh264 fallback só no Android)             │
            └───────────────────────────────────────────────────────────────────────────────────────┘
```

Regra 8 do `AGENTS.md` continua: código de SO só em `taildesk-platform`, backends de codec,
interop do render e overlay. `taildesk-mobile` é a "casca Rust" (equivalente ao binário
`taildesk` do desktop), não um lugar para lógica de SO.

## 2. Workspace (quando executar)

```
crates/
  taildesk-overlay/src/mobile.rs       # MobileVpnOverlay (§3)
  taildesk-platform/src/android/       # estado de rede, clipboard, MediaProjection, Accessibility (M4), áudio
  taildesk-platform/src/ios/           # estado de rede (getifaddrs/NWPathMonitor), clipboard, áudio
  taildesk-codec/src/mediacodec.rs     # AMediaCodec (ndk 0.9) → AHardwareBuffer
  taildesk-codec/src/videotoolbox.rs   # VTDecompressionSession (objc2-video-toolbox) → CVPixelBuffer
  taildesk-render/src/interop/         # AHardwareBuffer→Vulkan, CVPixelBuffer→Metal (zero-copy)
  taildesk-ui/ui/mobile/               # telas .slint do celular/tablet
  taildesk-mobile/                     # crate da casca mobile (cdylib Android, staticlib iOS)
apps/
  android/                             # Gradle + cargo-ndk; Kotlin mínimo
  ios/                                 # xcodegen (project.yml) + Swift mínimo; extensão de compartilhar
```

Targets: `aarch64-linux-android` (+ `x86_64-linux-android` p/ emulador), `aarch64-apple-ios`,
`aarch64-apple-ios-sim`. Ferramentas: `cargo-ndk`, `xcodegen` (o guia iOS do Slint usa).

CI (quando executar): job Android em `ubuntu-latest` (NDK + `cargo ndk build` + `gradlew
assembleDebug` + testes JVM), job iOS em `macos-latest` (build para simulador + testes Rust no
host). Os dois **não entram no `cargo xtask check` padrão** para não deixar o desktop lento;
ganham `cargo xtask mobile-check`. A pipeline de release do desktop (`release.yml`) não muda até a
M1 terminar; depois ganha um job que anexa o APK assinado.

## 3. `MobileVpnOverlay`

Implementa a trait `Overlay` sem LocalAPI:

| Método da trait | Desktop (`SystemOverlay`) | Mobile (`MobileVpnOverlay`) |
|---|---|---|
| estado / pronto | LocalAPI `status` | Interface com IP 100.x + DNS 100.100.100.100 responde (`requisitos.md` §4.3) |
| peers | `status.Peer` | `PeerList` do desktop pareado + favoritos + manual (não autoritativo: só sugestão) |
| `identify(addr)` | `whois` | `{ ip, name: PTR em 100.100.100.100 (validar), user: None, os: None }` |
| caminho direto/relay | `ping` disco | `None` local; usa `PathInfo` do host (§4) só para exibição e como pista do adaptador |
| socket | UDP do SO no IP da tailnet | UDP do SO (`0.0.0.0`), o túnel do Tailscale roteia |

Pontos de atenção:
- `identify()` no controlador serve para **mostrar** quem é o host e para fixar a chave; quem
  **autoriza** a sessão é o host desktop, que tem `whois` completo. Isso mantém a regra 3.
- Se o PTR do MagicDNS não responder, `name` = o nome que o usuário escolheu/pareou para aquele IP.
- MTU: o túnel do Tailscale costuma ter MTU 1280; datagramas sempre ≤ `max_datagram_size()`.

## 4. Mudanças de protocolo propostas (para `07-protocolo.md`, todas com round-trip)

| Mensagem | Direção | Para quê | Fase |
|---|---|---|---|
| `Hello.client_kind = DESKTOP \| PHONE \| TABLET` e `Hello.viewport{w_px, h_px, dpi}` | C → H | Host limita resolução/encoder ao tamanho útil (MF-82); diálogo mostra "celular" | M1 (pode ser reservado já na fase 1 do desktop, campo opcional) |
| `PathInfo{direct: bool, relay_region?, rtt_ms}` | H → C | Host mede pelo LocalAPI e informa; controlador mobile exibe e usa como pista | M1 |
| `PeerListRequest` / `PeerList{entries[name, dns, ip, os, online, has_taildesk]}` (≤ 256 entradas, `name` ≤ 128 B) | C → H / H → C | Lista de dispositivos no celular (MF-11); host só responde ao mesmo usuário (whois) | M1 |
| `PairRequest{code}` / `PairResult{ok, cert_fingerprint}` | C → H | Pareamento por QR/código (MF-20/21) | M1 |
| `Touch{id, phase, x, y, pressure}` (já existe em `07`) | C → H | Só para host Android (M4); no host desktop o celular manda mouse | M4 |
| `NavAction{BACK \| HOME \| RECENTS \| NOTIFICATIONS}` | C → H | Botões de navegação do host Android | M4 |
| `KeyboardInfo.source = VIRTUAL \| HARDWARE` | C → H | Teclado virtual não tem layout físico: tudo imprimível vai como `KeyChar` | M1 |

Nada disso muda o header de 16 bytes nem os streams. O teste de conexão (`14` §7) passa a
aceitar peers mobile sem mudança de fio.

## 5. Mídia no celular

### 5.1 Decode
- **Android**: `AMediaCodec` (crate `ndk` 0.9) com saída em `AImageReader` → `AHardwareBuffer`
  → import Vulkan no wgpu (`VK_ANDROID_external_memory_android_hardware_buffer`), zero-copy
  (`04` §3). Fallback: openh264 → upload NV12 (já existe no desktop). Probe na primeira execução.
- **iOS**: `VTDecompressionSession` → `CVPixelBuffer` (IOSurface) → `CVMetalTextureCache` →
  textura Metal no wgpu. **Sem openh264 no iOS** (binário da Cisco não pode ser baixado por app da
  App Store e o decoder do sistema sempre tem H.264).
- Um decoder por vez no celular (um monitor); tablet até 2, conforme probe.
- Configuração de baixa latência: sem B-frames já é regra do host; no MediaCodec pedir
  `KEY_LOW_LATENCY` (Android 11+); no VT, `kVTDecodeFrame_EnableAsynchronousDecompression` desligado
  e saída imediata **(validar)**.

### 5.2 Render
- wgpu: Vulkan no Android (GLES fallback em aparelhos antigos), Metal no iOS.
- Zoom/pan local (pinça) é só transformação na GPU; não pede nada ao host.
- Cursor do host desenhado localmente a partir de `CursorShape`/posição (já no protocolo), maior
  no modo touchpad.

### 5.3 Áudio
- Opus decode (crate `opus`) + `cpal` (AAudio no Android, CoreAudio no iOS). Jitter buffer igual ao
  desktop. `AVAudioSession` categoria `.playback` só quando o som estiver ligado.

## 6. Toque → eventos

| Gesto (modo touchpad) | Evento enviado |
|---|---|
| Arrastar 1 dedo | `MouseMove{display_id, rel}` com aceleração local |
| Toque | `MouseButton{left, down}` + `up` |
| Toque com 2 dedos | botão direito |
| Toque com 3 dedos | botão do meio |
| Tocar-segurar-arrastar | left down → moves → up |
| Arrastar 2 dedos | `Scroll{dx, dy, precise=true}` |
| Pinça | zoom **local** (não envia nada) |
| Deslizar 3 dedos horizontal | trocar monitor (`StopVideo` + `StartVideo`, `13` §3) |

Modo toque direto: posição absoluta `MouseMove{display_id, x, y}` normalizada no monitor,
considerando zoom/pan local. A máquina de estados dos gestos vive em `taildesk-mobile` e tem testes
com sequências gravadas de toques.

## 7. Host Android (M4)

- **Captura**: Activity de consentimento (Kotlin, ~50 linhas) → `MediaProjection` →
  `VirtualDisplay` desenhando direto na `Surface` de entrada do `MediaCodec` encoder (zero-copy).
  Foreground Service tipo `mediaProjection`. Android 14+: consentimento a cada sessão (regra do SO).
- **Input**: `AccessibilityService` com `dispatchGesture` (toque/arrasto/scroll) e
  `performGlobalAction` (Voltar/Início/Recentes). Texto: `ACTION_SET_TEXT` no nó focado ou colar.
  O mouse do desktop vira gestos; o host anuncia `can_inject_input=false` se o serviço estiver
  desligado (modo só ver).
- **Áudio**: `AudioPlaybackCapture` (Android 10+) via JNI, só quando `Permissions.audio` +
  `StartAudio` aceito (regra do `15-audio.md`).
- **Consentimento**: o celular não tem `whois`, então o host Android **não consegue saber o
  usuário** do controlador; mostra IP 100.x + nome do PTR + nome do `Hello` marcado como "dito
  pelo outro lado". Por isso o host Android exige pareamento prévio (QR mostrado no celular e lido
  pelo desktop, ou código) além do consentimento a cada sessão. **(decisão a confirmar com o Yuri)**
- **Teclado**: o host Android não tem layout físico relevante para texto; recebe `KeyChar`.

## 8. ADRs a abrir quando o mobile for executado

| ADR | Decisão | Por quê |
|---|---|---|
| ADR-M1 | Overlay mobile sem LocalAPI (`MobileVpnOverlay`), identidade no controlador = IP 100.x + MagicDNS; autorização sempre no host | O app do Tailscale não expõe LocalAPI a outros apps |
| ADR-M2 | Lista de dispositivos vinda de desktop pareado (`PeerList`) | Sem `status` no celular; token da API é opcional |
| ADR-M3 | Decode só por HW do sistema no iOS (sem openh264) | App Store e patentes; bateria |
| ADR-M4 | Casca nativa mínima (Kotlin/Swift) + núcleo Rust; Slint se passar no gate M0 | Slint iOS ainda tech preview |
| ADR-M5 | Host Android exige pareamento prévio | Sem `whois` no celular (regra 3) |
| ADR-M6 | Build Play só controlador até a revisão de Accessibility aprovar | Política do Google Play |

## 9. Dependências novas previstas (licenças conferidas)

| Crate | Uso | Licença |
|---|---|---|
| `android-activity` 0.6 | Ciclo de vida Android | MIT/Apache-2.0 |
| `jni` 0.22, `ndk` 0.9 | JNI, MediaCodec, AHardwareBuffer | MIT/Apache-2.0 |
| `objc2`, `objc2-video-toolbox`, `objc2-core-video`, `objc2-ui-kit` | iOS | MIT (objc2 é MIT/Zlib/Apache conforme crate) |
| `uniffi` 0.32 (só se a casca nativa for o caminho) | Bindings Kotlin/Swift | MPL-2.0 (permitido no `AGENTS.md`) |
| `security-framework` | Keychain | MIT/Apache-2.0 |

Versões a reconferir no crates.io no dia de executar (as acima vêm do pacote `04`/`05`).
