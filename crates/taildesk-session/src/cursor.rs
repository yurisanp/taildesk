//! Cursor separado (`10-plano-windows-linux.md` fase 2, `06-arquitetura.md` §7): o host captura
//! o vídeo sem o cursor e manda posição e forma à parte; o controlador desenha o cursor por cima
//! do vídeo na posição do próprio mouse, então o movimento parece instantâneo mesmo com o vídeo
//! atrasado.
//!
//! - [`CursorHost`]: só manda o que mudou e cada forma uma vez ([`taildesk_proto::cursor`]).
//! - [`CursorControlador`]: cache das formas e onde desenhar.

use std::collections::VecDeque;

use taildesk_proto::control::{CursorPos, CursorShapeRef};
use taildesk_proto::cursor::{ShapeHeader, limits};

/// O que fazer com a forma atual do cursor no host.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AcaoForma {
    /// A mesma de antes: nada a mandar.
    Nada,
    /// O controlador já tem: só o `CursorShapeRef`.
    Ref(CursorShapeRef),
    /// Primeiro a forma no stream de cursor, depois o `CursorShapeRef`.
    EnviarERef(CursorShapeRef),
}

/// Lado do host.
#[derive(Debug, Default)]
pub struct CursorHost {
    pos: Option<CursorPos>,
    forma: Option<u64>,
    /// Formas que o controlador tem, da menos para a mais usada.
    enviadas: VecDeque<u64>,
}

impl CursorHost {
    pub fn new() -> Self {
        Self::default()
    }

    /// Posição e visibilidade capturadas agora. `Some` só quando mudaram.
    pub fn posicao(&mut self, pos: CursorPos) -> Option<CursorPos> {
        // Invisível não tem posição que importe.
        let igual = self
            .pos
            .is_some_and(|p| p.visible == pos.visible && (!pos.visible || p == pos));
        if igual {
            return None;
        }
        self.pos = Some(pos);
        Some(pos)
    }

    /// Forma capturada agora, pelo hash do bitmap.
    pub fn forma(&mut self, hash: u64) -> AcaoForma {
        if self.forma == Some(hash) {
            return AcaoForma::Nada;
        }
        self.forma = Some(hash);
        let r = CursorShapeRef { hash };
        if let Some(i) = self.enviadas.iter().position(|&h| h == hash) {
            self.enviadas.remove(i);
            self.enviadas.push_back(hash);
            return AcaoForma::Ref(r);
        }
        if self.enviadas.len() == limits::CACHE {
            self.enviadas.pop_front();
        }
        self.enviadas.push_back(hash);
        AcaoForma::EnviarERef(r)
    }
}

/// Uma forma recebida.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Forma {
    pub header: ShapeHeader,
    /// RGBA8, `header.pixels_len()` bytes.
    pub pixels: Vec<u8>,
}

/// Onde e como desenhar o cursor neste quadro.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Desenho<'a> {
    pub display_id: u32,
    /// Posição normalizada (0..=65535) no monitor; o ponto ativo da forma vai nela.
    pub x: u16,
    pub y: u16,
    /// `None` enquanto a forma não chegou: desenhe a seta padrão.
    pub forma: Option<&'a Forma>,
}

/// Lado do controlador.
#[derive(Debug, Default)]
pub struct CursorControlador {
    pos: Option<CursorPos>,
    atual: Option<u64>,
    /// Da menos para a mais usada. O dobro do cache do host, para absorver a ordem entre o
    /// stream de cursor e o de controle.
    cache: VecDeque<(u64, Forma)>,
}

impl CursorControlador {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forma lida do stream de cursor. `false` se os pixels não batem com o cabeçalho.
    pub fn forma_recebida(&mut self, forma: Forma) -> bool {
        if !forma.header.is_valid() || forma.pixels.len() != forma.header.pixels_len() {
            return false;
        }
        let hash = forma.header.hash;
        self.cache.retain(|(h, _)| *h != hash);
        if self.cache.len() == 2 * limits::CACHE {
            self.cache.pop_front();
        }
        self.cache.push_back((hash, forma));
        true
    }

    pub fn on_shape_ref(&mut self, r: CursorShapeRef) {
        self.atual = Some(r.hash);
        if let Some(i) = self.cache.iter().position(|(h, _)| *h == r.hash)
            && let Some(item) = self.cache.remove(i)
        {
            self.cache.push_back(item);
        }
    }

    pub fn on_pos(&mut self, pos: CursorPos) {
        self.pos = Some(pos);
    }

    /// O que desenhar. `local` é a posição do mouse do controlador sobre o vídeo do monitor
    /// (`display_id`, x, y normalizados), quando está controlando e o mouse está em cima do
    /// vídeo: aí o cursor segue o mouse local sem esperar o host. Fora disso, segue o host.
    /// `None` = não desenhar (cursor escondido no host ou ainda sem posição).
    pub fn desenho(&self, local: Option<(u32, u16, u16)>) -> Option<Desenho<'_>> {
        let pos = self.pos?;
        if !pos.visible {
            return None;
        }
        let (display_id, x, y) = local.unwrap_or((
            pos.display_id,
            pos.x.min(u32::from(u16::MAX)) as u16,
            pos.y.min(u32::from(u16::MAX)) as u16,
        ));
        let forma = self
            .atual
            .and_then(|a| self.cache.iter().find(|(h, _)| *h == a))
            .map(|(_, f)| f);
        Some(Desenho {
            display_id,
            x,
            y,
            forma,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(x: u32, visible: bool) -> CursorPos {
        CursorPos {
            display_id: 0,
            x,
            y: 10,
            visible,
        }
    }

    fn forma(hash: u64) -> Forma {
        let header = ShapeHeader {
            hash,
            width: 2,
            height: 2,
            hot_x: 1,
            hot_y: 1,
        };
        Forma {
            header,
            pixels: vec![0xFF; header.pixels_len()],
        }
    }

    #[test]
    fn host_so_manda_o_que_mudou() {
        let mut h = CursorHost::new();
        assert_eq!(h.posicao(pos(5, true)), Some(pos(5, true)));
        assert_eq!(h.posicao(pos(5, true)), None);
        assert_eq!(h.posicao(pos(6, false)), Some(pos(6, false)));
        assert_eq!(h.posicao(pos(9, false)), None);
        assert_eq!(h.posicao(pos(9, true)), Some(pos(9, true)));

        let r = |hash| CursorShapeRef { hash };
        assert_eq!(h.forma(1), AcaoForma::EnviarERef(r(1)));
        assert_eq!(h.forma(1), AcaoForma::Nada);
        assert_eq!(h.forma(2), AcaoForma::EnviarERef(r(2)));
        assert_eq!(h.forma(1), AcaoForma::Ref(r(1)));
    }

    #[test]
    fn host_reenvia_o_que_saiu_do_cache() {
        let mut h = CursorHost::new();
        for hash in 0..limits::CACHE as u64 + 1 {
            assert!(matches!(h.forma(hash), AcaoForma::EnviarERef(_)));
        }
        // A 0 foi a menos usada e saiu.
        assert!(matches!(h.forma(0), AcaoForma::EnviarERef(_)));
        assert!(matches!(h.forma(limits::CACHE as u64), AcaoForma::Ref(_)));
    }

    #[test]
    fn controlador_desenha_no_mouse_local_e_espera_a_forma() {
        let mut c = CursorControlador::new();
        assert_eq!(c.desenho(None), None);
        c.on_pos(pos(100, true));
        c.on_shape_ref(CursorShapeRef { hash: 7 });
        // A forma ainda não chegou pelo outro stream: seta padrão.
        let d = c.desenho(None).unwrap();
        assert_eq!((d.display_id, d.x, d.y, d.forma), (0, 100, 10, None));
        assert!(c.forma_recebida(forma(7)));
        let d = c.desenho(Some((1, 300, 400))).unwrap();
        assert_eq!((d.display_id, d.x, d.y), (1, 300, 400));
        assert_eq!(d.forma.unwrap().header.hash, 7);
        c.on_pos(pos(100, false));
        assert_eq!(c.desenho(Some((1, 300, 400))), None);
    }

    #[test]
    fn controlador_recusa_pixels_errados() {
        let mut c = CursorControlador::new();
        let mut f = forma(1);
        f.pixels.pop();
        assert!(!c.forma_recebida(f));
        assert!(c.cache.is_empty());
    }

    #[test]
    fn controlador_guarda_tudo_que_o_host_acha_que_ele_tem() {
        let mut h = CursorHost::new();
        let mut c = CursorControlador::new();
        // Sequência com reuso e despejo; tudo que o host só referencia tem que estar no cache.
        let hashes: Vec<u64> = (0..300).map(|i| (i * 7919 % 97) as u64).collect();
        for hash in hashes {
            match h.forma(hash) {
                AcaoForma::Nada => {}
                AcaoForma::Ref(r) => {
                    c.on_shape_ref(r);
                    assert!(c.cache.iter().any(|(x, _)| *x == hash), "{hash}");
                }
                AcaoForma::EnviarERef(r) => {
                    c.forma_recebida(forma(hash));
                    c.on_shape_ref(r);
                }
            }
        }
    }
}
