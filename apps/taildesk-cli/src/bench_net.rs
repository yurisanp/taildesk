//! `taildesk-cli bench-net`: teste de conexão (`docs/context/14-selecao-adaptativa.md` §7).
//!
//! Host:        taildesk-cli bench-net --ouvir [endereço[:porta]]
//! Controlador: taildesk-cli bench-net <nome-ou-ip[:porta]> (--pin <sha256> | --inseguro)
//!              [--max-mbps N] [--relay] [--json]
//!
//! Usa o Tailscale instalado na máquina: o host escuta só no IP da tailnet, o controlador aceita
//! o nome do dispositivo na tailnet e o caminho (direto na LAN, direto pela internet ou relay)
//! vem do `ping` do `tailscaled`.

use std::fmt::Write as _;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::process::ExitCode;

use taildesk_media::adapt::profile;
use taildesk_overlay::{Overlay, PathInfo, SystemOverlay};
use taildesk_proto::DEFAULT_PORT;
use taildesk_proto::net::{NetClass, PathKind};
use taildesk_transport::bench::{self, BenchOptions, BenchOutcome};
use taildesk_transport::tls::{from_hex, to_hex};
use taildesk_transport::{Identity, Pin, client_endpoint, connect, server_endpoint};

pub const USAGE: &str = "uso: taildesk-cli bench-net --ouvir [endereço[:porta]]\n\
     \x20      taildesk-cli bench-net <nome-ou-ip[:porta]> (--pin <sha256> | --inseguro) \
     [--max-mbps N] [--relay] [--json]";

#[derive(Debug, PartialEq)]
enum Mode {
    /// `None` = IP da tailnet desta máquina, porta padrão.
    Listen(Option<SocketAddr>),
    Run {
        target: String,
        pin: Pin,
        opts: BenchOptions,
        /// `--relay` força o caminho em vez de perguntar ao `tailscaled`.
        force_relay: bool,
        json: bool,
    },
}

/// Separa "host", "host:porta", "ip", "ip:porta", "[ipv6]:porta" ou "ipv6".
fn split_target(s: &str) -> Result<(String, u16), String> {
    if let Ok(a) = s.parse::<SocketAddr>() {
        return Ok((a.ip().to_string(), a.port()));
    }
    if s.parse::<IpAddr>().is_ok() {
        return Ok((s.to_owned(), DEFAULT_PORT));
    }
    match s.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => {
            let port = port.parse().map_err(|_| format!("porta inválida em {s}"))?;
            Ok((host.to_owned(), port))
        }
        _ => Ok((s.to_owned(), DEFAULT_PORT)),
    }
}

fn parse(args: &[String]) -> Result<Mode, String> {
    let mut it = args.iter().map(String::as_str);
    let first = it.next().ok_or(USAGE)?;
    if first == "--ouvir" {
        let addr = match it.next() {
            Some(a) => {
                let (host, port) = split_target(a)?;
                let ip: IpAddr = host
                    .parse()
                    .map_err(|_| format!("--ouvir precisa de um IP, não {host}"))?;
                Some((ip, port).into())
            }
            None => None,
        };
        return Ok(Mode::Listen(addr));
    }
    split_target(first)?;
    let mut pin = None;
    let mut opts = BenchOptions::default();
    let mut force_relay = false;
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
            "--relay" => force_relay = true,
            "--json" => json = true,
            other => return Err(format!("opção desconhecida: {other}")),
        }
    }
    let pin = pin.ok_or("informe --pin <sha256> (mostrado pelo host) ou --inseguro")?;
    Ok(Mode::Run {
        target: first.to_owned(),
        pin,
        opts,
        force_relay,
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

fn path_text(p: &PathInfo) -> String {
    match p.kind {
        PathKind::DirectLan => "Direto · mesma rede local".to_owned(),
        PathKind::DirectWan => "Direto pela internet".to_owned(),
        PathKind::Relayed => match &p.derp_region {
            Some(r) => format!("Via relay ({r})"),
            None => "Via relay".to_owned(),
        },
        PathKind::Unknown => "Desconhecido".to_owned(),
    }
}

fn path_code(k: PathKind) -> &'static str {
    match k {
        PathKind::DirectLan => "direct_lan",
        PathKind::DirectWan => "direct_wan",
        PathKind::Relayed => "relayed",
        PathKind::Unknown => "unknown",
    }
}

fn mbps(kbps: u32) -> String {
    format!("{:.1}", f64::from(kbps) / 1_000.0)
}

fn ms(us: u32) -> String {
    format!("{:.1}", f64::from(us) / 1_000.0)
}

fn text(target: &str, path: &PathInfo, out: &BenchOutcome) -> String {
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
    let _ = writeln!(s, "Caminho    {}", path_text(path));
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
        "Por quê    {}, banda {} Mbit/s, latência {} ms, perda {:.2} %",
        path_text(path).to_lowercase(),
        mbps(r.down_kbps.min(r.up_kbps)),
        ms(r.rtt_median_us),
        f64::from(r.loss_ppm) / 10_000.0
    );
    if path.kind == PathKind::Relayed {
        let _ = write!(s, "\n           no relay a classe máxima é Média");
    }
    s
}

fn json(target: &str, path: &PathInfo, out: &BenchOutcome) -> String {
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
    let clean = |s: &str| s.replace(['"', '\\'], "");
    format!(
        "{{\"target\":\"{}\",\"path\":\"{}\",\"derp_region\":\"{}\",\"class\":\"{}\",\"rtt_min_us\":{},\"rtt_median_us\":{},\"rtt_p95_us\":{},\"jitter_us\":{},\"loss_ppm\":{},\"down_kbps\":{},\"up_kbps\":{},\"max_datagram\":{},\"down_steps\":[{}],\"up_steps\":[{}]}}",
        clean(target),
        path_code(path.kind),
        clean(path.derp_region.as_deref().unwrap_or("")),
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

async fn listen(addr: Option<SocketAddr>) -> Result<(), String> {
    let addr = match addr {
        Some(a) => a,
        None => {
            let ip = SystemOverlay::default().local_addr().await.map_err(|e| {
                format!("{e}\nou informe um endereço: bench-net --ouvir <ip[:porta]>")
            })?;
            (ip, DEFAULT_PORT).into()
        }
    };
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

/// Resolve o alvo: IP direto, nome da tailnet pelo `tailscaled`, ou DNS do sistema.
async fn resolve(overlay: &SystemOverlay, target: &str) -> Result<SocketAddr, String> {
    let (host, port) = split_target(target)?;
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok((ip, port).into());
    }
    match overlay.resolve(&host).await {
        Ok(ip) => Ok((ip, port).into()),
        Err(ts_err) => (host.as_str(), port)
            .to_socket_addrs()
            .ok()
            .and_then(|mut a| a.next())
            .ok_or_else(|| format!("não consegui encontrar {host}: {ts_err}")),
    }
}

async fn run(
    target: &str,
    pin: Pin,
    mut opts: BenchOptions,
    force_relay: bool,
    as_json: bool,
) -> Result<(), String> {
    let overlay = SystemOverlay::default();
    let addr = resolve(&overlay, target).await?;
    let path = if force_relay {
        PathInfo {
            kind: PathKind::Relayed,
            ..PathInfo::default()
        }
    } else {
        overlay.path(addr.ip()).await
    };
    opts.path = path.kind;
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
        println!("{}", json(target, &path, &out));
    } else {
        println!("{}", text(target, &path, &out));
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
                force_relay,
                json,
            } => run(&target, pin, opts, force_relay, json).await,
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
            Ok(Mode::Listen(Some("127.0.0.1:9000".parse().unwrap())))
        );
        assert_eq!(parse(&args(&["--ouvir"])), Ok(Mode::Listen(None)));
        assert!(parse(&args(&["--ouvir", "desktop-casa"])).is_err());
        let hex = "ab".repeat(32);
        let Ok(Mode::Run {
            target,
            pin,
            opts,
            force_relay,
            json,
        }) = parse(&args(&[
            "desktop-casa",
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
        assert_eq!(target, "desktop-casa");
        assert_eq!(pin, Pin::Sha256([0xab; 32]));
        assert_eq!(opts.max_kbps, Some(50_000));
        assert!(force_relay && json);
    }

    #[test]
    fn requires_pin_and_valid_options() {
        assert!(parse(&args(&["100.64.0.2"])).is_err());
        assert!(parse(&args(&["100.64.0.2", "--pin", "xyz"])).is_err());
        assert!(parse(&args(&["100.64.0.2", "--inseguro", "--max-mbps", "0"])).is_err());
        assert!(parse(&args(&["100.64.0.2", "--inseguro", "--x"])).is_err());
        assert!(parse(&args(&["host:porta", "--inseguro"])).is_err());
        assert!(parse(&args(&[])).is_err());
    }

    #[test]
    fn splits_targets() {
        let t = |s: &str| split_target(s).unwrap();
        assert_eq!(t("127.0.0.1"), ("127.0.0.1".into(), DEFAULT_PORT));
        assert_eq!(t("127.0.0.1:9"), ("127.0.0.1".into(), 9));
        assert_eq!(t("::1"), ("::1".into(), DEFAULT_PORT));
        assert_eq!(t("[::1]:9"), ("::1".into(), 9));
        assert_eq!(t("desktop-casa"), ("desktop-casa".into(), DEFAULT_PORT));
        assert_eq!(t("desktop-casa:9"), ("desktop-casa".into(), 9));
    }

    #[test]
    fn path_texts() {
        let relay = PathInfo {
            kind: PathKind::Relayed,
            derp_region: Some("gru".into()),
            ..PathInfo::default()
        };
        assert_eq!(path_text(&relay), "Via relay (gru)");
        assert_eq!(path_code(PathKind::DirectLan), "direct_lan");
    }
}
