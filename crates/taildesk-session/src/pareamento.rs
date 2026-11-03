//! Pareamento de celulares com este computador (`mobile/android-detalhado.md` §7, MF-20..22).
//!
//! "Configurações → Celulares → Conectar um celular" gera um código de uso único que vale
//! [`taildesk_proto::pair::CODE_TTL_SECS`] e aparece no QR. O celular manda o código num
//! `PairRequest`; acertou, fica pareado pelo `node_stable_id` do `whois` (nunca pelo nome que ele
//! diz). Celular pareado pode pedir a lista de dispositivos ([`pode_listar`]); pedir acesso
//! continua passando pelo diálogo, como qualquer controlador.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use taildesk_overlay::PeerIdentity;
use taildesk_proto::control::PairError;
use taildesk_proto::pair::{CODE_TTL_SECS, code_from_random, normalize_code};

use crate::policy::HostSettings;

/// Códigos errados de um mesmo nó até o bloqueio.
pub const TENTATIVAS: u32 = 5;
/// Quanto tempo um nó fica sem conferir código depois de errar demais.
pub const BLOQUEIO: Duration = Duration::from_secs(5 * 60);

/// Um celular pareado (Configurações → Celulares).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairedPhone {
    /// Do `whois`: sobrevive a troca de IP e de nome.
    pub node_stable_id: String,
    /// Nome do nó na tailnet (do `whois`).
    pub name: String,
    /// Nome que o celular disse no `PairRequest` (só para exibir).
    pub device_name: String,
    /// Quando pareou, em ms desde 1970.
    pub paired_at_ms: i64,
}

#[derive(Clone, Debug)]
struct Codigo {
    codigo: String,
    prazo: Instant,
}

#[derive(Clone, Copy, Debug, Default)]
struct Erros {
    n: u32,
    ate: Option<Instant>,
}

/// Código ativo e tentativas erradas (só em memória: some ao fechar o app).
#[derive(Debug, Default)]
pub struct Pareamento {
    ativo: Option<Codigo>,
    erros: HashMap<String, Erros>,
}

impl Pareamento {
    /// Gera um código novo (o anterior deixa de valer). `aleatorio` vem do gerador do sistema.
    pub fn novo_codigo(&mut self, aleatorio: [u8; 6], agora: Instant) -> String {
        let codigo = code_from_random(aleatorio);
        self.ativo = Some(Codigo {
            codigo: codigo.clone(),
            prazo: agora + Duration::from_secs(CODE_TTL_SECS),
        });
        codigo
    }

    /// Fechou a tela do QR: o código para de valer.
    pub fn cancelar(&mut self) {
        self.ativo = None;
    }

    /// Código em vigor e quanto falta para vencer.
    pub fn ativo(&self, agora: Instant) -> Option<(&str, Duration)> {
        let c = self.ativo.as_ref()?;
        let falta = c.prazo.checked_duration_since(agora)?;
        (!falta.is_zero()).then_some((c.codigo.as_str(), falta))
    }

    /// Confere o código que o nó `node` mandou. Acertou: o código é gasto.
    pub fn conferir(
        &mut self,
        digitado: &str,
        node: &str,
        agora: Instant,
    ) -> Result<(), PairError> {
        let erros = self.erros.entry(node.to_owned()).or_default();
        if erros.ate.is_some_and(|ate| agora < ate) {
            return Err(PairError::TooManyAttempts);
        }
        let Some(c) = &self.ativo else {
            return Err(PairError::BadCode);
        };
        if agora >= c.prazo {
            self.ativo = None;
            return Err(PairError::Expired);
        }
        if normalize_code(digitado).as_deref() != Some(c.codigo.as_str()) {
            erros.n += 1;
            if erros.n >= TENTATIVAS {
                erros.n = 0;
                erros.ate = Some(agora + BLOQUEIO);
                // Muita tentativa errada: troca o código, que pode ter vazado em parte.
                self.ativo = None;
            }
            return Err(PairError::BadCode);
        }
        self.erros.remove(node);
        self.ativo = None;
        Ok(())
    }
}

impl HostSettings {
    pub fn is_paired(&self, peer: &PeerIdentity) -> bool {
        !peer.node_stable_id.is_empty()
            && self
                .phones
                .iter()
                .any(|p| p.node_stable_id == peer.node_stable_id)
    }

    /// Acrescenta (ou atualiza) o celular pareado.
    pub fn pair(&mut self, peer: &PeerIdentity, device_name: &str, agora_ms: i64) {
        if peer.node_stable_id.is_empty() {
            return;
        }
        let device_name: String = device_name.chars().filter(|c| !c.is_control()).collect();
        let novo = PairedPhone {
            node_stable_id: peer.node_stable_id.clone(),
            name: peer.hostname.clone(),
            device_name,
            paired_at_ms: agora_ms,
        };
        match self
            .phones
            .iter_mut()
            .find(|p| p.node_stable_id == peer.node_stable_id)
        {
            Some(p) => *p = novo,
            None => self.phones.push(novo),
        }
    }

    /// Remove o celular (Configurações → Celulares → Remover).
    pub fn unpair(&mut self, node_stable_id: &str) {
        self.phones.retain(|p| p.node_stable_id != node_stable_id);
    }
}

/// Pode receber a lista de dispositivos: o mesmo usuário da tailnet que o dono deste computador
/// (`dono` = login do `tailscaled` daqui) ou um celular pareado. Nó com tag só se pareado.
pub fn pode_listar(settings: &HostSettings, peer: &PeerIdentity, dono: Option<&str>) -> bool {
    if peer.node_stable_id.is_empty() {
        return false;
    }
    let mesmo_usuario = matches!(
        (peer.user_login.as_deref(), dono),
        (Some(a), Some(b)) if !a.is_empty() && a.eq_ignore_ascii_case(b)
    );
    mesmo_usuario || settings.is_paired(peer)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(id: &str, user: Option<&str>) -> PeerIdentity {
        PeerIdentity {
            node_stable_id: id.into(),
            hostname: "pixel-8".into(),
            user_login: user.map(Into::into),
            tags: Vec::new(),
            app_caps: Vec::new(),
        }
    }

    #[test]
    fn codigo_vale_uma_vez_e_vence() {
        let t0 = Instant::now();
        let mut p = Pareamento::default();
        assert_eq!(p.conferir("ABC123", "n1", t0), Err(PairError::BadCode));
        let c = p.novo_codigo([1, 2, 3, 4, 5, 6], t0);
        assert_eq!(p.ativo(t0).unwrap().0, c);
        // Digitado em minúsculas e com hífen também vale.
        let digitado = format!("{}-{}", &c[..3], &c[3..]).to_lowercase();
        assert_eq!(p.conferir(&digitado, "n1", t0), Ok(()));
        // Gasto.
        assert_eq!(p.conferir(&c, "n2", t0), Err(PairError::BadCode));
        assert!(p.ativo(t0).is_none());

        let c = p.novo_codigo([9; 6], t0);
        let depois = t0 + Duration::from_secs(CODE_TTL_SECS);
        assert!(p.ativo(depois).is_none());
        assert_eq!(p.conferir(&c, "n1", depois), Err(PairError::Expired));
    }

    #[test]
    fn errar_demais_bloqueia_e_troca_o_codigo() {
        let t0 = Instant::now();
        let mut p = Pareamento::default();
        let c = p.novo_codigo([7; 6], t0);
        for _ in 0..TENTATIVAS {
            assert_eq!(p.conferir("ZZZZZZ", "intruso", t0), Err(PairError::BadCode));
        }
        // O código antigo não vale mais, e o nó está bloqueado.
        assert_eq!(
            p.conferir(&c, "intruso", t0),
            Err(PairError::TooManyAttempts)
        );
        let c2 = p.novo_codigo([8; 6], t0);
        assert_eq!(
            p.conferir(&c2, "intruso", t0 + Duration::from_secs(1)),
            Err(PairError::TooManyAttempts)
        );
        // Outro nó não é afetado; o bloqueado volta depois do tempo.
        assert_eq!(p.conferir(&c2, "celular", t0), Ok(()));
        let c3 = p.novo_codigo([10; 6], t0);
        assert_eq!(p.conferir(&c3, "intruso", t0 + BLOQUEIO), Ok(()));
    }

    #[test]
    fn lista_so_para_o_dono_ou_pareado() {
        let mut s = HostSettings::default();
        let dono = Some("yuri@example.com");
        assert!(pode_listar(&s, &peer("n1", Some("Yuri@Example.com")), dono));
        assert!(!pode_listar(&s, &peer("n1", Some("ana@example.com")), dono));
        assert!(!pode_listar(&s, &peer("n1", None), dono));
        assert!(!pode_listar(
            &s,
            &peer("n1", Some("yuri@example.com")),
            None
        ));
        assert!(!pode_listar(&s, &peer("", Some("yuri@example.com")), dono));

        let ana = peer("n2", Some("ana@example.com"));
        s.pair(&ana, "Galaxy\nda Ana", 1_000);
        assert!(pode_listar(&s, &ana, dono));
        assert_eq!(s.phones[0].device_name, "Galaxyda Ana");
        // Parear de novo atualiza em vez de duplicar.
        s.pair(&ana, "Galaxy S24", 2_000);
        assert_eq!(s.phones.len(), 1);
        assert_eq!(s.phones[0].paired_at_ms, 2_000);
        s.unpair("n2");
        assert!(!pode_listar(&s, &ana, dono));
    }
}
