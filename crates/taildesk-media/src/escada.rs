//! Laço lento do vídeo (`14-selecao-adaptativa.md` §3.1 e §3.3): resolução e FPS de um monitor.
//!
//! O laço rápido ([`crate::taxa`]) só mexe na taxa. Com a rede apertada isso deixa a imagem
//! borrada (1080p60 a 1 Mbit/s). Este laço anda numa escada de degraus de altura máxima e FPS,
//! para gastar os bits em menos pixels e manter a imagem nítida e andando:
//!
//! - **desce** um degrau quando a taxa que o laço rápido consegue fica abaixo de
//!   [`Escada::RAZAO_DESCER`] do que o degrau precisa ([`BPP`] bit por pixel por quadro, ou o
//!   teto, se for menor) por 2 s; no máximo uma descida a cada 2 s;
//! - **sobe** um degrau depois de 5 s sem congestão (laço rápido no teto, sem fila), no mínimo
//!   10 s depois da última troca e 15 s depois da última descida. Subida desfeita em menos de
//!   10 s dobra essa espera, até 2 min: a rede não aguentou o degrau de cima.
//!
//! A preferência do usuário escolhe a escada: Nitidez corta FPS antes, Fluidez corta resolução
//! antes e Economizar dados começa em 1080p30. Degraus que não mudam nada para a captura (1440p
//! num monitor 1080p, 60 fps num encoder de 30) são pulados.
//!
//! Sem I/O nem relógio próprio: quem chama passa os tempos.

use std::time::{Duration, Instant};

/// Bits por pixel por quadro do teto automático do host (1080p60 ≈ 8,7 Mbit/s).
pub const BPP: f64 = 0.07;

/// Preferência de qualidade (`QualityPrefs.mode`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Modo {
    #[default]
    Auto,
    /// Corta FPS antes de resolução (texto, código).
    Nitidez,
    /// Corta resolução antes de FPS (vídeo, jogo).
    Fluidez,
    /// Começa em 1080p30.
    Economia,
}

/// Um degrau: altura máxima do lado menor (`0` = nativa) e teto de FPS.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Degrau {
    pub altura_max: u32,
    pub fps: f32,
}

const fn d(altura_max: u32, fps: f32) -> Degrau {
    Degrau { altura_max, fps }
}

const AUTO: &[Degrau] = &[
    d(0, 60.0),
    d(1440, 60.0),
    d(1080, 60.0),
    d(1080, 30.0),
    d(720, 30.0),
    d(720, 15.0),
    d(540, 10.0),
];
const NITIDEZ: &[Degrau] = &[
    d(0, 60.0),
    d(0, 30.0),
    d(1440, 30.0),
    d(1080, 30.0),
    d(1080, 15.0),
    d(720, 15.0),
    d(720, 10.0),
    d(540, 5.0),
];
const FLUIDEZ: &[Degrau] = &[
    d(0, 60.0),
    d(1440, 60.0),
    d(1080, 60.0),
    d(720, 60.0),
    d(540, 60.0),
    d(540, 30.0),
    d(540, 15.0),
];

impl Modo {
    fn degraus(self) -> &'static [Degrau] {
        match self {
            Modo::Auto | Modo::Economia => AUTO,
            Modo::Nitidez => NITIDEZ,
            Modo::Fluidez => FLUIDEZ,
        }
    }

    /// Primeiro degrau permitido.
    fn topo(self) -> usize {
        match self {
            Modo::Economia => 3,
            _ => 0,
        }
    }
}

/// Tamanho de saída de uma captura `largura`×`altura` com o lado menor limitado a `altura_max`
/// (`0` = sem limite). Reduzida, sai com lados pares e a mesma proporção.
pub fn escalar(largura: u32, altura: u32, altura_max: u32) -> (u32, u32) {
    let menor = largura.min(altura);
    if altura_max == 0 || menor <= altura_max {
        return (largura, altura);
    }
    let lado =
        |v: u32| ((u64::from(v) * u64::from(altura_max) / u64::from(menor)) as u32 & !1).max(2);
    (lado(largura), lado(altura))
}

/// O que sai do encoder num degrau.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Formato {
    pub largura: u32,
    pub altura: u32,
    pub fps: f32,
}

impl Formato {
    /// Taxa que este formato precisa para ficar nítido, em bit/s.
    pub fn necessario_bps(&self) -> f64 {
        f64::from(self.largura) * f64::from(self.altura) * f64::from(self.fps) * BPP
    }
}

/// Por que o degrau mudou (para o log).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MotivoEscada {
    /// Taxa abaixo do que o degrau precisa.
    TaxaBaixa,
    /// Rede folgada há um tempo.
    Folga,
    /// Preferência ou teto de FPS mudou.
    Preferencia,
}

impl MotivoEscada {
    pub fn texto(self) -> &'static str {
        match self {
            MotivoEscada::TaxaBaixa => "taxa baixa para a resolução",
            MotivoEscada::Folga => "rede folgada",
            MotivoEscada::Preferencia => "preferência",
        }
    }
}

/// Sinais de um passo.
#[derive(Clone, Copy, Debug)]
pub struct SinaisEscada {
    pub agora: Instant,
    /// Alvo atual do encoder (teto × fator do laço rápido), em bit/s.
    pub alvo_bps: u64,
    /// Teto do monitor no degrau atual, em bit/s.
    pub teto_bps: u64,
    /// Laço rápido no teto e sem fila no caminho.
    pub livre: bool,
    /// Tamanho da captura (`(0, 0)` antes do primeiro quadro).
    pub captura: (u32, u32),
}

/// Escada de um monitor.
#[derive(Debug, Clone)]
pub struct Escada {
    modo: Modo,
    /// Teto de FPS do encoder e do usuário.
    fps_max: f32,
    indice: usize,
    baixo_desde: Option<Instant>,
    livre_desde: Option<Instant>,
    ultima_troca: Option<Instant>,
    ultima_subida: Option<Instant>,
    sem_subir_ate: Option<Instant>,
    espera: Duration,
}

impl Escada {
    /// Desce quando a taxa fica abaixo disto do que o degrau precisa.
    pub const RAZAO_DESCER: f64 = 0.45;
    const SEGURAR_DESCIDA: Duration = Duration::from_secs(2);
    const INTERVALO_DESCIDA: Duration = Duration::from_secs(2);
    const SEGURAR_SUBIDA: Duration = Duration::from_secs(5);
    const INTERVALO_SUBIDA: Duration = Duration::from_secs(10);
    const ESPERA_SUBIR: Duration = Duration::from_secs(15);
    const ESPERA_MAX: Duration = Duration::from_secs(120);
    /// Subida desfeita antes disso conta como falha.
    const SUBIDA_FALHOU: Duration = Duration::from_secs(10);

    pub fn new(modo: Modo, fps_max: f32) -> Escada {
        Escada {
            modo,
            fps_max: fps_max.max(1.0),
            indice: modo.topo(),
            baixo_desde: None,
            livre_desde: None,
            ultima_troca: None,
            ultima_subida: None,
            sem_subir_ate: None,
            espera: Self::ESPERA_SUBIR,
        }
    }

    pub fn degrau(&self) -> Degrau {
        let d = self.modo.degraus()[self.indice];
        Degrau {
            fps: d.fps.min(self.fps_max),
            ..d
        }
    }

    fn formato_em(&self, indice: usize, (l, a): (u32, u32)) -> Formato {
        let d = self.modo.degraus()[indice];
        let (largura, altura) = escalar(l, a, d.altura_max);
        Formato {
            largura,
            altura,
            fps: d.fps.min(self.fps_max),
        }
    }

    /// Formato atual para uma captura deste tamanho.
    pub fn formato(&self, captura: (u32, u32)) -> Formato {
        self.formato_em(self.indice, captura)
    }

    /// Abaixo do topo da escada (a rede reduziu a resolução ou o FPS).
    pub fn reduzido(&self, captura: (u32, u32)) -> bool {
        self.formato(captura) != self.formato_em(self.modo.topo(), captura)
    }

    /// Muda a preferência ou o teto de FPS. Escada nova volta ao topo (a rede, se apertada,
    /// desce de novo em 2 s); só o teto de FPS mudando (prioridade do monitor), o degrau da rede
    /// fica. Devolve `true` se o degrau mudou.
    pub fn preferir(&mut self, modo: Modo, fps_max: f32, agora: Instant) -> bool {
        let fps_max = fps_max.max(1.0);
        if modo == self.modo && fps_max == self.fps_max {
            return false;
        }
        let antes = self.degrau();
        if modo != self.modo {
            self.modo = modo;
            self.indice = modo.topo();
            self.baixo_desde = None;
            self.livre_desde = None;
            self.ultima_troca = Some(agora);
        }
        self.fps_max = fps_max;
        self.degrau() != antes
    }

    /// Próximo índice (para baixo ou para cima) que muda o formato desta captura.
    fn vizinho(&self, captura: (u32, u32), descer: bool) -> Option<usize> {
        let atual = self.formato(captura);
        let n = self.modo.degraus().len();
        let mut i = self.indice;
        loop {
            i = if descer {
                (i + 1 < n).then_some(i + 1)?
            } else {
                (i > self.modo.topo()).then(|| i - 1)?
            };
            if self.formato_em(i, captura) != atual {
                return Some(i);
            }
        }
    }

    /// Um passo do laço (a cada ~100 ms). Devolve o formato novo e o motivo quando o degrau muda.
    pub fn passo(&mut self, s: &SinaisEscada) -> Option<(Formato, MotivoEscada)> {
        if s.captura.0 == 0 || s.captura.1 == 0 {
            return None;
        }
        let agora = s.agora;
        let desde_troca = |e: &Escada| e.ultima_troca.map(|t| agora.duration_since(t));
        let necessario = self
            .formato(s.captura)
            .necessario_bps()
            .min(s.teto_bps as f64);
        let baixo = necessario > 0.0 && (s.alvo_bps as f64) < necessario * Self::RAZAO_DESCER;
        let baixo_por = segurar(&mut self.baixo_desde, baixo, agora);
        let livre_por = segurar(&mut self.livre_desde, s.livre, agora);

        if baixo_por >= Self::SEGURAR_DESCIDA
            && desde_troca(self).is_none_or(|d| d >= Self::INTERVALO_DESCIDA)
            && let Some(i) = self.vizinho(s.captura, true)
        {
            let falhou = self
                .ultima_subida
                .is_some_and(|t| agora.duration_since(t) < Self::SUBIDA_FALHOU);
            self.espera = if falhou {
                (self.espera * 2).min(Self::ESPERA_MAX)
            } else {
                Self::ESPERA_SUBIR
            };
            self.sem_subir_ate = Some(agora + self.espera);
            self.ultima_subida = None;
            return Some(self.trocar(i, s.captura, agora, MotivoEscada::TaxaBaixa));
        }

        if livre_por >= Self::SEGURAR_SUBIDA
            && desde_troca(self).is_none_or(|d| d >= Self::INTERVALO_SUBIDA)
            && self.sem_subir_ate.is_none_or(|t| agora >= t)
            && let Some(i) = self.vizinho(s.captura, false)
        {
            self.ultima_subida = Some(agora);
            return Some(self.trocar(i, s.captura, agora, MotivoEscada::Folga));
        }
        None
    }

    fn trocar(
        &mut self,
        indice: usize,
        captura: (u32, u32),
        agora: Instant,
        motivo: MotivoEscada,
    ) -> (Formato, MotivoEscada) {
        self.indice = indice;
        self.ultima_troca = Some(agora);
        self.baixo_desde = None;
        self.livre_desde = None;
        (self.formato(captura), motivo)
    }
}

/// Há quanto tempo `cond` está verdadeira (zera quando fica falsa).
fn segurar(desde: &mut Option<Instant>, cond: bool, agora: Instant) -> Duration {
    if cond {
        agora.duration_since(*desde.get_or_insert(agora))
    } else {
        *desde = None;
        Duration::ZERO
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TICK: Duration = Duration::from_millis(100);
    const FHD: (u32, u32) = (1920, 1080);

    /// Roda `dur` com sinais fixos; devolve o tempo final e as trocas (tempo, formato, motivo).
    fn rodar(
        e: &mut Escada,
        t0: Instant,
        dur: Duration,
        captura: (u32, u32),
        sinais: impl Fn(&Escada) -> (u64, bool),
    ) -> (Instant, Vec<(Duration, Formato, MotivoEscada)>) {
        let mut t = t0;
        let mut trocas = Vec::new();
        while t < t0 + dur {
            t += TICK;
            let f = e.formato(captura);
            let teto = f.necessario_bps().clamp(4e6, 30e6) as u64;
            let (alvo, livre) = sinais(e);
            if let Some((f, m)) = e.passo(&SinaisEscada {
                agora: t,
                alvo_bps: alvo.min(teto),
                teto_bps: teto,
                livre,
                captura,
            }) {
                trocas.push((t - t0, f, m));
            }
        }
        (t, trocas)
    }

    #[test]
    fn escalar_mantem_proporcao_e_lados_pares() {
        assert_eq!(escalar(1920, 1080, 0), (1920, 1080));
        assert_eq!(escalar(1920, 1080, 1080), (1920, 1080));
        assert_eq!(escalar(1920, 1080, 720), (1280, 720));
        assert_eq!(escalar(2560, 1600, 1080), (1728, 1080));
        assert_eq!(escalar(3840, 2160, 540), (960, 540));
        // Monitor em pé: o limite vale para o lado menor.
        assert_eq!(escalar(1080, 1920, 720), (720, 1280));
        // Tamanho ímpar só muda quando reduz.
        assert_eq!(escalar(1365, 767, 0), (1365, 767));
        assert_eq!(escalar(1365, 767, 540), (960, 540));
    }

    #[test]
    fn rede_boa_nao_muda_nada() {
        let mut e = Escada::new(Modo::Auto, 60.0);
        let t0 = Instant::now();
        let (_, trocas) = rodar(&mut e, t0, Duration::from_secs(120), FHD, |_| {
            (u64::MAX, true)
        });
        assert!(trocas.is_empty(), "{trocas:?}");
        assert!(!e.reduzido(FHD));
        assert_eq!(
            e.formato(FHD),
            Formato {
                largura: 1920,
                altura: 1080,
                fps: 60.0
            }
        );
    }

    #[test]
    fn taxa_baixa_desce_um_degrau_por_vez_pulando_os_inuteis() {
        let mut e = Escada::new(Modo::Auto, 60.0);
        let t0 = Instant::now();
        // 500 kbit/s fixos: 1080p60 precisa de ~8,7 Mbit/s e 720p15 de ~1.
        let (_, trocas) = rodar(&mut e, t0, Duration::from_secs(10), FHD, |_| {
            (500_000, false)
        });
        // 1440p60 e 1080p60 não mudam um monitor 1080p: o primeiro degrau útil é 1080p30.
        let fmt: Vec<_> = trocas.iter().map(|t| (t.1.altura, t.1.fps)).collect();
        assert_eq!(fmt, [(1080, 30.0), (720, 30.0), (720, 15.0)]);
        assert!(trocas.iter().all(|t| t.2 == MotivoEscada::TaxaBaixa));
        // A primeira depois de 2 s de taxa baixa, as outras a cada 2 s.
        assert!(trocas[0].0 >= Duration::from_secs(2));
        assert!(trocas[1].0 - trocas[0].0 >= Duration::from_secs(2));
        assert!(e.reduzido(FHD));
    }

    #[test]
    fn queda_curta_nao_desce() {
        let mut e = Escada::new(Modo::Auto, 60.0);
        let t0 = Instant::now();
        let (t, trocas) = rodar(&mut e, t0, Duration::from_millis(1_500), FHD, |_| {
            (500_000, false)
        });
        assert!(trocas.is_empty());
        let (_, trocas) = rodar(&mut e, t, Duration::from_secs(10), FHD, |_| {
            (8_000_000, false)
        });
        assert!(trocas.is_empty(), "{trocas:?}");
    }

    #[test]
    fn sobe_com_folga_depois_da_espera() {
        let mut e = Escada::new(Modo::Auto, 60.0);
        let t0 = Instant::now();
        let (t, trocas) = rodar(&mut e, t0, Duration::from_millis(2_100), FHD, |_| {
            (1_000_000, false)
        });
        assert_eq!(trocas.len(), 1);
        let desceu = trocas[0].0;
        // Rede voltou: sobe só depois de 15 s da descida.
        let (_, trocas) = rodar(&mut e, t, Duration::from_secs(20), FHD, |_| {
            (u64::MAX, true)
        });
        assert_eq!(trocas.len(), 1, "{trocas:?}");
        assert_eq!(trocas[0].2, MotivoEscada::Folga);
        assert_eq!((trocas[0].1.altura, trocas[0].1.fps), (1080, 60.0));
        let subiu = t - t0 + trocas[0].0;
        assert!(subiu - desceu >= Duration::from_secs(15));
        assert!(!e.reduzido(FHD));
    }

    #[test]
    fn subida_que_falha_dobra_a_espera() {
        let mut e = Escada::new(Modo::Auto, 60.0);
        let t0 = Instant::now();
        // Desce para 1080p30, sobe com folga e a rede não aguenta: desce de novo.
        let (t, _) = rodar(&mut e, t0, Duration::from_millis(2_100), FHD, |_| {
            (1_000_000, false)
        });
        let (t, trocas) = rodar(&mut e, t, Duration::from_secs(16), FHD, |_| {
            (u64::MAX, true)
        });
        assert_eq!(trocas.len(), 1);
        let (t, trocas) = rodar(&mut e, t, Duration::from_millis(2_100), FHD, |_| {
            (1_000_000, false)
        });
        assert_eq!(trocas.len(), 1);
        // Agora a espera é de 30 s: com folga, nada em 25 s, sobe antes de 35.
        let (t, trocas) = rodar(&mut e, t, Duration::from_secs(25), FHD, |_| {
            (u64::MAX, true)
        });
        assert!(trocas.is_empty(), "{trocas:?}");
        let (_, trocas) = rodar(&mut e, t, Duration::from_secs(10), FHD, |_| {
            (u64::MAX, true)
        });
        assert_eq!(trocas.len(), 1);
    }

    #[test]
    fn preferencia_escolhe_o_que_cortar_primeiro() {
        let t0 = Instant::now();
        let qhd = (2560, 1440);
        let primeira = |modo| {
            let mut e = Escada::new(modo, 60.0);
            let (_, trocas) = rodar(&mut e, t0, Duration::from_millis(2_100), qhd, |_| {
                (1_000_000, false)
            });
            (trocas[0].1.altura, trocas[0].1.fps)
        };
        assert_eq!(primeira(Modo::Auto), (1080, 60.0));
        assert_eq!(primeira(Modo::Nitidez), (1440, 30.0));
        assert_eq!(primeira(Modo::Fluidez), (1080, 60.0));
        // Economizar dados começa em 1080p30 e não sobe acima disso.
        let mut e = Escada::new(Modo::Economia, 60.0);
        assert_eq!(e.formato(qhd).altura, 1080);
        assert_eq!(e.formato(qhd).fps, 30.0);
        let (_, trocas) = rodar(&mut e, t0, Duration::from_secs(60), qhd, |_| {
            (u64::MAX, true)
        });
        assert!(trocas.is_empty());
        assert!(!e.reduzido(qhd));
    }

    #[test]
    fn fps_do_encoder_limita_os_degraus() {
        // openh264 a 30 fps: o primeiro degrau útil num 1080p é 720p30.
        let mut e = Escada::new(Modo::Auto, 30.0);
        assert_eq!(e.formato(FHD).fps, 30.0);
        let (_, trocas) = rodar(
            &mut e,
            Instant::now(),
            Duration::from_millis(2_100),
            FHD,
            |_| (500_000, false),
        );
        assert_eq!((trocas[0].1.altura, trocas[0].1.fps), (720, 30.0));
    }

    #[test]
    fn taxa_fixa_baixa_conta_pelo_teto() {
        // Usuário fixou 2 Mbit/s: o teto vale como "necessário" e só a rede derruba.
        let mut e = Escada::new(Modo::Auto, 60.0);
        let t0 = Instant::now();
        let mut t = t0;
        for _ in 0..100 {
            t += TICK;
            let r = e.passo(&SinaisEscada {
                agora: t,
                alvo_bps: 2_000_000,
                teto_bps: 2_000_000,
                livre: true,
                captura: FHD,
            });
            assert!(r.is_none());
        }
    }

    #[test]
    fn preferir_volta_ao_topo() {
        let t0 = Instant::now();
        let mut e = Escada::new(Modo::Auto, 60.0);
        rodar(&mut e, t0, Duration::from_millis(2_100), FHD, |_| {
            (1_000_000, false)
        });
        assert!(e.reduzido(FHD));
        assert!(e.preferir(Modo::Fluidez, 60.0, t0 + Duration::from_secs(3)));
        assert!(!e.reduzido(FHD));
        assert!(!e.preferir(Modo::Fluidez, 60.0, t0 + Duration::from_secs(4)));
        assert!(e.preferir(Modo::Fluidez, 24.0, t0 + Duration::from_secs(5)));
        assert_eq!(e.formato(FHD).fps, 24.0);
        // Só o FPS (monitor sem foco): o degrau da rede fica.
        rodar(
            &mut e,
            t0 + Duration::from_secs(5),
            Duration::from_millis(2_100),
            FHD,
            |_| (300_000, false),
        );
        let altura = e.formato(FHD).altura;
        assert!(altura < 1080);
        assert!(e.preferir(Modo::Fluidez, 15.0, t0 + Duration::from_secs(8)));
        assert_eq!(e.formato(FHD).altura, altura);
        assert!(e.formato(FHD).fps <= 15.0);
    }
}
