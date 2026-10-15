//! Mensagens de controle da seleção adaptativa (`14-selecao-adaptativa.md` §4).
//!
//! Tipos Rust e limites de parser. A codificação protobuf entra com o `prost` junto com o
//! transporte (fase 0); os valores numéricos dos enums abaixo são os do `.proto` e não mudam.

use std::fmt;

/// Classe de rede, da pior para a melhor (a ordem de `Ord` é a de qualidade).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NetClass {
    Minimal = 1,
    Slow = 2,
    Medium = 3,
    Fast = 4,
    Lan = 5,
}

impl NetClass {
    pub const ALL: [NetClass; 5] = [
        NetClass::Minimal,
        NetClass::Slow,
        NetClass::Medium,
        NetClass::Fast,
        NetClass::Lan,
    ];

    /// Valor no fio; `None` para valores desconhecidos (0 = não informado).
    pub fn from_wire(v: u32) -> Option<NetClass> {
        NetClass::ALL.into_iter().find(|c| *c as u32 == v)
    }

    pub fn to_wire(self) -> u32 {
        self as u32
    }

    /// Uma classe abaixo (satura em `Minimal`).
    #[must_use]
    pub fn lower(self) -> NetClass {
        NetClass::from_wire(self as u32 - 1).unwrap_or(NetClass::Minimal)
    }

    /// Uma classe acima (satura em `Lan`).
    #[must_use]
    pub fn higher(self) -> NetClass {
        NetClass::from_wire(self as u32 + 1).unwrap_or(NetClass::Lan)
    }
}

/// Por onde os pacotes passam até o peer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PathKind {
    #[default]
    Unknown = 0,
    DirectLan = 1,
    DirectWan = 2,
    Relayed = 3,
}

impl PathKind {
    pub fn from_wire(v: u32) -> PathKind {
        match v {
            1 => PathKind::DirectLan,
            2 => PathKind::DirectWan,
            3 => PathKind::Relayed,
            _ => PathKind::Unknown,
        }
    }

    pub fn to_wire(self) -> u32 {
        self as u32
    }
}

/// Backend do Tailscale em uso.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum OverlayMode {
    #[default]
    Unknown = 0,
    Embedded = 1,
    System = 2,
}

/// Motivo de uma troca de classe. O texto mostrado ao usuário vem do i18n, pelo código.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Reason {
    /// Classe inicial, antes de medir (pista do caminho).
    Initial = 1,
    /// Resultado da sondagem de banda.
    Probe = 2,
    /// Perda acima do limite por tempo demais.
    LossHigh = 3,
    /// Bitrate preso no piso do perfil.
    BitrateFloor = 4,
    /// RTT subiu muito acima do mínimo (fila crescendo).
    RttHigh = 5,
    /// Caminho passou a ser relay DERP.
    PathRelayed = 6,
    /// Conexão tarifada.
    Metered = 7,
    /// Preferência do usuário mudou.
    Preference = 8,
}

impl Reason {
    const ALL: [Reason; 8] = [
        Reason::Initial,
        Reason::Probe,
        Reason::LossHigh,
        Reason::BitrateFloor,
        Reason::RttHigh,
        Reason::PathRelayed,
        Reason::Metered,
        Reason::Preference,
    ];

    pub fn from_wire(v: u32) -> Option<Reason> {
        Reason::ALL.into_iter().find(|r| *r as u32 == v)
    }

    pub fn to_wire(self) -> u32 {
        self as u32
    }

    /// Código estável para logs e i18n.
    pub fn code(self) -> &'static str {
        match self {
            Reason::Initial => "initial",
            Reason::Probe => "probe",
            Reason::LossHigh => "loss_high",
            Reason::BitrateFloor => "bitrate_floor",
            Reason::RttHigh => "rtt_high",
            Reason::PathRelayed => "path_relayed",
            Reason::Metered => "metered",
            Reason::Preference => "preference",
        }
    }
}

/// Limites dos parsers (regra do `AGENTS.md`).
pub mod limits {
    /// Degraus por sondagem; igual a `probe::MAX_STEPS`.
    pub const PROBE_STEPS: usize = crate::probe::MAX_STEPS as usize;
    pub const PROBE_STEP_MS: u32 = 500;
    pub const PROBE_MAX_KBPS: u32 = 1_000_000;
    pub const BENCH_DURATION_MS: u32 = 30_000;
    pub const BENCH_MAX_KBPS: u32 = 1_000_000;
    pub const DERP_REGION_LEN: usize = 32;
}

/// Mensagem fora dos limites.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LimitError {
    pub field: &'static str,
    pub value: u64,
    pub max: u64,
}

impl fmt::Display for LimitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "campo {} = {} acima do limite {}",
            self.field, self.value, self.max
        )
    }
}

impl std::error::Error for LimitError {}

fn check(field: &'static str, value: u64, max: u64) -> Result<(), LimitError> {
    if value > max {
        Err(LimitError { field, value, max })
    } else {
        Ok(())
    }
}

/// H→C: caminho atual até o controlador.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LinkInfo {
    pub path: PathKind,
    /// Código da região DERP ("sao", "mia"); vazio se direto ou desconhecido.
    pub derp_region: String,
    pub same_lan_hint: bool,
    pub overlay: OverlayMode,
}

impl LinkInfo {
    pub fn validate(&self) -> Result<(), LimitError> {
        check(
            "derp_region",
            self.derp_region.len() as u64,
            limits::DERP_REGION_LEN as u64,
        )
    }
}

/// Pede uma rajada de sondagem; quem recebe os datagramas responde com [`ProbeResult`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeRequest {
    pub probe_id: u32,
    pub steps_kbps: Vec<u32>,
    pub step_ms: u32,
}

impl ProbeRequest {
    pub fn validate(&self) -> Result<(), LimitError> {
        check(
            "steps_kbps",
            self.steps_kbps.len() as u64,
            limits::PROBE_STEPS as u64,
        )?;
        check("step_ms", self.step_ms.into(), limits::PROBE_STEP_MS.into())?;
        for &k in &self.steps_kbps {
            check("steps_kbps[]", k.into(), limits::PROBE_MAX_KBPS.into())?;
        }
        Ok(())
    }
}

/// Resultado de um degrau, medido no receptor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProbeStep {
    /// Taxa pedida pelo remetente.
    pub sent_kbps: u32,
    /// Taxa que chegou.
    pub recv_kbps: u32,
    /// Perda em partes por milhão.
    pub loss_ppm: u32,
    /// Quanto o atraso de ida médio do degrau subiu em relação ao menor visto na sondagem.
    pub owd_delta_us: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProbeResult {
    pub probe_id: u32,
    pub steps: Vec<ProbeStep>,
}

impl ProbeResult {
    pub fn validate(&self) -> Result<(), LimitError> {
        check("steps", self.steps.len() as u64, limits::PROBE_STEPS as u64)?;
        for s in &self.steps {
            check("loss_ppm", s.loss_ppm.into(), 1_000_000)?;
        }
        Ok(())
    }
}

/// H→C: o host mudou a classe de um stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProfileChanged {
    pub class: NetClass,
    /// Stream de vídeo (monitor) a que a classe se aplica.
    pub display_id: u32,
    pub reason: Reason,
}

/// O que a aba "Teste de conexão" pede ao host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BenchKind {
    Rtt = 1,
    Upload = 2,
    Download = 3,
    Demo = 4,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BenchRequest {
    pub kind: BenchKind,
    pub duration_ms: u32,
    pub max_kbps: u32,
}

impl BenchRequest {
    pub fn validate(&self) -> Result<(), LimitError> {
        check(
            "duration_ms",
            self.duration_ms.into(),
            limits::BENCH_DURATION_MS.into(),
        )?;
        check(
            "max_kbps",
            self.max_kbps.into(),
            limits::BENCH_MAX_KBPS.into(),
        )
    }
}

/// Resultado de um teste, para a aba e para `bench-net --json`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BenchReport {
    pub rtt_min_us: u32,
    pub rtt_median_us: u32,
    pub rtt_p95_us: u32,
    pub jitter_us: u32,
    pub loss_ppm: u32,
    pub up_kbps: u32,
    pub down_kbps: u32,
    pub max_datagram: u16,
}

/// Codificação binária provisória de [`ProbeRequest`] e [`ProbeResult`], usada pelo `bench-net`
/// enquanto o protobuf não entra. Little-endian; o tamanho das listas é validado antes de alocar.
pub mod wire {
    use super::{ProbeRequest, ProbeResult, ProbeStep, limits};

    /// Dados malformados ou fora dos limites.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct WireError;

    impl std::fmt::Display for WireError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("mensagem de sondagem malformada")
        }
    }

    impl std::error::Error for WireError {}

    struct Reader<'a>(&'a [u8]);

    impl Reader<'_> {
        fn u8(&mut self) -> Result<u8, WireError> {
            let (&b, rest) = self.0.split_first().ok_or(WireError)?;
            self.0 = rest;
            Ok(b)
        }

        fn u32(&mut self) -> Result<u32, WireError> {
            let (head, rest) = self.0.split_first_chunk::<4>().ok_or(WireError)?;
            self.0 = rest;
            Ok(u32::from_le_bytes(*head))
        }

        fn count(&mut self) -> Result<usize, WireError> {
            let n = usize::from(self.u8()?);
            if n > limits::PROBE_STEPS {
                return Err(WireError);
            }
            Ok(n)
        }

        fn end(&self) -> Result<(), WireError> {
            if self.0.is_empty() {
                Ok(())
            } else {
                Err(WireError)
            }
        }
    }

    /// Escreve `req` em `out`. Pedidos fora dos limites são recusados.
    pub fn encode_probe_request(req: &ProbeRequest, out: &mut Vec<u8>) -> Result<(), WireError> {
        req.validate().map_err(|_| WireError)?;
        out.extend_from_slice(&req.probe_id.to_le_bytes());
        out.extend_from_slice(&req.step_ms.to_le_bytes());
        out.push(req.steps_kbps.len() as u8);
        for k in &req.steps_kbps {
            out.extend_from_slice(&k.to_le_bytes());
        }
        Ok(())
    }

    pub fn decode_probe_request(buf: &[u8]) -> Result<ProbeRequest, WireError> {
        let mut r = Reader(buf);
        let probe_id = r.u32()?;
        let step_ms = r.u32()?;
        let n = r.count()?;
        let steps_kbps = (0..n).map(|_| r.u32()).collect::<Result<_, _>>()?;
        r.end()?;
        let req = ProbeRequest {
            probe_id,
            steps_kbps,
            step_ms,
        };
        req.validate().map_err(|_| WireError)?;
        Ok(req)
    }

    pub fn encode_probe_result(res: &ProbeResult, out: &mut Vec<u8>) -> Result<(), WireError> {
        res.validate().map_err(|_| WireError)?;
        out.extend_from_slice(&res.probe_id.to_le_bytes());
        out.push(res.steps.len() as u8);
        for s in &res.steps {
            for v in [s.sent_kbps, s.recv_kbps, s.loss_ppm, s.owd_delta_us] {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        Ok(())
    }

    pub fn decode_probe_result(buf: &[u8]) -> Result<ProbeResult, WireError> {
        let mut r = Reader(buf);
        let probe_id = r.u32()?;
        let n = r.count()?;
        let steps = (0..n)
            .map(|_| {
                Ok(ProbeStep {
                    sent_kbps: r.u32()?,
                    recv_kbps: r.u32()?,
                    loss_ppm: r.u32()?,
                    owd_delta_us: r.u32()?,
                })
            })
            .collect::<Result<_, WireError>>()?;
        r.end()?;
        let res = ProbeResult { probe_id, steps };
        res.validate().map_err(|_| WireError)?;
        Ok(res)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn net_class_wire_round_trip_and_order() {
        for c in NetClass::ALL {
            assert_eq!(NetClass::from_wire(c.to_wire()), Some(c));
        }
        assert_eq!(NetClass::from_wire(0), None);
        assert_eq!(NetClass::from_wire(6), None);
        assert!(NetClass::Lan > NetClass::Fast && NetClass::Slow > NetClass::Minimal);
        assert_eq!(NetClass::Minimal.lower(), NetClass::Minimal);
        assert_eq!(NetClass::Lan.higher(), NetClass::Lan);
        assert_eq!(NetClass::Medium.lower(), NetClass::Slow);
        assert_eq!(NetClass::Medium.higher(), NetClass::Fast);
    }

    #[test]
    fn path_and_reason_wire_round_trip() {
        for p in [
            PathKind::Unknown,
            PathKind::DirectLan,
            PathKind::DirectWan,
            PathKind::Relayed,
        ] {
            assert_eq!(PathKind::from_wire(p.to_wire()), p);
        }
        assert_eq!(PathKind::from_wire(42), PathKind::Unknown);
        for r in Reason::ALL {
            assert_eq!(Reason::from_wire(r.to_wire()), Some(r));
            assert!(!r.code().is_empty());
        }
        assert_eq!(Reason::from_wire(0), None);
    }

    #[test]
    fn probe_request_limits() {
        let ok = ProbeRequest {
            probe_id: 1,
            steps_kbps: vec![5_000, 20_000, 50_000],
            step_ms: 100,
        };
        assert_eq!(ok.validate(), Ok(()));
        let too_many = ProbeRequest {
            steps_kbps: vec![1; limits::PROBE_STEPS + 1],
            ..ok.clone()
        };
        assert_eq!(too_many.validate().unwrap_err().field, "steps_kbps");
        let too_long = ProbeRequest {
            step_ms: 501,
            ..ok.clone()
        };
        assert_eq!(too_long.validate().unwrap_err().field, "step_ms");
        let too_fast = ProbeRequest {
            steps_kbps: vec![limits::PROBE_MAX_KBPS + 1],
            ..ok
        };
        assert_eq!(too_fast.validate().unwrap_err().field, "steps_kbps[]");
    }

    #[test]
    fn probe_result_and_link_and_bench_limits() {
        let bad_loss = ProbeResult {
            probe_id: 1,
            steps: vec![ProbeStep {
                loss_ppm: 1_000_001,
                ..ProbeStep::default()
            }],
        };
        assert_eq!(bad_loss.validate().unwrap_err().field, "loss_ppm");
        let link = LinkInfo {
            derp_region: "x".repeat(limits::DERP_REGION_LEN + 1),
            ..LinkInfo::default()
        };
        assert_eq!(link.validate().unwrap_err().field, "derp_region");
        let bench = BenchRequest {
            kind: BenchKind::Download,
            duration_ms: 30_001,
            max_kbps: 10,
        };
        assert_eq!(bench.validate().unwrap_err().field, "duration_ms");
    }

    #[test]
    fn probe_request_wire_round_trip() {
        let req = ProbeRequest {
            probe_id: 0xabcd_0001,
            steps_kbps: vec![5_000, 20_000, 200_000],
            step_ms: 100,
        };
        let mut buf = Vec::new();
        wire::encode_probe_request(&req, &mut buf).unwrap();
        assert_eq!(buf.len(), 4 + 4 + 1 + 3 * 4);
        assert_eq!(wire::decode_probe_request(&buf), Ok(req));
        assert_eq!(
            wire::decode_probe_request(&buf[..buf.len() - 1]),
            Err(wire::WireError)
        );
        buf.push(0);
        assert_eq!(wire::decode_probe_request(&buf), Err(wire::WireError));
    }

    #[test]
    fn probe_result_wire_round_trip() {
        let res = ProbeResult {
            probe_id: 9,
            steps: vec![
                ProbeStep {
                    sent_kbps: 5_000,
                    recv_kbps: 4_900,
                    loss_ppm: 0,
                    owd_delta_us: 120,
                },
                ProbeStep {
                    sent_kbps: 20_000,
                    recv_kbps: 11_000,
                    loss_ppm: 40_000,
                    owd_delta_us: 9_000,
                },
            ],
        };
        let mut buf = Vec::new();
        wire::encode_probe_result(&res, &mut buf).unwrap();
        assert_eq!(wire::decode_probe_result(&buf), Ok(res));
    }

    #[test]
    fn wire_rejects_out_of_limit_lists() {
        // probe_id, step_ms = 100 e uma contagem acima do limite, sem os valores.
        let mut buf = vec![0, 0, 0, 0, 100, 0, 0, 0, limits::PROBE_STEPS as u8 + 1];
        assert_eq!(wire::decode_probe_request(&buf), Err(wire::WireError));
        // step_ms acima do limite com uma lista válida.
        buf[4..8].copy_from_slice(&10_000u32.to_le_bytes());
        buf[8] = 0;
        assert_eq!(wire::decode_probe_request(&buf), Err(wire::WireError));
        let too_long = ProbeRequest {
            probe_id: 1,
            steps_kbps: vec![1; limits::PROBE_STEPS + 1],
            step_ms: 100,
        };
        assert_eq!(
            wire::encode_probe_request(&too_long, &mut Vec::new()),
            Err(wire::WireError)
        );
    }
}
