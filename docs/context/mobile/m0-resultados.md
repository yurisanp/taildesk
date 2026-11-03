# TailDesk Android — Resultados do M0 (spikes com gate)

> Aberto em 03/11/2026 na issue #104 (PRs A2, A3 e A4 do `android-detalhado.md` §10). O código de
> cada spike está no repositório; **as medições precisam de um aparelho real** (a sessão que
> escreveu o código não tinha celular, SDK nem emulador). Cada linha diz como medir e onde anotar.
> Ao medir, troque "pendente" pelo número, a data e o aparelho, e atualize o ADR-M4 no
> `08-roadmap.md` se o gate S1 reprovar.

## Resumo

| Spike | Gate | Código | Estado |
|---|---|---|---|
| S1 Slint + vídeo a 60 fps | ≥ 55 fps sem travar a UI | Modo `s1` do app (`App::medir_s1`) | **Pendente: medir em aparelho** |
| S3 QUIC pela VPN do Tailscale | Conecta pelo nome MagicDNS; ≥ 20 Mbit/s em Wi-Fi direto; maior datagrama medido | Teste de conexão do app + `taildesk-cli bench-net --ouvir` | **Pendente: medir em aparelho** |
| S4 Estados do Tailscale | Os 5 estados distinguidos em aparelho real | `taildesk_overlay::mobile::classificar` (testado no Linux) + `TailscaleState.kt` (Robolectric) | Lógica pronta e testada; **pendente: roteiro em aparelho** |
| S5 Nome reverso no MagicDNS | O PTR de 100.x em 100.100.100.100 responde? | `taildesk_overlay::mobile::ptr` (testado contra um MagicDNS falso) | **Pendente: conferir no aparelho** |
| S6 Decode por hardware | Decode 1080p ≤ 8 ms, sem cópia pela CPU | `taildesk_codec::mediacodec::DecoderMediaCodec` (saída em `ByteBuffer`) | Decoder pronto com uma cópia (YUV → RGBA na CPU); **zero-copy ainda não feito**; medir o tempo de decode |
| S7 Inbound no Android | Celular escuta UDP no 100.x e o desktop conecta | — | Fica para o M4 (host Android) |

## S1 — Slint + imagem trocada a cada quadro

**O que mede.** O caminho do vídeo do M1: uma imagem 1920×1080 RGBA nova a cada 16 ms numa
`Image` do Slint (renderer Skia da NativeActivity), com a barra da sessão por cima. Conta os
quadros que o Slint desenhou de fato (`RenderingState::AfterRendering`) e mostra no HUD e no
logcat.

**Como medir.**
1. Instale o APK de debug do CI (artefato `taildesk-android-debug-apk`).
2. Abra no modo S1:
   ```bash
   adb shell am start -n com.santech.taildesk.debug/com.santech.taildesk.MainActivity -e taildesk.modo s1
   ```
3. Leia o HUD ("S1 · NN fps · 1920×1080") ou o logcat:
   ```bash
   adb logcat -s taildesk
   ```
4. Durante a medição, abra e feche a barra e o teclado: a UI não pode engasgar.

**Gate.** ≥ 55 fps num aparelho médio (Galaxy A54, Pixel 6a ou equivalente). Abaixo disso, o
caminho por textura do wgpu (`slint::Image::try_from(wgpu::Texture)`, feature `unstable-wgpu-30`
do backend Android) entra antes do M1; se nem assim passar, plano B do ADR-M4.

| Aparelho | Android | fps | Data |
|---|---|---|---|
| pendente | | | |

## S3 — QUIC pela VPN do Tailscale

**Como medir.**
1. No computador (Linux ou Windows com Tailscale):
   ```bash
   taildesk-cli bench-net --ouvir <IP 100.x do computador>:7421
   ```
2. No celular, com o Tailscale ligado: Dispositivos → o computador → **Testar conexão**. O app
   conecta na porta do TailDesk (7420) e, se achar o `bench-net` na 7421, mede a banda nos dois
   sentidos, o maior datagrama e a classe de rede.
3. Repita no Wi-Fi da casa (caminho direto) e no 4G (direto pela internet ou relay; o HUD da sessão
   mostra o caminho que o host informou no `PathInfo`).

**Gate.** Conecta pelo nome MagicDNS (pareamento digitando o nome) e por IP; ≥ 20 Mbit/s em Wi-Fi
direto; maior datagrama anotado (o túnel móvel costuma ter MTU 1280).

| Aparelho | Rede | Caminho | RTT | Banda ↓ | Banda ↑ | Maior datagrama | Data |
|---|---|---|---|---|---|---|---|
| pendente | Wi-Fi | | | | | | |
| pendente | 4G | | | | | | |

## S4 — Estados do Tailscale

**Lógica.** `classificar` (em `crates/taildesk-overlay/src/mobile.rs`), com testes no Linux:
interface 100.x + rede ativa VPN + DNS da tailnet sem falhar = conectado; interface 100.x sem a
rede ativa ser VPN (ou com o DNS falhando) = TailDesk fora do túnel; VPN sem 100.x = outra VPN;
o resto = desligado ou não instalado. Versão abaixo de 1.102 só avisa.

**Roteiro em aparelho** (marque cada um):
- [ ] Sem o app do Tailscale → "Instale o Tailscale" e o botão abre a Play Store
- [ ] Tailscale instalado e desligado → "Ligue o Tailscale" e o botão abre o app dele
- [ ] Outra VPN ligada (WireGuard, Proton…) → "Outra VPN está ativa"
- [ ] Tailscale ligado com o TailDesk em "aplicativos excluídos" → "O Tailscale está deixando o TailDesk de fora"
- [ ] Tailscale ligado → "Tudo pronto" e a lista aparece
- [ ] Ligar e desligar o Tailscale com o app aberto muda a tela sozinho (`NetworkCallback`)

## S5 — Nome reverso no MagicDNS

**Código.** `ptr::consultar` pergunta o PTR do IP do host a `100.100.100.100`; sem resposta, vale o
nome pareado (`MobileVpnOverlay::nome_de`). Testado contra um MagicDNS falso em localhost.

**Conferir no aparelho.** Conectar num host pelo IP (pareamento digitando o IP) e ver se a tela
"Conectando" e o diálogo do host mostram o nome do computador. Anote se o PTR respondeu:

| Aparelho | Tailscale | PTR respondeu? | Data |
|---|---|---|---|
| pendente | | | |

## S6 — Decode por hardware

**Feito.** `DecoderMediaCodec` (`crates/taildesk-codec/src/mediacodec.rs`): `video/avc`,
`low-latency = 1`, `priority = 0`, saída em `ByteBuffer` com o layout que o codec informa
(`stride`, `slice-height`, recorte, NV12/I420/flexível; formatos proprietários como o tiled da
Qualcomm são recusados e a sessão avisa). A conversão YUV → RGBA é em inteiros na thread de
decode (`crates/taildesk-mobile/src/video.rs`) e a interface pega só o quadro mais novo.

**Falta (o gate pede sem cópia).** `AImageReader` com `AHARDWAREBUFFER_USAGE_GPU_SAMPLED_IMAGE` →
`AHardwareBuffer` → `VkImage` importado no wgpu (`VK_ANDROID_external_memory_android_hardware_buffer`,
com conversão YCbCr) → textura entregue ao Slint. Precisa de aparelho para escrever e validar.

**Como medir o decode de hoje.** Na sessão, ligue o HUD: ele mostra "decode N ms" (a média do
`ReceiverReport.decode_us` que o celular também manda ao host a cada 250 ms). Anote com o host
mandando 1080p30:

| Aparelho | Componente (HUD/teste) | Decode 1080p | Data |
|---|---|---|---|
| pendente | | | |
