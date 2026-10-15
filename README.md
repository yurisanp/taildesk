# TailDesk

Acesso remoto (desktop remoto + troca de arquivos) em **Rust**, para **Windows e Linux** (Ubuntu, Fedora e Arch), usando a
**tailnet do Tailscale** (servidor de controle do Tailscale) como rede e identidade. Sem servidor próprio de rendezvous ou relay.

> Estado: esqueleto (fase 0). Nada conecta ainda.

## Documentação
O contexto completo está em [`docs/context/`](docs/context/README.md). Comece por:
- [`10-plano-windows-linux.md`](docs/context/10-plano-windows-linux.md): escopo do MVP, decisões e fases.
- [`11-teclado.md`](docs/context/11-teclado.md): tradução automática entre layouts (US-Intl ↔ ABNT2).
- [`13-multimonitor.md`](docs/context/13-multimonitor.md): multi-monitor (um por vez, janelas separadas).
- [`06-arquitetura.md`](docs/context/06-arquitetura.md) e [`07-protocolo.md`](docs/context/07-protocolo.md).

Os documentos `00`–`09` usam o codinome antigo `halyard`; leia como `taildesk`.

## Workspace
| Crate | Papel |
|---|---|
| `taildesk-proto` | Protocolo de fio; header binário de mídia (pronto, com testes) |
| `taildesk-overlay` | Trait `Overlay` (tailscale-rs embutido, tailscaled do sistema, LAN de dev) |
| `taildesk-tsapi` | Cliente da LocalAPI do tailscaled |
| `taildesk-transport` | QUIC (quinn) sobre o overlay |
| `taildesk-codec` | Encoders/decoders (openh264 → Media Foundation / VA-API) |
| `taildesk-media` | Pipelines do host e do controlador |
| `taildesk-platform` | Captura, input, cursor e segredos por SO |
| `taildesk-render` | Vídeo em wgpu |
| `taildesk-files` | Troca de arquivos; validação de caminhos (pronta, com testes) |
| `taildesk-keyboard` | Tradução de layouts; classificação e busca reversa (pronta, com testes) |
| `taildesk-session` | Estados da sessão, autorização, consentimento |
| `taildesk-ui` | Interface Slint |
| `apps/taildesk` | App desktop |
| `apps/taildesk-cli` | CLI (`host`, `connect`, `probe`, `bench-net`) |

## Downloads
As builds saem na aba [Releases](https://github.com/yurisanp/taildesk/releases):

| Sistema | Arquivo |
|---|---|
| Qualquer Linux x86_64 | `TailDesk-<versão>-x86_64.AppImage` (universal: `chmod +x` e rode; `./TailDesk...AppImage cli --version` abre a CLI) |
| Ubuntu / Debian | `taildesk_<versão>_amd64.deb` |
| Fedora | `taildesk-<versão>-1.x86_64.rpm` |
| Arch Linux | `taildesk-bin` no AUR (modelo em `packaging/arch/PKGBUILD`), ou o AppImage |
| Outros Linux | `taildesk-<versão>-linux-x86_64.tar.gz` |
| Windows 10/11 x64 | `taildesk-<versão>-windows-x86_64.zip` (instalador MSI na fase 5) |

`SHA256SUMS` acompanha cada versão. Para publicar uma versão: suba a versão no `Cargo.toml`, faça o merge e
`git tag v0.1.0 && git push origin v0.1.0`. Tags com hífen (`v0.1.0-beta.1`) saem como pre-release.

## Desenvolvimento
```bash
cargo xtask check                      # fmt + clippy -D warnings + testes (igual ao CI)
cargo run -p taildesk-cli -- --version
```
Toolchain fixa em `rust-toolchain.toml` (1.97, MSRV do crate `tailscale`).

## Licença
Open-source sob **MIT OR Apache-2.0**, à sua escolha ([`LICENSE-MIT`](LICENSE-MIT), [`LICENSE-APACHE`](LICENSE-APACHE)).
Contribuições são aceitas sob os mesmos termos.

Dependências com licença própria: Slint (usado sob a Royalty-free License, que pede atribuição na
tela "Sobre"), OpenH264 (binário da Cisco baixado em runtime). Não copiar código do RustDesk (AGPL-3.0).
