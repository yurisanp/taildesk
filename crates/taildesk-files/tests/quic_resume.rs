//! Ponta a ponta sobre QUIC real em localhost: oferta, aceite (offset + prefixo) e hash final
//! num stream bidi, bytes num stream unidirecional, queda da conexão no meio e retomada numa
//! conexão nova. As mensagens de controle aqui são um enquadramento de teste; as de verdade são
//! as do `oneof` de controle.

use std::fs;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

use taildesk_files::{Incoming, Source, XferError};
use taildesk_proto::files::{DataHeader, Hash};
use taildesk_transport::{Identity, Pin, client_endpoint, connect, server_endpoint};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("taildesk-quic-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Mensagens de teste no stream de controle.
async fn write_offer(s: &mut quinn::SendStream, size: u64, mtime_ms: i64) {
    s.write_u64_le(size).await.unwrap();
    s.write_i64_le(mtime_ms).await.unwrap();
}

async fn write_accept(s: &mut quinn::SendStream, offset: u64, prefix: Option<Hash>) {
    s.write_u64_le(offset).await.unwrap();
    s.write_all(&prefix.unwrap_or_default()).await.unwrap();
}

async fn read_accept(s: &mut quinn::RecvStream) -> (u64, Option<Hash>) {
    let offset = s.read_u64_le().await.unwrap();
    let mut prefix = [0; 32];
    s.read_exact(&mut prefix).await.unwrap();
    (offset, (offset > 0).then_some(prefix))
}

async fn read_hash(s: &mut quinn::RecvStream) -> Hash {
    let mut h = [0; 32];
    s.read_exact(&mut h).await.unwrap();
    h
}

const REL_PATH: &str = "backup/disco.img";
const XFER: u32 = 1;

/// Lado que recebe: aceita uma conexão, uma oferta e o stream de dados. Com `cut`, lê só esse
/// tanto de bytes do arquivo e derruba a conexão.
async fn receive_once(
    server: &quinn::Endpoint,
    inbox: &Path,
    cut: Option<u64>,
) -> Result<PathBuf, XferError> {
    let conn = server.accept().await.unwrap().await.unwrap();
    let (mut ctl_tx, mut ctl_rx) = conn.accept_bi().await.unwrap();
    let size = ctl_rx.read_u64_le().await.unwrap();
    let mtime_ms = ctl_rx.read_i64_le().await.unwrap();
    let mut incoming = Incoming::prepare(inbox, REL_PATH, size, mtime_ms).await?;
    write_accept(&mut ctl_tx, incoming.offset(), incoming.prefix()).await;

    let mut data = conn.accept_uni().await.unwrap();
    let received = match cut {
        Some(n) => {
            let mut limited = (&mut data).take(DataHeader::LEN as u64 + n);
            incoming.receive(XFER, &mut limited, &mut |_| {}).await
        }
        None => incoming.receive(XFER, &mut data, &mut |_| {}).await,
    };
    if let Err(e) = received {
        conn.close(1u32.into(), b"queda simulada");
        return Err(e);
    }
    let blake3 = read_hash(&mut ctl_rx).await;
    let out = incoming.finish(blake3).await;
    ctl_tx.finish().unwrap();
    conn.closed().await;
    out
}

/// Lado que envia: oferece `src`, respeita o `Accept` e manda o `Done`.
async fn send_once(addr: std::net::SocketAddr, fp: [u8; 32], src: &Path) -> Result<u64, XferError> {
    let client = client_endpoint(addr).unwrap();
    let conn = connect(&client, addr, Pin::Sha256(fp)).await.unwrap();
    let (mut ctl_tx, mut ctl_rx) = conn.open_bi().await.unwrap();
    let mut source = Source::open(src).await?;
    write_offer(&mut ctl_tx, source.size(), source.mtime_ms()).await;
    let (offset, prefix) = read_accept(&mut ctl_rx).await;
    let mut data = conn.open_uni().await.unwrap();
    let hash = source
        .send(XFER, offset, prefix, &mut data, &mut |_| {})
        .await?;
    data.finish()
        .map_err(|e| XferError::Io(std::io::Error::other(e)))?;
    ctl_tx.write_all(&hash).await.unwrap();
    // Espera quem recebe conferir e fechar.
    let _ = ctl_rx.read_to_end(16).await;
    conn.close(0u32.into(), b"fim");
    Ok(offset)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_mid_transfer_then_resume_on_new_connection() {
    let tmp = TempDir::new("resume");
    let src = tmp.0.join("disco.img");
    let body: Vec<u8> = (0..12 * 1024 * 1024u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect();
    fs::write(&src, &body).unwrap();
    let inbox = tmp.0.join("Downloads TailDesk");
    fs::create_dir(&inbox).unwrap();

    let id = Identity::generate().unwrap();
    let server = server_endpoint((Ipv4Addr::LOCALHOST, 0).into(), &id).unwrap();
    let addr = server.local_addr().unwrap();
    let fp = id.fingerprint();

    // 1ª conexão cai depois de 5 MB.
    let cut = 5 * 1024 * 1024;
    let receiving = {
        let server = server.clone();
        let inbox = inbox.clone();
        tokio::spawn(async move { receive_once(&server, &inbox, Some(cut)).await })
    };
    let sent = send_once(addr, fp, &src).await;
    assert!(sent.is_err(), "o envio tem que falhar com a queda");
    let err = receiving.await.unwrap().unwrap_err();
    assert!(matches!(err, XferError::Incomplete { received } if received == cut));
    let part = inbox.join("backup").join("disco.img.part");
    assert_eq!(fs::metadata(&part).unwrap().len(), cut);

    // 2ª conexão retoma do .part e confere tudo pelo BLAKE3.
    let receiving = {
        let server = server.clone();
        let inbox = inbox.clone();
        tokio::spawn(async move { receive_once(&server, &inbox, None).await })
    };
    let offset = send_once(addr, fp, &src).await.unwrap();
    assert_eq!(offset, cut);
    let out = receiving.await.unwrap().unwrap();
    assert_eq!(
        out,
        inbox
            .canonicalize()
            .unwrap()
            .join("backup")
            .join("disco.img")
    );
    assert!(fs::read(&out).unwrap() == body);
    assert!(!part.exists());
}
