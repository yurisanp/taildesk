# AGENTS.md — TailDesk

Instruções para agentes de código. Antes de mudar arquitetura, leia `docs/context/10-plano-windows-linux.md`
(vale por cima de `00`–`09` no MVP) e o documento da área (`06`, `07`, `11`).

## Produto
Desktop remoto + troca de arquivos em Rust, **Windows x64 e Linux x64 (Ubuntu/Debian, Fedora, Arch)** no MVP, rede e identidade pela tailnet do Tailscale.

## Decisões travadas (mudar só com ADR em `docs/context/08-roadmap.md`)
1. Rede só pela trait `Overlay` (`taildesk-overlay`): embutido = crate `tailscale` **=0.6.1**; sistema = tailscaled + LocalAPI (`taildesk-tsapi`). Nunca escrever rendezvous/relay próprio.
2. Transporte QUIC (`quinn`): stream bidi 0 = controle; streams uni = arquivos; datagramas = mídia com header de 16 bytes (`taildesk-proto::media`).
   Vídeo é **por monitor** desde a fase 1: um stream, encoder e `stream_id` por `display_id`; mouse com `display_id` (`13-multimonitor.md`).
   Áudio (fase 4): `kind=1`, `stream_id` ≥ 0xF0; sem `Permissions.audio` + `StartAudio` aceito o host não captura nem envia nada (`15-audio.md`).
3. Identidade do peer vem de `Overlay::identify()`, nunca do conteúdo das mensagens.
4. Vídeo: openh264 primeiro, depois Media Foundation (Windows) e VA-API (Linux), sempre com probe + fallback.
5. Linux host: Wayland (portal + PipeWire, input pelo portal/libei) é caminho principal; X11 secundário.
6. Teclado: modo Auto de `11-teclado.md` (físico para teclas sem texto, atalho pela letra, caractere com teclas mortas resolvidas no controlador e busca reversa no host). Nunca assumir layouts iguais.
7. Arquivos: todo caminho do peer passa por `taildesk_files::resolve_in_root`; escrita em `.part` + rename; nunca sobrescrever.
8. UI Slint + vídeo wgpu. Código de SO só em `taildesk-platform`, backends de codec, interop do render e overlay.
9. Não copiar código do RustDesk (AGPL-3.0).

## Regras
- Licença MIT OR Apache-2.0. Dependências novas precisam ser compatíveis (MIT, Apache-2.0, BSD, ISC, Zlib, MPL-2.0); GPL/AGPL só com ADR.
- Rede: servidor de controle do Tailscale (SaaS); Headscale não é alvo do MVP.
- Edição 2024, toolchain 1.97. Todo `unsafe` com `// SAFETY:`.
- Hot path de mídia sem alocação, sem lock contendido, sem `await` em thread de encode/decode; canais bounded "latest-wins".
- Datagramas ≤ `max_datagram_size()` do quinn; nunca assumir 1500.
- Parsers de mensagens com limites de tamanho e testes de round-trip.
- Segredos só via `keyring`; nunca logar chaves ou auth keys.
- Textos de UI em pt-BR e en.

## Comandos
```bash
cargo xtask check   # fmt + clippy -D warnings + testes
```

## Pronto quando
`cargo xtask check` passa no Linux e no Windows (CI); mudança de protocolo tem teste de round-trip;
mudança de teclado tem caso novo nos fixtures de `taildesk-keyboard`.
