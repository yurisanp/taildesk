# 16 — Atualização automática (AppImage e demais pacotes)

> Pesquisa de 15/10/2026, pedida pelo Yuri: que metadados o AppImage precisa para se atualizar
> sozinho, como isso entra no `release.yml` e o que o app precisa fazer. Fontes: spec do AppImage
> (`AppImage/AppImageSpec`, `draft.md`, seção "Update information") e o `appimagetool`
> (`AppImage/appimagetool`, opções `-u` e `--sign`).

## 1. Resumo

- O AppImage carrega a **update information** numa seção ELF `.upd_info`. Ferramentas como o
  AppImageUpdate leem essa string, acham o `.zsync` da versão nova e baixam só os blocos que mudaram.
- A partir deste PR, todo AppImage do TailDesk sai com
  `gh-releases-zsync|yurisanp|taildesk|latest|TailDesk-*x86_64.AppImage.zsync`
  e cada release publica o `.zsync` ao lado do AppImage (e no `SHA256SUMS`).
- O **atualizador dentro do app** (fase 1, junto com o app gráfico e a bandeja) usa os mesmos
  metadados: consulta a release mais nova no GitHub, baixa o AppImage inteiro, confere o SHA-256 e
  troca o arquivo. O delta via zsync fica para ferramentas externas; o AppImage é pequeno.
- A **v0.1.0** saiu antes disso e não tem `.upd_info`: quem a usa baixa a próxima versão uma vez
  à mão; daí em diante o update funciona.

## 2. Os metadados

### 2.1 Update information (`.upd_info`)
Uma string de texto na seção ELF `.upd_info` do runtime (AppImage tipo 2). Só um transporte por
AppImage. Formatos da spec:

| Formato | Sintaxe | Uso |
|---|---|---|
| `zsync` | `zsync\|<URL fixa do .zsync>` | Servidor próprio com URL estável ("latest") |
| `gh-releases-zsync` | `gh-releases-zsync\|<usuário>\|<repo>\|<tag>\|<arquivo .zsync, aceita *>` | GitHub Releases (o nosso) |
| `pling-v1-zsync` | `pling-v1-zsync\|<id do produto>\|<padrão do arquivo>` | Pling / AppImageHub |

A `<tag>` do `gh-releases-zsync` aceita `latest` (release estável mais nova), `latest-pre`
(pre-release mais nova), `latest-all` (a mais nova das duas) ou uma tag fixa.

**Escolha do TailDesk** (`packaging/linux/build-packages.sh`):
- versão estável (`0.2.0`): `gh-releases-zsync|yurisanp|taildesk|latest|TailDesk-*x86_64.AppImage.zsync`
- pre-release (`0.2.0-beta.1`): mesmo, com `latest-all`, para quem testa beta receber também a
  estável seguinte.
- `TAILDESK_UPDATE_INFO=""` desliga (build local); outro valor substitui a string.

O padrão `TailDesk-*x86_64.AppImage.zsync` exige que cada release tenha **um único** `.zsync`
com esse nome. Não renomear o AppImage sem trocar a string.

### 2.2 Arquivo `.zsync`
O `appimagetool -u "<string>"` embute a string e, com `zsyncmake` instalado, gera
`TailDesk-<versão>-x86_64.AppImage.zsync` na pasta atual. O `.zsync` traz tamanho, SHA-1 e os
checksums por bloco (2 KiB), e a URL relativa do AppImage (mesma pasta da release). O CI instala o
pacote `zsync` no Ubuntu 22.04 e o teste rápido confere a seção `.upd_info` e o `.zsync`.

### 2.3 Assinatura (`.sha256_sig`) — fase 5
A spec prevê a seção `.sha256_sig` com a assinatura do SHA-256 do AppImage (calculado com a seção
zerada); o `appimagetool --sign --sign-key <id>` assina com GPG (`APPIMAGETOOL_SIGN_PASSPHRASE`
no CI). Hoje a integridade vem do HTTPS do GitHub e do `SHA256SUMS`, que não prova autoria. Junto
com a assinatura do instalador Windows (fase 5), assinar o `SHA256SUMS` (minisign; verificação no
app com a crate `minisign-verify`, MIT) e/ou o AppImage com GPG. Chave privada só em secret do
GitHub Actions.

## 3. Quem consome

- **AppImageUpdate** (`appimageupdatetool`, CLI e GUI) e integradores de desktop que leem a
  update information (ex.: Gear Lever, AppImageLauncher; conferir o suporte de cada um antes de
  citar na documentação de usuário). Funcionam sem nada no TailDesk.
- **O próprio TailDesk** (§4).

## 4. Atualizador dentro do app (fase 1, com o app gráfico)

Só quando roda como AppImage: o runtime define `APPIMAGE` (caminho do arquivo) e `APPDIR`.

1. **Checagem**: ao abrir e a cada 24 h, se a opção estiver ligada, `GET
   https://api.github.com/repos/yurisanp/taildesk/releases/latest` (canal estável) ou a lista de
   releases (canal pré-lançamento). Sem token; o limite anônimo (60/h por IP) sobra.
2. **Comparação**: versão da tag (`semver`) maior que `CARGO_PKG_VERSION`.
3. **Download**: o asset `TailDesk-*-x86_64.AppImage` e o `SHA256SUMS` da mesma release, para
   `"$APPIMAGE.part"` na mesma pasta (mesmo sistema de arquivos, para o rename ser atômico).
4. **Verificação**: SHA-256 do `.part` igual à linha do `SHA256SUMS` (e, na fase 5, assinatura).
5. **Troca**: `chmod 755` e `rename(.part → $APPIMAGE)`. O processo em execução mantém o inode
   antigo, então nada quebra até reiniciar.
6. **Reinício**: o app pede para reiniciar e faz `exec` do novo `$APPIMAGE`.

Regras:
- **Nunca trocar nem reiniciar com sessão ativa** (host ou controlador); adiar até a sessão acabar.
- Pasta sem permissão de escrita (ex.: `/opt`): não baixa; mostra a versão nova com link para a
  página da release.
- Falha de rede ou de hash: apaga o `.part`, mantém a versão atual e tenta na próxima checagem.
- Sem telemetria: a única chamada é a consulta à API pública do GitHub, e o usuário pode
  desligá-la.
- Dependências sugeridas: `ureq` com rustls (ou o `reqwest` que o app já tiver), `semver`, `sha2`;
  todas MIT/Apache.
- Delta via zsync no próprio app só se o AppImage crescer muito (hoje ~1–20 MB).

### Outros formatos
| Pacote | Atualização |
|---|---|
| `.deb`, `.rpm`, AUR | Gerenciador de pacotes da distro (repositório próprio/COPR/AUR na fase 5). O app **só avisa** que há versão nova, sem baixar. |
| `.tar.gz` | Só aviso com link. |
| Windows `.zip` | Só aviso com link até o instalador MSI da fase 5; depois, avaliar winget ou atualização pelo MSI. |

O app sabe o formato pelo `APPIMAGE` (AppImage) e por um marcador de build
(`TAILDESK_PACKAGE=deb|rpm|tar|zip`, definido no empacotamento) para o texto do aviso.

## 5. O que a UI precisa (para a thread do app gráfico)

- **Configurações → Atualizações**: "Procurar atualizações automaticamente" (ligado por padrão) e
  canal "Estável" / "Pré-lançamento".
- **Aviso** (bandeja e faixa na tela inicial): "TailDesk 0.2.0 disponível" com "Atualizar agora",
  "Depois" e "Novidades" (abre as notas da release).
- **Progresso** do download e, ao fim, "Reiniciar para concluir".
- Com sessão ativa: "A atualização será instalada quando a sessão terminar."
- Fora do AppImage: "Baixar no GitHub" no lugar de "Atualizar agora" (ou o comando do gerenciador
  de pacotes).
- Textos em pt-BR e en.

## 6. Feito e pendente

- [x] `.upd_info` + `.zsync` em todo AppImage (`build-packages.sh`, `release.yml`) e teste no CI.
- [x] Crate `taildesk-update` e `taildesk-cli atualizar` (procura, baixa, confere o SHA-256 e troca o AppImage).
- [x] UI do atualizador no app (§5): faixa na tela inicial, item na bandeja e Configurações → Atualizações
  (`apps/taildesk/src/atualizacao.rs`, `crates/taildesk-ui/ui/atualizacao.slint`).
- [ ] Assinatura (fase 5).
