# TailDesk Android — Publicação na Play e no GitHub

Guia no formato do `docs/PLAY_STORE.md` do babylock-android. Detalhamento completo em
`docs/context/mobile/android-detalhado.md` (§9 a §13).

## O que já está pronto no repositório

| Peça | Onde |
|---|---|
| App (controlador, M1) | `apps/android` (casca Kotlin) + `crates/taildesk-mobile` (Rust/Slint) |
| Pacote | `com.santech.taildesk` (debug: `com.santech.taildesk.debug`, instala ao lado) |
| CI a cada push/PR | `.github/workflows/android.yml`: `cargo xtask mobile-check`, APK de debug (artefato `taildesk-android-debug-apk`), release sem assinatura (R8 + lint de release), checagem de 16 KB, Robolectric, lint, emulador API 30 e 34, capturas (`capturas-play`) |
| "Versão para a Play" | `.github/workflows/android-release.yml`: `.aab` assinado, rascunho na faixa escolhida, release `android-vX.Y.Z` no GitHub com o APK assinado e `SHA256SUMS` |
| Política de privacidade | `apps/android/site/privacidade/` → `https://yurisanp.github.io/taildesk/privacidade/` (`.github/workflows/android-pages.yml`) |
| Ficha da loja | `apps/android/fastlane/metadata/android/{pt-BR,en-US}/` (título, descrições, ícone, gráfico de destaque, capturas, changelog) |
| Novidades da versão | `apps/android/distribution/whatsnew/whatsnew-{pt-BR,en-US}` (até 500 caracteres) |
| Marca | `apps/android/marca/` (ícone 512, destaque 1024×500 nos dois idiomas) |
| Modo demonstração | "Ver como funciona" no onboarding: sessão completa sem tailnet (para o revisor e as capturas) |

## O que precisa das suas mãos (na ordem)

1. ~~Nome do pacote~~: decidido em 03/11/2026, `com.santech.taildesk`. Não muda depois do
   primeiro envio.
2. **Conta de desenvolvedor**: a do babylock (nada a pagar de novo).
3. **Chave de envio** (uma vez, guarde fora do computador; sem ela não dá para atualizar o app):
   ```bash
   keytool -genkeypair -v -keystore taildesk-upload.jks -keyalg RSA -keysize 4096 \
     -validity 10000 -alias taildesk-upload
   base64 -w0 taildesk-upload.jks > taildesk-upload.b64
   ```
   Secrets em `yurisanp/taildesk` → Settings → Secrets and variables → Actions:
   - `TAILDESK_ANDROID_KEYSTORE_BASE64` = conteúdo de `taildesk-upload.b64`
   - `TAILDESK_ANDROID_KEYSTORE_PASSWORD`
   - `TAILDESK_ANDROID_KEY_ALIAS` = `taildesk-upload`
   - `TAILDESK_ANDROID_KEY_PASSWORD`
4. **Ligar o GitHub Pages**: Settings → Pages → Source: **GitHub Actions**. Depois Actions →
   "Política de privacidade (Pages)" → Run workflow. Confira
   `https://yurisanp.github.io/taildesk/privacidade/`.
5. **Criar o app no Play Console**: TailDesk, Português (Brasil), app, gratuito. Aceite a
   Assinatura de apps do Google Play. Rode Actions → "Versão para a Play" → Run workflow (faixa
   `internal`), baixe o `.aab` do artefato `taildesk-android-play-N` e faça o **primeiro envio à
   mão** no teste interno (a API só envia depois disso).
6. **(Opcional) Conta de serviço** para o envio automático: convide no app novo o mesmo e-mail da
   conta de serviço do babylock, com permissão de lançar em faixas de teste, e salve o JSON em
   `PLAY_SERVICE_ACCOUNT_JSON`. Sem ela, o workflow só gera o `.aab` e o APK.
7. **Preencher o Play Console** com as respostas abaixo.
8. **Teste fechado**: 12 testadores por 14 dias seguidos (regra para conta pessoal) antes de pedir
   produção. Quem testar precisa de Tailscale e de um computador com o TailDesk: comece pelos
   usuários do desktop.

## Respostas para o Play Console

### Ficha da loja
- Nome: **TailDesk**. Idioma padrão: **Português (Brasil)**; tradução: Inglês (EUA).
- Descrição curta e completa, ícone, gráfico de destaque e capturas: `fastlane/metadata/android/<idioma>/`
  (as capturas mais novas também saem no artefato `capturas-play` do CI).
- Categoria: **Ferramentas**. Gratuito, sem anúncios, sem compras no app.
- Site: `https://github.com/yurisanp/taildesk`. Política de privacidade: endereço do passo 4.
- E-mail de contato: o mesmo da ficha do babylock.

### Público e conteúdo
- Público-alvo: **18 anos ou mais**.
- Anúncios: **Não**. Compras no app: **Não**.
- Classificação (IARC): Utilitário/Ferramenta, "Não" para tudo (esperado: Livre).
- **Acesso ao app**: "As funções principais dependem de uma rede Tailscale com um computador
  TailDesk ligado. Para revisar sem conta: na primeira tela, toque em **Ver como funciona
  (demonstração)**; a sessão de demonstração mostra a tela de um computador, o touchpad, o
  teclado com a faixa de teclas, o zoom e a troca de monitor." Se a Play pedir acesso real,
  combinar uma tailnet de demonstração com um host ligado (decisão em aberto).

### Segurança dos dados
- O app **não coleta** dados para o desenvolvedor (sem telemetria, anúncios ou análise).
- A tela, os toques e o teclado vão do celular direto para os computadores do próprio usuário,
  criptografados (WireGuard do Tailscale + TLS do QUIC), sem servidor do TailDesk. Pelas regras da
  Play, transferência iniciada pelo usuário para o próprio destino, criptografada de ponta a ponta,
  tende a não contar como coleta **(conferir a resposta exata no formulário no dia)**.
- Criptografado em trânsito: **Sim**. Pedido de exclusão: não se aplica (nada no servidor).

### Permissões
- M1: nenhuma sensível. Câmera só na tela do QR (texto de uso no próprio app).
- M3 (som em segundo plano): formulário de serviço em primeiro plano `mediaPlayback`.
- M4 (host Android): declaração de Accessibility. Decidido em 03/11/2026: o host só entra no
  build da Play depois da aprovação; antes disso, só no APK do GitHub.

## Antes de cada versão

1. Atualize `versionName` em `apps/android/app/build.gradle.kts` (o `versionCode` é o número da
   execução do workflow).
2. Escreva as novidades em `distribution/whatsnew/whatsnew-pt-BR` e `whatsnew-en-US` (até 500
   caracteres) e copie para `fastlane/metadata/android/<idioma>/changelogs/default.txt`.
3. Actions → **Versão para a Play** → Run workflow (faixa `internal`, "Criar a release no GitHub"
   ligado). Ou empurre a tag `android-vX.Y.Z` (a tag `v*` é do desktop).
4. No Play Console, revise o rascunho e promova da faixa interna para a fechada ou produção.

## Dois canais, duas chaves

O APK do GitHub Releases é assinado com a **chave de envio**; o da Play, com a **chave da Play**.
Um não atualiza o outro: quem trocar de canal precisa desinstalar antes (a página de cada release
avisa).
