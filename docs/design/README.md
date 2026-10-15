# Design do TailDesk

- [`12-levantamento-ux.md`](12-levantamento-ux.md): levantamento de UX/UI (referências, princípios, telas, fluxos, textos, plano de teste).
- Protótipo estático em Slint: [`apps/taildesk-prototipo`](../../apps/taildesk-prototipo). Só dados de exemplo; nada de rede, captura ou input.

## Rodar o protótipo

```bash
cargo run -p taildesk-prototipo                      # lista de dispositivos; "Conectar" abre a sessão
cargo run -p taildesk-prototipo -- --tela pedido      # pedido de acesso no host
cargo run -p taildesk-prototipo -- --tema escuro      # força o tema (padrão: o do sistema)
```

Telas isoladas (`--tela`): `principal`, `transferencias`, `configuracoes`, `pedido`, `faixa`,
`sessao`, `sessao-arquivos`, `sessao-navegar`, `sessao-relay`, `sessao-esperando`, `sessao-reconectando`.

O que dá para clicar: busca e seleção de dispositivos, Conectar (abre a janela da sessão, que passa
por "Encontrando…" e "Esperando permitir…"), Enviar arquivo, Copiar, abas da janela principal,
"Simular pedido recebido" (pedido de acesso → Permitir → faixa de sessão ativa no host). Na sessão:
menu do teclado (Auto/Físico/Texto), Arquivos, Recolher/Fixar barra, `⋯` com simulações de relay,
queda de conexão e caractere impossível.

## Capturas

| | |
|---|---|
| ![Dispositivos](capturas/principal.png) | ![Dispositivos, tema escuro](capturas/principal-escuro.png) |
| ![Pedido de acesso](capturas/pedido.png) | ![Faixa de sessão ativa no host](capturas/faixa.png) |
| ![Sessão](capturas/sessao.png) | ![Sessão esperando permissão](capturas/sessao-esperando.png) |
| ![Arquivos: transferências](capturas/sessao-arquivos.png) | ![Arquivos: navegar no host, tema escuro](capturas/sessao-navegar-escuro.png) |
| ![Sessão via relay, barra recolhida](capturas/sessao-relay.png) | ![Configurações](capturas/configuracoes.png) |

Geradas no Linux com `Xvfb` + `SLINT_BACKEND=winit-software` + `import -window root`.
