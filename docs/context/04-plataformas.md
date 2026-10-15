# 04 — Contexto agnóstico de plataforma (Windows, macOS, Linux, Android, iOS)

## 1. Princípio

O núcleo (protocolo, sessão, codecs, QoS, UI) é **100% independente de SO**. Tudo que toca o SO
fica atrás de **traits** em `halyard-platform`, com uma implementação por SO em módulos
`#[cfg(target_os = ...)]`, e cada implementação **declara capacidades em runtime**
(`PlatformCaps`). A UI e a negociação consultam essas capacidades — nunca `cfg!` espalhado
pelo código de produto.

```rust
pub struct PlatformCaps {
    pub can_host: bool,            // pode ser controlado
    pub can_inject_input: bool,    // iOS = false; Android = só c/ Accessibility ativo
    pub can_capture_login_screen: bool,
    pub can_capture_system_audio: bool,
    pub can_file_clipboard: bool,
    pub needs_user_consent_per_session: bool, // Wayland portal, Android 14+, iOS
    pub multi_monitor: bool,
    pub virtual_display: bool,
    pub overlay_mode: OverlayMode, // Embedded | System | Both
}
```

## 2. Matriz de papéis

| SO | Controlador (cliente) | Host com controle | Host só visualização | Tela de login/UAC | Não-assistido |
|---|---|---|---|---|---|
| **Windows 10/11** | ✅ | ✅ | ✅ | ✅ (serviço SYSTEM) | ✅ |
| **macOS 13+** | ✅ | ✅ (TCC) | ✅ | ⚠️ limitado (tela de login exige LaunchDaemon + privilégios; FileVault pré-boot impossível) | ✅ após permissões |
| **Linux X11** | ✅ | ✅ | ✅ | ⚠️ depende do display manager | ✅ |
| **Linux Wayland** | ✅ | ✅ via portal RemoteDesktop/libei (diálogo do usuário) | ✅ via portal ScreenCast | ❌ em geral (GNOME 46+ tem "Remote Login" próprio via RDP; não-padronizado) | ⚠️ via `restore_token` do portal (KDE/GNOME) ou uinput+DRM com privilégio |
| **Android 10+** | ✅ | ✅ com **AccessibilityService** habilitado | ✅ MediaProjection | ❌ | ⚠️ MediaProjection exige consentimento por sessão (Android 14+) |
| **iOS/iPadOS 17+** | ✅ | ❌ **(sem API de injeção de input)** | ⚠️ via **ReplayKit Broadcast Upload Extension** (usuário inicia; limite de memória ~50 MB na extensão) | ❌ | ❌ |

## 3. Por subsistema

### 3.1 Captura de tela

| SO | API recomendada | Crate | Observações |
|---|---|---|---|
| Windows | **Windows.Graphics.Capture (WGC)** p/ janelas e telas; **DXGI Desktop Duplication** p/ secure desktop (UAC/login) e baixa latência | `windows-capture` **2.0.1** (WGC), `windows` 0.62 (DXGI direto) | Saída = textura **D3D11 BGRA** → zero-copy para encoder MF/NVENC. DXGI dá *dirty rects* e *move rects* nativos. Borda amarela do WGC pode ser desligada (Win11). |
| macOS | **ScreenCaptureKit** (`SCStream`) | `objc2-screen-capture-kit` **0.3.2** | Saída = `CVPixelBuffer`/`IOSurface` (pode pedir NV12 direto) → VideoToolbox zero-copy. Permissão "Screen Recording" (TCC); aviso de privacidade periódico no macOS 15+. |
| Linux X11 | XShm + XDamage (dirty regions), XFixes (cursor) | `x11rb` | CPU-bound; ok até 4K/30. |
| Linux Wayland | **xdg-desktop-portal ScreenCast → PipeWire** (dmabuf) | `ashpd` **0.13.13** (portal), `pipewire` **0.10.1** | Importar dmabuf no Vulkan/VA-API = zero-copy. Guardar `restore_token` p/ não perguntar sempre. Alternativa privilegiada: KMS/DRM direto (o RustDesk tem `drm_reader`). |
| Android | **MediaProjection** → `VirtualDisplay` → `Surface` do **MediaCodec** (encoder) | `jni` **0.22.4** + `ndk` 0.9 (código Kotlin mínimo para a Activity de consentimento + Foreground Service tipo `mediaProjection`) | Zero-copy natural: a tela é desenhada direto na Surface de entrada do encoder. |
| iOS | **ReplayKit** (`RPBroadcastSampleHandler` numa *Broadcast Upload Extension*) | `objc2-replay-kit` 0.3.2 | Extensão separada, memória restrita → encoder **VideoToolbox HW** obrigatório e sem buffers grandes; comunicação com o app/tailnet de dentro da extensão. |

Crates "tudo-em-um" (`xcap` 0.9.8, `scap` 0.0.8) servem para protótipo; **para produção use as APIs nativas** (precisamos de texturas GPU, dirty rects e cursor separado).

### 3.2 Injeção de input

| SO | API | Crate | Observações |
|---|---|---|---|
| Windows | `SendInput` (+ `SetCursorPos`), Touch Injection, Pen | `windows` | Para UAC/login: o processo precisa rodar no desktop `Winlogon` (lançado pelo serviço SYSTEM com `CreateProcessAsUser`). Ctrl+Alt+Del: `SendSAS` (exige policy `SoftwareSASGeneration`). |
| macOS | `CGEventPost` / `CGEventCreateKeyboardEvent` | `objc2-core-graphics` | Permissão **Accessibility** (TCC). Teclado: mapear para virtual keycodes ANSI/ISO/JIS. |
| Linux X11 | XTest | `x11rb` | Simples. |
| Linux Wayland | **portal RemoteDesktop + libei** (EIS) | `reis` **0.7.1** (libei/libeis em Rust puro), `ashpd` | Alternativa: **uinput** (`evdev` 0.13 / `input-linux` 0.7) com acesso a `/dev/uinput` (grupo/udev rule ou serviço root). |
| Android | **AccessibilityService** (`dispatchGesture`, `performGlobalAction`) + IME p/ texto | via JNI (Kotlin) | Usuário precisa habilitar o serviço manualmente nas Configurações; Play Store tem política restritiva para uso de Accessibility — documentar o uso declarado. |
| iOS | — | — | **Não existe.** Host iOS é sempre view-only. |

`enigo` (0.6.1) é ok para protótipo (Win/mac/X11), mas o produto precisa de controle fino de
scancode vs. caractere — ver §3.6.

### 3.3 Cursor
Enviar **forma** (bitmap RGBA + hotspot, com cache por hash) e **posição** separadamente do
vídeo; o controlador desenha localmente. APIs: `GetCursorInfo`/DXGI pointer shape (Win),
`NSCursor`/SCK `showsCursor=false` + CGS (mac), XFixes (X11), metadata de cursor do PipeWire
(Wayland), não aplicável em Android/iOS (toque).

### 3.4 Áudio
Ver `03-codecs.md` §8. Opus em todos. Microfone do controlador → host (chamada de voz) é opcional.

### 3.5 Clipboard
| SO | Texto/imagem | Arquivos |
|---|---|---|
| Windows | `arboard` **3.7.0** / `clipboard-rs` 0.3.5 | `CF_HDROP` + IDataObject virtual (delayed rendering) |
| macOS | `arboard` | `NSPasteboard` com file promises |
| Linux X11 | `arboard` | `text/uri-list` |
| Linux Wayland | `arboard` (feature wayland) / `wl-clipboard-rs` 0.9.4 | `text/uri-list`; requer foco ou protocolo data-control (wlroots/KDE) |
| Android | `ClipboardManager` (JNI); Android 10+ só lê com app em foco | — |
| iOS | `UIPasteboard` (aviso de "colou de outro app") | — |

Para arquivos: **não sincronizar o clipboard de arquivos automaticamente**; oferecer
transferência explícita (stream QUIC dedicado) + drag & drop.

### 3.6 Teclado (importante para pt-BR)
> Desenho completo da tradução automática entre layouts (modo Auto) em `11-teclado.md`.

- Enviar **dois modos**: `Physical` (scancode USB HID + modificadores, independente de layout — ideal para jogos/atalhos) e `Text` (unicode composto — ideal quando layouts diferem, ex.: ABNT2 ↔ US-Intl, dead keys `´ ~ ^ ç`).
- Padronizar keycodes em **USB HID usage IDs** no fio; converter por SO na borda (`xkbcommon` 0.9 no Linux, tabela VK↔HID no Windows, kVK↔HID no macOS).
- Atalhos de sistema (Win, Cmd, Alt+Tab, Ctrl+Alt+Del) capturados no controlador: Windows exige low-level keyboard hook; macOS exige event tap + Input Monitoring.

### 3.7 Codecs de hardware por SO
Ver tabela em `03-codecs.md` §4.2. Resumo: Windows = Media Foundation (+NVENC/AMF/QSV opcionais);
macOS/iOS = VideoToolbox; Linux = VA-API (+NVENC, Vulkan Video); Android = MediaCodec.
PyroWave: Vulkan (Win/Linux/Android) e Metal (macOS/iOS, Apple7+ = A14/M1 em diante).

### 3.8 Renderização (lado controlador)
`wgpu` **30.0.1** cobre todos: D3D12/Vulkan (Win), Metal (mac/iOS), Vulkan/GL (Linux), Vulkan/GLES (Android).
Zero-copy de decode→render por SO:
| SO | Decoder → Render |
|---|---|
| Windows | textura D3D11 do MF/D3D11VA → shared handle → D3D12/Vulkan no wgpu (`wgpu-hal` interop) |
| macOS/iOS | `CVPixelBuffer` (IOSurface) → `CVMetalTextureCache` → textura Metal no wgpu |
| Linux | VA surface → export **dmabuf** → import Vulkan (`VK_EXT_external_memory_dma_buf`) |
| Android | `AImageReader` → `AHardwareBuffer` → import Vulkan (`VK_ANDROID_external_memory_android_hardware_buffer`) |
| Fallback | Upload NV12 em 2 planos (`queue.write_texture`) + shader |

### 3.9 Rede / Tailscale por SO
| SO | Overlay padrão | Notas |
|---|---|---|
| Windows x86_64 | **Embutido** (`tailscale` crate) ou Sistema | Windows ARM64 **não** listado pelo tailscale-rs → Sistema. LocalAPI via named pipe. |
| macOS ARM64 | **Embutido** ou Sistema | macOS Intel não listado pelo tailscale-rs → Sistema. LocalAPI difere entre app da App Store e `tailscaled`. Sandbox da App Store limita sockets/Network Extension — distribuir fora da App Store (Developer ID + notarização) para o host. |
| Linux x86_64/ARM64 | **Embutido** ou Sistema | LocalAPI via unix socket (permissão: grupo/operador `tailscale set --operator=$USER`). |
| Android | **Sistema** (app oficial Tailscale, VpnService) | tailscale-rs não suporta Android. Só um VPN ativo por vez no Android: se o usuário usa outra VPN, não funciona. Futuro: embutido (userspace, sem VpnService) quando suportado. |
| iOS | **Sistema** (app oficial, NetworkExtension) | Idem; um único perfil VPN ativo. A Broadcast Extension usa as rotas do sistema normalmente. |

### 3.10 Ciclo de vida, serviço e privilégios
| SO | Modelo de processo do host |
|---|---|
| Windows | **Serviço Windows (SYSTEM)** `halyard-service` → spawna `halyard-agent` na sessão ativa (`WTSGetActiveConsoleSessionId` + `CreateProcessAsUser`); reage a troca de sessão/lock/UAC (desktop `Winlogon`). IPC: named pipe com ACL. |
| macOS | **LaunchAgent** por usuário (captura/input exigem sessão GUI) + **LaunchDaemon** opcional p/ tela de login; SMAppService (macOS 13+) para registrar. Permissões TCC (Screen Recording, Accessibility, Input Monitoring) — fluxo de onboarding guiado; MDM pode pré-aprovar via PPPC profile (útil p/ frota). |
| Linux | **systemd user service** (sessão gráfica) + system service opcional (uinput/DRM). Detectar `XDG_SESSION_TYPE`. |
| Android | **Foreground Service** (`foregroundServiceType="mediaProjection|connectedDevice"`) + AccessibilityService; tela de consentimento por sessão; otimização de bateria (pedir exceção). |
| iOS | App controlador normal; host = Broadcast Upload Extension iniciada pelo usuário via Control Center/RPSystemBroadcastPickerView. Background: app é suspenso — sessões só em foreground (controlador) ou durante broadcast (host). |

### 3.11 Armazenamento de segredos
`keyring` **4.2.0** (Credential Manager / Keychain / Secret Service). Em Android: Keystore via JNI; iOS: Keychain (via `keyring` ou `security-framework`). Estado do nó Tailscale embutido (`PersistState`) contém chaves privadas — **sempre** em armazenamento protegido, nunca em arquivo legível por outros usuários.

### 3.12 Empacotamento e distribuição
| SO | Formato | Assinatura |
|---|---|---|
| Windows | MSI (WiX) ou MSIX; EXE portátil p/ suporte avulso | Authenticode (EV evita SmartScreen) |
| macOS | `.app` em DMG/PKG (Developer ID), Universal ou ARM64 | Notarização obrigatória; hardened runtime + entitlements |
| Linux | `.deb`/`.rpm` + AppImage; Flatpak (portal-only, sem uinput) | Assinatura de repositório |
| Android | AAB (Play) + APK (direto/F-Droid) | Play exige declaração de uso de Accessibility e Foreground Service |
| iOS | App Store / TestFlight; extensão de broadcast no mesmo bundle | App Store Review — explicar o uso |

Ferramentas: `cargo-dist` / `cargo-packager`, `cargo-ndk` **4.1.2** (Android), `xcodebuild` + `cargo` com targets `aarch64-apple-ios`/`aarch64-apple-ios-sim`.

## 4. Targets de compilação (CI)

| Target triple | SO | Prioridade |
|---|---|---|
| `x86_64-pc-windows-msvc` | Windows | P0 |
| `aarch64-pc-windows-msvc` | Windows ARM | P2 (overlay sistema) |
| `aarch64-apple-darwin` | macOS Apple Silicon | P0 |
| `x86_64-apple-darwin` | macOS Intel | P2 (overlay sistema) |
| `x86_64-unknown-linux-gnu` | Linux | P0 |
| `aarch64-unknown-linux-gnu` | Linux ARM (Raspberry, servidores) | P1 |
| `aarch64-linux-android` (+ `x86_64-linux-android` p/ emulador) | Android | P1 |
| `aarch64-apple-ios` (+ `aarch64-apple-ios-sim`) | iOS | P1 (controlador) |

CI: GitHub Actions/GitLab CI com runners Windows, macOS (cobre iOS) e Linux (cobre Android via NDK). Testes de captura/encode HW precisam de **runners com GPU** (self-hosted) — marcar esses testes com `#[ignore]` + job dedicado.

## 5. Checklist de "portar para um novo SO"
1. Implementar `ScreenCapturer`, `InputInjector`, `CursorSource`, `ClipboardProvider`, `AudioCapturer`, `SecretStore`, `ServiceHost` (ou marcar como não suportado em `PlatformCaps`).
2. Registrar backends de codec HW no `CodecRegistry` + probe.
3. Implementar import zero-copy de frames decodificados no renderer (ou cair no upload CPU).
4. Escolher overlay padrão (embutido vs sistema) e o cliente LocalAPI do SO.
5. Fluxo de permissões/onboarding (UI) + textos de consentimento.
6. Empacotamento + assinatura + job de CI.
