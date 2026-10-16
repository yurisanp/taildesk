//! Painel de diagnóstico da janela da sessão: codec e backend dos dois lados, GPU, quadros por
//! segundo, taxa, tempos de encode e decode, RTT, perda e rota (direta ou relay DERP).
//!
//! Junta três fontes, uma vez por segundo: o `VideoStats` do host, os [`Contadores`] do vídeo
//! aqui e as estatísticas da conexão QUIC e do Tailscale ([`Rede`], da task da sessão).

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use taildesk_overlay::{PathInfo, PathKind};
use taildesk_proto::control::{Codec, CodecSelected, VideoStats};
use taildesk_ui::LinhaDiag;

use crate::video::Contadores;

/// O que a task da sessão mede na conexão.
#[derive(Clone, Debug, Default)]
pub struct Rede {
    pub rtt: Duration,
    /// Rota do Tailscale (atualizada a cada poucos segundos; `None` antes da primeira).
    pub rota: Option<PathInfo>,
}

#[derive(Clone, Copy, Debug, Default)]
struct Amostra {
    bytes: u64,
    decodificados: u64,
    decode_us: u64,
    pedidos_chave: u64,
    exibidos: u64,
}

impl Amostra {
    fn ler(c: &Contadores) -> Amostra {
        Amostra {
            bytes: c.bytes.load(Ordering::Relaxed),
            decodificados: c.decodificados.load(Ordering::Relaxed),
            decode_us: c.decode_us.load(Ordering::Relaxed),
            pedidos_chave: c.pedidos_chave.load(Ordering::Relaxed),
            exibidos: c.exibidos.load(Ordering::Relaxed),
        }
    }
}

/// Estado do painel de uma sessão.
#[derive(Debug)]
pub struct Diagnostico {
    anterior: (Instant, Amostra),
    /// Último `VideoStats` do monitor que a janela mostra.
    host: Option<VideoStats>,
    video: Option<CodecSelected>,
    rede: Rede,
    perdidos: u64,
}

impl Default for Diagnostico {
    fn default() -> Self {
        Diagnostico {
            anterior: (Instant::now(), Amostra::default()),
            host: None,
            video: None,
            rede: Rede::default(),
            perdidos: 0,
        }
    }
}

fn ms(us: f64) -> String {
    format!("{:.1} ms", us / 1000.0).replace('.', ",")
}

fn num(v: f64, casas: usize) -> String {
    format!("{v:.casas$}").replace('.', ",")
}

fn rota(r: Option<&PathInfo>) -> String {
    let Some(r) = r else {
        return "medindo…".into();
    };
    match r.kind {
        PathKind::DirectLan => "direta · mesma rede local".into(),
        PathKind::DirectWan => "direta pela internet".into(),
        PathKind::Relayed => match &r.derp_region {
            Some(regiao) => format!("relay DERP ({regiao})"),
            None => "relay".into(),
        },
        PathKind::Unknown => "desconhecida".into(),
    }
}

fn titulo(t: &str) -> LinhaDiag {
    LinhaDiag {
        rotulo: t.into(),
        valor: Default::default(),
        titulo: true,
    }
}

fn linha(rotulo: &str, valor: String) -> LinhaDiag {
    LinhaDiag {
        rotulo: rotulo.into(),
        valor: valor.into(),
        titulo: false,
    }
}

impl Diagnostico {
    /// Vídeo novo (outro monitor ou outro tamanho): as estatísticas do host recomeçam.
    pub fn video(&mut self, c: &CodecSelected) {
        if self.video.is_none_or(|v| v.display_id != c.display_id) {
            self.host = None;
        }
        self.video = Some(*c);
    }

    pub fn host(&mut self, s: VideoStats) {
        if self.video.is_some_and(|v| v.display_id == s.display_id) {
            self.host = Some(s);
        }
    }

    pub fn rede(&mut self, r: Rede) {
        self.rede = r;
    }

    /// Linhas do painel com o que mudou desde a última chamada.
    pub fn linhas(&mut self, c: &Contadores) -> Vec<LinhaDiag> {
        let (agora, a) = (Instant::now(), Amostra::ler(c));
        let (antes, b) = std::mem::replace(&mut self.anterior, (agora, a));
        let s = agora.duration_since(antes).as_secs_f64().max(1e-3);
        let decodificados = a.decodificados - b.decodificados;
        let decode_us = (a.decode_us - b.decode_us) as f64 / decodificados.max(1) as f64;
        self.perdidos += a.pedidos_chave - b.pedidos_chave;
        let (decoder, gpu_dec) = c.decoder();

        let mut l = vec![titulo("Vídeo")];
        match &self.host {
            Some(h) => l.push(linha(
                "Encoder",
                format!(
                    "{} · {}",
                    h.encoder,
                    if h.gpu.is_empty() { "CPU" } else { &h.gpu }
                ),
            )),
            None => l.push(linha("Encoder", "esperando o host…".into())),
        }
        l.push(linha(
            "Decoder",
            if decoder.is_empty() {
                "—".into()
            } else {
                format!(
                    "{decoder} · {}",
                    if gpu_dec.is_empty() { "CPU" } else { &gpu_dec }
                )
            },
        ));
        if let Some(v) = &self.video {
            // O tamanho real vem do host (a captura no Wayland com escala é maior que a tela
            // lógica do `CodecSelected`).
            let (w, h) = match &self.host {
                Some(h) if h.width > 0 => (h.width, h.height),
                _ => (v.width, v.height),
            };
            l.push(linha(
                "Imagem",
                format!(
                    "{w}×{h} · {} · até {} fps",
                    match v.codec() {
                        Codec::Hevc => "HEVC",
                        Codec::Av1 => "AV1",
                        _ => "H.264",
                    },
                    v.fps_target
                ),
            ));
        }
        let exibidos = (a.exibidos - b.exibidos) as f64 / s;
        l.push(linha(
            "Quadros/s",
            match &self.host {
                Some(h) => format!(
                    "{} enviados · {} exibidos",
                    num(f64::from(h.fps_x100) / 100.0, 1),
                    num(exibidos, 1)
                ),
                None => format!("{} exibidos", num(exibidos, 1)),
            },
        ));
        let mbps = (a.bytes - b.bytes) as f64 * 8.0 / 1e6 / s;
        l.push(linha(
            "Taxa",
            match &self.host {
                Some(h) => format!(
                    "{} Mbit/s (alvo {})",
                    num(mbps, 1),
                    num(f64::from(h.target_kbps) / 1000.0, 1)
                ),
                None => format!("{} Mbit/s", num(mbps, 1)),
            },
        ));
        l.push(linha(
            "Encode / decode",
            format!(
                "{} / {}",
                self.host
                    .as_ref()
                    .map_or("—".into(), |h| ms(f64::from(h.encode_us))),
                ms(decode_us)
            ),
        ));
        l.push(linha(
            "Quadros perdidos",
            format!("{} na sessão", self.perdidos),
        ));

        l.push(titulo("Rede"));
        l.push(linha("Rota", rota(self.rede.rota.as_ref())));
        let rtt_us = self.rede.rtt.as_secs_f64() * 1e6;
        l.push(linha("RTT", ms(rtt_us)));
        if let Some(h) = &self.host {
            l.push(linha(
                "Perda",
                format!(
                    "{} % (host → aqui)",
                    num(f64::from(h.loss_ppm) / 10_000.0, 2)
                ),
            ));
            // Encode + meio RTT + decode: não inclui captura nem a espera pelo próximo
            // quadro da tela.
            let total = f64::from(h.encode_us) + rtt_us / 2.0 + decode_us;
            l.push(linha("Latência estimada", ms(total)));
        }
        l
    }
}
