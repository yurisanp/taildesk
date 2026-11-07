# TailDesk Android — Detalhamento do app e da publicação

> Escrito em 03/11/2026 a pedido do Yuri: "detalhe bem o app Android que iremos construir; ele deverá
> seguir o mesmo modelo de subida do babylock". Complementa `requisitos.md` (IDs `MF-*`/`MNF-*`),
> `arquitetura.md` e `plano.md` desta pasta, e os mockups em `telas/`. O modelo de publicação foi
> tirado do repositório `yurisanp/babylock-android` (workflows, `docs/PLAY_STORE.md`,
> `app/build.gradle.kts`, `fastlane/`, `distribution/whatsnew/`), lido em 03/11/2026.
> O que precisa ser medido ou conferido está marcado **(validar)**.

## 0. Estado da implementação (03/11/2026, #104)

| PR | Estado | Onde |
|---|---|---|
| A1 | ✅ | `crates/taildesk-mobile`, `apps/android`, `cargo xtask mobile-check`, `.github/workflows/android.yml` |
| A2 (S1) | 🟡 ferramenta pronta, medir | modo `s1` do app; `m0-resultados.md` |
| A3 (S3/S4/S5) | 🟡 lógica testada, medir | `taildesk_overlay::mobile`, teste de conexão com `bench-net`; `m0-resultados.md` |
| A4 (S6) | 🟡 decoder com uma cópia; zero-copy pendente | `taildesk_codec::mediacodec`, `crates/taildesk-mobile/src/video.rs` |
| A5 | ✅ | `taildesk-proto` (`client_kind`, `viewport`, `KeyboardInfo.source`, tags 120–124, `pair`), `07-protocolo.md` §3.4; o host limita o vídeo ao viewport |
| A6 | ✅ | `taildesk_session::pareamento`, `conexao::servico`, Configurações → Celulares no desktop |
| A7 | ✅ (roteiro em aparelho pendente) | `MobileVpnOverlay`, onboarding, dispositivos, pareamento |
| A8 | ✅ | `gestos.rs`, `zoom.rs`, `controle.rs`, tela da sessão |
| A9 | ✅ | `taildesk_keyboard::virtual_kb` + casos na matriz ("ação", Ctrl+C) |
| A10 | ✅ | monitores (◀ ▶ e lista), HUD, `ciclo.rs`, `rede.rs` |
| A11 | ✅ (conta da Play pendente) | `android-release.yml`, `android-pages.yml`, `fastlane/`, `distribution/whatsnew`, `site/`, `marca/`, `PLAY_STORE.md`, modo demonstração |

Desvios do que está escrito abaixo, decididos na implementação:
- **NativeActivity, não GameActivity** (ADR-M4): o backend Android do Slint 1.18 usa a
  NativeActivity (e renderer Skia). A `MainActivity` estende `NativeActivity`.
- **Telas em `crates/taildesk-mobile/ui/`**, não em `taildesk-ui/ui/mobile/`: o celular usa outras
  features do Slint (backend Android, estilo Material) e não pode herdar as do desktop.
- **Multitoque**: o Slint entrega só o primeiro dedo à `TouchArea`; rolagem, pinça e toque com dois
  dedos vêm do `ScaleRotateGestureHandler`. A troca de monitor fica nos botões ◀ ▶ da barra (o
  §6 já permitia); a máquina de gestos aceita três dedos para quando houver multitoque completo.
- **Capturas da Play** saem das mesmas telas Slint desenhadas pelo software renderer
  (`examples/capturas.rs`, job `capturas`), em pt-BR e en-US; o job do emulador também guarda
  capturas do app rodando (lista de exemplo e demonstração).
- **Ponta a ponta no emulador** usa o modo demonstração; o host de teste com `LanOverlay` do §8
  fica para quando o `LanOverlay` existir (hoje só há o `SystemOverlay`).
- **openh264 e Opus** são features opcionais (ADR-M3): o build do Android não leva nenhum dos dois.
- **Pareamento sem câmera**: nome do computador + código, com o certificado do primeiro contato
  conferido contra o `PairResult` (`conexao::servico::parear_com_codigo`).

## 1. Resumo

- **O que é**: app Android que **controla** os computadores Windows e Linux da tailnet (ver a tela,
  tocar, digitar com tradução de layout, trocar de monitor; depois arquivos e som). Ser controlado
  (host Android) vem depois, na M4.
- **Rede**: exige o app oficial do Tailscale instalado e conectado no celular. Nada embutido
  (ADR-009). Sem LocalAPI no Android, então a lista de computadores vem de um **desktop TailDesk
  pareado por QR**.
- **Código**: mesmo núcleo Rust do desktop (`taildesk-proto`, `-transport`, `-session`,
  `-keyboard`, `-files`), UI Slint + vídeo wgpu, casca Kotlin mínima. Fica **no mesmo repositório**
  (`apps/android`), não num repositório separado como o babylock, porque o núcleo é compartilhado.
- **Publicação**: igual ao babylock. CI compila a cada push e guarda o APK de teste; o workflow
  "Versão para a Play" gera o `.aab` assinado por tag ou pelo botão Run workflow e, com a conta de
  serviço, sobe sozinho para o teste interno como rascunho. Ficha da loja em `fastlane/metadata`,
  novidades em `distribution/whatsnew`, capturas tiradas pelo CI no emulador, política de
  privacidade no GitHub Pages e um `PLAY_STORE.md` com o que falta fazer na conta.
- **Decisões do Yuri (03/11/2026)**: pacote `com.santech.taildesk`; APK assinado para baixar no
  GitHub Releases em toda versão; host Android na Play só depois da aprovação do Google.
- **Diferenças para o babylock**: sem anúncios e sem compras (projeto open-source, sem telemetria);
  dois idiomas (pt-BR e en, regra do projeto) em vez de três; tag `android-v*` para não disparar o
  release do desktop; APK assinado também no GitHub Releases.

## 2. O modelo de publicação do babylock (o que vamos copiar)

| Peça no babylock | O que faz | No TailDesk |
|---|---|---|
| `.github/workflows/android.yml` | A cada push na `main` e em PR: `assembleDebug`, `bundleRelease assembleRelease` sem assinatura (R8 + lint de release), testes JVM (Robolectric), Kover, lint, resumo dos testes no `GITHUB_STEP_SUMMARY`, artefato `babylock-debug-apk`. Job de emulador (API 30 e 34, KVM liberado, `reactivecircus/android-emulator-runner@v2`). Job de capturas da Play em 3 idiomas | `.github/workflows/android.yml` com os mesmos jobs, mais a compilação Rust com `cargo-ndk`. Roda só quando mudam `apps/android/**`, `crates/**`, `Cargo.*` (filtro `paths`) |
| `.github/workflows/release.yml` "Versão para a Play" | Gatilho: tag `v*` ou Run workflow com entrada `track` (internal, alpha, beta, production). `versionCode = github.run_number`. Keystore vem de `BABYLOCK_KEYSTORE_BASE64` decodificado em `$RUNNER_TEMP`. Roda testes e `bundleRelease`. Avisa se não tem keystore. Artefato com `.aab` + `mapping.txt`. Envio pela `r0adkll/upload-google-play@v1` com `PLAY_SERVICE_ACCOUNT_JSON`, `status: draft`, `whatsNewDirectory: distribution/whatsnew` | `.github/workflows/android-release.yml` igual, com tag **`android-v*`** (a tag `v*` já é do desktop), secrets `TAILDESK_ANDROID_*` e um passo a mais que anexa o APK assinado numa release do GitHub |
| `.github/workflows/pages.yml` | Publica `docs/` no GitHub Pages (política de privacidade em 3 idiomas) | Publica só `apps/android/site/` (a pasta `docs/` do TailDesk tem os documentos de contexto, que não devem virar site) |
| `app/build.gradle.kts` | Assinatura por variáveis de ambiente ou `keystore.properties` local fora do git; `versionCode` do ambiente; `minifyEnabled` + `shrinkResources`; `ndk.debugSymbolLevel = SYMBOL_TABLE`; `generateLocaleConfig`; `language.enableSplit = false`; lint com `abortOnError` | Igual. Os símbolos nativos aqui importam mais (a lib principal é Rust) |
| `fastlane/metadata/android/<idioma>/` | Título, descrição curta e completa, changelog, ícone, gráfico de destaque e 5 capturas por idioma | `apps/android/fastlane/metadata/android/{pt-BR,en-US}/` (também serve ao F-Droid se um dia entrar) |
| `distribution/whatsnew/whatsnew-<idioma>` | Novidades da versão (até 500 caracteres) | `apps/android/distribution/whatsnew/` |
| `.github/scripts/play_screenshots.sh` + teste `PlayScreenshots` | Troca o idioma do emulador, roda o teste que tira as capturas, junta tudo no artefato `capturas-play` | Igual, com o app em **modo demonstração** (§8.4) para ter uma sessão "de verdade" na captura |
| `.github/scripts/test_summary.py` | Resumo dos resultados JUnit no Actions | Copiar (é do próprio Yuri; conferir licença do arquivo antes) |
| `docs/PLAY_STORE.md` | O que já está pronto, o que precisa das mãos do Yuri (em ordem), respostas para os formulários do Play Console, o que fazer antes de cada versão | `apps/android/PLAY_STORE.md` com o mesmo formato; rascunho das respostas no §11 abaixo |
| `docs/marca/` | Ícone 512, banner 1024×500, capturas com título | `apps/android/marca/` a partir do ícone do desktop |

O que o babylock tem e o TailDesk **não** terá: AdMob, UMP, Play Billing, IDs de anúncio no Gradle,
`app-ads.txt`. F-Droid: o babylock não publica lá; o TailDesk também não no começo (§12).

## 3. Escopo do Android por fase

| Fase | Entrega no Android | Requisitos |
|---|---|---|
| **M0** spikes (2–3 semanas) | S1 Slint + wgpu a 60 fps; S3 QUIC pela VPN do Tailscale; S4 detecção dos estados do Tailscale; S5 PTR do MagicDNS; S6 MediaCodec zero-copy; S7 inbound (pré-requisito do M4) | `plano.md` M0 |
| **M1** controlador (4–6 semanas) | Onboarding, dispositivos, pareamento QR, conectar, sessão (touchpad e toque direto), faixa de teclas + teclado virtual, troca de monitor básica, HUD, ciclo de vida, rede tarifada. Publicação na Play (teste interno → fechado) | MF-01..03, 10..14, 20..22, 30, 40..44, 46..50, 80..82 |
| **M3** paridade (4–6 semanas) | Arquivos (folha + SAF), "Compartilhar com TailDesk", áudio, teclado e mouse físicos, tablet com 2 monitores, Teste de conexão completo, clipboard de texto. Produção na Play | MF-45, 47, 60..64, 70..72, 83, 100 |
| **M4** host (5–7 semanas) | MediaProjection + MediaCodec encoder, AccessibilityService, AudioPlaybackCapture, notificação permanente, pareamento obrigatório. Na Play só depois de aprovada a declaração de Accessibility | MF-90..95 |

## 4. Arquitetura concreta

### 4.1 Pastas no repositório

```
crates/
  taildesk-mobile/            # cdylib "taildesk_mobile" (libtaildesk_mobile.so): entrada android_main,
                              # ciclo de vida, gestos → InputEvent, faixa de teclas, ponte JNI
  taildesk-overlay/src/mobile.rs     # MobileVpnOverlay (arquitetura.md §3)
  taildesk-platform/src/android/     # rede (ConnectivityManager via JNI), clipboard, notificações
  taildesk-codec/src/mediacodec.rs   # AMediaCodec → AImageReader → AHardwareBuffer
  taildesk-render/src/interop/ahb.rs # AHardwareBuffer → VkImage (zero-copy)
  taildesk-ui/ui/mobile/*.slint      # telas do celular e do tablet
apps/android/
  settings.gradle.kts, build.gradle.kts, gradle/libs.versions.toml, gradlew
  app/build.gradle.kts        # chama cargo-ndk antes do mergeJniLibs
  app/src/main/AndroidManifest.xml
  app/src/main/java/com/santech/taildesk/   # Kotlin mínimo (§4.3)
  app/src/test/               # Robolectric (casca Kotlin)
  app/src/androidTest/        # emulador: ponta a ponta + PlayScreenshots
  fastlane/metadata/android/{pt-BR,en-US}/
  distribution/whatsnew/
  site/privacidade/index.html # GitHub Pages
  marca/
  PLAY_STORE.md
.github/workflows/android.yml, android-release.yml, android-pages.yml
xtask: `cargo xtask mobile-check` (fmt + clippy dos targets Android + testes host de taildesk-mobile)
```

### 4.2 Versões e alvos (iguais ao babylock onde fizer sentido)

| Item | Valor | Por quê |
|---|---|---|
| `minSdk` | 29 (Android 10) | MNF-05; `AudioPlaybackCapture` do M4 é API 29 |
| `compileSdk` / `targetSdk` | 36 | Igual ao babylock; a Play exige target recente a cada agosto |
| JDK / AGP / Kotlin | 17 / 8.11.1 / 2.2.0 | Mesmo `libs.versions.toml` do babylock como base (conferir versões no dia) |
| NDK | r28 ou mais novo **(validar)** | Alinha bibliotecas nativas em 16 KB por padrão, exigido pela Play para apps com target 35+ |
| ABIs | `arm64-v8a` (loja), `x86_64` (emulador do CI) | MNF-06: APK ≤ 40 MB por ABI; o `.aab` entrega só a ABI do aparelho |
| Rust | 1.97 (toolchain do repo), targets `aarch64-linux-android`, `x86_64-linux-android` | `rust-toolchain.toml` ganha os targets |
| Slint | 1.18.1 (o do desktop), backend Android (`android-activity`) | Mesma versão do workspace |
| wgpu | 30 (Vulkan; GLES de reserva em aparelho antigo) | Mesma versão do desktop |

Build Rust dentro do Gradle: uma task `cargoBuild` roda
`cargo ndk -t arm64-v8a -t x86_64 -o app/src/main/jniLibs build --release -p taildesk-mobile`
antes de `mergeJniLibFolders`. Em debug, perfil `dev` com `CARGO_PROFILE_DEV_OPT_LEVEL=1` (otimizado nível 1, para o vídeo
não ficar inutilizável no emulador). Os `.so` vão sem strip para o AGP extrair os símbolos para a
Play (`debugSymbolLevel`).

### 4.3 Casca Kotlin (mínima, como manda a regra 8 do `AGENTS.md`)

| Classe | Papel |
|---|---|
| `MainActivity : GameActivity` | Hospeda o Slint (via `android-activity`), repassa insets, teclado do sistema e IME |
| `TailscaleState` | Lê `PackageManager` (`com.tailscale.ipn`, com `<queries>`), `ConnectivityManager` (rede `TRANSPORT_VPN`, interfaces 100.x), `isActiveNetworkMetered`; entrega ao Rust por JNI a cada mudança |
| `OpenTailscale` | `getLaunchIntentForPackage("com.tailscale.ipn")`, ou Play Store se não instalado |
| `QrScanActivity` | CameraX + decodificação em Rust (`rqrr`, MIT/Apache) para não depender do ML Kit, que é proprietário |
| `SessionService` (M3) | Foreground Service `mediaPlayback` para o som continuar em segundo plano (MF-72) |
| `ShareReceiverActivity` (M3) | Alvo de "Compartilhar com TailDesk" (MF-63) |
| `SafBridge` (M3) | `ACTION_OPEN_DOCUMENT` e gravação em `Download/TailDesk` pelo MediaStore |
| `TecladoFisico` (M3) | Teclado USB/Bluetooth na sessão: recebe em `dispatchKeyEvent` o que o Slint não aceitou (o backend dele só tem o mapa americano), aplica o layout do aparelho (`getUnicodeChar`), resolve teclas mortas (`getDeadChar`) e trata AltGr como texto; manda ao Rust por `Ponte.nativeTeclaFisica` (`taildesk_mobile::fisico`). Tab e Esc ficam com um `FocusScope` da sessão no Slint, que os comeria na navegação de foco |
| `HostProjectionActivity`, `HostAccessibilityService` (M4) | Consentimento do MediaProjection e injeção por `dispatchGesture` |

Todo o resto (estado das telas, lista de dispositivos, sessão, gestos, teclado) é Rust/Slint e tem
teste em Rust.

**Plano B do S1** (Slint não segura 55 fps ou o IME): casca em Jetpack Compose + Material 3
(como o babylock) com o núcleo Rust via `uniffi` e o vídeo num `SurfaceView` desenhado pelo wgpu.
Os testes de tela passariam a ser Compose + Robolectric, exatamente como no babylock.

### 4.4 Manifesto (M1)

| Permissão | Para quê | Observação para a Play |
|---|---|---|
| `INTERNET`, `ACCESS_NETWORK_STATE` | QUIC e detecção da VPN | — |
| `CAMERA` (opcional, `required=false`) | Ler o QR do pareamento | Pedida só na tela do QR (MNF-10) |
| `POST_NOTIFICATIONS` | Aviso de sessão e de transferência | Android 13+ |
| `<queries><package android:name="com.tailscale.ipn"/></queries>` | Saber se o Tailscale está instalado | Evita `QUERY_ALL_PACKAGES`, que a Play restringe |
| `FOREGROUND_SERVICE`, `FOREGROUND_SERVICE_MEDIA_PLAYBACK` (M3) | Som em segundo plano | Formulário de serviço em primeiro plano + vídeo |
| `FOREGROUND_SERVICE_MEDIA_PROJECTION`, `BIND_ACCESSIBILITY_SERVICE` (M4) | Host | Declaração de Accessibility + vídeo (como o babylock fez) |

Exceção de bateria: abrir a tela do sistema (`ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS`) com
explicação, sem declarar `REQUEST_IGNORE_BATTERY_OPTIMIZATIONS` (a Play só aceita essa permissão
em casos listados).

## 5. Tailscale no Android (como o app se comporta)

Resumo do `requisitos.md` §4, já no formato de implementação:

1. Ao abrir e a cada `NetworkCallback`, `TailscaleState` classifica: **não instalado**, **VPN
   desligada**, **outra VPN ativa**, **TailDesk fora do túnel** (split tunneling por app),
   **conectado**. Cada estado tem tela e botão próprios (mockups 01 e 18).
2. Conectado = interface com IP 100.x **e** consulta DNS para `100.100.100.100` respondendo do
   processo do TailDesk.
3. Versão do Tailscale lida pelo `versionName`; abaixo de 1.102 mostra aviso, não bloqueia.
4. Identidade do host = IP 100.x da conexão + nome do PTR no MagicDNS (S5); o nome que o host diz no
   `Hello` é só exibição. Certificado do host fixado no pareamento (QR traz a impressão digital) ou
   na primeira conexão (TOFU), com aviso se mudar.
5. A autorização é sempre do host desktop, pelo `whois` do IP do celular (regra 3).
6. Datagramas sempre ≤ `max_datagram_size()`; o túnel móvel costuma ter MTU 1280 (medir no S3).

## 6. Telas (mockups em `telas/`)

| Mockup | Tela | Comportamento principal |
|---|---|---|
| 01 `onboarding-tailscale` | Primeira execução, passo 1 | Estado do Tailscale e botão "Abrir o Tailscale" / "Instalar" |
| 02 `encontrar-computadores` | Passo 2 | "Parear com um computador" (QR ou código de 6 caracteres) ou "Digitar nome" |
| 03 `pareamento-qr` | Câmera | Lê o QR do desktop (nome, IP, porta, impressão digital, código de uso único de 5 min) |
| 04 `dispositivos` | Início | Lista com favoritos, recentes, estados (online com/sem TailDesk, offline), busca, puxar para atualizar |
| 05 `detalhe` | Cartão do computador | Conectar, Testar conexão, Enviar arquivo (M3), Copiar endereço, Favoritar, Esquecer |
| 06 `conectando` | Etapas | "Encontrando…" → "Esperando aceitar…" → sessão; Cancelar; timeout de 60 s explicado |
| 07 `sessao-touchpad` | Sessão | Touchpad padrão no celular; barra recolhível com monitor, teclado, arquivos, som, modo, HUD, Encerrar |
| 08 `sessao-teclado` | Sessão com teclado | Faixa Esc/Tab/Ctrl/Alt/Super/Shift/setas/F1–F12/Ctrl+Alt+Del com travas de um toque |
| 09 `monitores` | Troca de monitor | Lista, ◀ ▶ e deslizar com 3 dedos; um monitor por vez no celular |
| 10 `arquivos` (M3) | Folha de arquivos | Navegar raízes do host, baixar, enviar |
| 11 `compartilhar` (M3) | Menu de compartilhar | Escolher computador e enviar sem abrir sessão |
| 12 `teste-conexao` | Teste de conexão | M1 mínimo (latência, caminho do host, decode HW); M3 completo |
| 13 `configuracoes` | Configurações | MF-100 |
| 14 `tablet` (M3) | Tablet | Toque direto padrão, 2 monitores em tela dividida |
| 15–17 (M4) | Host Android | Ativar, pedido de acesso, desktop controlando o celular |
| 18 `escuro-outra-vpn` | Erro | Outra VPN ativa, tema escuro |

Textos em pt-BR e en (strings no Slint com `@tr`, mesma ferramenta do desktop). Alvos de toque
≥ 48 dp, TalkBack fora da área de vídeo, contraste AA (MNF-07).

## 7. Mudanças no desktop e no protocolo que o M1 precisa

Tudo com round-trip em `taildesk-proto` e atualização do `07-protocolo.md`:

| Mudança | Lado | Detalhe |
|---|---|---|
| `Hello.client_kind` (DESKTOP, PHONE, TABLET) e `Hello.viewport{w_px, h_px, dpi}` | C → H | Host limita resolução do encoder ao viewport (MF-82); diálogo do host mostra "celular" |
| `PathInfo{direct, relay_region?, rtt_ms}` | H → C | Host mede pela LocalAPI; celular mostra no HUD e usa como pista do adaptador |
| `PeerListRequest` / `PeerList` (≤ 256 entradas, nome ≤ 128 B) | C ↔ H | Só para o mesmo usuário da tailnet (whois) ou celular pareado |
| `PairRequest{code}` / `PairResult{ok, cert_fingerprint}` | C → H | Código de uso único do QR |
| `KeyboardInfo.source` (VIRTUAL, HARDWARE) | C → H | Teclado virtual manda tudo imprimível como `KeyChar` |
| Tela "Configurações → Celulares → Conectar um celular" | Desktop | Mostra o QR, lista e remove celulares pareados |

Tags novas no intervalo de sessão (1–19) ou num intervalo novo para mobile, a decidir na thread de
protocolo; nada muda no header de 16 bytes da mídia.

## 8. Testes (mesmas camadas do babylock, adaptadas)

| Camada | Onde | O que cobre |
|---|---|---|
| Rust na máquina do CI | `crates/taildesk-mobile/tests`, `taildesk-overlay` | Máquina de estados dos gestos com sequências gravadas de toques, faixa de teclas (travas Ctrl/Alt), classificador de estado do Tailscale, `MobileVpnOverlay`, round-trip das mensagens novas |
| Teclado | fixtures de `taildesk-keyboard` | Casos novos: teclado virtual → host ABNT2 ("ação"), Bluetooth ABNT2 → host US-Intl (M3) |
| Kotlin na JVM (Robolectric) | `apps/android/app/src/test` | `TailscaleState` com `ShadowConnectivityManager`, intents de abrir o Tailscale e a loja, permissões pedidas só na hora certa |
| Ponta a ponta no emulador (API 30 e 34) | `app/src/androidTest` + UiAutomator | O CI sobe `taildesk-cli` como host com `LanOverlay` (só build debug) e o emulador conecta em `10.0.2.2`: abre o app, conecta, recebe quadro, toque vira clique no host (host de teste registra os eventos), faixa de teclas, sair com `ReleaseAll` |
| Capturas para a Play | `PlayScreenshots` + `play_screenshots.sh` (pt-BR, en-US) | Capturas reais no modo demonstração (§8.4) |
| Lint | Android Lint (`abortOnError`), clippy dos targets Android | Falta de tradução quebra o build, como no babylock |
| Manual em aparelho | roteiro do §8.5 | O que o emulador não mostra: VPN real, MediaCodec, bateria |

### 8.4 Modo demonstração

Uma sessão local com um "host de mentira" dentro do app (quadros gerados e eco do input), aberta
por "Ver como funciona" no onboarding. Serve para três coisas: capturas da Play sem tailnet, a
revisão do Google (o revisor não tem tailnet; vai na resposta de "Acesso ao app") e a primeira
impressão de quem ainda não instalou o Tailscale.

### 8.5 Roteiro em aparelho (M1)

Aparelhos: o celular do Yuri + pelo menos um Samsung e um Xiaomi (otimização de bateria agressiva).

- [ ] Cada estado do Tailscale (sem app, desligado, outra VPN, split tunneling, conectado) mostra a tela certa
- [ ] Parear pelo QR do notebook CachyOS; lista de dispositivos aparece
- [ ] Rafael R-mobile-1: no 4G, conectar no notebook, trocar de monitor, digitar `sudo systemctl restart nginx` com Ctrl+C funcionando, em < 60 s
- [ ] Ana Paula A-mobile-1: toque direto num botão, digitar "ação" com acento certo
- [ ] Girar a tela, ir para segundo plano e voltar: reconecta em ≤ 3 s sem perder zoom
- [ ] HUD mostra direto/relay informado pelo host e o decode por hardware
- [ ] Medir MNF-01 (toque → quadro ≤ 80 ms p50), MNF-02 (primeiro quadro ≤ 1,5 s), MNF-03 (bateria em 30 min)

## 9. CI e publicação, workflow por workflow

### 9.1 `android.yml` (a cada push na `main` e em PR que toca o Android ou os crates)

1. `actions/checkout@v5`, `actions/setup-java@v5` (temurin 17), `gradle/actions/setup-gradle@v4`
   (como no babylock).
2. Rust 1.97 com os targets Android, `cargo install cargo-ndk` com cache, NDK fixo pelo
   `sdkmanager`.
3. `cargo xtask mobile-check`.
4. `./gradlew assembleDebug`, depois `bundleRelease assembleRelease` sem assinatura (pega erro de R8
   e lint de release cedo, como no babylock).
5. `testDebugUnitTest`, `lintDebug`, resumo com `test_summary.py`.
6. Artefatos: `taildesk-android-debug-apk` e relatórios.
7. Job `emulator` (matriz API 30 e 34, KVM liberado, `android-emulator-runner@v2`, `x86_64`) com o
   host de teste rodando no runner.
8. Job `screenshots` (API 34, `pixel_2`, 1080×1920) → artefato `capturas-play`.

### 9.2 `android-release.yml` — "Versão para a Play"

```yaml
on:
  push:
    tags: ["android-v*"]          # v* continua sendo do desktop
  workflow_dispatch:
    inputs:
      track:
        description: "Faixa da Play (internal, alpha = teste fechado, beta, production)"
        default: internal
        required: true
permissions:
  contents: write                  # para anexar o APK na release do GitHub
```

Passos, na ordem do babylock:

1. `TAILDESK_ANDROID_VERSION_CODE = github.run_number`; `HAS_KEYSTORE` e `HAS_PLAY` pelos secrets.
2. Decodifica `TAILDESK_ANDROID_KEYSTORE_BASE64` em `$RUNNER_TEMP/upload.jks`.
3. `./gradlew testDebugUnitTest bundleRelease assembleRelease` com `TAILDESK_ANDROID_KEYSTORE_PASSWORD`,
   `TAILDESK_ANDROID_KEY_ALIAS`, `TAILDESK_ANDROID_KEY_PASSWORD`.
4. Aviso `::warning::` se não houver keystore (sai sem assinatura e a Play não aceita).
5. Artefato `taildesk-android-play-N` com `.aab`, `mapping.txt` e símbolos nativos.
6. `r0adkll/upload-google-play@v1` com `packageName: com.santech.taildesk`, `track` da entrada (padrão `internal`),
   `status: draft` até a primeira revisão, `whatsNewDirectory`, `mappingFile`.
7. **Novo em relação ao babylock**: com tag, cria a release `android-vX.Y.Z` no GitHub com o APK
   universal assinado + `SHA256SUMS` (canal fora da loja do `requisitos.md` §7 e, depois, o host M4).

Como as sessões não conseguem dar push em tag (403), o caminho normal é o Yuri clicar Run
workflow; tag `android-v*` por `git push` fica para quando ele quiser publicar também no GitHub
Releases. Como o APK no GitHub é obrigatório (decisão do Yuri, 03/11/2026), o Run workflow também cria a
release `android-vX.Y.Z` a partir do `versionName` (entrada `github_release`, padrão `true`), sem
precisar de push de tag.

Atenção: o APK do GitHub é assinado com a **chave de envio**, e o da Play com a **chave da Play**
(Assinatura de apps do Google Play). Um não atualiza o outro; quem trocar de canal precisa
desinstalar. Isso vai escrito na página da release.

### 9.3 `android-pages.yml`

Igual ao `pages.yml` do babylock, mas com `path: apps/android/site` e `paths:
["apps/android/site/**"]`. Endereço: `https://yurisanp.github.io/taildesk/privacidade/`.

### 9.4 Antes de cada versão (vai no `PLAY_STORE.md`)

1. Atualizar `versionName` em `apps/android/app/build.gradle.kts` (o `versionCode` é automático).
2. Escrever as novidades em `apps/android/distribution/whatsnew/` (pt-BR e en-US, até 500 caracteres)
   e em `fastlane/metadata/android/<idioma>/changelogs/default.txt`.
3. Rodar Actions → "Versão para a Play" → Run workflow (faixa `internal`), ou `git push` da tag
   `android-vX.Y.Z`.
4. No Play Console, revisar o rascunho e promover da faixa interna para a fechada ou produção.

## 10. Plano de PRs (M0 → M1 → publicação)

| PR | Conteúdo | Pronto quando |
|---|---|---|
| A1 | `apps/android` (Gradle, `GameActivity`, manifesto), `crates/taildesk-mobile` com uma tela Slint vazia, `cargo xtask mobile-check`, `android.yml` (build + JVM + artefato do APK) | CI verde, APK abre no emulador |
| A2 | S1: textura wgpu atualizada a 60 fps sob a UI Slint; medir num aparelho médio | ≥ 55 fps sem travar a UI (senão, plano B do §4.3 e ADR-M4) |
| A3 | S3 + S4 + S5: QUIC pela VPN do Tailscale contra `taildesk-cli`, classificador de estado, PTR | Gates registrados em `m0-resultados.md` |
| A4 | S6: `taildesk-codec::mediacodec` + interop `AHardwareBuffer` → Vulkan | Decode 1080p ≤ 8 ms sem cópia pela CPU |
| A5 | Protocolo mobile (§7) com round-trip + `07-protocolo.md` | Testes de round-trip; host respeita `viewport` |
| A6 | Desktop: tela de QR, `PeerList`, `PairRequest` | Celular de teste pareia; lista restrita ao mesmo usuário |
| A7 | `MobileVpnOverlay` + onboarding + dispositivos + pareamento (mockups 01–05, 18) | Testes Rust e Robolectric; roteiro dos estados do Tailscale |
| A8 | Sessão: vídeo, gestos de touchpad e toque direto, zoom/pan local, cursor ampliado (mockups 06–07) | Ponta a ponta no emulador |
| A9 | Teclado: faixa de teclas, `KeyChar`/`KeyShortcut`, IME, indicador (mockup 08) + fixtures | Ana Paula digita "ação" certo |
| A10 | Monitores, HUD, ciclo de vida, rede tarifada (mockups 09, 12) | Roteiro do §8.5 |
| A11 | Publicação: `android-release.yml`, `android-pages.yml`, `fastlane/metadata`, `whatsnew`, `PLAY_STORE.md`, modo demonstração, job de capturas, marca | Run workflow gera `.aab` assinado e (com a conta de serviço) sobe rascunho no teste interno |

M3 e M4 viram issues próprias quando começarem, com a mesma estrutura.

## 11. Rascunho das respostas do Play Console (vai para o `PLAY_STORE.md`)

### Ficha da loja
- Nome: **TailDesk**. Idioma padrão **Português (Brasil)**, tradução em Inglês (EUA).
- Descrição curta (pt-BR): "Acesse e controle seus computadores pela sua rede Tailscale."
- Categoria: **Ferramentas** (ou Produtividade). Gratuito, sem anúncios, sem compras.
- Ícone 512, gráfico de destaque 1024×500 e capturas: `apps/android/fastlane/metadata/android/<idioma>/images/`.
- Site: `https://github.com/yurisanp/taildesk`. Política de privacidade: endereço do §9.3.
- Contato: o e-mail que o Yuri já usa na ficha do babylock.

### Público e conteúdo
- Público-alvo: **18 anos ou mais** (ferramenta para adultos administrarem os próprios computadores).
- Anúncios: **Não**. Compras no app: **Não**.
- Classificação (IARC): Utilitário/Ferramenta; "Não" para tudo; esperado: Livre.
- **Acesso ao app**: as funções principais dependem de uma tailnet do Tailscale com um computador
  TailDesk ligado. Instruções para o revisor: abrir "Ver como funciona" (modo demonstração), que
  mostra uma sessão completa sem conta. Se a Play pedir acesso real, fornecer uma tailnet de
  demonstração com um host ligado **(decidir com o Yuri)**.

### Segurança dos dados
- O app **não coleta nada para o desenvolvedor**: sem telemetria, sem anúncios, sem análise (MNF-10).
- A tela, os toques e os arquivos vão direto do celular para os computadores do próprio usuário,
  criptografados (WireGuard do Tailscale + TLS do QUIC), sem passar por servidor do TailDesk.
  Pelas regras da Play, transferência iniciada pelo usuário para o próprio destino e criptografada
  de ponta a ponta tende a não contar como "coleta" **(validar a resposta exata no formulário no dia)**.
- Criptografado em trânsito: **Sim**. Pedido de exclusão: não se aplica (não há servidor).

### Permissões
- M1: nenhuma permissão sensível (câmera só para o QR, com texto de uso).
- M3: formulário de serviço em primeiro plano (`mediaPlayback`): "Mantém o som do computador
  tocando quando o TailDesk vai para segundo plano, a pedido do usuário. Para ao encerrar a sessão."
  Vídeo curto mostrando o botão de som e a notificação.
- M4 (host): declaração de Accessibility no mesmo formato do babylock (divulgação proeminente
  dentro do app antes de abrir as configurações, consentimento, vídeo de até 1 minuto). Enquanto não
  for aprovada, o host fica fora do build da Play (ADR-M6) e sai só no APK do GitHub.

### Teste fechado
Conta pessoal: cada app novo precisa de **12 testadores por 14 dias seguidos** num teste fechado
antes de pedir produção (mesma regra que o babylock enfrentou). Quem testar precisa ter Tailscale e
acesso a um computador com TailDesk, então convém juntar desde já pessoas que já usam o desktop.

## 12. Riscos e pontos de atenção

| Risco | Mitigação |
|---|---|
| Slint + wgpu no Android não chega a 55 fps ou o IME falha | Gate S1; plano B Compose + `uniffi` (§4.3) |
| **openh264 no Android**: a Play proíbe baixar código executável fora da loja, então o binário da Cisco (que dá a cobertura de patente) não pode ser baixado em runtime; compilado do código-fonte, perde essa cobertura | Proposta: **sem openh264 no build da Play**; o Android exige decoder H.264 por hardware, e o MediaCodec cobre. Ampliar o ADR-M3 para o Android (hoje fala só do iOS) |
| Bibliotecas nativas fora do alinhamento de 16 KB | NDK r28+ e checagem no CI (`zipalign -c -P 16`) **(validar o comando)** |
| Revisão da Play sem tailnet | Modo demonstração e instruções em "Acesso ao app" |
| Outra VPN no celular, otimização de bateria | Telas de estado e ajuda por fabricante (como as dicas por marca do babylock) |
| Tag `v*` dispara o release do desktop | Prefixo `android-v*` |
| F-Droid | Fica fora: a licença royalty-free do Slint não é livre para o F-Droid, e usar o Slint pela GPLv3 exigiria ADR (regra de licenças). Metadados no formato fastlane já deixam a porta aberta |
| Dois canais com chaves diferentes (Play e GitHub) | Aviso na release; o host M4 pode usar `applicationIdSuffix` próprio se precisar coexistir |

## 13. O que precisa do Yuri (na ordem, igual ao `PLAY_STORE.md` do babylock)

1. ~~Nome do pacote~~ **Decidido em 03/11/2026: `com.santech.taildesk`** (mesmo padrão do
   `com.santech.babylock`). Não muda depois do primeiro envio.
2. **Conta de desenvolvedor**: reaproveitar a do babylock (nada a pagar de novo).
3. **Chave de envio**:
   ```bash
   keytool -genkeypair -v -keystore taildesk-upload.jks -keyalg RSA -keysize 4096 \
     -validity 10000 -alias taildesk-upload
   base64 -w0 taildesk-upload.jks > taildesk-upload.b64
   ```
   Secrets no `yurisanp/taildesk`: `TAILDESK_ANDROID_KEYSTORE_BASE64`, `TAILDESK_ANDROID_KEYSTORE_PASSWORD`,
   `TAILDESK_ANDROID_KEY_ALIAS` (`taildesk-upload`), `TAILDESK_ANDROID_KEY_PASSWORD`. Guardar o `.jks`
   e as senhas fora do computador (sem eles não dá para atualizar o app).
4. **Ligar o GitHub Pages** do `taildesk` (Settings → Pages → Source: GitHub Actions) e rodar o
   workflow da política de privacidade.
5. **Criar o app no Play Console** (TailDesk, Português do Brasil, gratuito, aceitar a Assinatura de
   apps do Google Play) e fazer o **primeiro envio manual** do `.aab` no teste interno (a API só
   envia depois disso).
6. **(Opcional) Conta de serviço**: pode reaproveitar a do babylock, convidando o mesmo e-mail no
   app novo com permissão de lançar em faixas de teste; salvar o JSON em `PLAY_SERVICE_ACCOUNT_JSON`
   no repositório `taildesk`.
7. **Teste fechado**: 12 testadores por 14 dias, depois pedir produção.
8. **Decidido em 03/11/2026**: o host Android só entra no build da Play depois que o Google aprovar
   a declaração de Accessibility; o APK para baixar sai **sempre** no GitHub Releases, desde a
   primeira versão. Ainda em aberto: tailnet de demonstração para a revisão, se a Play pedir.
