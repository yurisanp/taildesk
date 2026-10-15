//! Autorização no host: quem pode pedir acesso e o que pode ser oferecido
//! (`06-arquitetura.md` §6, `10-plano-windows-linux.md` §2.1, `12-levantamento-ux.md` §5.9).

use taildesk_overlay::PeerIdentity;
use taildesk_proto::control::{Feature, Permissions};

/// Um dispositivo que o host marcou "Lembrar deste dispositivo".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrustedDevice {
    /// `node_stable_id` da tailnet: sobrevive a troca de IP e de nome.
    pub node_stable_id: String,
    /// Nome mostrado em Configurações.
    pub name: String,
}

/// Configurações "Este computador".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostSettings {
    /// "Aceitar conexões". Desligado = todo pedido é recusado sem diálogo.
    pub accept_connections: bool,
    /// "Pedir permissão sempre": ignora a lista de confiáveis e mostra o diálogo toda vez.
    pub ask_always: bool,
    pub trusted: Vec<TrustedDevice>,
    /// "Permitir que quem se conecta ouça o som deste computador" (`15-audio.md` §8).
    pub allow_audio: bool,
    /// Troca de arquivos (fase 3). Pode ficar desligada por política.
    pub allow_files: bool,
}

impl Default for HostSettings {
    fn default() -> Self {
        HostSettings {
            accept_connections: true,
            ask_always: true,
            trusted: Vec::new(),
            allow_audio: true,
            allow_files: true,
        }
    }
}

impl HostSettings {
    pub fn is_trusted(&self, peer: &PeerIdentity) -> bool {
        !peer.node_stable_id.is_empty()
            && self
                .trusted
                .iter()
                .any(|t| t.node_stable_id == peer.node_stable_id)
    }

    /// "Lembrar deste dispositivo": acrescenta (ou renomeia) na lista.
    pub fn remember(&mut self, peer: &PeerIdentity) {
        if peer.node_stable_id.is_empty() {
            return;
        }
        match self
            .trusted
            .iter_mut()
            .find(|t| t.node_stable_id == peer.node_stable_id)
        {
            Some(t) => t.name.clone_from(&peer.hostname),
            None => self.trusted.push(TrustedDevice {
                node_stable_id: peer.node_stable_id.clone(),
                name: peer.hostname.clone(),
            }),
        }
    }

    /// Revoga um dispositivo confiável.
    pub fn forget(&mut self, node_stable_id: &str) {
        self.trusted.retain(|t| t.node_stable_id != node_stable_id);
    }
}

/// O que fazer com um pedido de acesso.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Decision {
    /// Fecha sem diálogo (`CloseReason::NotAllowed`).
    Refuse,
    /// Mostra o diálogo de consentimento oferecendo estas permissões.
    Ask(Permissions),
    /// Dispositivo confiável: entra direto com estas permissões (a faixa de sessão ativa
    /// aparece do mesmo jeito).
    Allow(Permissions),
}

/// Permissões que podem ser oferecidas, a partir do que o controlador anunciou no `Hello`.
pub fn offer(settings: &HostSettings, features: &[i32]) -> Permissions {
    let has = |f: Feature| features.contains(&(f as i32));
    Permissions {
        view: true,
        control: true,
        clipboard: has(Feature::Clipboard),
        files: settings.allow_files && has(Feature::Files),
        audio: settings.allow_audio && has(Feature::Audio),
    }
}

/// Decide o pedido de `peer` (identidade da tailnet, nunca do conteúdo das mensagens).
pub fn decide(settings: &HostSettings, peer: &PeerIdentity, features: &[i32]) -> Decision {
    if !settings.accept_connections || peer.node_stable_id.is_empty() {
        return Decision::Refuse;
    }
    let offer = offer(settings, features);
    if !settings.ask_always && settings.is_trusted(peer) {
        Decision::Allow(offer)
    } else {
        Decision::Ask(offer)
    }
}

/// `granted` limitado ao que foi oferecido; controlar exige ver.
pub fn clamp(granted: Permissions, offer: Permissions) -> Permissions {
    let view = granted.view && offer.view;
    Permissions {
        view,
        control: view && granted.control && offer.control,
        clipboard: view && granted.clipboard && offer.clipboard,
        files: granted.files && offer.files,
        audio: view && granted.audio && offer.audio,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn peer(id: &str) -> PeerIdentity {
        PeerIdentity {
            node_stable_id: id.into(),
            hostname: "notebook-yuri".into(),
            user_login: Some("yuri@".into()),
            tags: Vec::new(),
            app_caps: Vec::new(),
        }
    }

    const ALL: [i32; 3] = [
        Feature::Files as i32,
        Feature::Audio as i32,
        Feature::Clipboard as i32,
    ];

    #[test]
    fn padrao_pergunta_sempre() {
        let s = HostSettings::default();
        let Decision::Ask(p) = decide(&s, &peer("n1"), &ALL) else {
            panic!()
        };
        assert!(p.view && p.control && p.files && p.audio && p.clipboard);
    }

    #[test]
    fn so_oferece_o_que_o_controlador_anunciou_e_a_configuracao_permite() {
        let s = HostSettings {
            allow_audio: false,
            ..HostSettings::default()
        };
        let Decision::Ask(p) = decide(&s, &peer("n1"), &[Feature::Files as i32]) else {
            panic!()
        };
        assert!(p.files && !p.audio && !p.clipboard);
    }

    #[test]
    fn confiavel_entra_direto_se_nao_pedir_sempre() {
        let mut s = HostSettings {
            ask_always: false,
            ..HostSettings::default()
        };
        assert!(matches!(decide(&s, &peer("n1"), &ALL), Decision::Ask(_)));
        s.remember(&peer("n1"));
        s.remember(&peer("n1"));
        assert_eq!(s.trusted.len(), 1);
        assert!(matches!(decide(&s, &peer("n1"), &ALL), Decision::Allow(_)));
        assert!(matches!(decide(&s, &peer("n2"), &ALL), Decision::Ask(_)));
        s.ask_always = true;
        assert!(matches!(decide(&s, &peer("n1"), &ALL), Decision::Ask(_)));
        s.ask_always = false;
        s.forget("n1");
        assert!(matches!(decide(&s, &peer("n1"), &ALL), Decision::Ask(_)));
    }

    #[test]
    fn recusa_sem_aceitar_conexoes_ou_sem_identidade() {
        let s = HostSettings {
            accept_connections: false,
            ..HostSettings::default()
        };
        assert_eq!(decide(&s, &peer("n1"), &ALL), Decision::Refuse);
        let s = HostSettings::default();
        assert_eq!(decide(&s, &peer(""), &ALL), Decision::Refuse);
    }

    #[test]
    fn clamp_nao_amplia_e_controlar_exige_ver() {
        let tudo = Permissions {
            view: true,
            control: true,
            clipboard: true,
            files: true,
            audio: true,
        };
        let pouco = Permissions {
            view: true,
            control: false,
            clipboard: false,
            files: false,
            audio: false,
        };
        assert_eq!(clamp(tudo, pouco), pouco);
        let sem_ver = Permissions {
            view: false,
            ..tudo
        };
        let r = clamp(sem_ver, tudo);
        assert!(!r.view && !r.control && !r.audio && r.files);
    }
}
