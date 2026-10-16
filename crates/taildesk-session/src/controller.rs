//! Sessão no controlador: as etapas de "Conectando" (`12-levantamento-ux.md` §5.3) e o motivo
//! do fim, para a UI dizer se o host recusou ou não respondeu.

use std::time::{Duration, Instant};

use taildesk_proto::control::{CloseReason, Consent, Msg, Permissions};

use crate::host::CONSENT_TIMEOUT;

/// Margem além do prazo do host antes de desistir sozinho (o `Close` do host deve chegar antes).
const MARGIN: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ControllerState {
    /// "Encontrando desktop-casa…": conexão e `Hello` enviados.
    Connecting,
    /// "Esperando desktop-casa aceitar…", com o que foi pedido.
    AwaitingConsent {
        asked: Permissions,
        since: Instant,
    },
    Live {
        permissions: Permissions,
    },
    Ended(EndReason),
}

/// Por que a sessão acabou, em termos que a UI mostra.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndReason {
    /// A pessoa no host clicou Recusar.
    Refused,
    /// Ninguém respondeu no host.
    NoAnswer,
    /// O host não aceita conexões deste computador.
    NotAllowed,
    /// Versões do TailDesk incompatíveis.
    VersionMismatch,
    /// O host encerrou.
    ByHost,
    /// Encerrada aqui.
    ByUser,
    /// Mensagem inesperada ou inválida.
    ProtocolError,
    /// A conexão caiu (rede, host desligado ou o app do host fechou).
    Lost,
    /// A senha de acesso não confere (ou o host não usa mais senha).
    BadPassword,
    /// Senhas erradas demais: o host não confere por alguns minutos.
    TooManyAttempts,
}

fn end_reason(reason: i32) -> EndReason {
    match CloseReason::try_from(reason) {
        Ok(CloseReason::ConsentDenied) => EndReason::Refused,
        Ok(CloseReason::ConsentTimeout) => EndReason::NoAnswer,
        Ok(CloseReason::NotAllowed) => EndReason::NotAllowed,
        Ok(CloseReason::VersionMismatch) => EndReason::VersionMismatch,
        Ok(CloseReason::ByController) => EndReason::ByUser,
        Ok(CloseReason::ProtocolError) => EndReason::ProtocolError,
        Ok(CloseReason::BadPassword) => EndReason::BadPassword,
        Ok(CloseReason::TooManyAttempts) => EndReason::TooManyAttempts,
        Ok(CloseReason::ByHost | CloseReason::Unspecified) | Err(_) => EndReason::ByHost,
    }
}

#[derive(Debug)]
pub struct ControllerSession {
    state: ControllerState,
}

impl Default for ControllerSession {
    fn default() -> Self {
        Self::new()
    }
}

impl ControllerSession {
    pub fn new() -> Self {
        ControllerSession {
            state: ControllerState::Connecting,
        }
    }

    pub fn state(&self) -> ControllerState {
        self.state
    }

    /// Mensagens de sessão vindas do host. As outras não mudam o estado.
    pub fn on_message(&mut self, msg: &Msg, now: Instant) {
        if matches!(self.state, ControllerState::Ended(_)) {
            return;
        }
        match (msg, self.state) {
            (Msg::HelloAck(ack), ControllerState::Connecting) => {
                let permissions = ack.permissions.unwrap_or_default();
                self.state = match Consent::try_from(ack.consent) {
                    Ok(Consent::NotRequired) => ControllerState::Live { permissions },
                    _ => ControllerState::AwaitingConsent {
                        asked: permissions,
                        since: now,
                    },
                };
            }
            (Msg::ConsentResult(r), ControllerState::AwaitingConsent { .. }) => {
                self.state = match (r.granted, r.permissions) {
                    (true, Some(permissions)) if permissions.view => {
                        ControllerState::Live { permissions }
                    }
                    _ => ControllerState::Ended(EndReason::Refused),
                };
            }
            (Msg::Close(c), _) => self.state = ControllerState::Ended(end_reason(c.reason)),
            (Msg::HelloAck(_) | Msg::ConsentResult(_), _) => {
                self.state = ControllerState::Ended(EndReason::ProtocolError);
            }
            _ => {}
        }
    }

    /// Desiste se o host não respondeu nem fechou depois do prazo do diálogo.
    pub fn tick(&mut self, now: Instant) {
        if let ControllerState::AwaitingConsent { since, .. } = self.state
            && now >= since + CONSENT_TIMEOUT + MARGIN
        {
            self.state = ControllerState::Ended(EndReason::NoAnswer);
        }
    }

    /// Cancelar enquanto espera, ou encerrar a sessão.
    pub fn end(&mut self) {
        if !matches!(self.state, ControllerState::Ended(_)) {
            self.state = ControllerState::Ended(EndReason::ByUser);
        }
    }

    /// A conexão caiu antes de um `Close`.
    pub fn lost(&mut self) {
        if !matches!(self.state, ControllerState::Ended(_)) {
            self.state = ControllerState::Ended(EndReason::Lost);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::{ConsentAnswer, HelloReply, HostSession};
    use crate::policy::HostSettings;
    use taildesk_overlay::PeerIdentity;
    use taildesk_proto::control::{Hello, HelloAck, PROTO_VERSION};

    fn peer() -> PeerIdentity {
        PeerIdentity {
            node_stable_id: "nABC".into(),
            hostname: "notebook-yuri".into(),
            user_login: Some("yuri@".into()),
            tags: Vec::new(),
            app_caps: Vec::new(),
        }
    }

    fn hello() -> Hello {
        Hello {
            proto_version: PROTO_VERSION,
            ..Hello::default()
        }
    }

    /// Liga o host e o controlador como se as mensagens fossem pelo fio.
    fn conectar(settings: &HostSettings, t0: Instant) -> (HostSession, ControllerSession) {
        let mut host = HostSession::new(peer());
        let mut ctl = ControllerSession::new();
        match host.on_hello(&hello(), settings, t0) {
            HelloReply::Ack(f) => ctl.on_message(
                &Msg::HelloAck(HelloAck {
                    proto_version: PROTO_VERSION,
                    consent: f.consent.into(),
                    permissions: Some(f.permissions),
                    ..HelloAck::default()
                }),
                t0,
            ),
            HelloReply::Close(c) => ctl.on_message(&Msg::Close(c), t0),
        }
        (host, ctl)
    }

    #[test]
    fn fluxo_com_consentimento() {
        let mut settings = HostSettings::default();
        let t0 = Instant::now();
        let (mut host, mut ctl) = conectar(&settings, t0);
        assert!(matches!(
            ctl.state(),
            ControllerState::AwaitingConsent { .. }
        ));
        let offer = host.prompt().unwrap().offer;
        for m in host.answer(
            ConsentAnswer::Allow {
                permissions: offer,
                remember: false,
            },
            &mut settings,
        ) {
            ctl.on_message(&m, t0);
        }
        assert_eq!(ctl.state(), ControllerState::Live { permissions: offer });
    }

    #[test]
    fn recusado_e_sem_resposta_sao_diferentes() {
        let mut settings = HostSettings::default();
        let t0 = Instant::now();
        let (mut host, mut ctl) = conectar(&settings, t0);
        for m in host.answer(ConsentAnswer::Refuse, &mut settings) {
            ctl.on_message(&m, t0);
        }
        assert_eq!(ctl.state(), ControllerState::Ended(EndReason::Refused));

        let (mut host, mut ctl) = conectar(&settings, t0);
        let close = host.tick(t0 + CONSENT_TIMEOUT).unwrap();
        ctl.on_message(&Msg::Close(close), t0 + CONSENT_TIMEOUT);
        assert_eq!(ctl.state(), ControllerState::Ended(EndReason::NoAnswer));

        // O host sumiu sem mandar nada: o controlador desiste sozinho.
        let (_host, mut ctl) = conectar(&settings, t0);
        ctl.tick(t0 + CONSENT_TIMEOUT);
        assert!(matches!(
            ctl.state(),
            ControllerState::AwaitingConsent { .. }
        ));
        ctl.tick(t0 + CONSENT_TIMEOUT + MARGIN);
        assert_eq!(ctl.state(), ControllerState::Ended(EndReason::NoAnswer));
    }

    #[test]
    fn nao_permitido_e_confiavel() {
        let t0 = Instant::now();
        let settings = HostSettings {
            accept_connections: false,
            ..HostSettings::default()
        };
        let (_, ctl) = conectar(&settings, t0);
        assert_eq!(ctl.state(), ControllerState::Ended(EndReason::NotAllowed));

        let mut settings = HostSettings {
            ask_always: false,
            ..HostSettings::default()
        };
        settings.remember(&peer());
        let (_, mut ctl) = conectar(&settings, t0);
        assert!(matches!(ctl.state(), ControllerState::Live { .. }));
        ctl.end();
        assert_eq!(ctl.state(), ControllerState::Ended(EndReason::ByUser));
    }
}
