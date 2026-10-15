//! Formas do cursor no fio (`07-protocolo.md` §1: stream uni de cursor).
//!
//! O host abre um stream QUIC unidirecional por sessão, que começa com [`STREAM_MAGIC`] e segue
//! com formas, cada uma com o [`ShapeHeader`] de 16 bytes e os pixels RGBA8 (sRGB, alfa não
//! pré-multiplicado, linha a linha, sem preenchimento). A forma vai uma vez só; depois o
//! `CursorShapeRef` do stream de controle aponta para ela pelo hash, e o controlador a guarda em
//! cache. O host manda a forma antes do primeiro `CursorShapeRef` com o hash dela.

/// Início do stream de cursor: `"TDC"`, versão 1.
pub const STREAM_MAGIC: [u8; 4] = *b"TDC\x01";

/// Limites das formas.
pub mod limits {
    /// Largura e altura máximas (o Windows usa até 256×256 com escala de acessibilidade).
    pub const MAX_DIM: u16 = 256;
    /// Formas que o controlador guarda; o host não reenvia as que já mandou dentro deste limite.
    pub const CACHE: usize = 64;
}

/// Cabeçalho de uma forma: `hash`, largura, altura e ponto ativo (little-endian).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ShapeHeader {
    /// Identifica a forma no `CursorShapeRef`; o host escolhe (hash do bitmap).
    pub hash: u64,
    pub width: u16,
    pub height: u16,
    pub hot_x: u16,
    pub hot_y: u16,
}

impl ShapeHeader {
    pub const LEN: usize = 16;

    pub fn encode(&self) -> [u8; Self::LEN] {
        let mut out = [0; Self::LEN];
        out[..8].copy_from_slice(&self.hash.to_le_bytes());
        out[8..10].copy_from_slice(&self.width.to_le_bytes());
        out[10..12].copy_from_slice(&self.height.to_le_bytes());
        out[12..14].copy_from_slice(&self.hot_x.to_le_bytes());
        out[14..].copy_from_slice(&self.hot_y.to_le_bytes());
        out
    }

    /// `None` se o tamanho for zero ou passar de [`limits::MAX_DIM`], ou o ponto ativo cair
    /// fora da forma.
    pub fn decode(buf: &[u8; Self::LEN]) -> Option<ShapeHeader> {
        let u16_em = |i: usize| u16::from_le_bytes([buf[i], buf[i + 1]]);
        let h = ShapeHeader {
            hash: u64::from_le_bytes(buf[..8].try_into().ok()?),
            width: u16_em(8),
            height: u16_em(10),
            hot_x: u16_em(12),
            hot_y: u16_em(14),
        };
        h.is_valid().then_some(h)
    }

    pub fn is_valid(&self) -> bool {
        (1..=limits::MAX_DIM).contains(&self.width)
            && (1..=limits::MAX_DIM).contains(&self.height)
            && self.hot_x < self.width
            && self.hot_y < self.height
    }

    /// Bytes de pixels que seguem o cabeçalho.
    pub fn pixels_len(&self) -> usize {
        usize::from(self.width) * usize::from(self.height) * 4
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shape_header_round_trip() {
        let h = ShapeHeader {
            hash: 0x0123_4567_89ab_cdef,
            width: 32,
            height: 48,
            hot_x: 31,
            hot_y: 0,
        };
        assert_eq!(ShapeHeader::decode(&h.encode()), Some(h));
        assert_eq!(h.pixels_len(), 32 * 48 * 4);
        let max = ShapeHeader {
            width: 256,
            height: 256,
            hot_x: 255,
            hot_y: 255,
            ..h
        };
        assert_eq!(ShapeHeader::decode(&max.encode()), Some(max));
    }

    #[test]
    fn shape_header_rejeita_fora_dos_limites() {
        let ok = ShapeHeader {
            hash: 1,
            width: 16,
            height: 16,
            hot_x: 0,
            hot_y: 0,
        };
        for ruim in [
            ShapeHeader { width: 0, ..ok },
            ShapeHeader { height: 257, ..ok },
            ShapeHeader { hot_x: 16, ..ok },
            ShapeHeader { hot_y: 99, ..ok },
        ] {
            assert_eq!(ShapeHeader::decode(&ruim.encode()), None, "{ruim:?}");
        }
    }
}
