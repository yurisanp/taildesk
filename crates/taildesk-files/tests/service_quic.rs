//! Sessão de arquivos completa sobre QUIC real em localhost: controlador e host, cada um com
//! um [`Files`], trocando `Control` em frames no stream bidi e arquivos em streams uni.

use std::fs;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use taildesk_files::audit::{AuditLog, Direction};
use taildesk_files::service::{Config, Event, Files, SharedRoot, Side};
use taildesk_proto::control::{
    Control, FileErrorCode, FsAccess, FsEntryKind, decode_frame, encode_frame,
};
use taildesk_proto::files::INBOX_ROOT;
use taildesk_transport::{Identity, Pin, client_endpoint, connect, server_endpoint};
use tokio::sync::mpsc;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("taildesk-svc-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }

    fn dir(&self, name: &str) -> PathBuf {
        let p = self.0.join(name);
        fs::create_dir_all(&p).unwrap();
        p
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Um lado da sessão: o [`Files`] e os eventos dele.
struct Peer {
    files: Files<quinn::Connection>,
    events: mpsc::Receiver<Event>,
    _conn: quinn::Connection,
}

impl Peer {
    /// Próximo evento que não seja progresso.
    async fn next(&mut self) -> Event {
        loop {
            let e = tokio::time::timeout(Duration::from_secs(20), self.events.recv())
                .await
                .expect("evento demorou demais")
                .expect("canal fechado");
            if !matches!(e, Event::Progress { .. } | Event::Receiving { .. }) {
                return e;
            }
        }
    }
}

/// Liga `Files` a uma conexão: lê frames do controle, escreve o canal `out` e aceita streams uni.
fn wire(
    conn: quinn::Connection,
    mut ctl_tx: quinn::SendStream,
    mut ctl_rx: quinn::RecvStream,
    cfg: Config,
) -> Peer {
    let (out_tx, mut out_rx) = mpsc::channel::<Control>(64);
    let (ev_tx, events) = mpsc::channel(256);
    let files = Files::new(cfg, conn.clone(), out_tx, ev_tx);

    tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            let mut buf = Vec::new();
            encode_frame(&msg, &mut buf);
            if ctl_tx.write_all(&buf).await.is_err() {
                break;
            }
        }
    });
    let reader = files.clone();
    tokio::spawn(async move {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            while let Ok(Some((msg, n))) = decode_frame(&buf) {
                buf.drain(..n);
                if let Some(m) = msg.msg {
                    reader.handle(m).await;
                }
            }
            match ctl_rx.read(&mut chunk).await {
                Ok(Some(n)) => buf.extend_from_slice(&chunk[..n]),
                Ok(None) | Err(_) => break,
            }
        }
    });
    let acceptor = files.clone();
    let c = conn.clone();
    tokio::spawn(async move {
        while let Ok(stream) = c.accept_uni().await {
            acceptor.accept_data(stream);
        }
    });
    Peer {
        files,
        events,
        _conn: conn,
    }
}

async fn session(host_cfg: Config, ctl_cfg: Config) -> (Peer, Peer) {
    let id = Identity::generate().unwrap();
    let server = server_endpoint((Ipv4Addr::LOCALHOST, 0).into(), &id).unwrap();
    let addr = server.local_addr().unwrap();
    let host = tokio::spawn(async move {
        let conn = server.accept().await.unwrap().await.unwrap();
        let (tx, rx) = conn.accept_bi().await.unwrap();
        // Mantém o endpoint vivo enquanto a conexão existir.
        std::mem::forget(server);
        (conn, tx, rx)
    });
    let client = client_endpoint(addr).unwrap();
    let conn = connect(&client, addr, Pin::Sha256(id.fingerprint()))
        .await
        .unwrap();
    std::mem::forget(client);
    let (mut tx, rx) = conn.open_bi().await.unwrap();
    // O stream bidi só aparece do outro lado depois do primeiro byte: um frame vazio.
    tx.write_all(&[0]).await.unwrap();
    let (hconn, htx, mut hrx) = host.await.unwrap();
    let mut first = [0u8; 1];
    hrx.read_exact(&mut first).await.unwrap();
    let ctl = wire(conn, tx, rx, ctl_cfg);
    let host = wire(hconn, htx, hrx, host_cfg);
    (host, ctl)
}

fn data(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(31) ^ seed)
        .collect()
}

fn host_config(tmp: &TempDir) -> Config {
    Config {
        side: Side::Host,
        peer: "yuri@yuri-notebook".into(),
        inbox: Some(tmp.dir("entrada")),
        roots: vec![SharedRoot {
            id: 1,
            name: "Documentos".into(),
            path: tmp.dir("docs"),
            access: FsAccess::Read,
        }],
        audit: Some(AuditLog::new(tmp.0.join("auditoria.jsonl"))),
    }
}

fn finished(e: Event) -> (u32, Direction, PathBuf) {
    match e {
        Event::Finished {
            xfer_id,
            direction,
            path,
        } => (xfer_id, direction, path),
        other => panic!("esperava Finished, veio {other:?}"),
    }
}

fn failed(e: Event) -> (u32, FileErrorCode) {
    match e {
        Event::Failed { xfer_id, code, .. } => (xfer_id, code),
        other => panic!("esperava Failed, veio {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn upload_download_and_browse() {
    let tmp = TempDir::new("ok");
    let local = tmp.dir("local");
    let body = data(3 * 1024 * 1024 + 17, 1);
    fs::write(local.join("relatório.pdf"), &body).unwrap();
    fs::create_dir_all(tmp.dir("docs").join("2026")).unwrap();
    fs::write(tmp.0.join("docs/2026/planilha.ods"), b"planilha").unwrap();

    let (mut host, mut ctl) = session(
        host_config(&tmp),
        Config::closed(Side::Controller, "yurisp"),
    )
    .await;

    // Enviar para a pasta de entrada do host.
    let id = ctl
        .files
        .send_file(&local.join("relatório.pdf"), INBOX_ROOT, "relatório.pdf")
        .await
        .unwrap();
    assert_eq!(id % 2, 1, "controlador usa ids ímpares");
    let (hid, dir, path) = finished(host.next().await);
    assert_eq!((hid, dir), (id, Direction::In));
    assert_eq!(fs::read(&path).unwrap(), body);
    let (cid, dir, _) = finished(ctl.next().await);
    assert_eq!((cid, dir), (id, Direction::Out));

    // Navegar.
    ctl.files.request_roots().await;
    let Event::Roots(roots) = ctl.next().await else {
        panic!("esperava Roots")
    };
    assert_eq!(roots.roots.len(), 1);
    assert_eq!(roots.roots[0].access(), FsAccess::Read);
    let req = ctl.files.list(1, "", 0).await;
    let Event::Listing(l) = ctl.next().await else {
        panic!("esperava Listing")
    };
    assert_eq!(l.req_id, req);
    assert_eq!(l.entries.len(), 1);
    assert_eq!(l.entries[0].name, "2026");
    assert_eq!(l.entries[0].kind(), FsEntryKind::Dir);

    // Baixar.
    let dest = tmp.dir("baixados");
    let id = ctl.files.download(1, "2026/planilha.ods", &dest).await;
    let (cid, dir, path) = finished(ctl.next().await);
    assert_eq!((cid, dir), (id, Direction::In));
    assert_eq!(path.file_name().unwrap(), "planilha.ods");
    assert_eq!(fs::read(&path).unwrap(), b"planilha");
    let (hid, dir, _) = finished(host.next().await);
    assert_eq!((hid, dir), (id, Direction::Out));

    // Auditoria do host: uma linha por arquivo, com o peer e o hash.
    let audit = fs::read_to_string(tmp.0.join("auditoria.jsonl")).unwrap();
    let lines: Vec<_> = audit.lines().collect();
    assert_eq!(lines.len(), 2, "{audit}");
    assert!(lines[0].contains("\"dir\":\"in\"") && lines[0].contains("relat"));
    assert!(lines[0].contains(&blake3::hash(&body).to_hex().to_string()));
    assert!(lines[1].contains("\"dir\":\"out\"") && lines[1].contains("\"result\":\"ok\""));
    assert!(lines.iter().all(|l| l.contains("yuri@yuri-notebook")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn many_files_in_parallel_and_a_folder() {
    let tmp = TempDir::new("pasta");
    let fotos = tmp.dir("Fotos");
    for i in 0..9u8 {
        fs::write(
            fotos.join(format!("{i}.jpg")),
            data(200_000 + i as usize, i),
        )
        .unwrap();
    }
    fs::create_dir(fotos.join("sub")).unwrap();
    fs::write(fotos.join("sub/x.txt"), b"x").unwrap();

    let (mut host, mut ctl) =
        session(host_config(&tmp), Config::closed(Side::Controller, "c")).await;
    let sender = ctl.files.clone();
    let fotos2 = fotos.clone();
    let ids = tokio::spawn(async move { sender.send_folder(&fotos2, INBOX_ROOT).await.unwrap() });
    let mut done = 0;
    while done < 10 {
        let (_, dir, _) = finished(host.next().await);
        assert_eq!(dir, Direction::In);
        done += 1;
    }
    assert_eq!(ids.await.unwrap().len(), 10);
    for _ in 0..10 {
        finished(ctl.next().await);
    }
    let inbox = tmp.0.join("entrada/Fotos");
    for i in 0..9u8 {
        assert_eq!(
            fs::read(inbox.join(format!("{i}.jpg"))).unwrap(),
            data(200_000 + i as usize, i)
        );
    }
    assert_eq!(fs::read(inbox.join("sub/x.txt")).unwrap(), b"x");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn diverging_part_restarts_from_zero() {
    let tmp = TempDir::new("diverge");
    let local = tmp.dir("local");
    let body = data(500_000, 9);
    fs::write(local.join("a.bin"), &body).unwrap();
    fs::write(tmp.dir("entrada").join("a.bin.part"), data(1_000, 3)).unwrap();

    let (mut host, mut ctl) =
        session(host_config(&tmp), Config::closed(Side::Controller, "c")).await;
    ctl.files
        .send_file(&local.join("a.bin"), INBOX_ROOT, "a.bin")
        .await
        .unwrap();
    let (_, _, path) = finished(host.next().await);
    assert_eq!(fs::read(path).unwrap(), body);
    finished(ctl.next().await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn refusals() {
    let tmp = TempDir::new("nao");
    let local = tmp.dir("local");
    fs::write(local.join("f"), b"conteudo").unwrap();

    // Host sem a permissão `files`.
    let (_host, mut ctl) = session(
        Config::closed(Side::Host, "h"),
        Config::closed(Side::Controller, "c"),
    )
    .await;
    let id = ctl
        .files
        .send_file(&local.join("f"), INBOX_ROOT, "f")
        .await
        .unwrap();
    assert_eq!(failed(ctl.next().await), (id, FileErrorCode::Denied));
    let id = ctl.files.download(1, "f", &local).await;
    assert_eq!(failed(ctl.next().await), (id, FileErrorCode::Denied));
    ctl.files.list(1, "", 0).await;
    let Event::Listing(l) = ctl.next().await else {
        panic!("esperava Listing")
    };
    assert_eq!(l.error(), FileErrorCode::Denied);
    assert!(l.entries.is_empty());

    // Host aberto, mas caminhos hostis e raiz só de leitura.
    let (_host, mut ctl) = session(host_config(&tmp), Config::closed(Side::Controller, "c")).await;
    let id = ctl
        .files
        .send_file(&local.join("f"), INBOX_ROOT, "../fora")
        .await
        .unwrap();
    assert_eq!(failed(ctl.next().await), (id, FileErrorCode::BadPath));
    let id = ctl.files.send_file(&local.join("f"), 1, "f").await.unwrap();
    assert_eq!(failed(ctl.next().await), (id, FileErrorCode::Denied));
    let id = ctl.files.download(1, "../../etc/passwd", &local).await;
    assert_eq!(failed(ctl.next().await), (id, FileErrorCode::BadPath));
    let id = ctl.files.download(1, "nao-existe", &local).await;
    assert_eq!(failed(ctl.next().await), (id, FileErrorCode::NotFound));
    assert!(!tmp.0.join("fora").exists());
    assert!(!Path::new(&local).join("passwd").exists());
}

/// Arquivo grande enquanto o host manda datagramas no ritmo de vídeo (60/s) para o
/// controlador: mede a vazão do arquivo e o atraso dos datagramas. Ignorado no CI pelo
/// tamanho; rode com `TAILDESK_BIG_MB=10240 cargo test --release -p taildesk-files
/// --test service_quic -- --ignored --nocapture`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "arquivo grande; rodar à mão"]
async fn big_file_does_not_stall_datagrams() {
    let mb: u64 = std::env::var("TAILDESK_BIG_MB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1024);
    let tmp = TempDir::new("grande");
    let src = tmp.dir("local").join("disco.img");
    // Arquivo esparso: a leitura é rápida e o teste mede o caminho de rede e de gravação.
    fs::File::create(&src)
        .unwrap()
        .set_len(mb * 1024 * 1024)
        .unwrap();

    let (mut host, mut ctl) =
        session(host_config(&tmp), Config::closed(Side::Controller, "c")).await;

    // "Vídeo": datagramas de 1200 bytes com o instante de envio, 60 por segundo.
    let start = std::time::Instant::now();
    let video_conn = host._conn.clone();
    let video = tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(16));
        loop {
            tick.tick().await;
            let mut d = vec![0u8; 1200];
            d[..8].copy_from_slice(&(start.elapsed().as_micros() as u64).to_le_bytes());
            if video_conn.send_datagram(d.into()).is_err() {
                break;
            }
        }
    });
    let rx_conn = ctl._conn.clone();
    let delays = tokio::spawn(async move {
        let mut delays = Vec::new();
        while let Ok(d) = rx_conn.read_datagram().await {
            let sent = u64::from_le_bytes(d[..8].try_into().unwrap());
            delays.push(start.elapsed().as_micros() as u64 - sent);
        }
        delays
    });

    let t0 = std::time::Instant::now();
    ctl.files
        .send_file(&src, INBOX_ROOT, "disco.img")
        .await
        .unwrap();
    let (_, _, path) = loop {
        let e = tokio::time::timeout(Duration::from_secs(3600), host.events.recv())
            .await
            .unwrap()
            .unwrap();
        if let Event::Finished { .. } = e {
            break finished(e);
        }
        if let Event::Failed { .. } = e {
            panic!("{e:?}");
        }
    };
    let secs = t0.elapsed().as_secs_f64();
    finished(ctl.next().await);
    video.abort();
    ctl._conn.close(0u32.into(), b"fim");
    let mut delays = delays.await.unwrap();
    delays.sort_unstable();
    let pct = |p: f64| delays[((delays.len() - 1) as f64 * p) as usize] as f64 / 1000.0;
    println!(
        "{mb} MiB em {secs:.1} s = {:.0} Mbit/s; datagramas: {} recebidos, atraso p50 {:.1} ms, p99 {:.1} ms, máx {:.1} ms",
        mb as f64 * 8.0 * 1.048_576 / secs,
        delays.len(),
        pct(0.5),
        pct(0.99),
        pct(1.0)
    );
    assert_eq!(fs::metadata(&path).unwrap().len(), mb * 1024 * 1024);
    let expected = secs * 60.0;
    assert!(
        delays.len() as f64 > expected * 0.9,
        "datagramas perdidos: {} de ~{expected:.0}",
        delays.len()
    );
    assert!(pct(0.99) < 100.0, "p99 do atraso alto demais");
}

/// Joga fora os eventos de um lado (a colagem espera pelos próprios canais).
fn drain(p: &mut Peer) {
    let (_tx, rx) = mpsc::channel(1);
    let mut events = std::mem::replace(&mut p.events, rx);
    tokio::spawn(async move { while events.recv().await.is_some() {} });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn paste_files_and_folder_from_clipboard() {
    use taildesk_proto::control::ClipFile;

    let tmp = TempDir::new("area");
    let copiados = tmp.dir("copiados");
    let doc = copiados.join("nota.txt");
    fs::write(&doc, data(70_000, 3)).unwrap();
    let pasta = copiados.join("Fotos");
    fs::create_dir_all(pasta.join("sub/fundo")).unwrap();
    fs::write(pasta.join("a.jpg"), data(300_000, 1)).unwrap();
    fs::write(pasta.join("sub/fundo/b.txt"), b"b").unwrap();

    // Sem a permissão `files`: a área de transferência basta.
    let (mut host, mut ctl) = session(
        Config::closed(Side::Host, "h"),
        Config::closed(Side::Controller, "c"),
    )
    .await;
    drain(&mut host);
    drain(&mut ctl);

    let lista = vec![
        ClipFile {
            name: "nota.txt".into(),
            size: 70_000,
            dir: false,
        },
        ClipFile {
            name: "Fotos".into(),
            size: 0,
            dir: true,
        },
    ];
    // Nada copiado ainda: não acha.
    let destino = tmp.0.join("colado");
    assert_eq!(
        ctl.files.paste_clipboard(&lista, &destino).await,
        Err(FileErrorCode::NotFound)
    );

    host.files.serve_clipboard(vec![doc.clone(), pasta.clone()]);
    let colados = ctl.files.paste_clipboard(&lista, &destino).await.unwrap();
    // No Windows o caminho volta com o nome longo da pasta temporária (sem `\\?\`).
    let nomes: Vec<_> = colados.iter().map(|p| p.file_name().unwrap()).collect();
    assert_eq!(nomes, ["nota.txt", "Fotos"]);
    for p in &colados {
        assert_eq!(p.parent().unwrap().file_name().unwrap(), "colado");
        assert!(!p.to_string_lossy().starts_with(r"\\?\"), "{p:?}");
    }
    assert_eq!(fs::read(&colados[0]).unwrap(), data(70_000, 3));
    assert_eq!(
        fs::read(colados[1].join("a.jpg")).unwrap(),
        data(300_000, 1)
    );
    assert_eq!(fs::read(colados[1].join("sub/fundo/b.txt")).unwrap(), b"b");

    // Nome trocado ou caminho para fora do item copiado: recusado.
    let errado = vec![ClipFile {
        name: "copiados".into(),
        size: 0,
        dir: true,
    }];
    assert!(
        ctl.files
            .paste_clipboard(&errado, &tmp.0.join("outro"))
            .await
            .is_err()
    );
    let fora = vec![ClipFile {
        name: "nota.txt/../Fotos".into(),
        size: 0,
        dir: false,
    }];
    assert!(
        ctl.files
            .paste_clipboard(&fora, &tmp.0.join("outro2"))
            .await
            .is_err()
    );
}
