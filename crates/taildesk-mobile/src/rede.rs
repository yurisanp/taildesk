//! Rede do celular durante a sessão (MF-80..82, MNF-04): rede tarifada pede menos ao host, o
//! consumo da sessão aparece no HUD, e a aba de teste mostra o caminho informado pelo host.

use taildesk_proto::control::{QualityMode, QualityPrefs};

/// Teto em dados móveis com "Economizar dados" (MNF-04: texto legível a ≤ 1 Mbit/s).
pub const ECONOMIA_KBPS: u32 = 1_000;
/// Teto da classe Lenta em rede tarifada sem economia (`14-selecao-adaptativa.md` §2: Média).
pub const TARIFADA_KBPS: u32 = 3_000;

/// Preferência de qualidade (Configurações, MF-100).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Qualidade {
    #[default]
    Automatica,
    Nitidez,
    Fluidez,
    EconomizarDados,
}

/// O que pedir ao host no `StartVideo`, pela rede e pela preferência. `economizar_em_movel`
/// vem ligado por padrão (MF-81).
pub fn prefs(
    qualidade: Qualidade,
    tarifada: bool,
    economizar_em_movel: bool,
) -> Option<QualityPrefs> {
    let economizar = qualidade == Qualidade::EconomizarDados || (tarifada && economizar_em_movel);
    if economizar {
        return Some(QualityPrefs {
            fps_max: 15,
            mode: QualityMode::LowBandwidth.into(),
            bitrate_kbps: ECONOMIA_KBPS,
            ..QualityPrefs::default()
        });
    }
    match (qualidade, tarifada) {
        (_, true) => Some(QualityPrefs {
            fps_max: 30,
            mode: QualityMode::Auto.into(),
            bitrate_kbps: TARIFADA_KBPS,
            ..QualityPrefs::default()
        }),
        (Qualidade::Nitidez, false) => Some(QualityPrefs {
            mode: QualityMode::Text.into(),
            ..QualityPrefs::default()
        }),
        (Qualidade::Fluidez, false) => Some(QualityPrefs {
            mode: QualityMode::Motion.into(),
            fps_max: 60,
            ..QualityPrefs::default()
        }),
        _ => None,
    }
}

/// Bytes recebidos na sessão (datagramas e streams), para o HUD e o fim da sessão (MF-81).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Consumo {
    pub bytes: u64,
}

impl Consumo {
    pub fn somar(&mut self, n: usize) {
        self.bytes = self.bytes.saturating_add(n as u64);
    }

    /// "12,4 MB" / "12.4 MB".
    pub fn texto(&self, idioma: crate::textos::Idioma) -> String {
        let mb = self.bytes as f64 / 1_000_000.0;
        let s = if mb < 10.0 {
            format!("{mb:.1} MB")
        } else {
            format!("{mb:.0} MB")
        };
        match idioma {
            crate::textos::Idioma::Pt => s.replace('.', ","),
            crate::textos::Idioma::En => s,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::textos::Idioma;

    #[test]
    fn dados_moveis_economizam_por_padrao() {
        let p = prefs(Qualidade::Automatica, true, true).unwrap();
        assert_eq!(p.bitrate_kbps, ECONOMIA_KBPS);
        assert_eq!(p.mode(), QualityMode::LowBandwidth);
        // Sem a economia, tarifada fica na Média.
        let p = prefs(Qualidade::Automatica, true, false).unwrap();
        assert_eq!(p.bitrate_kbps, TARIFADA_KBPS);
        // Wi-Fi automático não pede nada (o host decide).
        assert_eq!(prefs(Qualidade::Automatica, false, true), None);
        assert_eq!(
            prefs(Qualidade::Nitidez, false, true).unwrap().mode(),
            QualityMode::Text
        );
        assert_eq!(
            prefs(Qualidade::EconomizarDados, false, false)
                .unwrap()
                .bitrate_kbps,
            ECONOMIA_KBPS
        );
    }

    #[test]
    fn consumo_em_mb() {
        let mut c = Consumo::default();
        c.somar(12_400_000);
        assert_eq!(c.texto(Idioma::En), "12 MB");
        let mut c = Consumo::default();
        c.somar(1_250_000);
        assert_eq!(c.texto(Idioma::Pt), "1,2 MB");
        assert_eq!(c.texto(Idioma::En), "1.2 MB");
    }
}
