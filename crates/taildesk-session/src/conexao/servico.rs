//! Conexões de serviço do celular (`07-protocolo.md` §3.4): uma pergunta no lugar do `Hello`,
//! uma resposta, e o host fecha. Sem diálogo, sem mídia.
//!
//! - [`pedir_lista`]: as páginas de `PeerList` (só para o dono da tailnet ou celular pareado).
//! - [`pedir_acordar`]: pede a um repetidor na LAN do alvo que mande o pacote mágico
//!   (`17-wake-on-lan.md` §4.3).
//! - [`parear`]: manda o código do QR; o certificado do host já é conferido contra o QR na
//!   conexão (`Pin::Sha256`), então um host falso nem chega a ver o código.

use std::io;
use std::net::SocketAddr;

use taildesk_proto::control::{
    CloseReason, Msg, PairRequest, PairResult, PeerEntry, PeerListRequest, WakeRequest, WakeResult,
    limits,
};
use taildesk_proto::pair::PairOffer;
use taildesk_transport::{Pin, connect};

use super::{ESPERA_HELLO, Leitor, controle};

/// Por que um pedido de serviço não deu certo.
#[derive(Debug)]
pub enum ErroServico {
    /// Rede, certificado diferente do fixado ou o host não respondeu a tempo.
    Rede(io::Error),
    /// O host recusou (`Close` com este motivo): `NOT_ALLOWED` = nem dono nem pareado.
    Recusado(CloseReason),
    /// Resposta fora do protocolo.
    Protocolo,
}

impl std::fmt::Display for ErroServico {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ErroServico::Rede(e) => write!(f, "rede: {e}"),
            ErroServico::Recusado(r) => write!(f, "o computador recusou ({r:?})"),
            ErroServico::Protocolo => f.write_str("resposta inválida do computador"),
        }
    }
}

impl std::error::Error for ErroServico {}

impl From<io::Error> for ErroServico {
    fn from(e: io::Error) -> Self {
        ErroServico::Rede(e)
    }
}

/// Abre a conexão, manda `pedido` e devolve a primeira resposta.
async fn perguntar(
    endpoint: &quinn::Endpoint,
    remoto: SocketAddr,
    pin: Pin,
    pedido: Msg,
) -> Result<Msg, ErroServico> {
    let conn = connect(endpoint, remoto, pin).await?;
    let (tx, rx) = conn.open_bi().await.map_err(io::Error::other)?;
    let (saida, escrita) = super::escritor(tx);
    saida
        .send(controle(pedido))
        .await
        .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
    let mut leitor = Leitor::new(rx);
    let resposta = tokio::time::timeout(ESPERA_HELLO, leitor.proxima()).await;
    drop(saida);
    let _ = escrita.await;
    conn.close(0u32.into(), b"fim");
    match resposta {
        Ok(Ok(Msg::Close(c))) => Err(ErroServico::Recusado(c.reason())),
        Ok(Ok(m)) => Ok(m),
        Ok(Err(_)) => Err(ErroServico::Protocolo),
        Err(_) => Err(ErroServico::Rede(io::ErrorKind::TimedOut.into())),
    }
}

/// Todos os dispositivos que o host conhece (até `limits::PEER_TOTAL`), página por página.
pub async fn pedir_lista(
    endpoint: &quinn::Endpoint,
    remoto: SocketAddr,
    pin: Pin,
) -> Result<Vec<PeerEntry>, ErroServico> {
    let mut todos = Vec::new();
    loop {
        let start = u32::try_from(todos.len()).unwrap_or(u32::MAX);
        let resposta = perguntar(
            endpoint,
            remoto,
            pin,
            Msg::PeerListRequest(PeerListRequest { start }),
        )
        .await?;
        let Msg::PeerList(pagina) = resposta else {
            return Err(ErroServico::Protocolo);
        };
        let vazia = pagina.entries.is_empty();
        todos.extend(pagina.entries);
        if !pagina.more || vazia || todos.len() >= limits::PEER_TOTAL {
            todos.truncate(limits::PEER_TOTAL);
            return Ok(todos);
        }
    }
}

/// Pede ao TailDesk de `remoto` que acorde `pedido.mac` na LAN dele. `Ok` traz o
/// `WakeResult`, que pode ser uma recusa (`NotSameLan`: tente outro repetidor).
pub async fn pedir_acordar(
    endpoint: &quinn::Endpoint,
    remoto: SocketAddr,
    pin: Pin,
    pedido: WakeRequest,
) -> Result<WakeResult, ErroServico> {
    match perguntar(endpoint, remoto, pin, Msg::WakeRequest(pedido)).await? {
        Msg::WakeResult(r) => Ok(r),
        _ => Err(ErroServico::Protocolo),
    }
}

/// Pareia com o desktop do QR. `Ok` traz o `PairResult` (que pode ser uma recusa com motivo).
pub async fn parear(
    endpoint: &quinn::Endpoint,
    oferta: &PairOffer,
    nome_do_aparelho: &str,
) -> Result<PairResult, ErroServico> {
    let remoto = SocketAddr::new(oferta.addr, oferta.port);
    let mut nome = nome_do_aparelho.to_owned();
    while nome.len() > limits::SHORT_TEXT_LEN {
        nome.pop();
    }
    let resposta = perguntar(
        endpoint,
        remoto,
        Pin::Sha256(oferta.fingerprint),
        Msg::PairRequest(PairRequest {
            code: oferta.code.clone(),
            device_name: nome,
        }),
    )
    .await?;
    match resposta {
        Msg::PairResult(r) => Ok(r),
        _ => Err(ErroServico::Protocolo),
    }
}

/// Pareia só com o código (sem câmera): a pessoa digitou o nome do computador e o código. O
/// primeiro contato aceita o certificado, e o pareamento só vale se o host devolver o mesmo
/// fingerprint da conexão. Devolve o resultado e o fingerprint para fixar.
pub async fn parear_com_codigo(
    endpoint: &quinn::Endpoint,
    remoto: SocketAddr,
    codigo: &str,
    nome_do_aparelho: &str,
) -> Result<(PairResult, [u8; 32]), ErroServico> {
    let conn = connect(endpoint, remoto, Pin::FirstContact).await?;
    let visto = taildesk_transport::peer_fingerprint(&conn).ok_or(ErroServico::Protocolo)?;
    conn.close(0u32.into(), b"fim");
    let mut nome = nome_do_aparelho.to_owned();
    while nome.len() > limits::SHORT_TEXT_LEN {
        nome.pop();
    }
    // A pergunta vai fixada no certificado que acabamos de ver.
    let resposta = perguntar(
        endpoint,
        remoto,
        Pin::Sha256(visto),
        Msg::PairRequest(PairRequest {
            code: codigo.to_owned(),
            device_name: nome,
        }),
    )
    .await?;
    match resposta {
        Msg::PairResult(r) if !r.ok || r.cert_fingerprint == visto => Ok((r, visto)),
        _ => Err(ErroServico::Protocolo),
    }
}
