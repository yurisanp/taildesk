//! Sessão completa sobre QUIC em localhost: consentimento, teclado traduzido, mouse, arquivos,
//! encerrar pelos dois lados e peer fora da tailnet.

use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use taildesk_files::service::Event;
use taildesk_keyboard::{Inject, Locks, fixtures};
use taildesk_overlay::PeerIdentity;
use taildesk_proto::control::{
    Button, CloseReason, Feature, Hello, Key, MouseButton, Msg, Permissions,
};
use taildesk_proto::files::INBOX_ROOT;
use taildesk_session::conexao::controlador::{EventoControlador, Sessao};
use taildesk_session::conexao::host::{
    ComandoHost, ConfigHost, Descricao, Entrada, EventoHost, FimHost, Identificador, Maquina,
    servir,
};
use taildesk_session::{ConsentAnswer, ControllerState, EndReason, HostSettings};
use taildesk_transport::{Identity, Pin, client_endpoint, server_endpoint};
use tokio::sync::mpsc;

const PRAZO: Duration = Duration::from_secs(20);

/// Diz que todo peer é "notebook", ou que ninguém é da tailnet.
struct Tailnet {
    conhecido: bool,
}

impl Identificador for Tailnet {
    async fn identificar(&self, _remoto: SocketAddr) -> io::Result<PeerIdentity> {
        if !self.conhecido {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "fora"));
        }
        Ok(PeerIdentity {
            node_stable_id: "nNOTE".into(),
            hostname: "notebook".into(),
            user_login: Some("yuri@example.com".into()),
            tags: Vec::new(),
            app_caps: Vec::new(),
        })
    }
}

type Registro = Arc<Mutex<Vec<String>>>;

struct EntradaFalsa(Registro);

impl Entrada for EntradaFalsa {
    fn travas(&self) -> Locks {
        Locks::default()
    }

    async fn teclas(&mut self, eventos: &[Inject]) {
        let mut r = self.0.lock().unwrap();
        for e in eventos {
            r.push(format!("{e:?}"));
        }
    }

    async fn mouse(&mut self, msg: &Msg) {
        if let Msg::MouseButton(b) = msg {
            self.0
                .lock()
                .unwrap()
                .push(format!("botão {} {}", b.button, b.down));
        }
    }

    async fn soltar_mouse(&mut self) {
        self.0.lock().unwrap().push("soltar mouse".into());
    }
}

struct MaquinaFalsa(Registro);

impl Maquina for MaquinaFalsa {
    type Entrada = EntradaFalsa;

    fn descricao(&self) -> Descricao {
        Descricao {
            nome: "desktop".into(),
            keymap: fixtures::abnt2(),
            ..Descricao::default()
        }
    }

    async fn entrada(&self) -> Option<EntradaFalsa> {
        Some(EntradaFalsa(self.0.clone()))
    }
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("taildesk-conexao-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Host {
    addr: SocketAddr,
    fingerprint: [u8; 32],
    eventos: mpsc::Receiver<EventoHost>,
    registro: Registro,
    settings: Arc<Mutex<HostSettings>>,
}

impl Host {
    fn iniciar(conhecido: bool, inbox: Option<PathBuf>) -> Host {
        let id = Identity::generate().unwrap();
        let endpoint = server_endpoint((Ipv4Addr::LOCALHOST, 0).into(), &id).unwrap();
        let addr = endpoint.local_addr().unwrap();
        let registro = Registro::default();
        let settings = Arc::new(Mutex::new(HostSettings::default()));
        let (tx, eventos) = mpsc::channel(64);
        tokio::spawn(servir(
            endpoint,
            Arc::new(Tailnet { conhecido }),
            Arc::new(MaquinaFalsa(registro.clone())),
            Arc::new(ConfigHost {
                settings: settings.clone(),
                inbox,
                raizes: Vec::new(),
                auditoria: None,
            }),
            tx,
        ));
        Host {
            addr,
            fingerprint: id.fingerprint(),
            eventos,
            registro,
            settings,
        }
    }

    async fn proximo(&mut self) -> EventoHost {
        tokio::time::timeout(PRAZO, self.eventos.recv())
            .await
            .expect("evento do host demorou")
            .expect("host parou")
    }

    fn registro(&self) -> Vec<String> {
        self.registro.lock().unwrap().clone()
    }
}

async fn conectar(
    host: &Host,
    pin: Pin,
) -> io::Result<(Sessao, mpsc::Receiver<EventoControlador>)> {
    let endpoint = client_endpoint(host.addr)?;
    let hello = Hello {
        app_version: "teste".into(),
        features: vec![Feature::Files.into()],
        ..Hello::default()
    };
    Sessao::conectar(&endpoint, host.addr, pin, hello, "desktop").await
}

/// Próximo estado, pulando os outros eventos.
async fn estado(ev: &mut mpsc::Receiver<EventoControlador>) -> ControllerState {
    loop {
        match tokio::time::timeout(PRAZO, ev.recv()).await {
            Ok(Some(EventoControlador::Estado(s))) => return s,
            Ok(Some(_)) => {}
            Ok(None) => panic!("sessão parou sem estado final"),
            Err(_) => panic!("estado demorou"),
        }
    }
}

/// Espera o registro da entrada ter `n` linhas.
async fn esperar_registro(host: &Host, n: usize) -> Vec<String> {
    for _ in 0..200 {
        let r = host.registro();
        if r.len() >= n {
            return r;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("registro ficou em {:?}", host.registro());
}

fn tecla(hid: u32, down: bool) -> Msg {
    Msg::Key(Key {
        hid,
        down,
        locks: None,
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn consentimento_teclado_mouse_e_fim_pelo_controlador() {
    let mut host = Host::iniciar(true, None);
    let (sessao, mut ev) = conectar(&host, Pin::FirstContact).await.unwrap();
    assert_eq!(sessao.fingerprint(), Some(host.fingerprint));
    assert_eq!(estado(&mut ev).await, ControllerState::Connecting);

    // Pedir permissão sempre (padrão): o diálogo aparece no host.
    let EventoHost::Pedido {
        prompt, resposta, ..
    } = host.proximo().await
    else {
        panic!("esperava o pedido")
    };
    assert_eq!(prompt.peer.hostname, "notebook");
    assert!(matches!(
        estado(&mut ev).await,
        ControllerState::AwaitingConsent { .. }
    ));
    let liberado = Permissions {
        view: true,
        control: true,
        ..Permissions::default()
    };
    resposta
        .send(ConsentAnswer::Allow {
            permissions: liberado,
            remember: true,
        })
        .unwrap();
    assert_eq!(
        estado(&mut ev).await,
        ControllerState::Live {
            permissions: liberado
        }
    );
    let EventoHost::AoVivo { permissoes, .. } = host.proximo().await else {
        panic!("esperava ao vivo")
    };
    assert_eq!(permissoes, liberado);
    assert!(host.settings.lock().unwrap().trusted.len() == 1);

    // Tecla A apertada e botão esquerdo, e o controlador some sem soltar nada.
    assert!(sessao.enviar(tecla(0x04, true)).await);
    assert!(
        sessao
            .enviar(Msg::MouseButton(MouseButton {
                button: Button::Left.into(),
                down: true,
            }))
            .await
    );
    esperar_registro(&host, 2).await;
    sessao.encerrar().await;
    let EventoHost::Fim { motivo, .. } = host.proximo().await else {
        panic!("esperava o fim")
    };
    assert_eq!(motivo, FimHost::Fechada(CloseReason::ByController));
    // No fim, o host solta o que ficou apertado.
    assert_eq!(
        host.registro(),
        vec![
            "Key { hid: 4, down: true }".to_owned(),
            "botão 1 true".to_owned(),
            "Key { hid: 4, down: false }".to_owned(),
            "soltar mouse".to_owned(),
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recusa_no_host() {
    let mut host = Host::iniciar(true, None);
    let pin = Pin::Sha256(host.fingerprint);
    let (_sessao, mut ev) = conectar(&host, pin).await.unwrap();
    let EventoHost::Pedido { resposta, .. } = host.proximo().await else {
        panic!("esperava o pedido")
    };
    resposta.send(ConsentAnswer::Refuse).unwrap();
    loop {
        if let ControllerState::Ended(r) = estado(&mut ev).await {
            assert_eq!(r, EndReason::Refused);
            break;
        }
    }
    let EventoHost::Fim { motivo, .. } = host.proximo().await else {
        panic!("esperava o fim")
    };
    assert_eq!(motivo, FimHost::Fechada(CloseReason::ConsentDenied));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fora_da_tailnet_nao_e_atendido() {
    let host = Host::iniciar(false, None);
    let (_sessao, mut ev) = conectar(&host, Pin::FirstContact).await.unwrap();
    assert_eq!(estado(&mut ev).await, ControllerState::Connecting);
    assert_eq!(
        estado(&mut ev).await,
        ControllerState::Ended(EndReason::NotAllowed)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn confiavel_entra_direto_e_host_encerra() {
    let dir = TempDir::new("inbox");
    let mut host = Host::iniciar(true, Some(dir.0.clone()));
    {
        let mut s = host.settings.lock().unwrap();
        s.ask_always = false;
        s.trusted.push(taildesk_session::TrustedDevice {
            node_stable_id: "nNOTE".into(),
            name: "notebook".into(),
        });
    }
    let (sessao, mut ev) = conectar(&host, Pin::Sha256(host.fingerprint))
        .await
        .unwrap();
    assert_eq!(estado(&mut ev).await, ControllerState::Connecting);
    let ControllerState::Live { permissions } = estado(&mut ev).await else {
        panic!("confiável entra sem diálogo")
    };
    assert!(permissions.control && permissions.files);
    let EventoHost::AoVivo { comandos, .. } = host.proximo().await else {
        panic!("esperava ao vivo")
    };

    // Arquivo para a pasta de entrada do host, pelo stream uni com o magic de arquivos.
    let origem = dir.0.join("origem.txt");
    std::fs::write(&origem, b"ola do controlador").unwrap();
    sessao
        .arquivos()
        .send_file(&origem, INBOX_ROOT, "chegou.txt")
        .await
        .unwrap();
    // O arquivo vale quando quem recebe confere o hash e renomeia o `.part`.
    loop {
        match host.proximo().await {
            EventoHost::Arquivos {
                evento: Event::Finished { .. },
                ..
            } => break,
            EventoHost::Arquivos {
                evento: Event::Failed { code, .. },
                ..
            } => panic!("envio falhou: {code:?}"),
            _ => {}
        }
    }
    assert_eq!(
        std::fs::read(dir.0.join("chegou.txt")).unwrap(),
        b"ola do controlador"
    );

    // Pausar solta a tecla presa; depois o host encerra.
    assert!(sessao.enviar(tecla(0x04, true)).await);
    esperar_registro(&host, 1).await;
    comandos
        .send(ComandoHost::PausarControle(true))
        .await
        .unwrap();
    esperar_registro(&host, 3).await;
    // Pausado: a tecla não chega.
    assert!(sessao.enviar(tecla(0x05, true)).await);
    comandos.send(ComandoHost::Encerrar).await.unwrap();
    assert_eq!(
        estado(&mut ev).await,
        ControllerState::Ended(EndReason::ByHost)
    );
    let r = host.registro();
    assert!(!r.iter().any(|l| l.contains("hid: 5")), "{r:?}");
    assert_eq!(
        &r[..3],
        [
            "Key { hid: 4, down: true }",
            "Key { hid: 4, down: false }",
            "soltar mouse"
        ]
    );
}
