//! Sessão no host: `Hello` -> (diálogo de consentimento) -> ao vivo, e o filtro de permissões
//! das mensagens que chegam do controlador.

use std::time::{Duration, Instant};

use taildesk_overlay::PeerIdentity;
use taildesk_proto::control::{
    Close, CloseReason, Consent, ConsentResult, Hello, Msg, PROTO_VERSION, Permissions,
};

use crate::policy::{Decision, HostSettings, clamp, decide, offer};

/// Quanto o diálogo espera antes de recusar sozinho (`12-levantamento-ux.md` §5.3).
pub const CONSENT_TIMEOUT: Duration = Duration::from_secs(60);

/// Resposta da pessoa no diálogo do host.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ConsentAnswer {
    /// "Permitir", com as caixas que ficaram marcadas.
    Allow {
        permissions: Permissions,
        remember: bool,
    },
    /// "Recusar" (ou fechar o diálogo).
    Refuse,
}

/// O que o diálogo mostra.
#[derive(Clone, Debug, PartialEq)]
pub struct ConsentPrompt {
    pub peer: PeerIdentity,
    pub offer: Permissions,
    pub deadline: Instant,
}

#[derive(Clone, Debug, PartialEq)]
pub enum HostState {
    AwaitHello,
    AwaitConsent(ConsentPrompt),
    Live {
        permissions: Permissions,
        /// "Pausar controle" na faixa do host: mouse e teclado do controlador são ignorados.
        control_paused: bool,
    },
    Closed(CloseReason),
}

/// Campos do `HelloAck` que dependem da autorização; o resto (monitores, codecs, teclado) vem
/// dos outros módulos.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AckFields {
    pub consent: Consent,
    /// Pedido: o que está sendo oferecido. Liberado: o que vale.
    pub permissions: Permissions,
}

/// Resposta ao `Hello`.
#[derive(Clone, Debug, PartialEq)]
pub enum HelloReply {
    /// Mande `HelloAck` com estes campos. Se `consent` for `Required`, mostre
    /// [`HostSession::prompt`] e espere [`HostSession::answer`].
    Ack(AckFields),
    /// Mande o `Close` e encerre (versão diferente ou acesso não permitido).
    Close(Close),
}

/// O que uma mensagem do controlador exige.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    Nothing,
    View,
    Control,
    /// Recusar responde `AudioStopped{NO_PERMISSION}` (`15-audio.md`).
    Audio,
}

/// Permissão que cada mensagem vinda do controlador exige no host.
pub fn need(msg: &Msg) -> Need {
    match msg {
        Msg::StartVideo(_)
        | Msg::StopVideo(_)
        | Msg::SetPriority(_)
        | Msg::SetVideoQuality(_)
        | Msg::RequestKeyframe(_)
        | Msg::PictureLoss(_)
        | Msg::Nack(_)
        | Msg::ReceiverReport(_) => Need::View,
        Msg::MouseMove(_)
        | Msg::MouseButton(_)
        | Msg::Scroll(_)
        | Msg::KeyboardLayoutChanged(_)
        | Msg::KeyboardMode(_)
        | Msg::Key(_)
        | Msg::KeyShortcut(_)
        | Msg::KeyChar(_)
        | Msg::SecureAttention(_) => Need::Control,
        Msg::StartAudio(_) | Msg::AudioConfig(_) => Need::Audio,
        // Arquivos passam sempre: `taildesk_files::service::Files` confere `Permissions.files`
        // e as raízes e responde `DENIED`, então o controlador nunca fica esperando resposta.
        Msg::FsRootsRequest(_)
        | Msg::FsList(_)
        | Msg::FileOffer(_)
        | Msg::FileRequest(_)
        | Msg::FileAccept(_)
        | Msg::FileDone(_)
        | Msg::FileCancel(_)
        | Msg::FileError(_) => Need::Nothing,
        // Idem a área de transferência: `clipboard::AreaTransferencia` responde `DENIED` sem
        // `Permissions.clipboard`.
        Msg::ClipboardOffer(_)
        | Msg::ClipboardRequest(_)
        | Msg::ClipboardData(_)
        | Msg::ClipboardError(_) => Need::Nothing,
        // Soltar teclas ou parar áudio nunca é bloqueado: só encerra algo, e bloquear deixaria
        // tecla presa.
        Msg::ReleaseAll(_)
        | Msg::StopAudio(_)
        | Msg::Hello(_)
        | Msg::HelloAck(_)
        | Msg::ConsentResult(_)
        | Msg::Close(_)
        | Msg::Ping(_)
        | Msg::Pong(_)
        | Msg::Error(_)
        | Msg::CodecSelected(_)
        | Msg::VideoStats(_)
        | Msg::VideoStopped(_)
        | Msg::DisplaysChanged(_)
        | Msg::CursorPos(_)
        | Msg::CursorShapeRef(_)
        | Msg::FsRoots(_)
        | Msg::FsListing(_)
        | Msg::AudioSelected(_)
        | Msg::AudioStopped(_)
        | Msg::ChatMessage(_) => Need::Nothing,
    }
}

/// Estado da sessão no host para um peer.
#[derive(Debug)]
pub struct HostSession {
    peer: PeerIdentity,
    state: HostState,
    /// Como entrou (para o registro de acessos).
    via: Option<crate::acessos::Via>,
}

impl HostSession {
    /// `peer` vem de `Overlay::identify()` da conexão.
    pub fn new(peer: PeerIdentity) -> Self {
        HostSession {
            peer,
            state: HostState::AwaitHello,
            via: None,
        }
    }

    /// Como o acesso foi liberado; `None` antes de entrar.
    pub fn via(&self) -> Option<crate::acessos::Via> {
        self.via
    }

    pub fn state(&self) -> &HostState {
        &self.state
    }

    pub fn peer(&self) -> &PeerIdentity {
        &self.peer
    }

    /// O diálogo a mostrar, enquanto espera a resposta.
    pub fn prompt(&self) -> Option<&ConsentPrompt> {
        match &self.state {
            HostState::AwaitConsent(p) => Some(p),
            _ => None,
        }
    }

    fn close(&mut self, reason: CloseReason) -> Close {
        self.state = HostState::Closed(reason);
        Close {
            reason: reason.into(),
            message: String::new(),
        }
    }

    pub fn on_hello(&mut self, hello: &Hello, settings: &HostSettings, now: Instant) -> HelloReply {
        if self.state != HostState::AwaitHello {
            return HelloReply::Close(self.close(CloseReason::ProtocolError));
        }
        if hello.proto_version != PROTO_VERSION {
            return HelloReply::Close(self.close(CloseReason::VersionMismatch));
        }
        let decisao = decide(settings, &self.peer, &hello.features);
        if !hello.password.is_empty() && decisao != Decision::Refuse {
            return self.com_senha(hello, settings, now);
        }
        match decisao {
            Decision::Refuse => HelloReply::Close(self.close(CloseReason::NotAllowed)),
            Decision::Allow(permissions) => {
                self.via = Some(crate::acessos::Via::Confiavel);
                self.state = HostState::Live {
                    permissions,
                    control_paused: false,
                };
                HelloReply::Ack(AckFields {
                    consent: Consent::NotRequired,
                    permissions,
                })
            }
            Decision::Ask(offer) => {
                self.state = HostState::AwaitConsent(ConsentPrompt {
                    peer: self.peer.clone(),
                    offer,
                    deadline: now + CONSENT_TIMEOUT,
                });
                HelloReply::Ack(AckFields {
                    consent: Consent::Required,
                    permissions: offer,
                })
            }
        }
    }

    /// `Hello` com senha: certa entra sem o diálogo, errada fecha (e conta para o bloqueio).
    fn com_senha(&mut self, hello: &Hello, settings: &HostSettings, now: Instant) -> HelloReply {
        let node = self.peer.node_stable_id.clone();
        if crate::senha::tentativas(|t| t.bloqueado(&node, now)) {
            return HelloReply::Close(self.close(CloseReason::TooManyAttempts));
        }
        let certa = settings
            .password_hash
            .as_deref()
            .is_some_and(|h| crate::senha::confere(h, &hello.password));
        if !certa {
            crate::senha::tentativas(|t| t.errou(&node, now));
            return HelloReply::Close(self.close(CloseReason::BadPassword));
        }
        crate::senha::tentativas(|t| t.acertou(&node));
        let permissions = offer(settings, &hello.features);
        self.via = Some(crate::acessos::Via::Senha);
        self.state = HostState::Live {
            permissions,
            control_paused: false,
        };
        HelloReply::Ack(AckFields {
            consent: Consent::NotRequired,
            permissions,
        })
    }

    /// Resposta do diálogo. Devolve o que mandar ao controlador: `ConsentResult` e, se
    /// recusado, também o `Close`. "Lembrar" grava o dispositivo em `settings`.
    pub fn answer(&mut self, answer: ConsentAnswer, settings: &mut HostSettings) -> Vec<Msg> {
        let HostState::AwaitConsent(prompt) = &self.state else {
            return Vec::new();
        };
        let offer = prompt.offer;
        let granted = match answer {
            ConsentAnswer::Allow {
                permissions,
                remember,
            } => {
                let p = clamp(permissions, offer);
                if p.view && remember {
                    settings.remember(&self.peer);
                }
                p
            }
            ConsentAnswer::Refuse => Permissions::default(),
        };
        if !granted.view {
            return vec![
                Msg::ConsentResult(ConsentResult {
                    granted: false,
                    permissions: None,
                }),
                Msg::Close(self.close(CloseReason::ConsentDenied)),
            ];
        }
        self.via = Some(crate::acessos::Via::Dialogo);
        self.state = HostState::Live {
            permissions: granted,
            control_paused: false,
        };
        vec![Msg::ConsentResult(ConsentResult {
            granted: true,
            permissions: Some(granted),
        })]
    }

    /// Chame periodicamente: fecha o pedido sem resposta depois de [`CONSENT_TIMEOUT`].
    pub fn tick(&mut self, now: Instant) -> Option<Close> {
        match &self.state {
            HostState::AwaitConsent(p) if now >= p.deadline => {
                Some(self.close(CloseReason::ConsentTimeout))
            }
            _ => None,
        }
    }

    /// "Pausar controle" / "Retomar controle" na faixa do host.
    pub fn set_control_paused(&mut self, paused: bool) {
        if let HostState::Live { control_paused, .. } = &mut self.state {
            *control_paused = paused;
        }
    }

    /// Permissões em vigor (nenhuma antes do consentimento).
    pub fn permissions(&self) -> Permissions {
        match self.state {
            HostState::Live { permissions, .. } => permissions,
            _ => Permissions::default(),
        }
    }

    /// Se a mensagem do controlador pode ser atendida agora. Vídeo, mouse e teclado recusados
    /// são descartados (e logados); áudio recusado responde `AudioStopped{NO_PERMISSION}`.
    /// Arquivos sempre passam: o serviço de arquivos é montado com [`Self::permissions`] e
    /// responde `DENIED` (recrie-o quando as permissões mudarem).
    pub fn admits(&self, msg: &Msg) -> bool {
        let HostState::Live {
            permissions: p,
            control_paused,
        } = self.state
        else {
            return need(msg) == Need::Nothing;
        };
        match need(msg) {
            Need::Nothing => true,
            Need::View => p.view,
            Need::Control => p.control && !control_paused,
            Need::Audio => p.audio,
        }
    }

    /// Encerrar pela faixa do host.
    pub fn end(&mut self) -> Close {
        self.close(CloseReason::ByHost)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use taildesk_proto::control::{
        Feature, Key, KeyChar, MouseButton, ReleaseAll, StartAudio, StartVideo,
    };

    fn peer() -> PeerIdentity {
        PeerIdentity {
            node_stable_id: "nXYZ".into(),
            hostname: "notebook-yuri".into(),
            user_login: Some("yuri@".into()),
            tags: Vec::new(),
            app_caps: Vec::new(),
        }
    }

    fn hello(features: &[Feature]) -> Hello {
        Hello {
            proto_version: PROTO_VERSION,
            features: features.iter().map(|&f| f as i32).collect(),
            ..Hello::default()
        }
    }

    fn tudo() -> Permissions {
        Permissions {
            view: true,
            control: true,
            clipboard: false,
            files: true,
            audio: true,
        }
    }

    fn key() -> Msg {
        Msg::Key(Key {
            hid: 4,
            down: true,
            locks: None,
        })
    }

    #[test]
    fn pedido_aceito_libera_so_o_marcado() {
        let mut settings = HostSettings::default();
        let t0 = Instant::now();
        let mut s = HostSession::new(peer());
        let r = s.on_hello(&hello(&[Feature::Files, Feature::Audio]), &settings, t0);
        assert_eq!(
            r,
            HelloReply::Ack(AckFields {
                consent: Consent::Required,
                permissions: tudo(),
            })
        );
        assert_eq!(s.prompt().unwrap().peer.hostname, "notebook-yuri");
        // Nada passa antes do consentimento.
        assert!(!s.admits(&key()));
        assert!(!s.admits(&Msg::StartVideo(StartVideo::default())));
        assert!(s.admits(&Msg::ReleaseAll(ReleaseAll {})));

        // O host desmarca "arquivos" e tenta marcar clipboard, que não foi oferecido.
        let marcado = Permissions {
            files: false,
            clipboard: true,
            ..tudo()
        };
        let out = s.answer(
            ConsentAnswer::Allow {
                permissions: marcado,
                remember: true,
            },
            &mut settings,
        );
        let esperado = Permissions {
            files: false,
            ..tudo()
        };
        assert_eq!(
            out,
            vec![Msg::ConsentResult(ConsentResult {
                granted: true,
                permissions: Some(esperado),
            })]
        );
        assert!(settings.is_trusted(&peer()));
        assert!(s.admits(&key()));
        assert!(s.admits(&Msg::StartAudio(StartAudio::default())));
        // Arquivos passam para o serviço de arquivos, que responde DENIED.
        assert!(s.admits(&Msg::FsList(Default::default())));
        assert!(!s.permissions().files);

        // Pausar controle bloqueia só mouse e teclado.
        s.set_control_paused(true);
        assert!(!s.admits(&key()));
        assert!(!s.admits(&Msg::MouseButton(MouseButton::default())));
        assert!(!s.admits(&Msg::SecureAttention(Default::default())));
        assert!(s.admits(&Msg::StartVideo(StartVideo::default())));
        assert!(s.admits(&Msg::ReleaseAll(ReleaseAll {})));
        s.set_control_paused(false);
        assert!(s.admits(&Msg::KeyChar(KeyChar::default())));
    }

    #[test]
    fn recusa_e_tempo_esgotado() {
        let mut settings = HostSettings::default();
        let t0 = Instant::now();
        let mut s = HostSession::new(peer());
        s.on_hello(&hello(&[]), &settings, t0);
        let out = s.answer(ConsentAnswer::Refuse, &mut settings);
        assert!(matches!(
            out.as_slice(),
            [Msg::ConsentResult(ConsentResult { granted: false, .. }), Msg::Close(c)]
                if c.reason == CloseReason::ConsentDenied as i32
        ));
        assert!(!settings.is_trusted(&peer()));

        let mut s = HostSession::new(peer());
        s.on_hello(&hello(&[]), &settings, t0);
        assert_eq!(s.tick(t0 + Duration::from_secs(59)), None);
        let c = s.tick(t0 + CONSENT_TIMEOUT).unwrap();
        assert_eq!(c.reason, CloseReason::ConsentTimeout as i32);
        assert_eq!(s.state(), &HostState::Closed(CloseReason::ConsentTimeout));
        // Resposta tardia não reabre.
        let out = s.answer(
            ConsentAnswer::Allow {
                permissions: tudo(),
                remember: false,
            },
            &mut settings,
        );
        assert!(out.is_empty());
    }

    #[test]
    fn desmarcar_ver_a_tela_e_recusar() {
        let mut settings = HostSettings::default();
        let mut s = HostSession::new(peer());
        s.on_hello(&hello(&[]), &settings, Instant::now());
        let out = s.answer(
            ConsentAnswer::Allow {
                permissions: Permissions {
                    view: false,
                    ..tudo()
                },
                remember: true,
            },
            &mut settings,
        );
        assert!(matches!(out.last(), Some(Msg::Close(_))));
        assert!(!settings.is_trusted(&peer()));
    }

    fn com_senha(node: &str, senha: &str) -> (HostSession, Hello) {
        let p = PeerIdentity {
            node_stable_id: node.into(),
            ..peer()
        };
        let h = Hello {
            password: senha.into(),
            ..hello(&[Feature::Files])
        };
        (HostSession::new(p), h)
    }

    fn motivo(r: HelloReply) -> Option<CloseReason> {
        match r {
            HelloReply::Close(c) => Some(c.reason()),
            HelloReply::Ack(_) => None,
        }
    }

    #[test]
    fn senha_certa_entra_sem_dialogo() {
        let mut settings = HostSettings {
            password_hash: Some(crate::senha::gerar_hash("senha do yuri").unwrap()),
            ..HostSettings::default()
        };
        // "Pedir sempre" ligado e dispositivo desconhecido: a senha basta.
        let (mut s, h) = com_senha("senha-ok", "senha do yuri");
        let r = s.on_hello(&h, &settings, Instant::now());
        let HelloReply::Ack(c) = r else { panic!() };
        assert_eq!(c.consent, Consent::NotRequired);
        assert!(c.permissions.files && c.permissions.control);
        assert!(s.admits(&key()));

        let (mut s, h) = com_senha("senha-errada", "outra senha");
        assert_eq!(
            motivo(s.on_hello(&h, &settings, Instant::now())),
            Some(CloseReason::BadPassword)
        );
        // Sem senha no Hello: o diálogo de sempre.
        let mut s = HostSession::new(peer());
        assert!(motivo(s.on_hello(&hello(&[]), &settings, Instant::now())).is_none());
        assert!(s.prompt().is_some());
        // "Aceitar conexões" desligado vale mesmo com a senha certa.
        settings.accept_connections = false;
        let (mut s, h) = com_senha("senha-ok", "senha do yuri");
        assert_eq!(
            motivo(s.on_hello(&h, &settings, Instant::now())),
            Some(CloseReason::NotAllowed)
        );
        // Host sem senha: a senha salva no controlador ficou velha.
        let settings = HostSettings::default();
        let (mut s, h) = com_senha("senha-velha", "senha do yuri");
        assert_eq!(
            motivo(s.on_hello(&h, &settings, Instant::now())),
            Some(CloseReason::BadPassword)
        );
    }

    #[test]
    fn senha_errada_demais_bloqueia_mesmo_a_certa() {
        let settings = HostSettings {
            password_hash: Some(crate::senha::gerar_hash("senha do yuri").unwrap()),
            ..HostSettings::default()
        };
        let agora = Instant::now();
        for _ in 0..crate::senha::ERROS_MAX {
            let (mut s, h) = com_senha("chutador", "chute");
            assert_eq!(
                motivo(s.on_hello(&h, &settings, agora)),
                Some(CloseReason::BadPassword)
            );
        }
        let (mut s, h) = com_senha("chutador", "senha do yuri");
        assert_eq!(
            motivo(s.on_hello(&h, &settings, agora)),
            Some(CloseReason::TooManyAttempts)
        );
        let (mut s, h) = com_senha("chutador", "senha do yuri");
        let depois = agora + crate::senha::BLOQUEIO;
        assert!(motivo(s.on_hello(&h, &settings, depois)).is_none());
    }

    #[test]
    fn confiavel_sem_dialogo_versao_e_hello_repetido() {
        let mut settings = HostSettings {
            ask_always: false,
            ..HostSettings::default()
        };
        settings.remember(&peer());
        let mut s = HostSession::new(peer());
        let r = s.on_hello(&hello(&[]), &settings, Instant::now());
        assert!(matches!(
            r,
            HelloReply::Ack(AckFields {
                consent: Consent::NotRequired,
                ..
            })
        ));
        assert!(s.admits(&key()));
        assert!(matches!(
            s.on_hello(&hello(&[]), &settings, Instant::now()),
            HelloReply::Close(c) if c.reason == CloseReason::ProtocolError as i32
        ));

        let mut s = HostSession::new(peer());
        let velho = Hello {
            proto_version: 0,
            ..hello(&[])
        };
        assert!(matches!(
            s.on_hello(&velho, &settings, Instant::now()),
            HelloReply::Close(c) if c.reason == CloseReason::VersionMismatch as i32
        ));

        let mut s = HostSession::new(peer());
        settings.accept_connections = false;
        assert!(matches!(
            s.on_hello(&hello(&[]), &settings, Instant::now()),
            HelloReply::Close(c) if c.reason == CloseReason::NotAllowed as i32
        ));
    }
}
