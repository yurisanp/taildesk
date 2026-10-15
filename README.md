# TailDesk

Acesso remoto (desktop remoto + troca de arquivos) em **Rust**, para **Windows e Linux**, usando a
**tailnet do Tailscale** como rede e identidade. Sem servidor próprio de rendezvous ou relay.

> Estado: esqueleto (fase 0). Nada conecta ainda.

## Documentação
O contexto completo está em [`docs/context/`](docs/context/README.md). Comece por:
- [`10-plano-windows-linux.md`](docs/context/10-plano-windows-linux.md): escopo do MVP, decisões e fases.
- [`11-teclado.md`](docs/context/11-teclado.md): tradução automática entre layouts (US-Intl ↔ ABNT2).
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

## Desenvolvimento
```bash
cargo xtask check                      # fmt + clippy -D warnings + testes (igual ao CI)
cargo run -p taildesk-cli -- --version
```
Toolchain fixa em `rust-toolchain.toml` (1.97, MSRV do crate `tailscale`).

## Licença
A definir.
