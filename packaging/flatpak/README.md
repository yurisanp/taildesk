# TailDesk em Flatpak (Flathub)

O id é `io.github.yurisanp.TailDesk` em tudo: o app_id da janela no Wayland, o `.desktop`, o
ícone, o MetaInfo e o Flatpak (o Flathub exige os quatro iguais).

## Montar aqui

```bash
packaging/flatpak/gerar-fontes.sh        # cargo-sources.json, a cada mudança no Cargo.lock
flatpak-builder --user --force-clean --install-deps-from=flathub --install \
  build-flatpak packaging/flatpak/io.github.yurisanp.TailDesk.yml
flatpak run io.github.yurisanp.TailDesk
```

O build roda sem rede (como o Flathub exige): o `cargo-sources.json` lista os crates do
`Cargo.lock`, que o flatpak-builder baixa antes. O `appstreamcli compose` roda fora do sandbox e
precisa ler o ícone SVG: no Debian/Ubuntu, `appstream-compose` e `librsvg2-common`.

O workflow `Flatpak` monta o pacote em PRs que mexem no empacotamento e deixa o `.flatpak` como
artefato.

## Permissões

| Permissão | Para quê |
|---|---|
| `wayland`, `fallback-x11`, `ipc`, `dri` | Janela e vídeo pelo wgpu; encode e decode por hardware |
| `network` | QUIC pela tailnet |
| `/run/tailscale:ro` | LocalAPI do `tailscaled` (identidade do peer, IP, rota) |
| `xdg-run/pipewire-0` | Som do sistema (captura no host, reprodução no controlador) |
| `xdg-download/TailDesk:create` | Pasta de entrada dos arquivos recebidos |
| `org.kde.StatusNotifierWatcher`, `org.kde.StatusNotifierItem-2-1` | Ícone na bandeja |
| `org.freedesktop.secrets` | Chave da identidade e token do portal no cofre do sistema |

Tela, mouse e teclado (RemoteDesktop/ScreenCast), área de transferência e escolha de arquivos
vêm pelos portais, sem permissão no manifesto. "Iniciar com o sistema" usa o portal Background
dentro do Flatpak (não há `systemctl` no sandbox).

Numa sessão X11 sem portal (XFCE, i3) o host usa o socket do X que o `fallback-x11` dá ao app
(XShm precisa do `ipc`); ainda não foi testado dentro do Flatpak.

## Publicar no Flathub (feito pelo Yuri)

1. Fork de https://github.com/flathub/flathub e um PR no branch `new-pr` com o manifesto,
   trocando a fonte `type: dir` por `type: git` com a tag da release e o `commit`, e o
   `cargo-sources.json` gerado para essa tag.
2. Depois do aceite, o Flathub cria `flathub/io.github.yurisanp.TailDesk`; cada release nova é
   um PR lá com a tag e o `cargo-sources.json` novos.
3. A verificação do app no Flathub (selo de verificado) usa o domínio `yurisanp.github.io`.

O PyroWave (opcional) ainda não entra no Flatpak.
