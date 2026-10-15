//! `taildesk-cli bench-net`: teste de conexão (`docs/context/14-selecao-adaptativa.md` §7).
//!
//! Host:        taildesk-cli bench-net --ouvir [endereço[:porta]]
//! Controlador: taildesk-cli bench-net <host[:porta]> (--pin <sha256> | --inseguro)
//!              [--max-mbps N] [--relay] [--json]

use std::fmt::Write as _;
use std::net::{Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::process::ExitCode;

use taildesk_media::adapt::profile;
use taildesk_proto::DEFAULT_PORT;
use taildesk_proto::net::{NetClass, PathKind};
use taildesk_transport::bench::{self, BenchOptions, BenchOutcome};
use taildesk_transport::tls::{from_hex, to_hex};
use taildesk_transport::{Identity, Pin, client_endpoint, connect, server_endpoint};

pub const USAGE: &str = "uso: taildesk-cli bench-net --ouvir [endereço[:porta]]\n\
     \x20      taildesk-cli bench-net <host[:porta]> (--pin <sha256> | --inseguro) \
     [--max-mbps N] [--relay] [--json]";

#[derive(Debug, PartialEq)]
enum Mode {
    Listen(SocketAddr),
    Run {
        target: String,
        pin: Pin,
        opts: BenchOptions,
        json: bool,
    },
}

fn resolve(s: &str) -> Result<SocketAddr, String> {
    let with_port = if s.parse::<SocketAddr>().is_ok()
        || s.rsplit_once(':')
            .is_some_and(|(h, p)| !h.contains(':') && p.parse::<u16>().is_ok())
    {
        s.to_owned()
    } else if s.contains(':') {
        format!("[{s}]:{DEFAULT_PORT}")
    } else {
        format!("{s}:{DEFAULT_PORT}")
    };
    with_port
        .to_socket_addrs()
        .map_err(|e| format!("não consegui resolver {s}: {e}"))?
        .next()
        .ok_or_else(|| format!("{s} não tem endereço"))
}

fn parse(args: &[String]) -> Result<Mode, String> {
    let mut it = args.iter().map(String::as_str);
    let first = it.next().ok_or(USAGE)?;
    if first == "--ouvir" {
        let addr = match it.next() {
            Some(a) => resolve(a)?,
            None => (Ipv6Addr::UNSPECIFIED, DEFAULT_PORT).into(),
        };
        return Ok(Mode::Listen(addr));
    }
    let mut pin = None;
    let mut opts = BenchOptions::default();
    let mut json = false;
    while let Some(a) = it.next() {
        match a {
            "--pin" => {
                let hex = it.next().ok_or("--pin precisa do fingerprint")?;
                pin = Some(Pin::Sha256(from_hex(hex).ok_or("fingerprint inválido")?));
            }
            "--inseguro" => pin = Some(Pin::Insecure),
            "--max-mbps" => {
                let n: u32 = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .filter(|&n| (1..=1_000).contains(&n))
                    .ok_or("--max-mbps precisa de um número de 1 a 1000")?;
                opts.max_kbps = Some(n * 1_000);
            }
            "--relay" => opts.path = PathKind::Relayed,
            "--json" => json = true,
            other => return Err(format!("opção desconhecida: {other}")),
        }
    }
    let pin = pin.ok_or("informe --pin <sha256> (mostrado pelo host) ou --inseguro")?;
    Ok(Mode::Run {
        target: first.to_owned(),
        pin,
        opts,
        json,
    })
}

fn class_name(c: NetClass) -> &'static str {
    match c {
        NetClass::Lan => "LAN",
        NetClass::Fast => "Rápida",
        NetClass::Medium => "Média",
        NetClass::Slow => "Lenta",
        NetClass::Minimal => "Mínima",
    }
}

fn mbps(kbps: u32) -> String {
    format!("{:.1}", f64::from(kbps) / 1_000.0)
}

fn ms(us: u32) -> String {
    format!("{:.1}", f64::from(us) / 1_000.0)
}

fn text(target: &str, out: &BenchOutcome) -> String {
    let r = &out.report;
    let p = profile(out.class);
    let res = if p.max_height == 0 {
        "resolução nativa".to_owned()
    } else {
        format!("até {}p", p.max_height)
    };
    let sat = |e: Option<taildesk_media::measure::Estimate>| match e {
        Some(e) if !e.saturated => " (ou mais: nenhum degrau saturou)",
        _ => "",
    };
    let mut s = String::new();
    let _ = writeln!(s, "Teste de conexão · {target}");
    let _ = writeln!(
        s,
        "Latência   {} ms (mín {} · p95 {})   Variação {} ms",
        ms(r.rtt_median_us),
        ms(r.rtt_min_us),
        ms(r.rtt_p95_us),
        ms(r.jitter_us)
    );
    let _ = writeln!(
        s,
        "Banda      ↓ {} Mbit/s{}   ↑ {} Mbit/s{}",
        mbps(r.down_kbps),
        sat(out.down_estimate),
        mbps(r.up_kbps),
        sat(out.up_estimate)
    );
    let _ = writeln!(
        s,
        "Perda      {:.2} %   Datagrama máx. {} B",
        f64::from(r.loss_ppm) / 10_000.0,
        r.max_datagram
    );
    let _ = writeln!(
        s,
        "Classe     {} — H.264, {res}, {} fps, até {} Mbit/s",
        class_name(out.class),
        p.fps_max,
        mbps(p.bitrate_max_kbps)
    );
    let _ = write!(
        s,
        "Por quê    banda {} Mbit/s, latência {} ms, perda {:.2} %",
        mbps(r.down_kbps.min(r.up_kbps)),
        ms(r.rtt_median_us),
        f64::from(r.loss_ppm) / 10_000.0
    );
    s
}

fn json(target: &str, out: &BenchOutcome) -> String {
    let r = &out.report;
    let steps = |res: &taildesk_proto::net::ProbeResult| {
        res.steps
            .iter()
            .map(|st| {
                format!(
                    "{{\"sent_kbps\":{},\"recv_kbps\":{},\"loss_ppm\":{},\"owd_delta_us\":{}}}",
                    st.sent_kbps, st.recv_kbps, st.loss_ppm, st.owd_delta_us
                )
            })
            .collect::<Vec<_>>()
            .join(",")
    };
    let target = target.replace(['"', '\\'], "");
    format!(
        "{{\"target\":\"{target}\",\"class\":\"{}\",\"rtt_min_us\":{},\"rtt_median_us\":{},\"rtt_p95_us\":{},\"jitter_us\":{},\"loss_ppm\":{},\"down_kbps\":{},\"up_kbps\":{},\"max_datagram\":{},\"down_steps\":[{}],\"up_steps\":[{}]}}",
        class_name(out.class),
        r.rtt_min_us,
        r.rtt_median_us,
        r.rtt_p95_us,
        r.jitter_us,
        r.loss_ppm,
        r.down_kbps,
        r.up_kbps,
        r.max_datagram,
        steps(&out.down),
        steps(&out.up)
    )
}

async fn listen(addr: SocketAddr) -> Result<(), String> {
    let id = Identity::generate().map_err(|e| e.to_string())?;
    let endpoint =
        server_endpoint(addr, &id).map_err(|e| format!("não consegui abrir {addr}: {e}"))?;
    println!(
        "bench-net ouvindo em {}\nfingerprint: {}",
        endpoint.local_addr().map_err(|e| e.to_string())?,
        to_hex(&id.fingerprint())
    );
    while let Some(incoming) = endpoint.accept().await {
        tokio::spawn(async move {
            let conn = match incoming.await {
                Ok(c) => c,
                Err(e) => return eprintln!("conexão recusada: {e}"),
            };
            let peer = conn.remote_address();
            println!("teste de {peer}");
            if let Err(e) = bench::serve(&conn).await {
                eprintln!("teste de {peer} falhou: {e}");
            }
        });
    }
    Ok(())
}

async fn run(target: &str, pin: Pin, opts: BenchOptions, as_json: bool) -> Result<(), String> {
    let addr = resolve(target)?;
    if pin == Pin::Insecure {
        eprintln!("aviso: --inseguro não confere o certificado do host");
    }
    let endpoint = client_endpoint(addr).map_err(|e| e.to_string())?;
    let conn = connect(&endpoint, addr, pin)
        .await
        .map_err(|e| format!("não consegui conectar a {addr}: {e}"))?;
    let out = bench::run(&conn, opts)
        .await
        .map_err(|e| format!("teste falhou: {e}"))?;
    conn.close(0u32.into(), b"fim");
    endpoint.wait_idle().await;
    if as_json {
        println!("{}", json(target, &out));
    } else {
        println!("{}", text(target, &out));
    }
    Ok(())
}

pub fn main(args: &[String]) -> ExitCode {
    let mode = match parse(args) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("{e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let result = rt.block_on(async {
        match mode {
            Mode::Listen(addr) => listen(addr).await,
            Mode::Run {
                target,
                pin,
                opts,
                json,
            } => run(&target, pin, opts, json).await,
        }
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn parses_listen_and_run() {
        assert_eq!(
            parse(&args(&["--ouvir", "127.0.0.1:9000"])),
            Ok(Mode::Listen("127.0.0.1:9000".parse().unwrap()))
        );
        assert!(matches!(
            parse(&args(&["--ouvir"])),
            Ok(Mode::Listen(a)) if a.port() == DEFAULT_PORT
        ));
        let hex = "ab".repeat(32);
        let Ok(Mode::Run {
            pin, opts, json, ..
        }) = parse(&args(&[
            "100.64.0.2",
            "--pin",
            &hex,
            "--max-mbps",
            "50",
            "--relay",
            "--json",
        ]))
        else {
            panic!("esperava Run");
        };
        assert_eq!(pin, Pin::Sha256([0xab; 32]));
        assert_eq!(opts.max_kbps, Some(50_000));
        assert_eq!(opts.path, PathKind::Relayed);
        assert!(json);
    }

    #[test]
    fn requires_pin_and_valid_options() {
        assert!(parse(&args(&["100.64.0.2"])).is_err());
        assert!(parse(&args(&["100.64.0.2", "--pin", "xyz"])).is_err());
        assert!(parse(&args(&["100.64.0.2", "--inseguro", "--max-mbps", "0"])).is_err());
        assert!(parse(&args(&["100.64.0.2", "--inseguro", "--x"])).is_err());
        assert!(parse(&args(&[])).is_err());
    }

    #[test]
    fn resolves_default_port() {
        assert_eq!(resolve("127.0.0.1").unwrap().port(), DEFAULT_PORT);
        assert_eq!(resolve("127.0.0.1:9").unwrap().port(), 9);
        assert_eq!(resolve("::1").unwrap().port(), DEFAULT_PORT);
        assert_eq!(resolve("[::1]:9").unwrap().port(), 9);
    }
}
