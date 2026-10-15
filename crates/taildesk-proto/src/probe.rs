//! Payload dos datagramas de sondagem de banda (`Kind::Probe`, `14-selecao-adaptativa.md` §3.2).
//!
//! Vai logo depois do header de mídia de 16 bytes. O resto do datagrama é enchimento com zeros;
//! o receptor só conta bytes, perda e atraso de ida, nunca decodifica nada além destes 16 bytes.
//!
//! | Offset | Tam. | Campo        | Descrição                                          |
//! |--------|------|--------------|----------------------------------------------------|
//! | 0      | 4    | `probe_id`   | id da sondagem (de `ProbeRequest`)                 |
//! | 4      | 1    | `step`       | índice do degrau de taxa (< [`MAX_STEPS`])          |
//! | 5      | 1    | reservado    | 0                                                  |
//! | 6      | 2    | `seq`        | sequência do datagrama dentro do degrau            |
//! | 8      | 2    | `step_pkts`  | quantos datagramas o degrau envia no total         |
//! | 10     | 2    | reservado    | 0                                                  |
//! | 12     | 4    | `send_us`    | relógio do remetente em µs (u32 com wrap)          |
//!
//! Little-endian, sem alocação.

/// Tamanho do cabeçalho de sondagem (depois do header de mídia).
pub const PROBE_LEN: usize = 16;

/// Máximo de degraus numa sondagem (igual ao limite de `ProbeRequest::steps_kbps`).
pub const MAX_STEPS: u8 = 16;

/// Cabeçalho de um datagrama de sondagem.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProbePayload {
    pub probe_id: u32,
    pub step: u8,
    pub seq: u16,
    pub step_pkts: u16,
    pub send_us: u32,
}

/// Erro ao ler um cabeçalho de sondagem.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeError {
    /// Payload menor que [`PROBE_LEN`].
    TooShort(usize),
    /// `step >= MAX_STEPS`.
    BadStep(u8),
    /// `step_pkts == 0` ou `seq >= step_pkts`.
    BadSeq { seq: u16, step_pkts: u16 },
}

impl std::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProbeError::TooShort(n) => write!(f, "sondagem com {n} bytes, mínimo {PROBE_LEN}"),
            ProbeError::BadStep(s) => write!(f, "degrau {s} acima do limite {MAX_STEPS}"),
            ProbeError::BadSeq { seq, step_pkts } => {
                write!(f, "sequência {seq} inválida para {step_pkts} datagramas")
            }
        }
    }
}

impl std::error::Error for ProbeError {}

impl ProbePayload {
    /// Escreve o cabeçalho nos primeiros [`PROBE_LEN`] bytes de `out`.
    ///
    /// # Panics
    /// Se `out` tiver menos de [`PROBE_LEN`] bytes.
    pub fn write(&self, out: &mut [u8]) {
        let out = &mut out[..PROBE_LEN];
        out[0..4].copy_from_slice(&self.probe_id.to_le_bytes());
        out[4] = self.step;
        out[5] = 0;
        out[6..8].copy_from_slice(&self.seq.to_le_bytes());
        out[8..10].copy_from_slice(&self.step_pkts.to_le_bytes());
        out[10..12].fill(0);
        out[12..16].copy_from_slice(&self.send_us.to_le_bytes());
    }

    /// Lê o cabeçalho do início do payload (já sem o header de mídia).
    pub fn parse(buf: &[u8]) -> Result<ProbePayload, ProbeError> {
        if buf.len() < PROBE_LEN {
            return Err(ProbeError::TooShort(buf.len()));
        }
        let u16_at = |i: usize| u16::from_le_bytes([buf[i], buf[i + 1]]);
        let u32_at = |i: usize| u32::from_le_bytes([buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]);
        let p = ProbePayload {
            probe_id: u32_at(0),
            step: buf[4],
            seq: u16_at(6),
            step_pkts: u16_at(8),
            send_us: u32_at(12),
        };
        if p.step >= MAX_STEPS {
            return Err(ProbeError::BadStep(p.step));
        }
        if p.step_pkts == 0 || p.seq >= p.step_pkts {
            return Err(ProbeError::BadSeq {
                seq: p.seq,
                step_pkts: p.step_pkts,
            });
        }
        Ok(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::{Flags, HEADER_LEN, Kind, MediaHeader};

    fn sample() -> ProbePayload {
        ProbePayload {
            probe_id: 0x0102_0304,
            step: 4,
            seq: 99,
            step_pkts: 100,
            send_us: 0xfeed_beef,
        }
    }

    #[test]
    fn round_trip_inside_media_datagram() {
        let mut dgram = [0xaau8; 1200];
        MediaHeader {
            kind: Kind::Probe,
            flags: Flags::empty(),
            stream_id: 0,
            frame_seq: 0,
            frag_idx: 0,
            frag_count: 1,
            pts_us: 0,
            fec_group: 0,
            codec_aux: 0,
        }
        .write(&mut dgram);
        sample().write(&mut dgram[HEADER_LEN..]);
        let (h, payload) = MediaHeader::parse(&dgram).unwrap();
        assert_eq!(h.kind, Kind::Probe);
        assert_eq!(ProbePayload::parse(payload), Ok(sample()));
    }

    #[test]
    fn layout_is_little_endian_with_zeroed_reserved() {
        let mut buf = [0xffu8; PROBE_LEN];
        sample().write(&mut buf);
        assert_eq!(&buf[0..4], &[0x04, 0x03, 0x02, 0x01]);
        assert_eq!(buf[5], 0);
        assert_eq!(&buf[10..12], &[0, 0]);
        assert_eq!(&buf[12..16], &[0xef, 0xbe, 0xed, 0xfe]);
    }

    #[test]
    fn rejects_short_payload() {
        assert_eq!(
            ProbePayload::parse(&[0u8; 15]),
            Err(ProbeError::TooShort(15))
        );
    }

    #[test]
    fn rejects_step_over_limit() {
        let mut p = sample();
        p.step = MAX_STEPS;
        let mut buf = [0u8; PROBE_LEN];
        p.write(&mut buf);
        assert_eq!(
            ProbePayload::parse(&buf),
            Err(ProbeError::BadStep(MAX_STEPS))
        );
    }

    #[test]
    fn rejects_seq_out_of_range() {
        for (seq, step_pkts) in [(0, 0), (5, 5)] {
            let p = ProbePayload {
                seq,
                step_pkts,
                ..sample()
            };
            let mut buf = [0u8; PROBE_LEN];
            p.write(&mut buf);
            assert_eq!(
                ProbePayload::parse(&buf),
                Err(ProbeError::BadSeq { seq, step_pkts })
            );
        }
    }
}
