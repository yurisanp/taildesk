# Contexto — Ferramenta de acesso remoto em Rust sobre Tailscale

> Pacote de arquivos de contexto para guiar o design e a implementação (por humanos e por
> agentes de código) de uma ferramenta de acesso remoto **em Rust**, **multiplataforma**
> (Windows, macOS, Linux, Android, iOS), com a **rede baseada no Tailscale** e suporte a
> **AV1, H.265, H.264, VP9, VP8 e PyroWave**.
>
> Pesquisa feita em **15/10/2026**. Versões de crates e status de projetos mudam rápido —
> revalide antes de travar dependências (ver `09-fontes.md`).

Codinome usado nos exemplos: **`halyard`** (placeholder — troque à vontade).

## Como usar

| Arquivo | Para quê | Quem lê |
|---|---|---|
| [`AGENTS.md`](AGENTS.md) | Regras curtas e decisões travadas para agentes de código (Claude Code, Codex etc.). Copie para a raiz do repo (e/ou como `CLAUDE.md`). | Agente |
| [`00-relatorio-executivo.md`](00-relatorio-executivo.md) | Relatório: o que foi estudado, conclusões, recomendações, riscos. | Humano |
| [`01-tailscale.md`](01-tailscale.md) | Como o Tailscale funciona, `tailscale-rs` (Rust oficial), LocalAPI, ACL/grants, app capabilities, Headscale. | Ambos |
| [`02-rustdesk.md`](02-rustdesk.md) | Engenharia reversa do RustDesk: arquitetura, protocolo protobuf, codecs, QoS, captura, input, licença. | Ambos |
| [`03-codecs.md`](03-codecs.md) | AV1, H.265, H.264, VP9, VP8, PyroWave: características, crates, HW accel, negociação, bitstream. | Ambos |
| [`04-plataformas.md`](04-plataformas.md) | Contexto agnóstico de plataforma: captura, input, áudio, clipboard, HW codec, permissões, rede, empacotamento por SO. | Ambos |
| [`05-stack-rust.md`](05-stack-rust.md) | Bibliotecas escolhidas (UI, render, rede, mídia, sistema) com versões e justificativas. | Ambos |
| [`06-arquitetura.md`](06-arquitetura.md) | Workspace Cargo, traits centrais, pipeline de vídeo, threads, segurança. | Agente |
| [`07-protocolo.md`](07-protocolo.md) | Protocolo de fio proposto (controle + mídia), handshake, negociação, mensagens. | Agente |
| [`08-roadmap.md`](08-roadmap.md) | Fases, critérios de aceite, ADRs, riscos e perguntas em aberto. | Ambos |
| [`10-plano-windows-linux.md`](10-plano-windows-linux.md) | Plano do MVP TailDesk: Windows + Linux, desktop remoto + arquivos. Vale por cima de 00–09 no MVP. | Ambos |
| [`11-teclado.md`](11-teclado.md) | Tradução automática entre layouts de teclado (ex.: US-Intl ↔ ABNT2). | Ambos |
| [`14-selecao-adaptativa.md`](14-selecao-adaptativa.md) | Classes de rede, perfis de mídia, troca adaptativa e aba "Teste de conexão". | Ambos |
| [`09-fontes.md`](09-fontes.md) | Links de referência e versões verificadas. | Ambos |

## Resumo em 10 linhas

1. **Rede**: o Tailscale vira a camada de conectividade + identidade. Sem servidor de rendezvous/relay próprio (o que o RustDesk precisa), sem senha de sessão como mecanismo primário.
2. **Dois modos de rede** atrás de uma trait `Overlay`: *embutido* (`tailscale` crate 0.6.x, userspace, nó próprio por app) e *sistema* (usa o `tailscaled`/app oficial já instalado e consulta a LocalAPI).
3. `tailscale-rs` passou por auditoria de cripto e já faz conexão direta (NAT traversal) em Linux/Windows/macOS — **mas não suporta Android/iOS**; no mobile usa-se o app oficial do Tailscale (modo sistema).
4. **Autorização** via tailnet: tags + grants + *app capabilities* (`seudominio.com/cap/remote`), não via senha.
5. **Codecs** atrás de traits `VideoEncoder`/`VideoDecoder` com negociação por capacidade (modelo do RustDesk) e cadeia de fallback HW → SW.
6. **PyroWave** = modo "LAN/ultra-baixa latência" (intra-only, 100–200+ Mbit/s, GPU, Vulkan + Metal). AV1/H.265/H.264 = modo internet. VP8/VP9 = compatibilidade/SW.
7. **UI**: Slint (desktop + Android estável, iOS em *tech preview*) com superfície de vídeo em **wgpu** (shader YUV→RGB, zero-copy quando possível).
8. **Transporte dentro da tailnet**: QUIC (`quinn`) sobre UDP da tailnet — streams confiáveis para controle/arquivos, datagramas para mídia.
9. **RustDesk é AGPL-3.0** → estudar, não copiar código (a menos que o produto também seja AGPL).
10. **iOS só pode ser cliente (controlador) ou host "view-only"** — o sistema não permite injeção de input.
