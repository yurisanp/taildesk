//! Teste de conexão do `taildesk-cli bench-net` e da aba "Teste de conexão"
//! (`14-selecao-adaptativa.md` §7): RTT pelo stream de controle e banda nos dois sentidos pela
//! sondagem de datagramas, com a classe de rede que o adaptador escolheria.
//!
//! Enquanto o protobuf não entra, o stream de controle do teste usa quadros simples:
//! `tipo: u8 | tamanho: u16 LE | payload` (no máximo [`MAX_FRAME`] bytes).

use std::io;
use std::time::{Duration, Instant};

use taildesk_media::adapt::AdaptConfig;
use taildesk_media::measure::{Estimate, RttStats, estimate, plan_steps};
use taildesk_proto::net::{BenchReport, NetClass, PathKind, ProbeRequest, ProbeResult, wire};

use crate::probe::{self, Clock};

const PING: u8 = 1;
const PONG: u8 = 2;
/// Cliente pede ao servidor que mande uma rajada (download do ponto de vista do cliente).
const PROBE_DOWN: u8 = 3;
/// Cliente avisa que vai mandar uma rajada; o servidor responde com [`RESULT`].
const PROBE_UP: u8 = 4;
const RESULT: u8 = 5;

/// Maior payload de quadro aceito.
pub const MAX_FRAME: usize = 4096;

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_owned())
}

async fn write_frame(send: &mut quinn::SendStream, kind: u8, payload: &[u8]) -> io::Result<()> {
    let len = u16::try_from(payload.len())
        .ok()
        .filter(|&l| usize::from(l) <= MAX_FRAME)
        .ok_or_else(|| bad("quadro grande demais"))?;
    let mut buf = Vec::with_capacity(3 + payload.len());
    buf.push(kind);
    buf.extend_from_slice(&len.to_le_bytes());
    buf.extend_from_slice(payload);
    send.write_all(&buf).await.map_err(io::Error::other)
}

/// `None` quando o outro lado fechou o stream entre dois quadros.
async fn read_frame(recv: &mut quinn::RecvStream) -> io::Result<Option<(u8, Vec<u8>)>> {
    let mut head = [0u8; 3];
    match recv.read_exact(&mut head).await {
        Ok(()) => {}
        Err(quinn::ReadExactError::FinishedEarly(0)) => return Ok(None),
        // O outro lado fechou a conexão de forma ordenada entre dois quadros.
        Err(quinn::ReadExactError::ReadError(quinn::ReadError::ConnectionLost(
            quinn::ConnectionError::ApplicationClosed(_) | quinn::ConnectionError::LocallyClosed,
        ))) => return Ok(None),
        Err(e) => return Err(io::Error::other(e)),
    }
    let len = usize::from(u16::from_le_bytes([head[1], head[2]]));
    if len > MAX_FRAME {
        return Err(bad("quadro grande demais"));
    }
    let mut payload = vec![0u8; len];
    recv.read_exact(&mut payload)
        .await
        .map_err(io::Error::other)?;
    Ok(Some((head[0], payload)))
}

async fn expect_frame(recv: &mut quinn::RecvStream, kind: u8) -> io::Result<Vec<u8>> {
    match read_frame(recv).await? {
        Some((k, payload)) if k == kind => Ok(payload),
        Some(_) => Err(bad("quadro inesperado")),
        None => Err(bad("stream fechado no meio do teste")),
    }
}

/// Lado do host: atende um teste nesta conexão até o cliente fechar o stream.
pub async fn serve(conn: &quinn::Connection) -> io::Result<()> {
    let (mut send, mut recv) = conn.accept_bi().await.map_err(io::Error::other)?;
    let clock = Clock::default();
    while let Some((kind, payload)) = read_frame(&mut recv).await? {
        match kind {
            PING => write_frame(&mut send, PONG, &payload).await?,
            PROBE_DOWN => {
                let req = wire::decode_probe_request(&payload).map_err(io::Error::other)?;
                probe::send_burst(conn, &req, &clock).await?;
            }
            PROBE_UP => {
                let req = wire::decode_probe_request(&payload).map_err(io::Error::other)?;
                let res = probe::receive_burst(conn, &req, &clock).await?;
                let mut out = Vec::new();
                wire::encode_probe_result(&res, &mut out).map_err(io::Error::other)?;
                write_frame(&mut send, RESULT, &out).await?;
            }
            _ => return Err(bad("tipo de quadro desconhecido")),
        }
    }
    let _ = send.finish();
    Ok(())
}

/// Opções do teste.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BenchOptions {
    pub pings: u8,
    pub step_ms: u32,
    /// Teto da sondagem em kbit/s (ex.: para não saturar um link de dados móveis).
    pub max_kbps: Option<u32>,
    /// Pista do caminho; no relay a sondagem para em 20 Mbit/s.
    pub path: PathKind,
}

impl Default for BenchOptions {
    fn default() -> Self {
        BenchOptions {
            pings: 10,
            step_ms: 100,
            max_kbps: None,
            path: PathKind::Unknown,
        }
    }
}

/// Resultado completo do teste.
#[derive(Clone, Debug)]
pub struct BenchOutcome {
    pub rtt: RttStats,
    pub down: ProbeResult,
    pub up: ProbeResult,
    pub down_estimate: Option<Estimate>,
    pub up_estimate: Option<Estimate>,
    pub max_datagram: usize,
    /// Classe que o adaptador escolheria com estas medições.
    pub class: NetClass,
    pub report: BenchReport,
}

fn steps_for(opts: &BenchOptions) -> Vec<u32> {
    let plan = plan_steps(opts.path);
    match opts.max_kbps {
        None => plan.to_vec(),
        Some(max) => {
            let mut v: Vec<u32> = plan.iter().copied().filter(|&k| k < max).collect();
            v.push(max);
            v
        }
    }
}

fn us(d: Option<Duration>) -> u32 {
    d.map_or(0, |d| u32::try_from(d.as_micros()).unwrap_or(u32::MAX))
}

/// Lado do controlador: roda o teste completo.
pub async fn run(conn: &quinn::Connection, opts: BenchOptions) -> io::Result<BenchOutcome> {
    let (mut send, mut recv) = conn.open_bi().await.map_err(io::Error::other)?;
    let clock = Clock::default();

    let mut rtt = RttStats::default();
    for i in 0..opts.pings.max(1) {
        let t = Instant::now();
        write_frame(&mut send, PING, &u64::from(i).to_le_bytes()).await?;
        expect_frame(&mut recv, PONG).await?;
        rtt.push(t.elapsed());
    }

    let steps = steps_for(&opts);
    let mut buf = Vec::new();

    let down_req = ProbeRequest {
        probe_id: 1,
        steps_kbps: steps.clone(),
        step_ms: opts.step_ms,
    };
    wire::encode_probe_request(&down_req, &mut buf).map_err(io::Error::other)?;
    write_frame(&mut send, PROBE_DOWN, &buf).await?;
    let down = probe::receive_burst(conn, &down_req, &clock).await?;

    let up_req = ProbeRequest {
        probe_id: 2,
        ..down_req
    };
    buf.clear();
    wire::encode_probe_request(&up_req, &mut buf).map_err(io::Error::other)?;
    write_frame(&mut send, PROBE_UP, &buf).await?;
    probe::send_burst(conn, &up_req, &clock).await?;
    let up_payload = expect_frame(&mut recv, RESULT).await?;
    let up = wire::decode_probe_result(&up_payload).map_err(io::Error::other)?;
    // Fecha o nosso lado e espera o host fechar o dele: assim ninguém derruba a conexão no meio.
    let _ = send.finish();
    if read_frame(&mut recv).await?.is_some() {
        return Err(bad("quadro inesperado no fim do teste"));
    }

    let down_estimate = estimate(&down);
    let up_estimate = estimate(&up);
    let kbps = match (down_estimate, up_estimate) {
        (Some(d), Some(u)) => d.kbps.min(u.kbps),
        (Some(e), None) | (None, Some(e)) => e.kbps,
        (None, None) => 0,
    };
    let loss_ppm = down_estimate
        .map_or(0, |e| e.loss_ppm)
        .max(up_estimate.map_or(0, |e| e.loss_ppm));
    let median = rtt.median().unwrap_or_default();
    let class = AdaptConfig::default().classify(kbps, median, loss_ppm, opts.path);
    let max_datagram = probe::datagram_len(conn)?;

    let report = BenchReport {
        rtt_min_us: us(rtt.min()),
        rtt_median_us: us(rtt.median()),
        rtt_p95_us: us(rtt.p95()),
        jitter_us: us(Some(rtt.jitter())),
        loss_ppm,
        up_kbps: up_estimate.map_or(0, |e| e.kbps),
        down_kbps: down_estimate.map_or(0, |e| e.kbps),
        max_datagram: u16::try_from(max_datagram).unwrap_or(u16::MAX),
    };
    Ok(BenchOutcome {
        rtt,
        down,
        up,
        down_estimate,
        up_estimate,
        max_datagram,
        class,
        report,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_kbps_caps_the_plan() {
        let o = BenchOptions {
            max_kbps: Some(30_000),
            ..BenchOptions::default()
        };
        assert_eq!(steps_for(&o), vec![5_000, 20_000, 30_000]);
        let relay = BenchOptions {
            path: PathKind::Relayed,
            ..BenchOptions::default()
        };
        assert!(steps_for(&relay).iter().all(|&k| k <= 20_000));
    }
}
