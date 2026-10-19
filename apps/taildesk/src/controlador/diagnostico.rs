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

use super::qualidade::{Medida, Variacao};
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
    /// Quadros perdidos em cada um dos últimos segundos (para o indicador de qualidade).
    perdidos_recentes: std::collections::VecDeque<u64>,
    variacao: Variacao,
    /// Quadros por segundo enviados (do host) e exibidos aqui nos últimos segundos: os segundos
    /// do host e os daqui não batem, e a 10–15 fps um único segundo desalinhado parece travada.
    fps_recentes: std::collections::VecDeque<(f64, f64)>,
    /// O que a última chamada de [`Diagnostico::linhas`] mediu.
    medida: Medida,
}

impl Default for Diagnostico {
    fn default() -> Self {
        Diagnostico {
            anterior: (Instant::now(), Amostra::default()),
            host: None,
            video: None,
            rede: Rede::default(),
            perdidos: 0,
            perdidos_recentes: Default::default(),
            variacao: Variacao::default(),
            fps_recentes: Default::default(),
            medida: Medida::default(),
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

    /// Último `VideoStats` do monitor mostrado.
    pub fn stats_host(&self) -> Option<&VideoStats> {
        self.host.as_ref()
    }

    pub fn codec_selecionado(&self) -> Option<&CodecSelected> {
        self.video.as_ref()
    }

    /// Números da conexão medidos na última chamada de [`Diagnostico::linhas`].
    pub fn medida(&self) -> &Medida {
        &self.medida
    }

    /// Linhas do painel com o que mudou desde a última chamada.
    pub fn linhas(&mut self, c: &Contadores) -> Vec<LinhaDiag> {
        let (agora, a) = (Instant::now(), Amostra::ler(c));
        let (antes, b) = std::mem::replace(&mut self.anterior, (agora, a));
        let s = agora.duration_since(antes).as_secs_f64().max(1e-3);
        let decodificados = a.decodificados - b.decodificados;
        let decode_us = (a.decode_us - b.decode_us) as f64 / decodificados.max(1) as f64;
        self.perdidos += a.pedidos_chave - b.pedidos_chave;
        if self.perdidos_recentes.len() == 5 {
            self.perdidos_recentes.pop_front();
        }
        self.perdidos_recentes
            .push_back(a.pedidos_chave - b.pedidos_chave);
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
            // O teto de FPS atual vem do host (o laço lento da rede pode ter cortado).
            let fps = match &self.host {
                Some(h) if h.fps_max > 0 => h.fps_max,
                _ => v.fps_target,
            };
            let reduzida = self.host.as_ref().is_some_and(|h| h.reduced);
            l.push(linha(
                "Imagem",
                format!(
                    "{w}×{h} · {} · até {fps} fps{}",
                    match v.codec() {
                        Codec::Hevc => "HEVC",
                        Codec::Av1 => "AV1",
                        Codec::PyroWave => "PyroWave",
                        _ => "H.264",
                    },
                    if reduzida {
                        " · reduzida pela rede"
                    } else {
                        ""
                    }
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
        let enviados = self
            .host
            .as_ref()
            .map_or(0.0, |h| f64::from(h.fps_x100) / 100.0);
        if self.fps_recentes.len() == 3 {
            self.fps_recentes.pop_front();
        }
        self.fps_recentes.push_back((enviados, exibidos));
        let n = self.fps_recentes.len() as f64;
        let (fps_enviados, fps_exibidos) = self
            .fps_recentes
            .iter()
            .fold((0.0, 0.0), |(e, x), (a, b)| (e + a / n, x + b / n));
        self.medida = Medida {
            rtt: self.rede.rtt,
            variacao: self.variacao.medir(self.rede.rtt),
            perda_ppm: self.host.as_ref().map_or(0, |h| h.loss_ppm),
            banda_kbps: self.host.as_ref().map_or(0, |h| h.est_kbps),
            video_kbps: (mbps * 1000.0) as u32,
            rota: self.rede.rota.clone(),
            quadros_perdidos: self.perdidos_recentes.iter().sum(),
            fps_enviados,
            fps_exibidos,
            com_host: self.host.is_some(),
        };
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
