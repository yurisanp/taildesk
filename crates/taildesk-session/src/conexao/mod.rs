//! A sessão sobre QUIC: liga as máquinas de estado puras deste crate ao stream de controle, aos
//! streams unidirecionais (cursor e arquivos) e ao serviço de arquivos (`07-protocolo.md` §1).
//!
//! - [`host`]: aceita conexões, identifica o peer pela tailnet, pede consentimento à interface,
//!   filtra por permissão e entrega mouse e teclado (já traduzido) à [`host::Entrada`].
//! - [`controlador`]: conecta, manda o `Hello`, acompanha "Conectando" -> "Esperando aceitar" ->
//!   ao vivo e entrega à interface o que o host manda.
//!
//! O código de SO (injeção, captura, keyring) fica fora: a interface e `taildesk-platform`
//! entram pelas traits de [`host`].

pub mod controlador;
pub mod host;

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use taildesk_proto::control::{Control, Msg, decode_frame, encode_frame};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Porta UDP do TailDesk no IP da tailnet.
pub const PORTA: u16 = 7420;

/// Prazo para o `Hello` chegar depois que a conexão abre.
const ESPERA_HELLO: Duration = Duration::from_secs(10);

/// Prazo para a outra ponta ler o `Close` antes de derrubar a conexão.
const ESPERA_CLOSE: Duration = Duration::from_secs(3);

/// Mensagens de controle esperando para sair. Mouse e teclado não podem ser descartados (uma
/// tecla solta perdida fica presa), então quem manda usa `send` e a fila é folgada.
const FILA_SAIDA: usize = 1024;

/// Relógio monotônico em ns para `Hello.clock_ns`, `HelloAck.clock_ns` e `Ping`.
pub fn relogio_ns() -> u64 {
    static INICIO: OnceLock<Instant> = OnceLock::new();
    INICIO.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

pub(crate) fn controle(msg: Msg) -> Control {
    Control { msg: Some(msg) }
}

/// Por que a leitura do stream de controle parou.
#[derive(Debug)]
pub(crate) enum FimLeitura {
    /// A outra ponta fechou o stream ou a conexão caiu.
    Rede,
    /// Frame inválido (tamanho, protobuf ou limites).
    Protocolo,
}

/// Lê os frames do stream de controle, um por vez.
#[derive(Debug)]
pub(crate) struct Leitor {
    rx: quinn::RecvStream,
    /// Nunca passa de um frame mais um pedaço: `decode_frame` confere o tamanho antes.
    buf: Vec<u8>,
}

impl Leitor {
    pub(crate) fn new(rx: quinn::RecvStream) -> Self {
        Leitor {
            rx,
            buf: Vec::new(),
        }
    }

    /// Próxima mensagem. Pode ser cancelada num `select!` sem perder bytes.
    pub(crate) async fn proxima(&mut self) -> Result<Msg, FimLeitura> {
        let mut pedaco = [0u8; 8 * 1024];
        loop {
            match decode_frame(&self.buf) {
                Ok(Some((c, n))) => {
                    self.buf.drain(..n);
                    // Mensagem de uma versão mais nova (oneof desconhecido): ignora.
                    if let Some(m) = c.msg {
                        return Ok(m);
                    }
                    continue;
                }
                Ok(None) => {}
                Err(_) => return Err(FimLeitura::Protocolo),
            }
            match self.rx.read(&mut pedaco).await {
                Ok(Some(n)) => self.buf.extend_from_slice(&pedaco[..n]),
                Ok(None) | Err(_) => return Err(FimLeitura::Rede),
            }
        }
    }
}

/// Escreve no stream de controle o que chega pelo canal; fecha o stream quando todos os
/// remetentes somem.
pub(crate) fn escritor(mut tx: quinn::SendStream) -> (mpsc::Sender<Control>, JoinHandle<()>) {
    let (s, mut r) = mpsc::channel::<Control>(FILA_SAIDA);
    let tarefa = tokio::spawn(async move {
        let mut buf = Vec::new();
        while let Some(msg) = r.recv().await {
            buf.clear();
            encode_frame(&msg, &mut buf);
            // Junta o que já estiver na fila numa escrita só.
            while buf.len() < 64 * 1024 {
                match r.try_recv() {
                    Ok(m) => encode_frame(&m, &mut buf),
                    Err(_) => break,
                }
            }
            if tx.write_all(&buf).await.is_err() {
                return;
            }
        }
        let _ = tx.finish();
    });
    (s, tarefa)
}

/// Fecha a sessão educadamente: o `Close` (se houver) sai, o stream fecha e a conexão cai depois
/// que a outra ponta leu, ou no prazo.
pub(crate) async fn encerrar(
    conn: &quinn::Connection,
    saida: mpsc::Sender<Control>,
    tarefa: JoinHandle<()>,
    close: Option<Msg>,
) {
    if let Some(c) = close {
        let _ = saida.send(controle(c)).await;
    }
    drop(saida);
    let _ = tokio::time::timeout(ESPERA_CLOSE, tarefa).await;
    let _ = tokio::time::timeout(ESPERA_CLOSE, conn.closed()).await;
    conn.close(0u32.into(), b"fim");
}

/// Tipo de um stream unidirecional, pelos 4 primeiros bytes.
#[derive(Debug)]
pub(crate) enum StreamUni<R> {
    Cursor(R),
    /// Arquivo: os 4 bytes voltam para a frente, porque `Files::accept_data` lê o cabeçalho
    /// inteiro.
    Arquivo(tokio::io::Chain<std::io::Cursor<[u8; 4]>, R>),
    Desconhecido,
}

pub(crate) async fn classificar<R: AsyncRead + Unpin>(mut rx: R) -> StreamUni<R> {
    let mut magic = [0u8; 4];
    if tokio::time::timeout(ESPERA_HELLO, rx.read_exact(&mut magic))
        .await
        .map_or(true, |r| r.is_err())
    {
        return StreamUni::Desconhecido;
    }
    if magic == taildesk_proto::cursor::STREAM_MAGIC {
        StreamUni::Cursor(rx)
    } else if magic == taildesk_proto::files::DataHeader::MAGIC {
        StreamUni::Arquivo(std::io::Cursor::new(magic).chain(rx))
    } else {
        StreamUni::Desconhecido
    }
}
