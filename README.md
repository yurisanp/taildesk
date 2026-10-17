# TailDesk

Acesso remoto (desktop remoto + troca de arquivos) em **Rust**, para **Windows e Linux** (Ubuntu, Fedora e Arch), usando a
**tailnet do Tailscale** (servidor de controle do Tailscale) como rede e identidade. Sem servidor próprio de rendezvous ou relay.

> Estado: fase 1 em andamento. O app abre com a lista de dispositivos da tailnet (lida do Tailscale do
> sistema) e fica na bandeja; a conexão remota (captura, vídeo e controle) ainda está sendo feita.

O TailDesk usa o **Tailscale instalado no computador** (versão 1.102 ou mais nova), não um embutido.
Fechar a janela deixa o app na bandeja; para sair, use **Sair** no menu do ícone. No GNOME, a bandeja
precisa da extensão AppIndicator (o Ubuntu já traz); sem bandeja, fechar a janela encerra o app.
`taildesk --oculto` abre só na bandeja.

## Documentação
O contexto completo está em [`docs/context/`](docs/context/README.md). Comece por:
- [`10-plano-windows-linux.md`](docs/context/10-plano-windows-linux.md): escopo do MVP, decisões e fases.
- [`11-teclado.md`](docs/context/11-teclado.md): tradução automática entre layouts (US-Intl ↔ ABNT2).
- [`13-multimonitor.md`](docs/context/13-multimonitor.md): multi-monitor (um por vez, janelas separadas).
- [`15-audio.md`](docs/context/15-audio.md): áudio do sistema (Opus, liga/desliga que corta o tráfego).
- [`06-arquitetura.md`](docs/context/06-arquitetura.md) e [`07-protocolo.md`](docs/context/07-protocolo.md).

Os documentos `00`–`09` usam o codinome antigo `halyard`; leia como `taildesk`.

## Workspace
| Crate | Papel |
|---|---|
| `taildesk-proto` | Protocolo de fio; header binário de mídia (pronto, com testes) |
| `taildesk-overlay` | Trait `Overlay` (tailscaled do sistema via LocalAPI, LAN de dev) |
| `taildesk-tsapi` | Cliente da LocalAPI do tailscaled |
| `taildesk-transport` | QUIC (quinn) sobre o overlay |
| `taildesk-codec` | Encoders/decoders (openh264 → Media Foundation / VA-API) |
| `taildesk-media` | Pipelines do host e do controlador |
| `taildesk-platform` | Código por SO: instância única, bandeja, sessão do portal e captura PipeWire no Linux, captura DXGI no Windows, mouse, teclado; segredos nas próximas etapas |
| `taildesk-render` | Vídeo em wgpu |
| `taildesk-files` | Troca de arquivos; validação de caminhos (pronta, com testes) |
| `taildesk-keyboard` | Tradução de layouts; classificação e busca reversa (pronta, com testes) |
| `taildesk-session` | Estados da sessão, autorização, consentimento |
| `taildesk-ui` | Interface Slint do app (janela principal e bandeja) |
| `apps/taildesk` | App desktop (janela + ícone na bandeja) |
| `apps/taildesk-cli` | CLI (`host`, `connect`, `probe`, `bench-net`) |

## Downloads
As builds saem na aba [Releases](https://github.com/yurisanp/taildesk/releases):

| Sistema | Arquivo |
|---|---|
| Qualquer Linux x86_64 | `TailDesk-<versão>-x86_64.AppImage` (universal: `chmod +x` e rode; `./TailDesk...AppImage cli --version` abre a CLI) |
| Ubuntu / Debian | `taildesk_<versão>_amd64.deb` |
| Fedora | `taildesk-<versão>-1.x86_64.rpm` |
| Arch Linux | `taildesk-bin` ou `taildesk-git` no AUR (`packaging/arch/`), repositório COPR, ou o AppImage |
| Outros Linux | `taildesk-<versão>-linux-x86_64.tar.gz` |
| Windows 10/11 x64 | `TailDesk-<versão>-x64.msi` (instalador; põe o `taildesk-cli` no PATH) ou o `taildesk-<versão>-windows-x86_64.zip` portátil |

`SHA256SUMS` acompanha cada versão. Para publicar uma versão: suba a versão no `Cargo.toml`, faça o merge e
`git tag v0.1.0 && git push origin v0.1.0`. Tags com hífen (`v0.1.0-beta.1`) saem como pre-release.

## Desenvolvimento
```bash
cargo xtask check                      # fmt + clippy -D warnings + testes (igual ao CI)
cargo run -p taildesk                  # app gráfico (janela + bandeja)
cargo run -p taildesk-cli -- --version
```
No Linux o build precisa do `fontconfig` e da `libpipewire-0.3` de desenvolvimento, do `libclang`, do `nasm` e do `cmake` (o Opus compila com ele):
`libfontconfig1-dev libpipewire-0.3-dev libclang-dev nasm cmake` (Ubuntu/Debian),
`fontconfig-devel pipewire-devel clang-devel gcc-c++ nasm cmake` (Fedora) ou `fontconfig pipewire clang nasm cmake` (Arch).
Sem o `nasm` o openh264 compila sem SIMD e o encode fica ~4x mais lento (o build avisa). No Windows, ponha no PATH o NASM
(`winget install NASM.NASM`) e o `cmake` (`winget install Kitware.CMake`, ou o que vem no Build Tools do Visual Studio).
`cargo run -p taildesk-cli -- portal [--video]` testa a sessão do portal e a captura de cada monitor; `--video` mede também o encode H.264.
No Windows, `cargo run -p taildesk-cli -- captura-dxgi [segundos] [--video]` lista os monitores e mede a captura de todos.
`taildesk-cli atualizar [--pre] [--instalar]` procura a versão nova e, no AppImage, instala.
O ícone da bandeja é gerado por `packaging/icone-bandeja.py`.
Toolchain fixa em `rust-toolchain.toml` (1.97, MSRV do crate `tailscale`).

## Licença
Open-source sob **MIT OR Apache-2.0**, à sua escolha ([`LICENSE-MIT`](LICENSE-MIT), [`LICENSE-APACHE`](LICENSE-APACHE)).
Contribuições são aceitas sob os mesmos termos.

Dependências com licença própria: Slint (usado sob a Royalty-free License, que pede atribuição na
tela "Sobre"), OpenH264 (binário da Cisco baixado em runtime). Não copiar código do RustDesk (AGPL-3.0).
