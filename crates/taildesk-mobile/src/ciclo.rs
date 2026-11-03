//! Ciclo de vida da sessão no celular (MF-49/50) e o HUD (MF-46/48).
//!
//! - Segundo plano: solta teclas e botões no host (`ReleaseAll`) e para o vídeo (não gasta
//!   bateria nem dados com uma tela que ninguém vê); a conexão de controle fica.
//! - Volta: pede o vídeo de novo (o primeiro quadro é chave). Se a conexão caiu enquanto o app
//!   estava fora, reconecta no mesmo host. Meta: imagem de volta em ≤ 3 s, com o mesmo zoom.
//! - Muito tempo fora ([`LIMITE_FUNDO_MS`]): encerra a sessão.
//! - Girar a tela não pausa nada: só muda a área do vídeo ([`crate::controle::Controle::area_mudou`]).

use taildesk_proto::control::{Msg, PathInfo, Priority, StartVideo, StopVideo};

use crate::controle::Controle;
use crate::textos::Idioma;

/// Depois de 5 minutos em segundo plano a sessão acaba.
pub const LIMITE_FUNDO_MS: u64 = 5 * 60 * 1000;

/// O que fazer ao voltar para a frente.
#[derive(Clone, Debug, PartialEq)]
pub enum AoVoltar {
    /// A conexão está viva: mande isto.
    Enviar(Vec<Msg>),
    /// Caiu enquanto estava fora: conecte de novo ao mesmo host.
    Reconectar,
    /// Ficou fora tempo demais e a sessão já foi encerrada.
    Nada,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ciclo {
    fundo_desde: Option<u64>,
    caiu: bool,
    encerrada: bool,
}

impl Ciclo {
    pub fn em_segundo_plano(&self) -> bool {
        self.fundo_desde.is_some()
    }

    /// `onPause`/`onStop`: devolve o que mandar antes de dormir.
    pub fn segundo_plano(&mut self, c: &mut Controle, t_ms: u64) -> Vec<Msg> {
        if self.fundo_desde.is_some() {
            return Vec::new();
        }
        self.fundo_desde = Some(t_ms);
        let mut out = Vec::new();
        c.soltar_tudo(&mut out);
        if let Some(d) = c.monitores.atual() {
            out.push(Msg::StopVideo(StopVideo { display_id: d.id }));
        }
        out
    }

    /// A conexão caiu (rede trocou de Wi-Fi para 4G, por exemplo).
    pub fn conexao_caiu(&mut self) {
        self.caiu = true;
    }

    /// Chame de vez em quando: `true` = encerre a sessão agora (tempo demais em segundo plano).
    pub fn tick(&mut self, t_ms: u64) -> bool {
        if let Some(desde) = self.fundo_desde
            && !self.encerrada
            && t_ms.saturating_sub(desde) >= LIMITE_FUNDO_MS
        {
            self.encerrada = true;
            return true;
        }
        false
    }

    /// `onResume`.
    pub fn frente(&mut self, c: &Controle) -> AoVoltar {
        if self.fundo_desde.take().is_none() && !self.caiu {
            return AoVoltar::Enviar(Vec::new());
        }
        if self.encerrada {
            return AoVoltar::Nada;
        }
        if std::mem::take(&mut self.caiu) {
            return AoVoltar::Reconectar;
        }
        AoVoltar::Enviar(
            c.monitores
                .atual()
                .map(|d| {
                    Msg::StartVideo(StartVideo {
                        display_id: d.id,
                        wanted: c.prefs,
                        priority: Priority::Focused.into(),
                    })
                })
                .into_iter()
                .collect(),
        )
    }
}

/// O que o HUD mostra (MF-48).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hud {
    pub rtt_ms: u32,
    /// Do `PathInfo` do host; `None` = ainda não chegou.
    pub caminho: Option<PathInfo>,
    pub fps: f32,
    pub decode_hw: bool,
    /// Perda de datagramas no último segundo (0..=1).
    pub perda: f32,
    /// Tempo médio de decode do último relatório, em µs (0 = sem medida).
    pub decode_us: u32,
}

impl Hud {
    /// "18 ms · direto · 60 fps · HW" / "120 ms · relay sao · 24 fps".
    pub fn linha(&self, idioma: Idioma) -> String {
        let mut partes = vec![format!("{} ms", self.rtt_ms)];
        match &self.caminho {
            Some(p) if p.direct() => partes.push(idioma.t("direto", "direct").to_owned()),
            Some(p) if p.path_kind() == taildesk_proto::net::PathKind::Relayed => {
                partes.push(if p.relay_region.is_empty() {
                    "relay".to_owned()
                } else {
                    format!("relay {}", p.relay_region)
                })
            }
            _ => {}
        }
        partes.push(format!("{:.0} fps", self.fps));
        if self.decode_hw {
            partes.push("HW".to_owned());
        }
        if self.decode_us > 0 {
            partes.push(format!("decode {:.1} ms", self.decode_us as f32 / 1000.0));
        }
        partes.join(" · ")
    }

    /// Aparece sozinho quando a rede piora (relay, RTT alto, perda).
    pub fn mostrar_sozinho(&self) -> bool {
        self.rtt_ms >= 150
            || self.perda >= 0.02
            || self
                .caminho
                .as_ref()
                .is_some_and(|p| p.path_kind() == taildesk_proto::net::PathKind::Relayed)
    }
}

/// Indicador do teclado (MF-46): "Toque → ABNT2 · Auto".
pub fn indicador_teclado(layout_host: &str, idioma: Idioma) -> String {
    let host = match layout_host {
        "" => idioma.t("host", "host").to_owned(),
        t => t.to_uppercase().replace("BR-", ""),
    };
    format!("{} → {host} · Auto", idioma.t("Toque", "Touch"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gestos::Modo;
    use crate::zoom::Retangulo;
    use taildesk_proto::control::DisplayInfo;
    use taildesk_proto::net::PathKind;

    fn controle() -> Controle {
        let mut c = Controle::new(
            Modo::Touchpad,
            1.0,
            Retangulo {
                x: 0.0,
                y: 0.0,
                w: 100.0,
                h: 100.0,
            },
        );
        let mut out = Vec::new();
        c.monitores_mudaram(
            &[DisplayInfo {
                id: 4,
                width: 1920,
                height: 1080,
                primary: true,
                shared: true,
                ..DisplayInfo::default()
            }],
            &mut out,
        );
        c
    }

    #[test]
    fn segundo_plano_para_o_video_e_volta() {
        let mut c = controle();
        let mut ciclo = Ciclo::default();
        let out = ciclo.segundo_plano(&mut c, 1_000);
        assert!(matches!(out[0], Msg::ReleaseAll(_)));
        assert_eq!(out[1], Msg::StopVideo(StopVideo { display_id: 4 }));
        // Pausar duas vezes não repete.
        assert!(ciclo.segundo_plano(&mut c, 1_100).is_empty());
        assert!(!ciclo.tick(60_000));
        let AoVoltar::Enviar(msgs) = ciclo.frente(&c) else {
            panic!()
        };
        assert!(matches!(&msgs[..], [Msg::StartVideo(s)] if s.display_id == 4));
        assert_eq!(ciclo.frente(&c), AoVoltar::Enviar(Vec::new()));
    }

    #[test]
    fn caiu_fora_reconecta_e_tempo_demais_encerra() {
        let mut c = controle();
        let mut ciclo = Ciclo::default();
        ciclo.segundo_plano(&mut c, 0);
        ciclo.conexao_caiu();
        assert_eq!(ciclo.frente(&c), AoVoltar::Reconectar);

        let mut ciclo = Ciclo::default();
        ciclo.segundo_plano(&mut c, 0);
        assert!(ciclo.tick(LIMITE_FUNDO_MS));
        assert!(!ciclo.tick(LIMITE_FUNDO_MS + 1), "encerra uma vez só");
        assert_eq!(ciclo.frente(&c), AoVoltar::Nada);
    }

    #[test]
    fn hud_e_indicador() {
        let mut h = Hud {
            rtt_ms: 18,
            caminho: Some(PathInfo {
                kind: PathKind::DirectLan as u32,
                ..PathInfo::default()
            }),
            fps: 59.8,
            decode_hw: true,
            perda: 0.0,
            decode_us: 0,
        };
        assert_eq!(h.linha(Idioma::Pt), "18 ms · direto · 60 fps · HW");
        h.decode_us = 4_200;
        assert_eq!(
            h.linha(Idioma::Pt),
            "18 ms · direto · 60 fps · HW · decode 4.2 ms"
        );
        h.decode_us = 0;
        assert!(!h.mostrar_sozinho());
        h.caminho = Some(PathInfo {
            kind: PathKind::Relayed as u32,
            relay_region: "sao".into(),
            rtt_ms: 0,
        });
        h.decode_hw = false;
        assert_eq!(h.linha(Idioma::En), "18 ms · relay sao · 60 fps");
        assert!(h.mostrar_sozinho());
        assert_eq!(
            indicador_teclado("br-abnt2", Idioma::Pt),
            "Toque → ABNT2 · Auto"
        );
        assert_eq!(indicador_teclado("us", Idioma::En), "Touch → US · Auto");
    }
}
