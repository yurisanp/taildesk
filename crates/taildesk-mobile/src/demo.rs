//! Modo demonstração (`android-detalhado.md` §8.4): um host de mentira dentro do app, aberto por
//! "Ver como funciona" no onboarding. Serve para as capturas da Play sem tailnet, para a revisão
//! do Google (o revisor não tem tailnet) e para quem ainda não instalou o Tailscale.
//!
//! O host recebe as mesmas mensagens que um host de verdade receberia ([`DemoHost::receber`]) e a
//! tela desenha o "computador" dele (janelas, o texto digitado, o cursor) direto no Slint. Assim
//! toque, faixa de teclas, teclado virtual, zoom e troca de monitor funcionam de verdade.

use std::collections::VecDeque;

use taildesk_keyboard::{KeyMsg, Mods};
use taildesk_proto::control::{CursorPos, DisplayInfo, Msg, Rotation, mouse_move};

/// Nome do computador de mentira.
pub const NOME: &str = "pc-demonstracao";

/// Linhas do registro de eventos (o que o host "recebeu").
const REGISTRO: usize = 6;

/// Estado do host de mentira.
#[derive(Clone, Debug)]
pub struct DemoHost {
    /// Texto do "documento" aberto no monitor 1.
    pub texto: String,
    /// Cursor em fração do monitor.
    pub cursor: (f32, f32),
    /// Monitor que o celular está vendo.
    pub monitor: u32,
    pub cliques: u32,
    /// O que chegou por último ("Ctrl+C", "clique direito"), mais novo no fim.
    pub registro: VecDeque<String>,
    shift_fisico: bool,
}

impl Default for DemoHost {
    fn default() -> Self {
        DemoHost {
            texto: "Prezada Ana,\nsegue o relatório de ação do mês".into(),
            cursor: (0.45, 0.4),
            monitor: 0,
            cliques: 0,
            registro: VecDeque::new(),
            shift_fisico: false,
        }
    }
}

/// Dois monitores, como o notebook com um externo dos roteiros.
pub fn monitores() -> Vec<DisplayInfo> {
    let m = |id: u32, nome: &str, primary: bool, x: i32| DisplayInfo {
        id,
        name: nome.into(),
        x,
        y: 0,
        width: 1920,
        height: 1080,
        logical_width: 1920,
        logical_height: 1080,
        scale: 1.0,
        refresh_mhz: 60_000,
        primary,
        rotation: Rotation::Normal.into(),
        shared: true,
    };
    vec![m(0, "Monitor 1", true, 0), m(1, "Monitor 2", false, 1920)]
}

impl DemoHost {
    fn anotar(&mut self, s: impl Into<String>) {
        if self.registro.len() == REGISTRO {
            self.registro.pop_front();
        }
        self.registro.push_back(s.into());
    }

    /// Uma mensagem do celular. Devolve o que um host mandaria de volta (posição do cursor).
    pub fn receber(&mut self, msg: &Msg) -> Vec<Msg> {
        match msg {
            Msg::StartVideo(s) => {
                self.monitor = s.display_id;
                Vec::new()
            }
            Msg::MouseMove(m) => {
                if let Some(mouse_move::Pos::Abs(p)) = m.pos {
                    self.cursor = crate::zoom::fio_para_fracao(p.x, p.y);
                    return vec![Msg::CursorPos(CursorPos {
                        display_id: p.display_id,
                        x: p.x,
                        y: p.y,
                        visible: true,
                    })];
                }
                Vec::new()
            }
            Msg::MouseButton(b) if b.down => {
                self.cliques += 1;
                let nome = match b.button() {
                    taildesk_proto::control::Button::Right => "clique direito",
                    taildesk_proto::control::Button::Middle => "clique do meio",
                    _ => "clique",
                };
                self.anotar(nome);
                Vec::new()
            }
            Msg::Scroll(s) if s.dx != 0 || s.dy != 0 => {
                self.anotar(format!("rolagem {}", s.dy));
                Vec::new()
            }
            Msg::SecureAttention(_) => {
                self.anotar("Ctrl+Alt+Del");
                Vec::new()
            }
            Msg::ReleaseAll(_) => {
                self.shift_fisico = false;
                Vec::new()
            }
            outro => {
                if let Some(k) = KeyMsg::from_proto(outro) {
                    self.tecla(&k);
                }
                Vec::new()
            }
        }
    }

    fn tecla(&mut self, k: &KeyMsg) {
        match k {
            KeyMsg::Char { text, .. } => self.texto.push_str(text),
            KeyMsg::Shortcut {
                base_char,
                mods,
                down: true,
                ..
            } => {
                let mut s = String::new();
                for (bit, nome) in [
                    (Mods::CTRL, "Ctrl+"),
                    (Mods::ALT, "Alt+"),
                    (Mods::META, "Super+"),
                    (Mods::SHIFT, "Shift+"),
                ] {
                    if mods.contains(bit) {
                        s.push_str(nome);
                    }
                }
                s.push(base_char.to_ascii_uppercase());
                self.anotar(s);
            }
            KeyMsg::Key {
                hid, down: true, ..
            } => match hid {
                0x28 => self.texto.push('\n'),
                0x2A => {
                    self.texto.pop();
                }
                0x2C => self.texto.push(' '),
                0xE1 | 0xE5 => self.shift_fisico = true,
                0xE0..=0xE7 => {}
                h => self.anotar(format!("tecla {h:#04x}")),
            },
            KeyMsg::Key {
                hid: 0xE1 | 0xE5,
                down: false,
                ..
            } => self.shift_fisico = false,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controle::Controle;
    use crate::gestos::Modo;
    use crate::zoom::Retangulo;
    use taildesk_keyboard::virtual_kb::TeclaFaixa;

    #[test]
    fn demonstracao_responde_como_um_host() {
        let mut c = Controle::new(
            Modo::ToqueDireto,
            1.0,
            Retangulo {
                x: 0.0,
                y: 0.0,
                w: 1920.0,
                h: 1080.0,
            },
        );
        let mut host = DemoHost::default();
        let mut out = Vec::new();
        c.monitores_mudaram(&monitores(), &mut out);
        c.texto(" — ação", &mut out);
        c.tecla(TeclaFaixa::Enter, &mut out);
        c.modificador(Mods::CTRL);
        c.texto("s", &mut out);
        c.monitores.trocar(1, None, &mut out);
        c.ctrl_alt_del(&mut out);
        let mut voltas = Vec::new();
        for m in &out {
            voltas.extend(host.receber(m));
        }
        assert!(host.texto.ends_with("do mês — ação\n"), "{:?}", host.texto);
        assert_eq!(host.monitor, 1);
        assert!(host.registro.contains(&"Ctrl+S".to_owned()));
        assert!(
            host.registro.iter().any(|r| r.contains("0x4c")),
            "{:?}",
            host.registro
        );
        assert!(voltas.is_empty());

        // Toque vira cursor e clique.
        let mut out = Vec::new();
        c.toque(
            crate::gestos::Toque {
                id: 0,
                fase: crate::gestos::Fase::Desceu,
                x: 960.0,
                y: 540.0,
                t_ms: 0,
            },
            &mut out,
        );
        c.toque(
            crate::gestos::Toque {
                id: 0,
                fase: crate::gestos::Fase::Subiu,
                x: 960.0,
                y: 540.0,
                t_ms: 50,
            },
            &mut out,
        );
        for m in &out {
            voltas.extend(host.receber(m));
        }
        assert_eq!(host.cliques, 1);
        assert!((host.cursor.0 - 0.5).abs() < 0.01);
        assert!(matches!(voltas[0], Msg::CursorPos(_)));
    }
}
