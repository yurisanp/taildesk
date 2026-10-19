//! Teclado e mouse numa sessão X11 pelo XTest (`11-teclado.md` §2.4, `13-multimonitor.md` §4.3).
//!
//! - Teclas pelo código físico: keycode do X = evdev + 8, e o layout do host decide o caractere,
//!   como a busca reversa de `taildesk_keyboard::Host` espera.
//! - Caractere fora do layout ([`Inject::Unicode`]): o keysym vai para um keycode sem nada
//!   mapeado (o X sempre tem sobra entre 8 e 255), que é apertado e solto. Uma roda de keycodes
//!   livres evita reescrever o mapa de uma tecla que o cliente ainda não leu.
//! - Mouse: posição absoluta no retângulo do monitor dentro da raiz; botões 1–3 e 8–9; a roda
//!   são os botões 4–7, um clique por dente (a rolagem precisa, em pixels, acumula 15 px por
//!   dente).

use std::io;
use std::time::{Duration, Instant};

use taildesk_keyboard::Inject;
use taildesk_keyboard::codes::hid_to_evdev;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{self, ConnectionExt as _};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use crate::mouse::{Botao, EventoMouse};
use crate::teclado::portal::keysym_de;
use crate::x11::{MonitorX11, conectar, erro, listar_na};

/// Pixels de rolagem precisa por dente de roda.
const PIXELS_POR_DENTE: i32 = 15;
/// Quantos keycodes livres a roda dos caracteres fora do layout usa.
const RODA: usize = 8;
/// Relê os monitores depois disso (um monitor ligado ou desligado muda os retângulos).
const RELER_MONITORES: Duration = Duration::from_secs(2);

/// Botão do X para cada botão do protocolo.
pub(crate) fn botao_x(b: Botao) -> u8 {
    match b {
        Botao::Esquerdo => 1,
        Botao::Meio => 2,
        Botao::Direito => 3,
        Botao::Voltar => 8,
        Botao::Avancar => 9,
    }
}

/// Acumulador da rolagem: devolve os botões da roda a clicar (4 = cima, 5 = baixo,
/// 6 = esquerda, 7 = direita) e guarda o resto da rolagem precisa.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Roda {
    resto: (i32, i32),
}

impl Roda {
    pub(crate) fn rolar(&mut self, dx: i32, dy: i32, precisa: bool) -> Vec<u8> {
        let (dentes_x, dentes_y) = if precisa {
            if dx == 0 && dy == 0 {
                // Fim do gesto do touchpad: o resto morre.
                self.resto = (0, 0);
                return Vec::new();
            }
            self.resto.0 += dx;
            self.resto.1 += dy;
            let d = (
                self.resto.0 / PIXELS_POR_DENTE,
                self.resto.1 / PIXELS_POR_DENTE,
            );
            self.resto.0 -= d.0 * PIXELS_POR_DENTE;
            self.resto.1 -= d.1 * PIXELS_POR_DENTE;
            d
        } else {
            (dx, dy)
        };
        let mut b = Vec::new();
        let vezes = |n: i32| n.unsigned_abs().min(64) as usize;
        b.extend(std::iter::repeat_n(
            if dentes_y > 0 { 5 } else { 4 },
            vezes(dentes_y),
        ));
        b.extend(std::iter::repeat_n(
            if dentes_x > 0 { 7 } else { 6 },
            vezes(dentes_x),
        ));
        b
    }
}

/// Keycodes sem nenhum keysym no mapa (`keysyms_por_keycode` por keycode, a partir de
/// `primeiro`), até `max`.
pub(crate) fn keycodes_livres(
    mapa: &[u32],
    primeiro: u8,
    keysyms_por_keycode: u8,
    max: usize,
) -> Vec<u8> {
    let por = usize::from(keysyms_por_keycode.max(1));
    mapa.chunks(por)
        .enumerate()
        .filter(|(_, ks)| ks.iter().all(|&k| k == 0))
        .map(|(i, _)| primeiro as usize + i)
        .filter(|&k| k <= 255)
        .map(|k| k as u8)
        .rev()
        .take(max)
        .collect()
}

/// Injetor de uma sessão X11.
pub struct EntradaX11 {
    conn: RustConnection,
    tela: usize,
    raiz: xproto::Window,
    monitores: Vec<MonitorX11>,
    lidos: Instant,
    livres: Vec<u8>,
    proximo_livre: usize,
    roda: Roda,
}

impl std::fmt::Debug for EntradaX11 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EntradaX11")
            .field("monitores", &self.monitores)
            .finish_non_exhaustive()
    }
}

impl EntradaX11 {
    /// Conecta no `DISPLAY` e confere que o servidor tem XTest.
    pub fn abrir() -> io::Result<EntradaX11> {
        let (conn, tela) = conectar()?;
        conn.xtest_get_version(2, 2)
            .map_err(erro)?
            .reply()
            .map_err(|e| io::Error::other(format!("X11 sem XTest: {e}")))?;
        let raiz = conn.setup().roots[tela].root;
        let monitores = listar_na(&conn, tela)?;
        let (min, max) = (conn.setup().min_keycode, conn.setup().max_keycode);
        let livres = conn
            .get_keyboard_mapping(min, max - min + 1)
            .map_err(erro)?
            .reply()
            .map(|r| keycodes_livres(&r.keysyms, min, r.keysyms_per_keycode, RODA))
            .unwrap_or_default();
        Ok(EntradaX11 {
            conn,
            tela,
            raiz,
            monitores,
            lidos: Instant::now(),
            livres,
            proximo_livre: 0,
            roda: Roda::default(),
        })
    }

    fn falso(&self, tipo: u8, detalhe: u8, x: i16, y: i16) -> io::Result<()> {
        self.conn
            .xtest_fake_input(tipo, detalhe, x11rb::CURRENT_TIME, self.raiz, x, y, 0)
            .map_err(erro)?;
        Ok(())
    }

    fn tecla(&self, keycode: u8, down: bool) -> io::Result<()> {
        let tipo = if down {
            xproto::KEY_PRESS_EVENT
        } else {
            xproto::KEY_RELEASE_EVENT
        };
        self.falso(tipo, keycode, 0, 0)
    }

    /// Digita um caractere que o layout não tem, por um keycode livre remapeado.
    fn caractere(&mut self, c: char) -> io::Result<()> {
        let Some(keysym) = keysym_de(c) else {
            return Ok(());
        };
        let Some(&kc) = self
            .livres
            .get(self.proximo_livre % self.livres.len().max(1))
        else {
            return Ok(());
        };
        self.proximo_livre = self.proximo_livre.wrapping_add(1);
        let ks = keysym as u32;
        self.conn
            .change_keyboard_mapping(1, kc, 2, &[ks, ks])
            .map_err(erro)?;
        // O servidor manda o MappingNotify antes do evento da tecla.
        self.conn.sync().map_err(erro)?;
        self.tecla(kc, true)?;
        self.tecla(kc, false)
    }

    /// Injeta na ordem. Teclas sem código evdev são puladas.
    pub fn teclas(&mut self, eventos: &[Inject]) -> io::Result<()> {
        for ev in eventos {
            match *ev {
                Inject::Key { hid, down } => {
                    if let Some(kc) = hid_to_evdev(hid).and_then(|e| u8::try_from(e + 8).ok()) {
                        self.tecla(kc, down)?;
                    }
                }
                Inject::Unicode(c) => self.caractere(c)?,
            }
        }
        self.conn.flush().map_err(erro)
    }

    fn monitor(&mut self, display_id: u32) -> Option<MonitorX11> {
        let conhecido = self.monitores.iter().any(|m| m.display_id == display_id);
        if !conhecido || self.lidos.elapsed() >= RELER_MONITORES {
            self.lidos = Instant::now();
            if let Ok(ms) = listar_na(&self.conn, self.tela) {
                self.monitores = ms;
            }
        }
        self.monitores
            .iter()
            .find(|m| m.display_id == display_id)
            .or_else(|| self.monitores.first())
            .cloned()
    }

    /// Um evento de mouse.
    pub fn mouse(&mut self, ev: &EventoMouse) -> io::Result<()> {
        match *ev {
            EventoMouse::Absoluto { display_id, x, y } => {
                if let Some(m) = self.monitor(display_id) {
                    let (rx, ry) = m.ponto(x, y);
                    self.falso(xproto::MOTION_NOTIFY_EVENT, 0, rx, ry)?;
                }
            }
            EventoMouse::Relativo { dx, dy } => {
                let c = |v: i32| v.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
                self.falso(xproto::MOTION_NOTIFY_EVENT, 1, c(dx), c(dy))?;
            }
            EventoMouse::Botao { botao, down } => {
                let tipo = if down {
                    xproto::BUTTON_PRESS_EVENT
                } else {
                    xproto::BUTTON_RELEASE_EVENT
                };
                self.falso(tipo, botao_x(botao), 0, 0)?;
            }
            EventoMouse::Rolagem { dx, dy, precisa } => {
                for b in self.roda.rolar(dx, dy, precisa) {
                    self.falso(xproto::BUTTON_PRESS_EVENT, b, 0, 0)?;
                    self.falso(xproto::BUTTON_RELEASE_EVENT, b, 0, 0)?;
                }
            }
        }
        self.conn.flush().map_err(erro)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn botoes_do_x() {
        assert_eq!(botao_x(Botao::Esquerdo), 1);
        assert_eq!(botao_x(Botao::Direito), 3);
        assert_eq!(botao_x(Botao::Meio), 2);
        assert_eq!(botao_x(Botao::Voltar), 8);
        assert_eq!(botao_x(Botao::Avancar), 9);
    }

    #[test]
    fn roda_em_dentes_e_pixels() {
        let mut r = Roda::default();
        assert_eq!(r.rolar(0, 2, false), [5, 5]);
        assert_eq!(r.rolar(-1, -1, false), [4, 6]);
        // Precisa: 10 px não chega a um dente; mais 10 completa um e sobram 5.
        assert!(r.rolar(0, 10, true).is_empty());
        assert_eq!(r.rolar(0, 10, true), [5]);
        assert_eq!(r.resto, (0, 5));
        // Fim do gesto zera o resto.
        assert!(r.rolar(0, 0, true).is_empty());
        assert_eq!(r.resto, (0, 0));
        // Rajada absurda é limitada.
        assert_eq!(r.rolar(0, 10_000, false).len(), 64);
    }

    #[test]
    fn keycodes_sem_nada_mapeado() {
        // 3 keycodes a partir do 8, 2 keysyms cada: o 9 está vazio, o 10 não.
        let mapa = [0x61, 0x41, 0, 0, 0x62, 0];
        assert_eq!(keycodes_livres(&mapa, 8, 2, 4), [9]);
        let vazio = [0u32; 8];
        // Os de cima primeiro, até o máximo pedido.
        assert_eq!(keycodes_livres(&vazio, 8, 2, 2), [11, 10]);
    }

    /// Com um servidor X (`DISPLAY`), move o ponteiro para um ponto do primeiro monitor e
    /// confere com `QueryPointer`. Sem X ou sem XTest, passa sem testar.
    #[test]
    fn move_o_ponteiro_no_servidor_x() {
        if std::env::var_os("DISPLAY").is_none() {
            return;
        }
        let Ok(mut e) = EntradaX11::abrir() else {
            return;
        };
        let m = e.monitores[0].clone();
        e.mouse(&EventoMouse::Absoluto {
            display_id: m.display_id,
            x: 32768,
            y: 16384,
        })
        .unwrap();
        let p = e.conn.query_pointer(e.raiz).unwrap().reply().unwrap();
        assert_eq!((i32::from(p.root_x), i32::from(p.root_y)), {
            let (x, y) = m.ponto(32768, 16384);
            (i32::from(x), i32::from(y))
        });
        // Caractere fora do layout não derruba nada.
        e.teclas(&[Inject::Unicode('€')]).unwrap();
    }
}
