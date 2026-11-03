# TailDesk Mobile (Android e iOS) — pacote pronto para executar depois

Pedido do Yuri em 15/10/2026. Só documentação e mockups; **nada mudou no repositório** nem na
publicação atual.

| Arquivo | O que tem |
|---|---|
| [requisitos.md](requisitos.md) | Escopo por plataforma, o que "Tailscale instalado" significa no celular, requisitos `MF-*`/`MNF-*`, lojas, decisões pendentes |
| [arquitetura.md](arquitetura.md) | Núcleo Rust compartilhado, `MobileVpnOverlay`, mudanças de protocolo propostas, decode HW, gestos, host Android, ADRs |
| [plano.md](plano.md) | Fases M0 (spikes com gate) a M5, critérios de pronto com as personas, riscos, brief para a thread que executar |
| [telas/index.html](telas/index.html) | 18 modelos de telas (HTML) |
| [telas/capturas/](telas/capturas/) | PNG de cada tela |

Pontos principais: celular é controlador primeiro (Android e iOS); Android vira host na M4; iOS
nunca pode ser controlado. Sem LocalAPI no celular, a lista de dispositivos vem de um desktop
pareado por QR e o host informa o caminho (direto/relay). Uma VPN por vez no celular.
