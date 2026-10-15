//! Seleção adaptativa de classe de rede e perfil de mídia (`14-selecao-adaptativa.md` §2–§3).
//!
//! O [`Adapter`] é lógica pura: recebe [`Signals`] com o tempo da sessão e devolve [`Action`]s.
//! Roda no host, que manda no encoder, com os `ReceiverReport` do controlador. Dois laços:
//! - **rápido** (100 ms, `07-protocolo.md` §5): ajusta o bitrate dentro do perfil; fica fora daqui
//!   e só chega como `Signals::bitrate_kbps`;
//! - **lento** (este módulo): troca de classe, com histerese. Desce rápido, sobe devagar e só
//!   depois de uma sondagem confirmar a banda.

use std::time::Duration;

use taildesk_proto::net::{NetClass, PathKind, Reason};

use crate::measure::Estimate;

/// Preferência de qualidade do usuário (`12-levantamento-ux.md` §5.9 + "Economizar dados").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Preference {
    #[default]
    Auto,
    /// Corta FPS antes de resolução (modo *Text*).
    Sharpness,
    /// Corta resolução antes de FPS (modo *Motion*).
    Smoothness,
    /// Teto na classe Lenta.
    SaveData,
}

impl Preference {
    fn cap(self) -> NetClass {
        match self {
            Preference::SaveData => NetClass::Slow,
            _ => NetClass::Lan,
        }
    }
}

/// Alvo do jitter buffer do controlador.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JitterTarget {
    /// Entrega assim que o quadro completa (LAN).
    Zero,
    /// Cerca do p95 da variação medida.
    P95,
}

/// Perfil de mídia do monitor em foco para uma classe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Profile {
    pub class: NetClass,
    /// Altura máxima em pixels; `0` = resolução nativa.
    pub max_height: u32,
    pub fps_max: u8,
    pub fps_min: u8,
    pub bitrate_min_kbps: u32,
    pub bitrate_max_kbps: u32,
    pub jitter: JitterTarget,
}

/// Tabela de `14-selecao-adaptativa.md` §3.1. No MVP o codec é sempre H.264; o perfil muda
/// bitrate, FPS, resolução e regras de transporte.
pub const fn profile(class: NetClass) -> Profile {
    match class {
        NetClass::Lan => Profile {
            class,
            max_height: 0,
            fps_max: 60,
            fps_min: 60,
            bitrate_min_kbps: 20_000,
            bitrate_max_kbps: 80_000,
            jitter: JitterTarget::Zero,
        },
        NetClass::Fast => Profile {
            class,
            max_height: 1440,
            fps_max: 60,
            fps_min: 30,
            bitrate_min_kbps: 8_000,
            bitrate_max_kbps: 25_000,
            jitter: JitterTarget::P95,
        },
        NetClass::Medium => Profile {
            class,
            max_height: 1080,
            fps_max: 60,
            fps_min: 30,
            bitrate_min_kbps: 3_000,
            bitrate_max_kbps: 8_000,
            jitter: JitterTarget::P95,
        },
        NetClass::Slow => Profile {
            class,
            max_height: 1080,
            fps_max: 30,
            fps_min: 15,
            bitrate_min_kbps: 1_000,
            bitrate_max_kbps: 3_000,
            jitter: JitterTarget::P95,
        },
        NetClass::Minimal => Profile {
            class,
            max_height: 720,
            fps_max: 15,
            fps_min: 5,
            bitrate_min_kbps: 300,
            bitrate_max_kbps: 1_000,
            jitter: JitterTarget::P95,
        },
    }
}

/// Regras de transporte que dependem da rede, não da classe (§2, modificadores).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransportRules {
    pub fec: bool,
    pub nack: bool,
}

/// FEC com perda > 1% (sempre na classe Mínima); NACK só com RTT ≤ 80 ms (fora da Mínima).
pub fn transport_rules(class: NetClass, rtt: Duration, loss_ppm: u32) -> TransportRules {
    TransportRules {
        fec: class == NetClass::Minimal || loss_ppm > 10_000,
        nack: class != NetClass::Minimal && rtt <= Duration::from_millis(80),
    }
}

/// Limiares e tempos do adaptador. Os padrões são os de `14-selecao-adaptativa.md`; o simulador
/// e o gate G0 vão calibrá-los.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdaptConfig {
    pub lan_min_kbps: u32,
    pub lan_max_rtt: Duration,
    pub lan_max_loss_ppm: u32,
    pub fast_min_kbps: u32,
    pub fast_max_rtt: Duration,
    pub medium_min_kbps: u32,
    pub slow_min_kbps: u32,
    /// Perda acima disso joga direto para a Mínima na classificação.
    pub minimal_loss_ppm: u32,
    /// Descer: perda acima disso por `loss_hold`.
    pub down_loss_ppm: u32,
    pub loss_hold: Duration,
    /// Descer: bitrate no piso do perfil por `floor_hold`.
    pub floor_hold: Duration,
    /// Descer: RTT > 2× o mínimo e fila > `rtt_queue` por `rtt_hold`.
    pub rtt_queue: Duration,
    pub rtt_hold: Duration,
    /// Intervalo mínimo entre duas descidas.
    pub down_interval: Duration,
    /// Subir: bitrate no teto, fila < `up_max_queue`, perda < `up_max_loss_ppm` por `up_hold`.
    pub up_hold: Duration,
    pub up_max_queue: Duration,
    pub up_max_loss_ppm: u32,
    /// Depois de descer (ou de uma sondagem que falhou), espera isso antes de tentar subir.
    pub up_backoff: Duration,
    /// Intervalo mínimo entre trocas para cima (cada troca pode custar um keyframe).
    pub change_interval: Duration,
    /// Sondagem sem resposta depois disso é abandonada.
    pub probe_timeout: Duration,
    /// Janela do RTT mínimo.
    pub min_rtt_window: Duration,
}

impl Default for AdaptConfig {
    fn default() -> Self {
        AdaptConfig {
            lan_min_kbps: 100_000,
            lan_max_rtt: Duration::from_millis(5),
            lan_max_loss_ppm: 5_000,
            fast_min_kbps: 25_000,
            fast_max_rtt: Duration::from_millis(40),
            medium_min_kbps: 8_000,
            slow_min_kbps: 2_000,
            minimal_loss_ppm: 50_000,
            down_loss_ppm: 30_000,
            loss_hold: Duration::from_secs(1),
            floor_hold: Duration::from_secs(2),
            rtt_queue: Duration::from_millis(20),
            rtt_hold: Duration::from_secs(1),
            down_interval: Duration::from_secs(2),
            up_hold: Duration::from_secs(5),
            up_max_queue: Duration::from_millis(5),
            up_max_loss_ppm: 5_000,
            up_backoff: Duration::from_secs(15),
            change_interval: Duration::from_secs(10),
            probe_timeout: Duration::from_secs(3),
            min_rtt_window: Duration::from_secs(10),
        }
    }
}

impl AdaptConfig {
    /// Classe pela banda, RTT, perda e caminho medidos (tabela da §2), antes dos tetos.
    pub fn classify(&self, kbps: u32, rtt: Duration, loss_ppm: u32, path: PathKind) -> NetClass {
        if loss_ppm > self.minimal_loss_ppm || kbps < self.slow_min_kbps {
            NetClass::Minimal
        } else if path != PathKind::Relayed
            && kbps >= self.lan_min_kbps
            && rtt < self.lan_max_rtt
            && loss_ppm < self.lan_max_loss_ppm
        {
            NetClass::Lan
        } else if kbps >= self.fast_min_kbps && rtt < self.fast_max_rtt {
            NetClass::Fast
        } else if kbps >= self.medium_min_kbps {
            NetClass::Medium
        } else {
            NetClass::Slow
        }
    }

    /// Banda mínima para ficar numa classe.
    pub fn min_kbps(&self, class: NetClass) -> u32 {
        match class {
            NetClass::Lan => self.lan_min_kbps,
            NetClass::Fast => self.fast_min_kbps,
            NetClass::Medium => self.medium_min_kbps,
            NetClass::Slow => self.slow_min_kbps,
            NetClass::Minimal => 0,
        }
    }
}

/// Uma amostra do estado da rede, a cada ~100 ms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Signals {
    /// Tempo desde o início da sessão (monotônico).
    pub now: Duration,
    /// RTT suavizado do QUIC.
    pub rtt: Duration,
    /// Perda recente (do `ReceiverReport`), em ppm.
    pub loss_ppm: u32,
    /// Bitrate alvo atual do laço rápido.
    pub bitrate_kbps: u32,
    pub path: PathKind,
    /// Conexão tarifada em qualquer um dos lados.
    pub metered: bool,
}

/// O que o adaptador pede ao pipeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Trocar para esta classe (manda `ProfileChanged` e reconfigura o encoder).
    SetClass { class: NetClass, reason: Reason },
    /// Rodar uma sondagem até esta taxa e chamar [`Adapter::on_probe`] com o resultado.
    Probe { up_to_kbps: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProbePurpose {
    /// Primeira medição ou caminho mudou: a classe vai para onde a medição mandar.
    Evaluate,
    /// Tentativa de subir uma classe.
    Up,
}

/// Mínimo numa janela deslizante aproximada por dois baldes, sem alocação.
#[derive(Clone, Copy, Debug)]
struct WindowMin {
    bucket: Duration,
    start: Duration,
    cur: Option<Duration>,
    prev: Option<Duration>,
}

impl WindowMin {
    fn new(window: Duration) -> Self {
        WindowMin {
            bucket: window / 2,
            start: Duration::ZERO,
            cur: None,
            prev: None,
        }
    }

    fn push(&mut self, now: Duration, v: Duration) -> Duration {
        if now.saturating_sub(self.start) >= self.bucket {
            self.prev = self.cur;
            self.cur = None;
            self.start = now;
        }
        self.cur = Some(self.cur.map_or(v, |c| c.min(v)));
        self.get().unwrap_or(v)
    }

    fn get(&self) -> Option<Duration> {
        match (self.cur, self.prev) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
}

/// Há quanto tempo `cond` está verdadeira (zera quando fica falsa).
fn held(since: &mut Option<Duration>, cond: bool, now: Duration) -> Duration {
    if cond {
        now.saturating_sub(*since.get_or_insert(now))
    } else {
        *since = None;
        Duration::ZERO
    }
}

/// Adaptador de um stream de vídeo (um por monitor ativo).
#[derive(Clone, Debug)]
pub struct Adapter {
    cfg: AdaptConfig,
    pref: Preference,
    class: NetClass,
    path: PathKind,
    metered: bool,
    min_rtt: WindowMin,
    loss_since: Option<Duration>,
    floor_since: Option<Duration>,
    rtt_since: Option<Duration>,
    headroom_since: Option<Duration>,
    last_change: Duration,
    /// Até quando não se tenta subir (depois de descer ou de sondagem que falhou).
    no_up_until: Duration,
    probe: Option<(ProbePurpose, Duration)>,
    /// Caminho mudou: reavaliar com uma sondagem assim que possível.
    need_eval: bool,
}

impl Adapter {
    /// Começa sem atrasar o primeiro quadro: classe Média, ou Rápida se a pista for de LAN
    /// (§3.2). Devolve também a sondagem inicial a disparar em paralelo.
    pub fn new(
        cfg: AdaptConfig,
        pref: Preference,
        path: PathKind,
        same_lan_hint: bool,
        metered: bool,
        now: Duration,
    ) -> (Adapter, [Action; 2]) {
        let hinted = if path == PathKind::DirectLan || (same_lan_hint && path != PathKind::Relayed)
        {
            NetClass::Fast
        } else {
            NetClass::Medium
        };
        let mut a = Adapter {
            cfg,
            pref,
            class: hinted,
            path,
            metered,
            min_rtt: WindowMin::new(cfg.min_rtt_window),
            loss_since: None,
            floor_since: None,
            rtt_since: None,
            headroom_since: None,
            last_change: now,
            no_up_until: now,
            probe: Some((ProbePurpose::Evaluate, now)),
            need_eval: false,
        };
        a.class = a.class.min(a.cap());
        let up_to = *crate::measure::plan_steps(path)
            .last()
            .unwrap_or(&a.cfg.lan_min_kbps);
        let class = a.class;
        (
            a,
            [
                Action::SetClass {
                    class,
                    reason: Reason::Initial,
                },
                Action::Probe { up_to_kbps: up_to },
            ],
        )
    }

    pub fn class(&self) -> NetClass {
        self.class
    }

    pub fn profile(&self) -> Profile {
        profile(self.class)
    }

    /// Teto atual pela preferência, caminho e conexão tarifada, com o motivo do teto.
    fn cap_with_reason(&self) -> (NetClass, Reason) {
        let mut cap = (self.pref.cap(), Reason::Preference);
        if self.path == PathKind::Relayed && NetClass::Medium < cap.0 {
            cap = (NetClass::Medium, Reason::PathRelayed);
        }
        if self.metered && NetClass::Medium < cap.0 {
            cap = (NetClass::Medium, Reason::Metered);
        }
        cap
    }

    fn cap(&self) -> NetClass {
        self.cap_with_reason().0
    }

    fn set(&mut self, class: NetClass, reason: Reason, now: Duration) -> Option<Action> {
        if class == self.class {
            return None;
        }
        if class < self.class {
            self.no_up_until = now + self.cfg.up_backoff;
        }
        self.class = class;
        self.last_change = now;
        self.loss_since = None;
        self.floor_since = None;
        self.rtt_since = None;
        self.headroom_since = None;
        Some(Action::SetClass { class, reason })
    }

    /// Muda a preferência do usuário.
    pub fn set_preference(&mut self, pref: Preference, now: Duration) -> Option<Action> {
        self.pref = pref;
        if self.class > self.cap() {
            let cap = self.cap();
            return self.set(cap, Reason::Preference, now);
        }
        // Teto subiu: a próxima folga dispara a sondagem normal.
        self.no_up_until = now;
        None
    }

    /// Resultado de uma sondagem pedida por [`Action::Probe`].
    pub fn on_probe(&mut self, now: Duration, est: Estimate, rtt: Duration) -> Option<Action> {
        let (purpose, _) = self.probe.take()?;
        let rtt = self.min_rtt.get().map_or(rtt, |m| m.min(rtt));
        let measured = self.cfg.classify(est.kbps, rtt, est.loss_ppm, self.path);
        let target = measured.min(self.cap());
        match purpose {
            ProbePurpose::Evaluate => self.set(target, Reason::Probe, now),
            ProbePurpose::Up => {
                if target > self.class {
                    self.set(self.class.higher(), Reason::Probe, now)
                } else {
                    self.no_up_until = now + self.cfg.up_backoff;
                    None
                }
            }
        }
    }

    /// Laço lento: chamar a cada amostra (≈100 ms).
    pub fn update(&mut self, s: Signals) -> Option<Action> {
        let now = s.now;
        let min_rtt = self.min_rtt.push(now, s.rtt);
        let queue = s.rtt.saturating_sub(min_rtt);

        if let Some((purpose, started)) = self.probe
            && now.saturating_sub(started) >= self.cfg.probe_timeout
        {
            // Sondagem para subir sem resposta conta como falha.
            if purpose == ProbePurpose::Up {
                self.no_up_until = now + self.cfg.up_backoff;
            }
            self.probe = None;
        }

        // Caminho ou tarifação mudaram.
        if s.path != self.path {
            self.need_eval = true;
            // Uma sondagem em andamento mediu o caminho antigo.
            self.probe = None;
        }
        self.path = s.path;
        self.metered = s.metered;
        let (cap, cap_reason) = self.cap_with_reason();
        if self.class > cap {
            return self.set(cap, cap_reason, now);
        }
        if self.need_eval && self.probe.is_none() {
            self.need_eval = false;
            self.probe = Some((ProbePurpose::Evaluate, now));
            let up_to = *crate::measure::plan_steps(s.path)
                .last()
                .unwrap_or(&self.cfg.lan_min_kbps);
            return Some(Action::Probe { up_to_kbps: up_to });
        }

        // Descer: uma classe por vez, no máximo uma vez a cada `down_interval`.
        let profile = profile(self.class);
        let loss_for = held(
            &mut self.loss_since,
            s.loss_ppm > self.cfg.down_loss_ppm,
            now,
        );
        let floor_for = held(
            &mut self.floor_since,
            s.bitrate_kbps <= profile.bitrate_min_kbps,
            now,
        );
        let rtt_for = held(
            &mut self.rtt_since,
            s.rtt > min_rtt * 2 && queue > self.cfg.rtt_queue,
            now,
        );
        let can_down = self.class > NetClass::Minimal
            && now.saturating_sub(self.last_change) >= self.cfg.down_interval;
        if can_down {
            let reason = if loss_for >= self.cfg.loss_hold {
                Some(Reason::LossHigh)
            } else if rtt_for >= self.cfg.rtt_hold {
                Some(Reason::RttHigh)
            } else if floor_for >= self.cfg.floor_hold {
                Some(Reason::BitrateFloor)
            } else {
                None
            };
            if let Some(reason) = reason {
                return self.set(self.class.lower(), reason, now);
            }
        }

        // Subir: folga sustentada, depois sondagem até a banda da classe de cima.
        let headroom_for = held(
            &mut self.headroom_since,
            u64::from(s.bitrate_kbps) * 100 >= u64::from(profile.bitrate_max_kbps) * 95
                && queue < self.cfg.up_max_queue
                && s.loss_ppm < self.cfg.up_max_loss_ppm,
            now,
        );
        if self.class < cap
            && self.probe.is_none()
            && now >= self.no_up_until
            && now.saturating_sub(self.last_change) >= self.cfg.change_interval
            && headroom_for >= self.cfg.up_hold
        {
            self.probe = Some((ProbePurpose::Up, now));
            self.headroom_since = None;
            let need = self.cfg.min_kbps(self.class.higher());
            return Some(Action::Probe {
                up_to_kbps: need + need / 5,
            });
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TICK: Duration = Duration::from_millis(100);

    fn est(kbps: u32) -> Estimate {
        Estimate {
            kbps,
            saturated: true,
            loss_ppm: 0,
        }
    }

    fn sig(now: Duration, rtt_ms: u64, loss_ppm: u32, bitrate_kbps: u32) -> Signals {
        Signals {
            now,
            rtt: Duration::from_millis(rtt_ms),
            loss_ppm,
            bitrate_kbps,
            path: PathKind::Unknown,
            metered: false,
        }
    }

    fn start(path: PathKind, hint: bool) -> (Adapter, [Action; 2]) {
        Adapter::new(
            AdaptConfig::default(),
            Preference::Auto,
            path,
            hint,
            false,
            Duration::ZERO,
        )
    }

    /// Roda o laço lento com o mesmo sinal por `dur` e devolve as ações com o tempo.
    fn run(
        a: &mut Adapter,
        from: Duration,
        dur: Duration,
        f: impl Fn(Duration) -> Signals,
    ) -> (Duration, Vec<(Duration, Action)>) {
        let mut t = from;
        let mut out = Vec::new();
        while t < from + dur {
            t += TICK;
            if let Some(act) = a.update(f(t)) {
                out.push((t, act));
            }
        }
        (t, out)
    }

    #[test]
    fn classification_table() {
        let c = AdaptConfig::default();
        let ms = Duration::from_millis;
        assert_eq!(
            c.classify(400_000, ms(2), 0, PathKind::DirectLan),
            NetClass::Lan
        );
        assert_eq!(
            c.classify(400_000, ms(2), 0, PathKind::Relayed),
            NetClass::Fast
        );
        assert_eq!(
            c.classify(400_000, ms(12), 0, PathKind::DirectWan),
            NetClass::Fast
        );
        assert_eq!(
            c.classify(30_000, ms(60), 0, PathKind::DirectWan),
            NetClass::Medium
        );
        assert_eq!(
            c.classify(10_000, ms(20), 0, PathKind::Unknown),
            NetClass::Medium
        );
        assert_eq!(
            c.classify(3_000, ms(20), 0, PathKind::Unknown),
            NetClass::Slow
        );
        assert_eq!(
            c.classify(1_500, ms(20), 0, PathKind::Unknown),
            NetClass::Minimal
        );
        assert_eq!(
            c.classify(400_000, ms(2), 60_000, PathKind::DirectLan),
            NetClass::Minimal
        );
    }

    #[test]
    fn profiles_are_monotonic() {
        for w in NetClass::ALL.windows(2) {
            let (lo, hi) = (profile(w[0]), profile(w[1]));
            assert!(lo.bitrate_max_kbps <= hi.bitrate_max_kbps);
            assert!(lo.fps_max <= hi.fps_max);
            assert!(lo.bitrate_min_kbps < lo.bitrate_max_kbps);
        }
    }

    #[test]
    fn transport_rules_follow_loss_and_rtt() {
        let ms = Duration::from_millis;
        assert_eq!(
            transport_rules(NetClass::Fast, ms(10), 0),
            TransportRules {
                fec: false,
                nack: true
            }
        );
        assert!(transport_rules(NetClass::Fast, ms(10), 20_000).fec);
        assert!(!transport_rules(NetClass::Medium, ms(120), 0).nack);
        let min = transport_rules(NetClass::Minimal, ms(10), 0);
        assert!(min.fec && !min.nack);
    }

    #[test]
    fn starts_without_waiting_and_probes() {
        let (a, acts) = start(PathKind::Unknown, false);
        assert_eq!(a.class(), NetClass::Medium);
        assert_eq!(
            acts[0],
            Action::SetClass {
                class: NetClass::Medium,
                reason: Reason::Initial
            }
        );
        assert_eq!(
            acts[1],
            Action::Probe {
                up_to_kbps: 200_000
            }
        );

        let (a, _) = start(PathKind::Unknown, true);
        assert_eq!(a.class(), NetClass::Fast);
        let (_, acts) = start(PathKind::Relayed, true);
        assert_eq!(
            acts,
            [
                Action::SetClass {
                    class: NetClass::Medium,
                    reason: Reason::Initial
                },
                Action::Probe { up_to_kbps: 20_000 }
            ]
        );

        let (a, _) = Adapter::new(
            AdaptConfig::default(),
            Preference::SaveData,
            PathKind::DirectLan,
            true,
            false,
            Duration::ZERO,
        );
        assert_eq!(a.class(), NetClass::Slow);
    }

    #[test]
    fn same_router_probe_reaches_lan() {
        let (mut a, _) = start(PathKind::DirectLan, true);
        let act = a.on_probe(
            Duration::from_millis(600),
            est(390_000),
            Duration::from_millis(2),
        );
        assert_eq!(
            act,
            Some(Action::SetClass {
                class: NetClass::Lan,
                reason: Reason::Probe
            })
        );
        // Sem sondagem pendente, um resultado atrasado é ignorado.
        assert_eq!(
            a.on_probe(Duration::from_secs(1), est(1_000), Duration::from_millis(2)),
            None
        );
    }

    #[test]
    fn relayed_probe_is_capped_at_medium() {
        let (mut a, _) = start(PathKind::Relayed, false);
        assert_eq!(
            a.on_probe(
                Duration::from_millis(600),
                est(400_000),
                Duration::from_millis(2)
            ),
            None
        );
        assert_eq!(a.class(), NetClass::Medium);
    }

    #[test]
    fn far_slow_link_goes_down_on_first_probe() {
        let (mut a, _) = start(PathKind::Unknown, false);
        a.on_probe(
            Duration::from_millis(700),
            est(2_500),
            Duration::from_millis(90),
        );
        assert_eq!(a.class(), NetClass::Slow);
    }

    #[test]
    fn stable_network_never_changes_class() {
        let (mut a, _) = start(PathKind::Unknown, false);
        a.on_probe(
            Duration::from_millis(500),
            est(12_000),
            Duration::from_millis(30),
        );
        let (_, acts) = run(&mut a, Duration::ZERO, Duration::from_secs(600), |t| {
            sig(t, 30, 1_000, 6_000)
        });
        assert!(acts.is_empty(), "{acts:?}");
        assert_eq!(a.class(), NetClass::Medium);
    }

    #[test]
    fn sustained_loss_steps_down_one_class_at_a_time() {
        let (mut a, _) = start(PathKind::DirectWan, false);
        a.on_probe(
            Duration::from_millis(500),
            est(50_000),
            Duration::from_millis(15),
        );
        assert_eq!(a.class(), NetClass::Fast);
        let t0 = Duration::from_secs(20);
        let (_, acts) = run(&mut a, t0, Duration::from_millis(5_000), |t| Signals {
            path: PathKind::DirectWan,
            ..sig(t, 15, 50_000, 10_000)
        });
        // 1ª descida depois de 1 s de perda; a seguinte respeita o intervalo de 2 s.
        assert_eq!(
            acts[0].1,
            Action::SetClass {
                class: NetClass::Medium,
                reason: Reason::LossHigh
            }
        );
        let first = acts[0].0 - t0;
        assert!(first >= Duration::from_secs(1) && first <= Duration::from_millis(1_200));
        assert!(acts[1].0 - acts[0].0 >= Duration::from_secs(2));
        assert_eq!(
            acts[1].1,
            Action::SetClass {
                class: NetClass::Slow,
                reason: Reason::LossHigh
            }
        );
    }

    #[test]
    fn brief_loss_spike_is_ignored() {
        let (mut a, _) = start(PathKind::Unknown, false);
        let (t, acts) = run(
            &mut a,
            Duration::from_secs(5),
            Duration::from_millis(800),
            |t| sig(t, 20, 80_000, 5_000),
        );
        assert!(acts.is_empty());
        let (_, acts) = run(&mut a, t, Duration::from_secs(5), |t| sig(t, 20, 0, 5_000));
        assert!(acts.is_empty());
    }

    #[test]
    fn queue_growth_and_bitrate_floor_step_down() {
        let (mut a, _) = start(PathKind::Unknown, false);
        let (t, _) = run(&mut a, Duration::ZERO, Duration::from_secs(5), |t| {
            sig(t, 20, 0, 5_000)
        });
        let (t, acts) = run(&mut a, t, Duration::from_millis(1_500), |t| {
            sig(t, 90, 0, 5_000)
        });
        assert_eq!(
            acts.first().map(|x| x.1),
            Some(Action::SetClass {
                class: NetClass::Slow,
                reason: Reason::RttHigh
            })
        );
        let (_, acts) = run(&mut a, t, Duration::from_secs(4), |t| sig(t, 20, 0, 900));
        assert_eq!(
            acts.first().map(|x| x.1),
            Some(Action::SetClass {
                class: NetClass::Minimal,
                reason: Reason::BitrateFloor
            })
        );
    }

    #[test]
    fn headroom_probes_then_climbs_one_class() {
        let (mut a, _) = start(PathKind::Unknown, false);
        a.on_probe(
            Duration::from_millis(500),
            est(10_000),
            Duration::from_millis(10),
        );
        let (t, acts) = run(&mut a, Duration::ZERO, Duration::from_secs(12), |t| {
            sig(t, 10, 0, 8_000)
        });
        assert_eq!(acts.len(), 1, "{acts:?}");
        assert_eq!(acts[0].1, Action::Probe { up_to_kbps: 30_000 });
        // A folga só conta depois de 10 s da última troca e 5 s seguidos.
        assert!(acts[0].0 >= Duration::from_secs(10));
        let act = a.on_probe(t, est(60_000), Duration::from_millis(10));
        assert_eq!(
            act,
            Some(Action::SetClass {
                class: NetClass::Fast,
                reason: Reason::Probe
            })
        );
    }

    #[test]
    fn failed_up_probe_backs_off() {
        let (mut a, _) = start(PathKind::Unknown, false);
        a.on_probe(
            Duration::from_millis(500),
            est(10_000),
            Duration::from_millis(10),
        );
        let (t, acts) = run(&mut a, Duration::ZERO, Duration::from_secs(12), |t| {
            sig(t, 10, 0, 8_000)
        });
        assert!(matches!(acts[0].1, Action::Probe { .. }));
        assert_eq!(a.on_probe(t, est(12_000), Duration::from_millis(10)), None);
        assert_eq!(a.class(), NetClass::Medium);
        let (t2, acts) = run(&mut a, t, Duration::from_secs(14), |t| sig(t, 10, 0, 8_000));
        assert!(acts.is_empty(), "{acts:?}");
        let (_, acts) = run(&mut a, t2, Duration::from_secs(3), |t| sig(t, 10, 0, 8_000));
        assert!(matches!(
            acts.first().map(|x| x.1),
            Some(Action::Probe { .. })
        ));
    }

    #[test]
    fn no_up_attempt_right_after_going_down() {
        let (mut a, _) = start(PathKind::Unknown, false);
        let (t, acts) = run(
            &mut a,
            Duration::from_secs(10),
            Duration::from_millis(1_200),
            |t| sig(t, 20, 50_000, 5_000),
        );
        assert_eq!(a.class(), NetClass::Slow);
        let down_at = acts[0].0;
        let (_, acts) = run(&mut a, t, Duration::from_secs(25), |t| sig(t, 20, 0, 3_000));
        let probe_at = acts
            .iter()
            .find(|x| matches!(x.1, Action::Probe { .. }))
            .map(|x| x.0)
            .unwrap();
        assert!(probe_at - down_at >= Duration::from_secs(15));
    }

    #[test]
    fn path_to_relay_drops_immediately_and_reprobes_on_change() {
        let (mut a, _) = start(PathKind::DirectLan, true);
        a.on_probe(
            Duration::from_millis(500),
            est(390_000),
            Duration::from_millis(2),
        );
        assert_eq!(a.class(), NetClass::Lan);
        let mut s = sig(Duration::from_secs(30), 40, 0, 50_000);
        s.path = PathKind::Relayed;
        assert_eq!(
            a.update(s),
            Some(Action::SetClass {
                class: NetClass::Medium,
                reason: Reason::PathRelayed
            })
        );
        s.now += TICK;
        assert_eq!(a.update(s), Some(Action::Probe { up_to_kbps: 20_000 }));
        // Volta para direto: nova avaliação, que pode subir mais de uma classe.
        s.path = PathKind::DirectLan;
        s.now = Duration::from_secs(40);
        a.on_probe(s.now, est(15_000), Duration::from_millis(40));
        s.now += TICK;
        assert_eq!(
            a.update(s),
            Some(Action::Probe {
                up_to_kbps: 200_000
            })
        );
        assert_eq!(
            a.on_probe(s.now, est(390_000), Duration::from_millis(2)),
            Some(Action::SetClass {
                class: NetClass::Lan,
                reason: Reason::Probe
            })
        );
    }

    #[test]
    fn metered_and_preference_caps() {
        let (mut a, _) = start(PathKind::DirectLan, true);
        a.on_probe(
            Duration::from_millis(500),
            est(390_000),
            Duration::from_millis(2),
        );
        let mut s = sig(Duration::from_secs(5), 2, 0, 50_000);
        s.path = PathKind::DirectLan;
        s.metered = true;
        assert_eq!(
            a.update(s),
            Some(Action::SetClass {
                class: NetClass::Medium,
                reason: Reason::Metered
            })
        );
        assert_eq!(
            a.set_preference(Preference::SaveData, Duration::from_secs(6)),
            Some(Action::SetClass {
                class: NetClass::Slow,
                reason: Reason::Preference
            })
        );
        assert_eq!(
            a.set_preference(Preference::Sharpness, Duration::from_secs(7)),
            None
        );
    }

    #[test]
    fn unanswered_up_probe_counts_as_failure() {
        let (mut a, _) = start(PathKind::Unknown, false);
        a.on_probe(
            Duration::from_millis(500),
            est(10_000),
            Duration::from_millis(10),
        );
        let (_, acts) = run(&mut a, Duration::ZERO, Duration::from_secs(27), |t| {
            sig(t, 10, 0, 8_000)
        });
        // Sondagem aos 10 s, sem resposta até 13 s, próxima só depois de 15 s de espera.
        let probes: Vec<_> = acts.iter().map(|x| x.0).collect();
        assert_eq!(probes.len(), 1, "{acts:?}");
    }

    #[test]
    fn unanswered_probe_times_out() {
        let (mut a, _) = start(PathKind::Unknown, false);
        let (t, _) = run(&mut a, Duration::ZERO, Duration::from_secs(4), |t| {
            sig(t, 10, 0, 5_000)
        });
        assert_eq!(a.on_probe(t, est(400_000), Duration::from_millis(2)), None);
    }
}
