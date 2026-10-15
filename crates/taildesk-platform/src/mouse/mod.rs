//! Mouse no host: as mensagens do controlador viram [`EventoMouse`] e cada SO injeta
//! (`07-protocolo.md`, `13-multimonitor.md` §4: posição absoluta é sempre dentro de um monitor).
//!
//! Convenção da rolagem: positivo = para baixo / para a direita (como o libinput). Sem
//! `precise` são dentes de roda; com `precise` são pixels lógicos e `dx = dy = 0` encerra o
//! gesto do touchpad.

use taildesk_proto::control::{Button, Msg, Scroll, mouse_move};

#[cfg(target_os = "linux")]
pub mod portal;
#[cfg(windows)]
pub mod sendinput;

/// Botões que o protocolo conhece.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Botao {
    Esquerdo,
    Direito,
    Meio,
    Voltar,
    Avancar,
}

impl Botao {
    pub const TODOS: [Botao; 5] = [
        Botao::Esquerdo,
        Botao::Direito,
        Botao::Meio,
        Botao::Voltar,
        Botao::Avancar,
    ];

    fn de_proto(b: i32) -> Option<Botao> {
        Some(match Button::try_from(b).ok()? {
            Button::Left => Botao::Esquerdo,
            Button::Right => Botao::Direito,
            Button::Middle => Botao::Meio,
            Button::Back => Botao::Voltar,
            Button::Forward => Botao::Avancar,
            Button::Unspecified => return None,
        })
    }

    /// Código evdev (`BTN_LEFT`…`BTN_EXTRA`).
    pub fn evdev(self) -> i32 {
        match self {
            Botao::Esquerdo => 0x110,
            Botao::Direito => 0x111,
            Botao::Meio => 0x112,
            Botao::Voltar => 0x113,
            Botao::Avancar => 0x114,
        }
    }

    fn bit(self) -> u8 {
        1 << self as u8
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventoMouse {
    /// Posição normalizada (0..=65535) dentro do monitor `display_id`.
    Absoluto {
        display_id: u32,
        x: u16,
        y: u16,
    },
    Relativo {
        dx: i32,
        dy: i32,
    },
    Botao {
        botao: Botao,
        down: bool,
    },
    Rolagem {
        dx: i32,
        dy: i32,
        precisa: bool,
    },
}

impl EventoMouse {
    /// Converte uma mensagem já validada. `None` para mensagens que não são de mouse ou
    /// botão desconhecido.
    pub fn de_msg(msg: &Msg) -> Option<EventoMouse> {
        Some(match msg {
            Msg::MouseMove(m) => match m.pos? {
                mouse_move::Pos::Abs(p) => EventoMouse::Absoluto {
                    display_id: p.display_id,
                    x: p.x.min(u32::from(u16::MAX)) as u16,
                    y: p.y.min(u32::from(u16::MAX)) as u16,
                },
                mouse_move::Pos::Rel(r) => EventoMouse::Relativo { dx: r.dx, dy: r.dy },
            },
            Msg::MouseButton(b) => EventoMouse::Botao {
                botao: Botao::de_proto(b.button)?,
                down: b.down,
            },
            &Msg::Scroll(Scroll { dx, dy, precise }) => EventoMouse::Rolagem {
                dx,
                dy,
                precisa: precise,
            },
            _ => return None,
        })
    }
}

/// Botões apertados no host por esta sessão, para soltar todos ao pausar o controle, perder a
/// conexão ou encerrar (nunca deixar um botão preso no host).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BotoesPresos(u8);

impl BotoesPresos {
    /// Registra o evento. Devolve `false` se ele não muda nada (soltar o que não está preso,
    /// apertar de novo o que já está), e então não precisa ser injetado.
    pub fn registrar(&mut self, ev: &EventoMouse) -> bool {
        let &EventoMouse::Botao { botao, down } = ev else {
            return true;
        };
        let antes = self.0;
        if down {
            self.0 |= botao.bit();
        } else {
            self.0 &= !botao.bit();
        }
        antes != self.0
    }

    /// Eventos para soltar tudo; esvazia o registro.
    pub fn soltar_todos(&mut self) -> impl Iterator<Item = EventoMouse> + use<> {
        let presos = std::mem::take(&mut self.0);
        Botao::TODOS
            .into_iter()
            .filter(move |b| presos & b.bit() != 0)
            .map(|botao| EventoMouse::Botao { botao, down: false })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use taildesk_proto::control::{AbsPos, MouseButton, MouseMove, RelPos};

    #[test]
    fn de_msg() {
        let abs = Msg::MouseMove(MouseMove {
            pos: Some(mouse_move::Pos::Abs(AbsPos {
                display_id: 1,
                x: 65535,
                y: 70000,
            })),
        });
        assert_eq!(
            EventoMouse::de_msg(&abs),
            Some(EventoMouse::Absoluto {
                display_id: 1,
                x: 65535,
                y: 65535
            })
        );
        let rel = Msg::MouseMove(MouseMove {
            pos: Some(mouse_move::Pos::Rel(RelPos { dx: -3, dy: 4 })),
        });
        assert_eq!(
            EventoMouse::de_msg(&rel),
            Some(EventoMouse::Relativo { dx: -3, dy: 4 })
        );
        assert_eq!(
            EventoMouse::de_msg(&Msg::MouseMove(MouseMove { pos: None })),
            None
        );
        let botao =
            |button| EventoMouse::de_msg(&Msg::MouseButton(MouseButton { button, down: true }));
        assert_eq!(
            botao(Button::Back as i32),
            Some(EventoMouse::Botao {
                botao: Botao::Voltar,
                down: true
            })
        );
        assert_eq!(botao(0), None);
        assert_eq!(botao(99), None);
        assert_eq!(
            EventoMouse::de_msg(&Msg::Scroll(Scroll {
                dx: 0,
                dy: -2,
                precise: false
            })),
            Some(EventoMouse::Rolagem {
                dx: 0,
                dy: -2,
                precisa: false
            })
        );
    }

    #[test]
    fn solta_so_o_que_esta_preso() {
        let ev = |botao, down| EventoMouse::Botao { botao, down };
        let mut p = BotoesPresos::default();
        assert!(p.registrar(&ev(Botao::Esquerdo, true)));
        assert!(!p.registrar(&ev(Botao::Esquerdo, true)));
        assert!(p.registrar(&ev(Botao::Avancar, true)));
        assert!(!p.registrar(&ev(Botao::Direito, false)));
        assert!(p.registrar(&EventoMouse::Relativo { dx: 1, dy: 1 }));
        let soltos: Vec<_> = p.soltar_todos().collect();
        assert_eq!(
            soltos,
            [ev(Botao::Esquerdo, false), ev(Botao::Avancar, false)]
        );
        assert_eq!(p.soltar_todos().count(), 0);
    }
}
