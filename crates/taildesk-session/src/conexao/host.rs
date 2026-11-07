//! Lado do host: aceita conexões na porta [`super::PORTA`] e atende cada controlador.
//!
//! Por conexão: identifica o peer pela tailnet ([`Identificador`], nunca pelo `Hello`), lê o
//! `Hello`, decide pela [`HostSettings`] e, se for preciso, pede o diálogo à interface
//! ([`EventoHost::Pedido`]). Ao vivo, filtra cada mensagem pelas permissões
//! ([`HostSession::admits`]), traduz o teclado para o layout do host e entrega à [`Entrada`],
//! liga o serviço de arquivos e entrega os pedidos de vídeo à [`Midia`]. No fim (qualquer fim)
//! solta teclas e botões presos e larga a mídia (o que para a captura).

use std::future::Future;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use taildesk_files::audit::AuditLog;
use taildesk_files::service::{Config, Event, Files, SharedRoot, Side};
use taildesk_keyboard::{Inject, KeyMsg, Keymap, Locks};
use taildesk_overlay::{Overlay, PeerIdentity, PeerInfo};
use taildesk_proto::control::{
    AudioStopReason, AudioStopped, ChatMessage, ClientKind, Close, CloseReason, Consent,
    DisplayInfo, HelloAck, KeyboardInfo, LanIface, Msg, PROTO_VERSION, PairError, PairRequest,
    PairResult, PathInfo, PeerEntry, PeerList, PeerListRequest, Permissions, Platform, Pong,
    TaildeskPresence, WakeError, WakeRequest, WakeResult, limits,
};
use tokio::sync::{mpsc, oneshot};

use super::clipboard::{ArquivosDaArea, DoSistema, Ponte, Sistema, sem_sistema};
use super::{ESPERA_HELLO, FimLeitura, Leitor, StreamUni, classificar, controle, encerrar};
use crate::acessos::{
    Acesso, Evento, MOTIVO_FORA_DA_TAILNET, MOTIVO_QUEDA, RegistroAcessos, nome_motivo,
};
use crate::acordar::{self, Limite};
use crate::host::{ConsentAnswer, ConsentPrompt, HelloReply, HostSession, HostState};
use crate::pareamento::{Pareamento, pode_listar};
use crate::policy::HostSettings;

/// Quem está do outro lado, segundo a tailnet. Em produção é o `SystemOverlay`.
pub trait Identificador: Send + Sync + 'static {
    fn identificar(
        &self,
        remoto: SocketAddr,
    ) -> impl Future<Output = io::Result<PeerIdentity>> + Send;

    /// Login do dono deste computador na tailnet (quem pode pedir a lista de dispositivos sem
    /// parear). O padrão não sabe.
    fn dono(&self) -> impl Future<Output = Option<String>> + Send {
        std::future::ready(None)
    }

    /// Dispositivos da tailnet, para a `PeerList` do celular. O padrão não tem nenhum.
    fn dispositivos(&self) -> impl Future<Output = io::Result<Vec<PeerInfo>>> + Send {
        std::future::ready(Ok(Vec::new()))
    }

    /// Caminho até o controlador, para o `PathInfo` do celular. `None` = não sabe.
    fn caminho(&self, _remoto: IpAddr) -> impl Future<Output = Option<PathInfo>> + Send {
        std::future::ready(None)
    }
}

impl<O: Overlay> Identificador for O {
    async fn identificar(&self, remoto: SocketAddr) -> io::Result<PeerIdentity> {
        self.identify(remoto).await
    }

    async fn dono(&self) -> Option<String> {
        self.self_user().await
    }

    async fn dispositivos(&self) -> io::Result<Vec<PeerInfo>> {
        self.peers().await
    }

    async fn caminho(&self, remoto: IpAddr) -> Option<PathInfo> {
        let c = self.path(remoto).await;
        Some(PathInfo {
            kind: c.kind as u32,
            relay_region: if c.kind == taildesk_overlay::PathKind::Relayed {
                c.derp_region.unwrap_or_default()
            } else {
                String::new()
            },
            rtt_ms: c
                .rtt
                .map_or(0, |r| u32::try_from(r.as_millis()).unwrap_or(u32::MAX)),
        })
    }
}

/// Mouse e teclado do host (SendInput no Windows, portal no Wayland).
pub trait Entrada: Send + 'static {
    /// Estado atual de Caps/Num/Scroll Lock no host.
    fn travas(&self) -> Locks;
    /// Teclas já traduzidas para o layout do host.
    fn teclas(&mut self, eventos: &[Inject]) -> impl Future<Output = ()> + Send;
    /// `MouseMove`, `MouseButton` ou `Scroll`.
    fn mouse(&mut self, msg: &Msg) -> impl Future<Output = ()> + Send;
    /// Solta os botões que ficaram apertados.
    fn soltar_mouse(&mut self) -> impl Future<Output = ()> + Send;
    /// Ctrl+Alt+Del de verdade ([`taildesk_proto::control::SecureAttention`]). Só é chamado se
    /// [`Descricao::atencao_segura`]; o padrão não faz nada.
    fn atencao_segura(&mut self) -> impl Future<Output = ()> + Send {
        async {}
    }
}

/// Por onde a mídia de uma sessão sai: datagramas e streams na conexão, mensagens
/// (`CodecSelected`, `VideoStopped`, cursor) na fila do stream de controle.
#[derive(Clone, Debug)]
pub struct CanalMidia {
    pub conn: quinn::Connection,
    pub saida: mpsc::Sender<taildesk_proto::control::Control>,
    /// Codecs que o controlador decodifica (`Hello.decode_caps`; vazio = só H.264).
    pub decode_caps: Vec<taildesk_proto::control::CodecCap>,
    /// Tela útil do celular (`Hello.viewport`): o vídeo não sai maior que isso (MF-82).
    pub viewport: Option<taildesk_proto::control::Viewport>,
}

impl CanalMidia {
    /// Põe `msg` na fila do controle sem esperar (da thread da captura ou do encode). Fila cheia
    /// descarta: só serve para o que pode se perder (posição do cursor).
    pub fn tentar_enviar(&self, msg: Msg) -> bool {
        self.saida.try_send(controle(msg)).is_ok()
    }

    pub async fn enviar(&self, msg: Msg) -> bool {
        self.saida.send(controle(msg)).await.is_ok()
    }
}

/// Vídeo (e depois áudio) de uma sessão ao vivo: recebe `StartVideo`, `StopVideo`,
/// `SetPriority`, `RequestKeyframe` e `PictureLoss`. Soltar para tudo.
pub trait Midia: Send + 'static {
    fn mensagem(&mut self, msg: &Msg) -> impl Future<Output = ()> + Send;

    /// Espera algo mudar nesta máquina (monitor ligado ou desligado, stream do portal
    /// fechado). Precisa ser seguro de cancelar: a sessão espera isto junto com a rede e só
    /// depois chama [`Midia::tratar_evento`]. O padrão nunca termina.
    fn evento(&mut self) -> impl Future<Output = ()> + Send {
        std::future::pending()
    }

    /// Trata o que [`Midia::evento`] viu.
    fn tratar_evento(&mut self) -> impl Future<Output = ()> + Send {
        async {}
    }
}

/// Host sem vídeo (testes, CLI de arquivos).
#[derive(Debug, Default)]
pub struct SemMidia;

impl Midia for SemMidia {
    async fn mensagem(&mut self, _: &Msg) {}
}

/// O que o host diz de si no `HelloAck`.
#[derive(Clone, Debug, Default)]
pub struct Descricao {
    pub nome: String,
    pub plataforma: Platform,
    pub monitores: Vec<DisplayInfo>,
    pub teclado: KeyboardInfo,
    /// Codecs que este host codifica (`HelloAck.encode_caps`; o seletor do controlador só
    /// oferece os que os dois lados têm).
    pub encode_caps: Vec<taildesk_proto::control::CodecCap>,
    /// Layout ativo, para traduzir o que o controlador digita.
    pub keymap: Keymap,
    /// Atende Ctrl+Alt+Del ([`Entrada::atencao_segura`]): o agente do serviço do Windows.
    pub atencao_segura: bool,
}

/// Este computador, visto pela sessão.
pub trait Maquina: Send + Sync + 'static {
    type Entrada: Entrada;
    type Midia: Midia;
    type Area: Sistema;
    fn descricao(&self) -> Descricao;
    /// Mouse e teclado para uma sessão com controle. `None` = indisponível (portal recusado,
    /// por exemplo): a sessão segue só vendo.
    fn entrada(&self) -> impl Future<Output = Option<Self::Entrada>> + Send;
    /// Vídeo para uma sessão ao vivo (ver é sempre permitido). Chamado depois de
    /// [`Maquina::entrada`], então pode reaproveitar o que ela abriu (a sessão do portal).
    fn midia(&self, canal: CanalMidia) -> impl Future<Output = Self::Midia> + Send;
    /// Área de transferência para uma sessão com `Permissions.clipboard`, chamada depois de
    /// [`Maquina::midia`] (no Wayland é da mesma sessão do portal). `None` = sem suporte: os
    /// pedidos do controlador voltam `UNAVAILABLE`.
    fn area(&self) -> impl Future<Output = Option<(Self::Area, mpsc::Receiver<DoSistema>)>> + Send;
    /// Placas de rede deste computador nas LANs (sem a da tailnet), para o `PowerInfo` e para
    /// servir de repetidor do Wake on LAN. O padrão não conhece nenhuma.
    fn placas_lan(&self) -> Vec<LanIface> {
        Vec::new()
    }
}

/// Configuração do host para todas as sessões.
#[derive(Debug)]
pub struct ConfigHost {
    /// "Este computador"; "Lembrar deste dispositivo" grava aqui.
    pub settings: Arc<Mutex<HostSettings>>,
    /// Pasta de entrada (`~/Downloads/TailDesk`).
    pub inbox: Option<PathBuf>,
    pub raizes: Vec<SharedRoot>,
    pub auditoria: Option<AuditLog>,
    /// Registro de acessos (quem entrou, como, quanto tempo). `None` = não registra.
    pub acessos: Option<Arc<RegistroAcessos>>,
    /// Host do agente do serviço do Windows na tela de entrada: cada acesso fica marcado.
    pub tela_de_entrada: bool,
    /// Código do QR em vigor (Configurações → Celulares); a interface gera e cancela.
    pub pareamento: Arc<Mutex<Pareamento>>,
    /// SHA-256 do certificado deste host, devolvido no `PairResult`.
    pub fingerprint: [u8; 32],
    /// Pedidos de Wake on LAN por MAC (este host como repetidor).
    pub acordar: Mutex<Limite>,
}

impl ConfigHost {
    /// Acrescenta ao registro de acessos (uma linha curta: escrever direto custa menos que
    /// mandar para outra thread, e mantém a ordem). Erro só vai para o log.
    fn registrar(&self, mut a: Acesso) {
        a.tela_de_entrada |= self.tela_de_entrada;
        if let Some(r) = &self.acessos
            && let Err(e) = r.registrar(&a)
        {
            eprintln!("taildesk: registro de acessos: {e}");
        }
    }
}

/// Identifica uma sessão nos eventos (único enquanto o app roda).
pub type IdSessao = u64;

/// O que a interface do host mostra ou decide.
#[derive(Debug)]
pub enum EventoHost {
    /// Mostre o diálogo de consentimento e responda pelo canal (largar o canal = Recusar).
    Pedido {
        sessao: IdSessao,
        prompt: ConsentPrompt,
        resposta: oneshot::Sender<ConsentAnswer>,
    },
    /// Sessão aceita: mostre a faixa "Fulano está vendo/controlando" com Pausar e Encerrar.
    AoVivo {
        sessao: IdSessao,
        peer: PeerIdentity,
        permissoes: Permissions,
        comandos: mpsc::Sender<ComandoHost>,
    },
    /// Mensagem que esta camada não trata (vídeo, áudio): para quem cuida da mídia.
    Mensagem {
        sessao: IdSessao,
        msg: Msg,
    },
    Arquivos {
        sessao: IdSessao,
        evento: Event,
    },
    /// Um celular acabou de parear: grave as configurações e atualize a lista de celulares.
    Pareado {
        peer: PeerIdentity,
    },
    /// Acabou (e o diálogo, se aberto, deve sumir).
    Fim {
        sessao: IdSessao,
        peer: PeerIdentity,
        motivo: FimHost,
    },
}

/// Pedidos da faixa do host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComandoHost {
    PausarControle(bool),
    Encerrar,
    /// Manda uma linha de chat para o controlador.
    Chat(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FimHost {
    /// Fechada com este motivo (pelo host, pelo controlador ou pela política).
    Fechada(CloseReason),
    /// A conexão caiu.
    Queda,
}

/// Aceita conexões até o endpoint fechar.
pub async fn servir<I: Identificador, M: Maquina>(
    endpoint: quinn::Endpoint,
    identificador: Arc<I>,
    maquina: Arc<M>,
    config: Arc<ConfigHost>,
    eventos: mpsc::Sender<EventoHost>,
) {
    static PROXIMA: AtomicU64 = AtomicU64::new(1);
    while let Some(chegando) = endpoint.accept().await {
        let (identificador, maquina, config, eventos) = (
            identificador.clone(),
            maquina.clone(),
            config.clone(),
            eventos.clone(),
        );
        tokio::spawn(async move {
            let Ok(conn) = chegando.await else { return };
            let sessao = PROXIMA.fetch_add(1, Ordering::Relaxed);
            atender(conn, sessao, &*identificador, &*maquina, &config, &eventos).await;
        });
    }
}

async fn atender<I: Identificador, M: Maquina>(
    conn: quinn::Connection,
    id: IdSessao,
    identificador: &I,
    maquina: &M,
    config: &ConfigHost,
    eventos: &mpsc::Sender<EventoHost>,
) {
    let identidade = identificador.identificar(conn.remote_address()).await;
    let Ok(Ok((tx, rx))) = tokio::time::timeout(ESPERA_HELLO, conn.accept_bi()).await else {
        conn.close(0u32.into(), b"sem controle");
        return;
    };
    let (saida, tarefa) = super::escritor(tx);
    let mut leitor = Leitor::new(rx);
    let fechar = |r: CloseReason| {
        Msg::Close(Close {
            reason: r.into(),
            message: String::new(),
        })
    };

    // Fora da tailnet (ou o `tailscaled` não soube dizer quem é): não atende.
    let remoto = conn.remote_address();
    let Ok(peer) = identidade else {
        config.registrar(
            Acesso::novo(Evento::Recusado, None, remoto).com_motivo(MOTIVO_FORA_DA_TAILNET),
        );
        encerrar(&conn, saida, tarefa, Some(fechar(CloseReason::NotAllowed))).await;
        return;
    };
    let hello = match tokio::time::timeout(ESPERA_HELLO, leitor.proxima()).await {
        Ok(Ok(Msg::Hello(h))) => h,
        // Conexões de serviço do celular: uma pergunta, uma resposta, e fecha.
        Ok(Ok(Msg::PeerListRequest(r))) => {
            let resposta = lista(identificador, config, &peer, r).await;
            encerrar(&conn, saida, tarefa, Some(resposta)).await;
            return;
        }
        Ok(Ok(Msg::WakeRequest(r))) => {
            let resposta = acordar(identificador, maquina, config, &peer, &r).await;
            encerrar(&conn, saida, tarefa, Some(resposta)).await;
            return;
        }
        Ok(Ok(Msg::PairRequest(r))) => {
            let resposta = parear(config, &peer, &r, Instant::now());
            let ok = resposta.ok;
            encerrar(&conn, saida, tarefa, Some(Msg::PairResult(resposta))).await;
            if ok {
                let _ = eventos.send(EventoHost::Pareado { peer }).await;
            }
            return;
        }
        _ => {
            encerrar(
                &conn,
                saida,
                tarefa,
                Some(fechar(CloseReason::ProtocolError)),
            )
            .await;
            return;
        }
    };

    let mut sessao = HostSession::new(peer.clone());
    let fim = |motivo| EventoHost::Fim {
        sessao: id,
        peer: peer.clone(),
        motivo,
    };
    let resposta = {
        let settings = config.settings.lock().unwrap_or_else(|e| e.into_inner());
        sessao.on_hello(&hello, &settings, Instant::now())
    };
    let campos = match resposta {
        HelloReply::Close(c) => {
            config.registrar(
                Acesso::novo(Evento::Recusado, Some(&peer), remoto)
                    .com_motivo(nome_motivo(c.reason())),
            );
            let motivo = FimHost::Fechada(c.reason());
            encerrar(&conn, saida, tarefa, Some(Msg::Close(c))).await;
            let _ = eventos.send(fim(motivo)).await;
            return;
        }
        HelloReply::Ack(c) => c,
    };
    let descricao = maquina.descricao();
    let ack = HelloAck {
        proto_version: PROTO_VERSION,
        host_name: descricao.nome.clone(),
        platform: Some(descricao.plataforma.clone()),
        displays: descricao.monitores.clone(),
        encode_caps: descricao.encode_caps.clone(),
        permissions: Some(campos.permissions),
        consent: campos.consent.into(),
        clock_ns: super::relogio_ns(),
        client_clock_ns: hello.clock_ns,
        keyboard: Some(descricao.teclado.clone()),
        password_enabled: config
            .settings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .password_hash
            .is_some(),
        secure_attention: descricao.atencao_segura,
    };
    if saida.send(controle(Msg::HelloAck(ack))).await.is_err() {
        let _ = eventos.send(fim(FimHost::Queda)).await;
        return;
    }

    if campos.consent == Consent::Required {
        match consentimento(&mut sessao, id, config, eventos, &saida, &mut leitor).await {
            Ok(()) => {}
            Err(motivo) => {
                config.registrar(
                    Acesso::novo(Evento::Recusado, Some(&peer), remoto)
                        .com_motivo(motivo_de(motivo)),
                );
                encerrar(&conn, saida, tarefa, None).await;
                let _ = eventos.send(fim(motivo)).await;
                return;
            }
        }
    }

    let inicio = Instant::now();
    config.registrar(
        Acesso::novo(Evento::Entrou, Some(&peer), remoto)
            .com_via(sessao.via())
            .com_permissoes(sessao.permissions()),
    );
    let celular = matches!(hello.client_kind(), ClientKind::Phone | ClientKind::Tablet);
    let motivo = ao_vivo(
        &conn,
        hello.decode_caps.clone(),
        hello.viewport,
        &mut sessao,
        id,
        maquina,
        descricao.keymap,
        descricao.atencao_segura,
        config,
        eventos,
        &saida,
        &mut leitor,
    );
    // O celular não tem LocalAPI: o host diz o caminho (direto/relay) enquanto a sessão dura.
    let caminho = async {
        if celular {
            informar_caminho(identificador, remoto.ip(), &saida).await;
        }
        std::future::pending::<()>().await
    };
    let motivo = async {
        tokio::select! {
            m = motivo => m,
            () = caminho => unreachable!(),
        }
    }
    .await;
    let close = match motivo {
        FimHost::Fechada(CloseReason::ByHost) => Some(Msg::Close(sessao.end())),
        FimHost::Fechada(CloseReason::ProtocolError) => Some(fechar(CloseReason::ProtocolError)),
        _ => None,
    };
    encerrar(&conn, saida, tarefa, close).await;
    config.registrar(
        Acesso::novo(Evento::Saiu, Some(&peer), remoto)
            .com_via(sessao.via())
            .com_motivo(motivo_de(motivo))
            .com_duracao(inicio.elapsed()),
    );
    let _ = eventos.send(fim(motivo)).await;
}

/// De quanto em quanto tempo o host confere o caminho até um celular.
const INTERVALO_CAMINHO: std::time::Duration = std::time::Duration::from_secs(10);

/// Manda `PathInfo` agora e a cada mudança, até a fila de saída fechar.
async fn informar_caminho<I: Identificador>(
    identificador: &I,
    remoto: IpAddr,
    saida: &mpsc::Sender<taildesk_proto::control::Control>,
) {
    let mut ultimo: Option<PathInfo> = None;
    loop {
        if let Some(c) = identificador.caminho(remoto).await {
            // RTT oscila: só reenvia se o tipo ou a região mudarem, ou o RTT andar 20 ms.
            let mudou = ultimo.as_ref().is_none_or(|u| {
                u.kind != c.kind
                    || u.relay_region != c.relay_region
                    || u.rtt_ms.abs_diff(c.rtt_ms) >= 20
            });
            if mudou {
                if saida
                    .send(controle(Msg::PathInfo(c.clone())))
                    .await
                    .is_err()
                {
                    return;
                }
                ultimo = Some(c);
            }
        }
        tokio::time::sleep(INTERVALO_CAMINHO).await;
    }
}

/// Uma página da lista de dispositivos, só para o dono ou um celular pareado.
async fn lista<I: Identificador>(
    identificador: &I,
    config: &ConfigHost,
    peer: &PeerIdentity,
    pedido: PeerListRequest,
) -> Msg {
    let dono = identificador.dono().await;
    let pode = {
        let settings = config.settings.lock().unwrap_or_else(|e| e.into_inner());
        pode_listar(&settings, peer, dono.as_deref())
    };
    if !pode {
        return Msg::Close(Close {
            reason: CloseReason::NotAllowed.into(),
            message: String::new(),
        });
    }
    let todos = identificador.dispositivos().await.unwrap_or_default();
    Msg::PeerList(pagina_de_dispositivos(&todos, pedido.start as usize))
}

/// Repetidor do Wake on LAN: manda o pacote mágico pela placa na LAN do alvo. Só para o dono da
/// tailnet ou um celular pareado (o mesmo critério da `PeerList` e da PeerAPI do Tailscale).
async fn acordar<I: Identificador, M: Maquina>(
    identificador: &I,
    maquina: &M,
    config: &ConfigHost,
    peer: &PeerIdentity,
    pedido: &WakeRequest,
) -> Msg {
    let dono = identificador.dono().await;
    let pode = {
        let settings = config.settings.lock().unwrap_or_else(|e| e.into_inner());
        pode_listar(&settings, peer, dono.as_deref())
    };
    let Some(mac) = acordar::mac_de(&pedido.mac).filter(|_| pode) else {
        return Msg::Close(Close {
            reason: CloseReason::NotAllowed.into(),
            message: String::new(),
        });
    };
    let resultado = |sent, error: WakeError| {
        Msg::WakeResult(WakeResult {
            ok: error == WakeError::Unspecified,
            sent,
            error: error.into(),
        })
    };
    let locais = maquina.placas_lan();
    let placas: Vec<LanIface> = acordar::placas_para(&locais, pedido.lan.as_ref())
        .into_iter()
        .cloned()
        .collect();
    if placas.is_empty() {
        return resultado(0, WakeError::NotSameLan);
    }
    let liberado = config
        .acordar
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .liberar(mac, Instant::now());
    if !liberado {
        return resultado(0, WakeError::RateLimited);
    }
    let envio = tokio::task::spawn_blocking(move || {
        let refs: Vec<&LanIface> = placas.iter().collect();
        acordar::enviar(mac, &refs)
    })
    .await;
    match envio {
        Ok(Ok(n)) if n > 0 => resultado(n, WakeError::Unspecified),
        Ok(Err(e)) => {
            eprintln!("taildesk: wake on lan: {e}");
            resultado(0, WakeError::SendFailed)
        }
        _ => resultado(0, WakeError::SendFailed),
    }
}

/// Corta o texto em até `max` bytes sem quebrar um caractere.
fn cortar(s: &str, max: usize) -> String {
    let mut fim = s.len().min(max);
    while !s.is_char_boundary(fim) {
        fim -= 1;
    }
    s[..fim].to_owned()
}

/// Monta a página que começa em `inicio` (online primeiro, depois por nome).
pub fn pagina_de_dispositivos(todos: &[PeerInfo], inicio: usize) -> PeerList {
    let mut ordem: Vec<&PeerInfo> = todos.iter().take(limits::PEER_TOTAL).collect();
    ordem.sort_by(|a, b| {
        b.online
            .cmp(&a.online)
            .then_with(|| a.hostname.to_lowercase().cmp(&b.hostname.to_lowercase()))
    });
    let entries: Vec<PeerEntry> = ordem
        .iter()
        .skip(inicio)
        .take(limits::PEER_PAGE)
        .map(|p| PeerEntry {
            name: cortar(&p.hostname, limits::PEER_NAME_LEN),
            dns_name: cortar(&p.dns_name, limits::DNS_NAME_LEN),
            ip: p.addr.to_string(),
            os: cortar(p.os.as_deref().unwrap_or_default(), limits::SHORT_TEXT_LEN),
            online: p.online,
            // O host não sonda os outros nós; o celular descobre ao tentar conectar.
            taildesk: TaildeskPresence::Unknown.into(),
        })
        .collect();
    let more = inicio + entries.len() < ordem.len();
    PeerList { entries, more }
}

/// Confere o código do QR e pareia o celular (identificado pelo `whois`, não pelo pedido).
fn parear(
    config: &ConfigHost,
    peer: &PeerIdentity,
    pedido: &PairRequest,
    agora: Instant,
) -> PairResult {
    let recusa = |e: PairError| PairResult {
        ok: false,
        cert_fingerprint: Vec::new(),
        error: e.into(),
    };
    if !config
        .settings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .allow_phones
    {
        return recusa(PairError::Disabled);
    }
    let conferido = config
        .pareamento
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .conferir(&pedido.code, &peer.node_stable_id, agora);
    if let Err(e) = conferido {
        return recusa(e);
    }
    let agora_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
    config
        .settings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .pair(peer, &pedido.device_name, agora_ms);
    PairResult {
        ok: true,
        cert_fingerprint: config.fingerprint.to_vec(),
        error: PairError::Unspecified.into(),
    }
}

fn motivo_de(m: FimHost) -> &'static str {
    match m {
        FimHost::Fechada(r) => nome_motivo(r),
        FimHost::Queda => MOTIVO_QUEDA,
    }
}

/// Espera o diálogo. `Ok` = liberado (o `ConsentResult` já saiu).
async fn consentimento(
    sessao: &mut HostSession,
    id: IdSessao,
    config: &ConfigHost,
    eventos: &mpsc::Sender<EventoHost>,
    saida: &mpsc::Sender<taildesk_proto::control::Control>,
    leitor: &mut Leitor,
) -> Result<(), FimHost> {
    let Some(prompt) = sessao.prompt().cloned() else {
        return Err(FimHost::Fechada(CloseReason::ProtocolError));
    };
    let prazo = prompt.deadline;
    let (resp_tx, mut resp_rx) = oneshot::channel();
    let _ = eventos
        .send(EventoHost::Pedido {
            sessao: id,
            prompt,
            resposta: resp_tx,
        })
        .await;
    let respostas = loop {
        tokio::select! {
            r = &mut resp_rx => {
                let r = r.unwrap_or(ConsentAnswer::Refuse);
                let mut settings = config.settings.lock().unwrap_or_else(|e| e.into_inner());
                break sessao.answer(r, &mut settings);
            }
            _ = tokio::time::sleep_until(prazo.into()) => {
                match sessao.tick(Instant::now()) {
                    Some(c) => break vec![Msg::Close(c)],
                    None => continue,
                }
            }
            m = leitor.proxima() => match m {
                // Desistiu enquanto o diálogo estava aberto.
                Ok(Msg::Close(_)) => return Err(FimHost::Fechada(CloseReason::ByController)),
                Ok(Msg::Ping(p)) => {
                    let _ = saida.send(controle(pong(p.t_ns))).await;
                }
                // Nada mais vale antes do consentimento (`admits` só deixa passar o que não
                // exige permissão, e sem sessão não há o que fazer com isso).
                Ok(_) => {}
                Err(FimLeitura::Rede) => return Err(FimHost::Queda),
                Err(FimLeitura::Protocolo) => {
                    return Err(FimHost::Fechada(CloseReason::ProtocolError));
                }
            },
        }
    };
    for m in respostas {
        let _ = saida.send(controle(m)).await;
    }
    match sessao.state() {
        HostState::Closed(r) => Err(FimHost::Fechada(*r)),
        _ => Ok(()),
    }
}

fn pong(t_echo_ns: u64) -> Msg {
    Msg::Pong(Pong {
        t_ns: super::relogio_ns(),
        t_echo_ns,
    })
}

/// Teclado do host: tradução + injeção, e o que soltar no fim.
struct Teclado<E> {
    host: taildesk_keyboard::Host,
    entrada: Option<E>,
    buf: Vec<Inject>,
}

impl<E: Entrada> Teclado<E> {
    async fn aplicar(&mut self, msg: &KeyMsg) {
        let Some(e) = &mut self.entrada else { return };
        self.buf.clear();
        self.host.apply(msg, e.travas(), &mut self.buf);
        if !self.buf.is_empty() {
            e.teclas(&self.buf).await;
        }
    }

    async fn soltar_tudo(&mut self) {
        self.aplicar(&KeyMsg::ReleaseAll).await;
        if let Some(e) = &mut self.entrada {
            e.soltar_mouse().await;
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn ao_vivo<M: Maquina>(
    conn: &quinn::Connection,
    decode_caps: Vec<taildesk_proto::control::CodecCap>,
    viewport: Option<taildesk_proto::control::Viewport>,
    sessao: &mut HostSession,
    id: IdSessao,
    maquina: &M,
    keymap: Keymap,
    atencao_segura: bool,
    config: &ConfigHost,
    eventos: &mpsc::Sender<EventoHost>,
    saida: &mpsc::Sender<taildesk_proto::control::Control>,
    leitor: &mut Leitor,
) -> FimHost {
    let permissoes = sessao.permissions();
    let peer = sessao.peer().clone();
    // No Linux entrada, mídia e área esperam o diálogo do portal, que pode ficar na tela sem
    // resposta: se o controlador desistir nesse meio tempo, a sessão acaba (e vai para o
    // histórico) em vez de ficar presa até alguém responder.
    let preparo = async {
        let entrada = if permissoes.control {
            maquina.entrada().await
        } else {
            None
        };
        let midia = maquina
            .midia(CanalMidia {
                conn: conn.clone(),
                saida: saida.clone(),
                decode_caps,
                viewport,
            })
            .await;
        let area = if permissoes.clipboard {
            maquina.area().await
        } else {
            None
        };
        (entrada, midia, area)
    };
    let (entrada, mut midia, area) = tokio::select! {
        p = preparo => p,
        _ = conn.closed() => return FimHost::Queda,
    };
    let mut teclado = Teclado {
        host: taildesk_keyboard::Host::new(keymap),
        entrada,
        buf: Vec::new(),
    };
    // Arquivos: o serviço confere `files` e as raízes sozinho.
    let quem = peer
        .user_login
        .clone()
        .map(|u| format!("{u} ({})", peer.hostname))
        .unwrap_or_else(|| peer.hostname.clone());
    let cfg = if permissoes.files {
        Config {
            side: Side::Host,
            peer: quem,
            inbox: config.inbox.clone(),
            roots: config.raizes.clone(),
            audit: config.auditoria.clone(),
        }
    } else {
        Config::closed(Side::Host, quem)
    };
    let (arq_tx, mut arq_rx) = mpsc::channel(256);
    let arquivos = Files::new(cfg, conn.clone(), saida.clone(), arq_tx);
    let aceitar = {
        let (conn, arquivos) = (conn.clone(), arquivos.clone());
        tokio::spawn(async move {
            while let Ok(rx) = conn.accept_uni().await {
                let arquivos = arquivos.clone();
                tokio::spawn(async move {
                    // O host não recebe cursor; só arquivos.
                    if let StreamUni::Arquivo(r) = classificar(rx).await {
                        arquivos.accept_data(r);
                    }
                });
            }
        })
    };
    let repassar = {
        let eventos = eventos.clone();
        tokio::spawn(async move {
            while let Some(evento) = arq_rx.recv().await {
                if eventos
                    .send(EventoHost::Arquivos { sessao: id, evento })
                    .await
                    .is_err()
                {
                    return;
                }
            }
        })
    };

    let area = area.map(|(s, rx)| {
        let arq: Arc<dyn ArquivosDaArea> = Arc::new(arquivos.clone());
        Ponte::com_arquivos(s, rx, saida.clone(), true, Some(arq))
    });

    let (cmd_tx, mut cmd_rx) = mpsc::channel(8);
    let _ = eventos
        .send(EventoHost::AoVivo {
            sessao: id,
            peer,
            permissoes,
            comandos: cmd_tx,
        })
        .await;

    let motivo = loop {
        tokio::select! {
            () = midia.evento() => {
                midia.tratar_evento().await;
                continue;
            }
            m = leitor.proxima() => {
                let msg = match m {
                    Ok(m) => m,
                    Err(FimLeitura::Rede) => break FimHost::Queda,
                    Err(FimLeitura::Protocolo) => {
                        break FimHost::Fechada(CloseReason::ProtocolError);
                    }
                };
                match msg {
                    Msg::Close(_) => break FimHost::Fechada(CloseReason::ByController),
                    Msg::Ping(p) => {
                        let _ = saida.send(controle(pong(p.t_ns))).await;
                        continue;
                    }
                    _ => {}
                }
                if !sessao.admits(&msg) {
                    // Áudio sem permissão: o controlador fica sabendo, e nada é capturado.
                    if let Msg::StartAudio(a) = &msg {
                        let _ = saida
                            .send(controle(Msg::AudioStopped(AudioStopped {
                                stream_id: a.stream_id,
                                reason: AudioStopReason::NoPermission.into(),
                            })))
                            .await;
                    }
                    continue;
                }
                match &msg {
                    Msg::Key(_) | Msg::KeyShortcut(_) | Msg::KeyChar(_) | Msg::ReleaseAll(_) => {
                        if let Some(k) = KeyMsg::from_proto(&msg) {
                            teclado.aplicar(&k).await;
                        }
                    }
                    Msg::SecureAttention(_) => {
                        if atencao_segura && let Some(e) = &mut teclado.entrada {
                            e.atencao_segura().await;
                        }
                    }
                    Msg::MouseMove(_) | Msg::MouseButton(_) | Msg::Scroll(_) => {
                        if let Some(e) = &mut teclado.entrada {
                            e.mouse(&msg).await;
                        }
                    }
                    Msg::FsRootsRequest(_)
                    | Msg::FsList(_)
                    | Msg::FileOffer(_)
                    | Msg::FileRequest(_)
                    | Msg::FileAccept(_)
                    | Msg::FileDone(_)
                    | Msg::FileCancel(_)
                    | Msg::FileError(_) => arquivos.handle(msg).await,
                    Msg::ClipboardOffer(_)
                    | Msg::ClipboardRequest(_)
                    | Msg::ClipboardData(_)
                    | Msg::ClipboardError(_) => match &area {
                        Some(a) => a.mensagem(msg).await,
                        None => {
                            // Sem permissão a resposta é `DENIED`; com ela e sem suporte aqui,
                            // `UNAVAILABLE`.
                            let r = if permissoes.clipboard {
                                sem_sistema(&msg)
                            } else {
                                crate::clipboard::AreaTransferencia::default()
                                    .receber(&msg, false)
                                    .1
                            };
                            if let Some(r) = r {
                                let _ = saida.send(controle(r)).await;
                            }
                        }
                    },
                    Msg::StartVideo(_)
                    | Msg::StopVideo(_)
                    | Msg::SetPriority(_)
                    | Msg::SetVideoQuality(_)
                    | Msg::RequestKeyframe(_)
                    | Msg::PictureLoss(_)
                    | Msg::ReceiverReport(_)
                    | Msg::StartAudio(_)
                    | Msg::StopAudio(_) => midia.mensagem(&msg).await,
                    // O layout e o modo escolhidos valem no controlador, que traduz.
                    Msg::KeyboardLayoutChanged(_) | Msg::KeyboardMode(_) => {}
                    _ => {
                        let _ = eventos.send(EventoHost::Mensagem { sessao: id, msg }).await;
                    }
                }
            }
            // Sem remetente (a interface largou o canal), o ramo fica desligado.
            Some(c) = cmd_rx.recv() => match c {
                ComandoHost::PausarControle(p) => {
                    sessao.set_control_paused(p);
                    if p {
                        teclado.soltar_tudo().await;
                    }
                }
                ComandoHost::Encerrar => break FimHost::Fechada(CloseReason::ByHost),
                ComandoHost::Chat(texto) => {
                    let _ = saida
                        .send(controle(Msg::ChatMessage(ChatMessage { text: texto })))
                        .await;
                }
            },
        }
    };
    teclado.soltar_tudo().await;
    drop(area);
    drop(midia);
    aceitar.abort();
    repassar.abort();
    motivo
}
