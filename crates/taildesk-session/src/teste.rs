//! Teste de conexão (`14-selecao-adaptativa.md` §7): o host aceita pedidos de quem a ACL da
//! tailnet já deixa chegar, sem diálogo (o teste não mostra a tela nem controla nada), no máximo
//! um a cada [`INTERVALO_TESTE`] por dispositivo; o controlador mede com
//! `taildesk_transport::bench` e mostra o resultado com [`linhas`] e [`relatorio`].

use std::collections::HashMap;
use std::fmt::Write as _;
use std::time::{Duration, Instant};

use taildesk_media::adapt::profile;
use taildesk_media::measure::Estimate;
use taildesk_proto::control::{Codec, CodecCap, PathInfo};
use taildesk_proto::net::{NetClass, PathKind};
use taildesk_transport::bench::BenchOutcome;

/// Um teste por dispositivo a cada 10 s.
pub const INTERVALO_TESTE: Duration = Duration::from_secs(10);
/// O host corta o teste que passar disto (a sondagem leva uns 15 s).
pub const DURACAO_MAXIMA: Duration = Duration::from_secs(60);
/// Dispositivos lembrados pelo limite (os mais antigos saem antes de passar disto).
const LEMBRADOS: usize = 256;

/// Limite de testes por dispositivo (pelo `node_stable_id` da tailnet).
#[derive(Debug, Default)]
pub struct LimiteTestes {
    ultimo: HashMap<String, Instant>,
}

impl LimiteTestes {
    /// `true` = pode testar agora (e conta este pedido).
    pub fn liberar(&mut self, dispositivo: &str, agora: Instant) -> bool {
        self.ultimo
            .retain(|_, t| agora.saturating_duration_since(*t) < INTERVALO_TESTE);
        if self.ultimo.contains_key(dispositivo) || self.ultimo.len() >= LEMBRADOS {
            return false;
        }
        self.ultimo.insert(dispositivo.to_owned(), agora);
        true
    }
}

/// O que um teste mediu.
#[derive(Clone, Debug)]
pub struct Resultado {
    /// Nome que o host deu no `TestReady`.
    pub host: String,
    /// Caminho visto pelo `tailscaled` do host; `None` = ele não soube dizer.
    pub caminho: Option<PathInfo>,
    /// Encoders do host.
    pub encode_caps: Vec<CodecCap>,
    /// Do pedido até a conexão QUIC pronta.
    pub conectar: Duration,
    pub bench: BenchOutcome,
}

fn t(en: bool, pt: &'static str, ingles: &'static str) -> &'static str {
    if en { ingles } else { pt }
}

fn nome_classe(c: NetClass, en: bool) -> &'static str {
    match c {
        NetClass::Lan => "LAN",
        NetClass::Fast => t(en, "Rápida", "Fast"),
        NetClass::Medium => t(en, "Média", "Medium"),
        NetClass::Slow => t(en, "Lenta", "Slow"),
        NetClass::Minimal => t(en, "Mínima", "Minimal"),
    }
}

fn texto_caminho(c: Option<&PathInfo>, en: bool) -> String {
    let Some(c) = c else {
        return t(en, "Desconhecido", "Unknown").to_owned();
    };
    match c.path_kind() {
        PathKind::DirectLan => t(
            en,
            "Direto · mesma rede local",
            "Direct · same local network",
        ),
        PathKind::DirectWan => t(en, "Direto pela internet", "Direct over the internet"),
        PathKind::Relayed if !c.relay_region.is_empty() => {
            return format!("{} ({})", t(en, "Via relay", "Via relay"), c.relay_region);
        }
        PathKind::Relayed => t(en, "Via relay", "Via relay"),
        PathKind::Unknown => t(en, "Desconhecido", "Unknown"),
    }
    .to_owned()
}

fn nome_codec(c: Codec) -> &'static str {
    match c {
        Codec::H264 => "H.264",
        Codec::Hevc => "HEVC",
        Codec::Av1 => "AV1",
        Codec::Vp8 => "VP8",
        Codec::Vp9 => "VP9",
        Codec::PyroWave => "PyroWave",
        Codec::Unspecified => "?",
    }
}

/// "H.264 HW, AV1" (sem repetir); "—" sem nenhum.
pub fn texto_codecs(caps: &[CodecCap], en: bool) -> String {
    let mut nomes: Vec<String> = Vec::new();
    for c in caps {
        let hw = if c.hw {
            t(en, " por hardware", " hardware")
        } else {
            ""
        };
        let n = format!("{}{hw}", nome_codec(c.codec()));
        if !nomes.contains(&n) {
            nomes.push(n);
        }
    }
    if nomes.is_empty() {
        "—".to_owned()
    } else {
        nomes.join(", ")
    }
}

fn ms(us: u32) -> String {
    if us >= 10_000 {
        format!("{}", us / 1_000)
    } else {
        format!("{:.1}", f64::from(us) / 1_000.0)
    }
}

fn mbps(kbps: u32) -> String {
    format!("{:.1} Mbit/s", f64::from(kbps) / 1_000.0)
}

fn banda(e: Option<Estimate>, kbps: u32) -> String {
    match e {
        // Nenhum degrau saturou: a banda é pelo menos isso.
        Some(e) if !e.saturated => format!("≥ {}", mbps(e.kbps)),
        Some(e) => mbps(e.kbps),
        None => mbps(kbps),
    }
}

fn perda(ppm: u32) -> String {
    format!("{:.1} %", f64::from(ppm) / 10_000.0)
}

/// O perfil que o adaptador usaria ("Média — H.264, até 1080p, 60 fps, até 8.0 Mbit/s").
pub fn texto_perfil(c: NetClass, en: bool) -> String {
    let p = profile(c);
    let res = if p.max_height == 0 {
        t(en, "resolução nativa", "native resolution").to_owned()
    } else {
        format!("{} {}p", t(en, "até", "up to"), p.max_height)
    };
    format!(
        "{} — H.264, {res}, {} fps, {} {}",
        nome_classe(c, en),
        p.fps_max,
        t(en, "até", "up to"),
        mbps(p.bitrate_max_kbps)
    )
}

/// Por que essa classe, em uma frase.
pub fn texto_porque(r: &Resultado, en: bool) -> String {
    let b = &r.bench.report;
    let mut s = format!(
        "{}, {} {}, {} {} ms, {} {}",
        texto_caminho(r.caminho.as_ref(), en).to_lowercase(),
        t(en, "banda", "bandwidth"),
        mbps(b.down_kbps.min(b.up_kbps)),
        t(en, "latência", "latency"),
        ms(b.rtt_median_us),
        t(en, "perda", "loss"),
        perda(b.loss_ppm)
    );
    if r.caminho.as_ref().map(PathInfo::path_kind) == Some(PathKind::Relayed) {
        s.push_str(t(
            en,
            "; no relay a classe máxima é Média",
            "; over a relay the best class is Medium",
        ));
    }
    s
}

/// Rótulo e valor de cada linha da tela de teste, na ordem do §7.2.
pub fn linhas(r: &Resultado, en: bool) -> Vec<(&'static str, String)> {
    let b = &r.bench.report;
    vec![
        (
            t(en, "Caminho", "Path"),
            texto_caminho(r.caminho.as_ref(), en),
        ),
        (
            t(en, "Latência", "Latency"),
            format!(
                "{} ms ({} {} · p95 {})",
                ms(b.rtt_median_us),
                t(en, "mín", "min"),
                ms(b.rtt_min_us),
                ms(b.rtt_p95_us)
            ),
        ),
        (
            t(en, "Variação", "Jitter"),
            format!("{} ms", ms(b.jitter_us)),
        ),
        (t(en, "Perda", "Loss"), perda(b.loss_ppm)),
        (
            t(en, "Banda para cá", "Bandwidth to here"),
            banda(r.bench.down_estimate, b.down_kbps),
        ),
        (
            t(en, "Banda daqui", "Bandwidth from here"),
            banda(r.bench.up_estimate, b.up_kbps),
        ),
        (
            t(en, "Maior datagrama", "Largest datagram"),
            format!("{} B", b.max_datagram),
        ),
        (
            t(en, "Tempo para conectar", "Time to connect"),
            format!("{} ms", r.conectar.as_millis()),
        ),
        (
            t(en, "Encoders do computador", "Computer's encoders"),
            texto_codecs(&r.encode_caps, en),
        ),
        (
            t(en, "Perfil escolhido", "Chosen profile"),
            texto_perfil(r.bench.class, en),
        ),
        (t(en, "Por quê", "Why"), texto_porque(r, en)),
    ]
}

fn limpo(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() && *c != '"' && *c != '\\')
        .collect()
}

/// Relatório para colar numa issue: as linhas e um JSON. Sem IPs, chaves ou nomes de usuário.
pub fn relatorio(r: &Resultado, en: bool) -> String {
    let mut s = format!(
        "{} · {}\n",
        t(en, "Teste de conexão", "Connection test"),
        limpo(&r.host)
    );
    for (rotulo, valor) in linhas(r, en) {
        let _ = writeln!(s, "{rotulo}: {valor}");
    }
    let b = &r.bench.report;
    let caminho = match r.caminho.as_ref().map(PathInfo::path_kind) {
        Some(PathKind::DirectLan) => "direct_lan",
        Some(PathKind::DirectWan) => "direct_wan",
        Some(PathKind::Relayed) => "relayed",
        _ => "unknown",
    };
    let _ = write!(
        s,
        "{{\"path\":\"{caminho}\",\"relay_region\":\"{}\",\"class\":\"{}\",\"connect_ms\":{},\"rtt_min_us\":{},\"rtt_median_us\":{},\"rtt_p95_us\":{},\"jitter_us\":{},\"loss_ppm\":{},\"down_kbps\":{},\"up_kbps\":{},\"max_datagram\":{},\"encoders\":\"{}\"}}",
        limpo(r.caminho.as_ref().map_or("", |c| c.relay_region.as_str())),
        nome_classe(r.bench.class, true),
        r.conectar.as_millis(),
        b.rtt_min_us,
        b.rtt_median_us,
        b.rtt_p95_us,
        b.jitter_us,
        b.loss_ppm,
        b.down_kbps,
        b.up_kbps,
        b.max_datagram,
        texto_codecs(&r.encode_caps, true)
    );
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn um_teste_a_cada_10_s_por_dispositivo() {
        let mut l = LimiteTestes::default();
        let t0 = Instant::now();
        assert!(l.liberar("a", t0));
        assert!(!l.liberar("a", t0 + Duration::from_secs(9)));
        assert!(l.liberar("b", t0 + Duration::from_secs(9)));
        assert!(l.liberar("a", t0 + INTERVALO_TESTE));
    }

    #[test]
    fn limite_nao_cresce_sem_fim() {
        let mut l = LimiteTestes::default();
        let t0 = Instant::now();
        for i in 0..LEMBRADOS {
            assert!(l.liberar(&i.to_string(), t0));
        }
        assert!(!l.liberar("outro", t0));
        assert!(l.liberar("outro", t0 + INTERVALO_TESTE));
    }

    #[test]
    fn textos() {
        let relay = PathInfo {
            kind: PathKind::Relayed.to_wire(),
            relay_region: "sao".into(),
            rtt_ms: 0,
        };
        assert_eq!(texto_caminho(Some(&relay), false), "Via relay (sao)");
        assert_eq!(texto_caminho(None, true), "Unknown");
        let caps = [
            CodecCap {
                codec: Codec::H264.into(),
                hw: true,
                ..CodecCap::default()
            },
            CodecCap {
                codec: Codec::H264.into(),
                hw: true,
                ..CodecCap::default()
            },
            CodecCap {
                codec: Codec::Av1.into(),
                ..CodecCap::default()
            },
        ];
        assert_eq!(texto_codecs(&caps, false), "H.264 por hardware, AV1");
        assert_eq!(texto_codecs(&[], true), "—");
        assert_eq!(
            texto_perfil(NetClass::Medium, false),
            "Média — H.264, até 1080p, 60 fps, até 8.0 Mbit/s"
        );
        assert_eq!(
            texto_perfil(NetClass::Lan, true),
            "LAN — H.264, native resolution, 60 fps, up to 80.0 Mbit/s"
        );
        assert_eq!(ms(1_500), "1.5");
        assert_eq!(ms(38_400), "38");
        assert_eq!(limpo("a\"b\\c\nd"), "abcd");
    }
}
