//! Sondagem de banda pelos datagramas QUIC (`14-selecao-adaptativa.md` §3.2).
//!
//! O remetente manda, degrau por degrau, datagramas `Kind::Probe` cadenciados na taxa pedida;
//! o receptor conta com [`taildesk_media::measure::ProbeReceiver`] e devolve o `ProbeResult`.

use std::io;
use std::time::{Duration, Instant};

use bytes::Bytes;
use taildesk_media::measure::{ProbeReceiver, step_packets};
use taildesk_proto::media::{Flags, HEADER_LEN, Kind, MediaHeader};
use taildesk_proto::net::{ProbeRequest, ProbeResult};
use taildesk_proto::probe::{PROBE_LEN, ProbePayload};

/// Tamanho máximo de um datagrama de sondagem: o menor entre isto e `max_datagram_size()`.
pub const MAX_PROBE_DATAGRAM: usize = 1200;

/// Folga depois do fim previsto da rajada antes de o receptor desistir.
pub const RECEIVE_GRACE: Duration = Duration::from_secs(1);

/// Relógio em µs (u32 com wrap) desde a criação; só diferenças importam.
#[derive(Clone, Copy, Debug)]
pub struct Clock(Instant);

impl Default for Clock {
    fn default() -> Self {
        Clock(Instant::now())
    }
}

impl Clock {
    pub fn now_us(&self) -> u32 {
        // Truncar é o wrap desejado.
        self.0.elapsed().as_micros() as u32
    }
}

/// Tamanho dos datagramas de sondagem nesta conexão.
pub fn datagram_len(conn: &quinn::Connection) -> io::Result<usize> {
    let len = conn
        .max_datagram_size()
        .ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported, "peer sem datagramas QUIC"))?
        .min(MAX_PROBE_DATAGRAM);
    if len < HEADER_LEN + PROBE_LEN {
        return Err(io::Error::other(format!(
            "datagrama máximo de {len} bytes é pequeno demais para a sondagem"
        )));
    }
    Ok(len)
}

/// Quantos datagramas a rajada tentou mandar e quantos o QUIC recusou na hora.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SendStats {
    pub sent: u64,
    pub rejected: u64,
}

/// Manda a rajada descrita em `req`. Cada degrau dura `step_ms`; a cadência é por lotes de até
/// 1 ms, porque o timer do tokio não tem resolução melhor que isso.
pub async fn send_burst(
    conn: &quinn::Connection,
    req: &ProbeRequest,
    clock: &Clock,
) -> io::Result<SendStats> {
    req.validate().map_err(io::Error::other)?;
    let len = datagram_len(conn)?;
    let mut buf = vec![0u8; len];
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
    .write(&mut buf);

    let step_dur = Duration::from_millis(req.step_ms.into());
    let mut stats = SendStats::default();
    for (step, &kbps) in req.steps_kbps.iter().enumerate() {
        let n = step_packets(kbps, req.step_ms, len);
        let start = Instant::now();
        for seq in 0..n {
            let due = step_dur * u32::from(seq) / u32::from(n);
            let elapsed = start.elapsed();
            if due > elapsed + Duration::from_millis(1) {
                tokio::time::sleep(due - elapsed).await;
            }
            let payload = ProbePayload {
                probe_id: req.probe_id,
                step: step as u8,
                seq,
                step_pkts: n,
                send_us: clock.now_us(),
            };
            payload.write(&mut buf[HEADER_LEN..]);
            match conn.send_datagram(Bytes::copy_from_slice(&buf)) {
                Ok(()) => stats.sent += 1,
                Err(quinn::SendDatagramError::ConnectionLost(e)) => {
                    return Err(io::Error::other(e));
                }
                Err(_) => stats.rejected += 1,
            }
        }
        if let Some(rest) = step_dur.checked_sub(start.elapsed()) {
            tokio::time::sleep(rest).await;
        }
    }
    Ok(stats)
}

/// Recebe a rajada descrita em `req` e devolve o resultado por degrau. Termina pouco depois do
/// último datagrama do último degrau, ou [`RECEIVE_GRACE`] depois do fim previsto.
pub async fn receive_burst(
    conn: &quinn::Connection,
    req: &ProbeRequest,
    clock: &Clock,
) -> io::Result<ProbeResult> {
    let mut rx = ProbeReceiver::new(req)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "pedido fora dos limites"))?;
    let expected = Duration::from_millis(u64::from(req.step_ms) * req.steps_kbps.len() as u64);
    let mut deadline = tokio::time::Instant::now() + expected + RECEIVE_GRACE;
    let last_step = req.steps_kbps.len().saturating_sub(1) as u8;
    loop {
        let dgram = match tokio::time::timeout_at(deadline, conn.read_datagram()).await {
            Err(_) => break,
            Ok(Err(e)) => return Err(io::Error::other(e)),
            Ok(Ok(d)) => d,
        };
        let arrival = clock.now_us();
        let Ok((h, payload)) = MediaHeader::parse(&dgram) else {
            continue;
        };
        if h.kind != Kind::Probe {
            continue;
        }
        let Ok(p) = ProbePayload::parse(payload) else {
            continue;
        };
        rx.record(&p, dgram.len(), arrival);
        if p.step == last_step && p.seq + 1 == p.step_pkts {
            // Último datagrama: espera só os atrasados que ainda estejam a caminho.
            deadline = deadline.min(tokio::time::Instant::now() + Duration::from_millis(100));
        }
    }
    Ok(rx.finish())
}
