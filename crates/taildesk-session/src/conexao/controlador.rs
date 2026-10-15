//! Lado do controlador: conecta ao host, manda o `Hello` e entrega à interface o que chega.
//!
//! O certificado do host é conferido pelo fingerprint fixado (TOFU, `06-arquitetura.md` §6):
//! no primeiro contato use `Pin::FirstContact` e grave [`Sessao::fingerprint`] ligado ao
//! `node_stable_id` do host; das próximas vezes, `Pin::Sha256`.

use std::io;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use taildesk_files::service::{Config, Event, Files, Side};
use taildesk_proto::control::{
    Close, CloseReason, Control, Hello, HelloAck, Msg, PROTO_VERSION, Pong,
};
use taildesk_proto::cursor::ShapeHeader;
use taildesk_transport::tls::Fingerprint;
use taildesk_transport::{Pin, connect, peer_fingerprint};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::mpsc;

use super::{FimLeitura, Leitor, StreamUni, classificar, controle};
use crate::controller::{ControllerSession, ControllerState};
use crate::cursor::Forma;

/// O que a janela da sessão recebe.
#[derive(Debug)]
pub enum EventoControlador {
    /// Mudou a etapa ("Conectando", "Esperando aceitar", ao vivo ou o fim e o motivo).
    Estado(ControllerState),
    /// Monitores, layout e plataforma do host.
    Ack(Box<HelloAck>),
    /// Forma de cursor do stream de cursor (para `CursorControlador::forma_recebida`).
    Forma(Forma),
    Arquivos(Event),
    /// O resto do que o host manda (vídeo, monitores, cursor, áudio).
    Mensagem(Box<Msg>),
}

/// Uma sessão aberta com um host.
#[derive(Debug)]
pub struct Sessao {
    conn: quinn::Connection,
    saida: mpsc::Sender<Control>,
    escrita: tokio::task::JoinHandle<()>,
    arquivos: Files<quinn::Connection>,
    fingerprint: Option<Fingerprint>,
}

/// O `Hello` que este controlador manda; `proto_version` e `clock_ns` são preenchidos aqui.
pub fn hello(mut h: Hello) -> Hello {
    h.proto_version = PROTO_VERSION;
    h.clock_ns = super::relogio_ns();
    h
}

impl Sessao {
    /// Conecta a `remoto` e manda o `Hello`. Os eventos chegam pelo canal devolvido; o primeiro
    /// é `Estado(Connecting)`.
    pub async fn conectar(
        endpoint: &quinn::Endpoint,
        remoto: SocketAddr,
        pin: Pin,
        h: Hello,
        nome_host: &str,
    ) -> io::Result<(Sessao, mpsc::Receiver<EventoControlador>)> {
        let conn = connect(endpoint, remoto, pin).await?;
        let fingerprint = peer_fingerprint(&conn);
        let (tx, rx) = conn.open_bi().await.map_err(io::Error::other)?;
        let (saida, escrita) = super::escritor(tx);
        saida
            .send(controle(Msg::Hello(hello(h))))
            .await
            .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;

        let (ev_tx, ev_rx) = mpsc::channel(256);
        let (arq_tx, mut arq_rx) = mpsc::channel(256);
        // O controlador não recebe nada sem ter pedido: sem pasta de entrada nem raízes.
        let arquivos = Files::new(
            Config::closed(Side::Controller, nome_host),
            conn.clone(),
            saida.clone(),
            arq_tx,
        );
        {
            let ev = ev_tx.clone();
            tokio::spawn(async move {
                while let Some(e) = arq_rx.recv().await {
                    if ev.send(EventoControlador::Arquivos(e)).await.is_err() {
                        return;
                    }
                }
            });
        }
        tokio::spawn(aceitar_uni(conn.clone(), arquivos.clone(), ev_tx.clone()));
        tokio::spawn(ler(Leitor::new(rx), saida.clone(), arquivos.clone(), ev_tx));
        Ok((
            Sessao {
                conn,
                saida,
                escrita,
                arquivos,
                fingerprint,
            },
            ev_rx,
        ))
    }

    /// Fingerprint do certificado do host nesta conexão (para gravar no primeiro contato).
    pub fn fingerprint(&self) -> Option<Fingerprint> {
        self.fingerprint
    }

    /// Fila de saída do stream de controle, para quem manda mouse e teclado de outra thread
    /// (`blocking_send` na thread da interface; a fila é folgada).
    pub fn saida(&self) -> mpsc::Sender<Control> {
        self.saida.clone()
    }

    pub async fn enviar(&self, msg: Msg) -> bool {
        self.saida.send(controle(msg)).await.is_ok()
    }

    /// Envio e download de arquivos.
    pub fn arquivos(&self) -> &Files<quinn::Connection> {
        &self.arquivos
    }

    /// Caminho até o host agora (para a faixa de qualidade).
    pub fn rtt(&self) -> Duration {
        self.conn.rtt()
    }

    /// Encerra pelo controlador (ou cancela a espera do consentimento).
    pub async fn encerrar(self) {
        let close = Msg::Close(Close {
            reason: CloseReason::ByController.into(),
            message: String::new(),
        });
        super::encerrar(&self.conn, self.saida, self.escrita, Some(close)).await;
    }
}

async fn ler(
    mut leitor: Leitor,
    saida: mpsc::Sender<Control>,
    arquivos: Files<quinn::Connection>,
    ev: mpsc::Sender<EventoControlador>,
) {
    let mut sessao = ControllerSession::new();
    let mut estado = sessao.state();
    let _ = ev.send(EventoControlador::Estado(estado)).await;
    let mut relogio = tokio::time::interval(Duration::from_secs(1));
    loop {
        let msg = tokio::select! {
            m = leitor.proxima() => m,
            _ = relogio.tick() => {
                sessao.tick(Instant::now());
                if sessao.state() != estado {
                    estado = sessao.state();
                    let _ = ev.send(EventoControlador::Estado(estado)).await;
                }
                if matches!(estado, ControllerState::Ended(_)) {
                    return;
                }
                continue;
            }
        };
        let msg = match msg {
            Ok(m) => m,
            Err(FimLeitura::Rede) => {
                sessao.lost();
                let _ = ev.send(EventoControlador::Estado(sessao.state())).await;
                return;
            }
            Err(FimLeitura::Protocolo) => {
                let _ = saida
                    .send(controle(Msg::Close(Close {
                        reason: CloseReason::ProtocolError.into(),
                        message: String::new(),
                    })))
                    .await;
                let _ = ev
                    .send(EventoControlador::Estado(ControllerState::Ended(
                        crate::controller::EndReason::ProtocolError,
                    )))
                    .await;
                return;
            }
        };
        sessao.on_message(&msg, Instant::now());
        match msg {
            Msg::HelloAck(a) => {
                let _ = ev.send(EventoControlador::Ack(Box::new(a))).await;
            }
            Msg::Ping(p) => {
                let _ = saida
                    .send(controle(Msg::Pong(Pong {
                        t_ns: super::relogio_ns(),
                        t_echo_ns: p.t_ns,
                    })))
                    .await;
            }
            Msg::FsRoots(_)
            | Msg::FsListing(_)
            | Msg::FileOffer(_)
            | Msg::FileRequest(_)
            | Msg::FileAccept(_)
            | Msg::FileDone(_)
            | Msg::FileCancel(_)
            | Msg::FileError(_) => arquivos.handle(msg).await,
            Msg::ConsentResult(_) | Msg::Close(_) | Msg::Pong(_) => {}
            outra => {
                let _ = ev.send(EventoControlador::Mensagem(Box::new(outra))).await;
            }
        }
        if sessao.state() != estado {
            estado = sessao.state();
            let _ = ev.send(EventoControlador::Estado(estado)).await;
        }
        if matches!(estado, ControllerState::Ended(_)) {
            return;
        }
    }
}

async fn aceitar_uni(
    conn: quinn::Connection,
    arquivos: Files<quinn::Connection>,
    ev: mpsc::Sender<EventoControlador>,
) {
    while let Ok(rx) = conn.accept_uni().await {
        let (arquivos, ev) = (arquivos.clone(), ev.clone());
        tokio::spawn(async move {
            match classificar(rx).await {
                StreamUni::Cursor(r) => ler_formas(r, ev).await,
                StreamUni::Arquivo(r) => arquivos.accept_data(r),
                StreamUni::Desconhecido => {}
            }
        });
    }
}

/// Formas do stream de cursor até ele fechar ou vir algo inválido.
async fn ler_formas<R: AsyncRead + Unpin>(mut r: R, ev: mpsc::Sender<EventoControlador>) {
    loop {
        let mut cab = [0u8; ShapeHeader::LEN];
        if r.read_exact(&mut cab).await.is_err() {
            return;
        }
        let Some(header) = ShapeHeader::decode(&cab) else {
            return;
        };
        // Até 256×256×4 = 256 KiB, conferido pelo `decode`.
        let mut pixels = vec![0u8; header.pixels_len()];
        if r.read_exact(&mut pixels).await.is_err() {
            return;
        }
        if ev
            .send(EventoControlador::Forma(Forma { header, pixels }))
            .await
            .is_err()
        {
            return;
        }
    }
}
