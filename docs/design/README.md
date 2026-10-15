# Design do TailDesk

- [`12-levantamento-ux.md`](12-levantamento-ux.md): levantamento de UX/UI (referências, princípios, telas, fluxos, textos, plano de teste).
- Protótipo estático em Slint: [`apps/taildesk-prototipo`](../../apps/taildesk-prototipo). Só dados de exemplo; nada de rede, captura ou input.

## Rodar o protótipo

```bash
cargo run -p taildesk-prototipo                      # lista de dispositivos; "Conectar" abre a sessão
cargo run -p taildesk-prototipo -- --tela boas-vindas # primeira execução: detecta o Tailscale do sistema
cargo run -p taildesk-prototipo -- --tela pedido      # pedido de acesso no host
cargo run -p taildesk-prototipo -- --tema escuro      # força o tema (padrão: o do sistema)
```

Telas isoladas (`--tela`): `boas-vindas`, `boas-vindas-instalar`, `boas-vindas-entrar`, `boas-vindas-windows`,
`boas-vindas-conectado`, `boas-vindas-acesso`, `boas-vindas-wayland`, `tailscale-parado`, `principal`, `teste`, `diagnostico`, `sessao-som`, `transferencias`, `configuracoes`, `pedido`, `faixa`,
`sessao`, `sessao-monitores`, `sessao-wayland`, `sessao-com-janela`, `sessao-destacada`, `sessao-arquivos`, `sessao-navegar`, `sessao-relay`, `sessao-esperando`, `sessao-reconectando`.

O TailDesk não embute o Tailscale: usa o que está instalado no computador. Em `boas-vindas`, o passo 1
começa em "não instalado" e a verificação automática (a cada 5 s, ou "Verificar de novo") simula a
instalação e o login até "conectado"; os passos 2 e 3 perguntam se este PC aceita conexões e avisam do
diálogo do sistema no Wayland. "Simular Tailscale parado" (barra lateral) mostra o aviso da janela
principal, e Configurações → Rede tem "Abrir o Tailscale" e "Primeira execução".

O que dá para clicar: busca e seleção de dispositivos, Conectar (abre a janela da sessão, que passa
por "Encontrando…" e "Esperando permitir…"), Enviar arquivo, Copiar, abas da janela principal,
"Testar conexão" (aba de teste com perfis e simulação de rede), "Simular pedido recebido" (pedido de
acesso → Permitir → faixa de sessão ativa no host, com "Desligar som"). Na sessão:
abas de monitores (⧉ abre o monitor em janela própria; fechar a janela devolve a aba;
`▾` mostra o layout do host e "Seguir o mouse"), menu do teclado (Auto/Físico/Texto), Som (liga/desliga, volume local, perfil), Arquivos, Recolher/Fixar barra, `⋯` com Diagnóstico da conexão e simulações de relay,
queda de conexão e caractere impossível.

## Capturas

| | |
|---|---|
| ![Primeira execução: Tailscale não instalado](capturas/boas-vindas-instalar.png) | ![Primeira execução: Tailscale desconectado no Linux, tema escuro](capturas/boas-vindas-entrar-escuro.png) |
| ![Primeira execução: Tailscale desconectado no Windows](capturas/boas-vindas-windows.png) | ![Primeira execução: Tailscale conectado](capturas/boas-vindas-conectado.png) |
| ![Primeira execução: acesso a este PC](capturas/boas-vindas-acesso.png) | ![Tailscale parou durante o uso](capturas/tailscale-parado.png) |
| ![Dispositivos](capturas/principal.png) | ![Dispositivos, tema escuro](capturas/principal-escuro.png) |
| ![Pedido de acesso](capturas/pedido.png) | ![Faixa de sessão ativa no host](capturas/faixa.png) |
| ![Sessão](capturas/sessao.png) | ![Sessão esperando permissão](capturas/sessao-esperando.png) |
| ![Arquivos: transferências](capturas/sessao-arquivos.png) | ![Arquivos: navegar no host, tema escuro](capturas/sessao-navegar-escuro.png) |
| ![Sessão via relay, barra recolhida](capturas/sessao-relay.png) | ![Configurações](capturas/configuracoes.png) |
| ![Menu de monitores](capturas/sessao-monitores.png) | ![Host Wayland com monitor não liberado, tema escuro](capturas/sessao-wayland-escuro.png) |
| ![Aba do monitor 2 aberta em janela](capturas/sessao-com-janela.png) | ![Janela própria do monitor 2](capturas/sessao-destacada.png) |
| ![Teste de conexão](capturas/teste-conexao.png) | ![Som do computador remoto e HUD com a classe de rede](capturas/sessao-som.png) |

Geradas no Linux com `Xvfb` + `SLINT_BACKEND=winit-software` + `import -window root`.
