//! Header binário dos datagramas de mídia (little-endian, 16 bytes).
//!
//! | Offset | Tam. | Campo                         |
//! |--------|------|-------------------------------|
//! | 0      | 1    | `ver:2 \| kind:2 \| flags:4`  |
//! | 1      | 1    | `stream_id`                   |
//! | 2      | 2    | `frame_seq`                   |
//! | 4      | 2    | `frag_idx`                    |
//! | 6      | 2    | `frag_count`                  |
//! | 8      | 4    | `pts_us`                      |
//! | 12     | 1    | `fec_group`                   |
//! | 13     | 1    | reservado (0)                 |
//! | 14     | 2    | `codec_aux`                   |
//!
//! Sem alocação: lê e escreve direto no buffer do datagrama.

/// Tamanho do header em bytes.
pub const HEADER_LEN: usize = 16;

/// Versão do header escrita nos 2 bits mais altos do primeiro byte.
pub const HEADER_VERSION: u8 = 1;

/// Tipo de payload do datagrama.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Video = 0,
    Audio = 1,
    Fec = 2,
    Probe = 3,
}

impl Kind {
    fn from_bits(bits: u8) -> Self {
        match bits & 0b11 {
            0 => Kind::Video,
            1 => Kind::Audio,
            2 => Kind::Fec,
            _ => Kind::Probe,
        }
    }
}

/// Flags de 4 bits do header.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Flags(u8);

impl Flags {
    pub const KEY: Flags = Flags(0b0001);
    pub const LAST: Flags = Flags(0b0010);
    pub const DISCARDABLE: Flags = Flags(0b0100);
    pub const CODEC_CHANGE: Flags = Flags(0b1000);

    pub const fn empty() -> Self {
        Flags(0)
    }

    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn contains(self, other: Flags) -> bool {
        self.0 & other.0 == other.0
    }

    #[must_use]
    pub const fn with(self, other: Flags) -> Self {
        Flags(self.0 | other.0)
    }
}

/// Header de um datagrama de mídia.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MediaHeader {
    pub kind: Kind,
    pub flags: Flags,
    pub stream_id: u8,
    pub frame_seq: u16,
    pub frag_idx: u16,
    pub frag_count: u16,
    pub pts_us: u32,
    pub fec_group: u8,
    pub codec_aux: u16,
}

/// Erro ao ler um header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderError {
    /// Buffer menor que [`HEADER_LEN`].
    TooShort(usize),
    /// Versão diferente de [`HEADER_VERSION`].
    BadVersion(u8),
    /// `frag_count == 0` ou `frag_idx >= frag_count`.
    BadFragment { idx: u16, count: u16 },
}

impl std::fmt::Display for HeaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HeaderError::TooShort(n) => write!(f, "datagrama com {n} bytes, mínimo {HEADER_LEN}"),
            HeaderError::BadVersion(v) => write!(f, "versão de header {v} não suportada"),
            HeaderError::BadFragment { idx, count } => {
                write!(f, "fragmento {idx} inválido para total {count}")
            }
        }
    }
}

impl std::error::Error for HeaderError {}

impl MediaHeader {
    /// Escreve o header nos primeiros [`HEADER_LEN`] bytes de `out`.
    ///
    /// # Panics
    /// Se `out` tiver menos de [`HEADER_LEN`] bytes.
    pub fn write(&self, out: &mut [u8]) {
        let out = &mut out[..HEADER_LEN];
        out[0] = (HEADER_VERSION << 6) | ((self.kind as u8) << 4) | (self.flags.bits() & 0x0f);
        out[1] = self.stream_id;
        out[2..4].copy_from_slice(&self.frame_seq.to_le_bytes());
        out[4..6].copy_from_slice(&self.frag_idx.to_le_bytes());
        out[6..8].copy_from_slice(&self.frag_count.to_le_bytes());
        out[8..12].copy_from_slice(&self.pts_us.to_le_bytes());
        out[12] = self.fec_group;
        out[13] = 0;
        out[14..16].copy_from_slice(&self.codec_aux.to_le_bytes());
    }

    /// Lê o header do início de `buf`; devolve o header e o payload.
    pub fn parse(buf: &[u8]) -> Result<(MediaHeader, &[u8]), HeaderError> {
        if buf.len() < HEADER_LEN {
            return Err(HeaderError::TooShort(buf.len()));
        }
        let version = buf[0] >> 6;
        if version != HEADER_VERSION {
            return Err(HeaderError::BadVersion(version));
        }
        let u16_at = |i: usize| u16::from_le_bytes([buf[i], buf[i + 1]]);
        let header = MediaHeader {
            kind: Kind::from_bits(buf[0] >> 4),
            flags: Flags(buf[0] & 0x0f),
            stream_id: buf[1],
            frame_seq: u16_at(2),
            frag_idx: u16_at(4),
            frag_count: u16_at(6),
            pts_us: u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]),
            fec_group: buf[12],
            codec_aux: u16_at(14),
        };
        if header.frag_count == 0 || header.frag_idx >= header.frag_count {
            return Err(HeaderError::BadFragment {
                idx: header.frag_idx,
                count: header.frag_count,
            });
        }
        Ok((header, &buf[HEADER_LEN..]))
    }
}

/// `a` é mais novo que `b` em aritmética serial de 16 bits (RFC 1982), para `frame_seq` com wrap.
pub fn seq_newer(a: u16, b: u16) -> bool {
    a != b && a.wrapping_sub(b) < 0x8000
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> MediaHeader {
        MediaHeader {
            kind: Kind::Video,
            flags: Flags::KEY.with(Flags::LAST),
            stream_id: 3,
            frame_seq: 0xfffe,
            frag_idx: 6,
            frag_count: 7,
            pts_us: 0xdead_beef,
            fec_group: 2,
            codec_aux: 0x1234,
        }
    }

    #[test]
    fn round_trip() {
        let mut buf = [0u8; HEADER_LEN + 3];
        sample().write(&mut buf);
        buf[HEADER_LEN..].copy_from_slice(b"abc");
        let (h, payload) = MediaHeader::parse(&buf).unwrap();
        assert_eq!(h, sample());
        assert!(h.flags.contains(Flags::KEY));
        assert!(!h.flags.contains(Flags::DISCARDABLE));
        assert_eq!(payload, b"abc");
    }

    #[test]
    fn layout_is_little_endian() {
        let mut buf = [0u8; HEADER_LEN];
        sample().write(&mut buf);
        assert_eq!(buf[0], 0b0100_0011);
        assert_eq!(&buf[2..4], &[0xfe, 0xff]);
        assert_eq!(&buf[8..12], &[0xef, 0xbe, 0xad, 0xde]);
    }

    #[test]
    fn rejects_short_buffer() {
        assert_eq!(
            MediaHeader::parse(&[0u8; 15]),
            Err(HeaderError::TooShort(15))
        );
    }

    #[test]
    fn rejects_other_version() {
        let mut buf = [0u8; HEADER_LEN];
        sample().write(&mut buf);
        buf[0] &= 0b0011_1111;
        assert_eq!(MediaHeader::parse(&buf), Err(HeaderError::BadVersion(0)));
    }

    #[test]
    fn rejects_bad_fragment() {
        let mut h = sample();
        h.frag_idx = 7;
        let mut buf = [0u8; HEADER_LEN];
        h.write(&mut buf);
        assert!(matches!(
            MediaHeader::parse(&buf),
            Err(HeaderError::BadFragment { .. })
        ));
    }

    #[test]
    fn serial_comparison_wraps() {
        assert!(seq_newer(1, 0));
        assert!(seq_newer(0, 0xffff));
        assert!(!seq_newer(0xffff, 0));
        assert!(!seq_newer(5, 5));
    }
}
