//! Envio e recepção por um pipe em memória: inteiro, interrompido e retomado, prefixo
//! divergente, conflito de nome, hash errado e caminhos hostis.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use taildesk_files::listing::{Entry, EntryKind, list_dir, page, walk};
use taildesk_files::{Incoming, Source, XferError};
use taildesk_proto::control::{FileErrorCode, FsEntry, FsEntryKind};
use taildesk_proto::files::{DataHeader, limits};
use tokio::io::AsyncWriteExt;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("taildesk-xfer-{tag}-{}", std::process::id()));
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

/// Conteúdo pseudoaleatório, para que cortes em lugares diferentes deem hashes diferentes.
fn data(len: usize, seed: u32) -> Vec<u8> {
    let mut x = seed.wrapping_mul(2_654_435_761).max(1);
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect()
}

/// Envia `src` para `rel` em `root` por um pipe em memória e devolve o caminho final.
async fn transfer(src: &Path, root: &Path, rel: &str) -> Result<PathBuf, XferError> {
    let mut source = Source::open(src).await?;
    let mut incoming = Incoming::prepare(root, rel, source.size(), source.mtime_ms()).await?;
    let (offset, prefix) = (incoming.offset(), incoming.prefix());
    let (mut tx, mut rx) = tokio::io::duplex(64 * 1024);
    let sending = async {
        let h = source.send(7, offset, prefix, &mut tx, &mut |_| {}).await;
        drop(tx);
        h
    };
    let mut noop = |_| {};
    let (sent, received) = tokio::join!(sending, incoming.receive(7, &mut rx, &mut noop));
    let hash = sent?;
    received?;
    incoming.finish(hash).await
}

#[tokio::test]
async fn whole_file_with_mtime() {
    let tmp = TempDir::new("whole");
    let src = tmp.0.join("origem.bin");
    let body = data(3 * limits::CHUNK + 123, 1);
    fs::write(&src, &body).unwrap();
    let mtime = UNIX_EPOCH + Duration::from_millis(1_700_000_000_000);
    fs::File::options()
        .write(true)
        .open(&src)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
    let root = tmp.0.join("entrada");
    fs::create_dir(&root).unwrap();

    let out = transfer(&src, &root, "pasta nova/vídeo.bin").await.unwrap();
    assert!(out.ends_with(Path::new("pasta nova").join("vídeo.bin")));
    assert_eq!(fs::read(&out).unwrap(), body);
    assert_eq!(fs::metadata(&out).unwrap().modified().unwrap(), mtime);
    assert!(!root.join("pasta nova/vídeo.bin.part").exists());
}

#[tokio::test]
async fn empty_file() {
    let tmp = TempDir::new("empty");
    let src = tmp.0.join("vazio");
    fs::write(&src, b"").unwrap();
    let out = transfer(&src, &tmp.0, "copia").await.unwrap();
    assert_eq!(fs::read(out).unwrap(), b"");
}

#[tokio::test]
async fn interrupted_then_resumed() {
    let tmp = TempDir::new("resume");
    let src = tmp.0.join("grande.bin");
    let body = data(2 * limits::CHUNK + 999, 2);
    fs::write(&src, &body).unwrap();
    let root = tmp.0.join("destino");
    fs::create_dir(&root).unwrap();

    // Primeira tentativa: o stream cai depois de 300 000 bytes.
    let cut = 300_000;
    let mut incoming = Incoming::prepare(&root, "grande.bin", body.len() as u64, 0)
        .await
        .unwrap();
    assert_eq!(incoming.offset(), 0);
    let (mut tx, mut rx) = tokio::io::duplex(1 << 20);
    tx.write_all(
        &DataHeader {
            xfer_id: 1,
            offset: 0,
        }
        .encode(),
    )
    .await
    .unwrap();
    tx.write_all(&body[..cut]).await.unwrap();
    drop(tx);
    let err = incoming.receive(1, &mut rx, &mut |_| {}).await.unwrap_err();
    assert!(matches!(err, XferError::Incomplete { received } if received == cut as u64));
    drop(incoming);

    // Segunda: retoma do tamanho do .part, com o prefixo conferido por quem envia.
    let mut incoming = Incoming::prepare(&root, "grande.bin", body.len() as u64, 0)
        .await
        .unwrap();
    assert_eq!(incoming.offset(), cut as u64);
    assert_eq!(
        incoming.prefix(),
        Some(*blake3::hash(&body[..cut]).as_bytes())
    );
    let mut source = Source::open(&src).await.unwrap();
    let (mut tx, mut rx) = tokio::io::duplex(64 * 1024);
    let mut first_progress = None;
    let (offset, prefix) = (incoming.offset(), incoming.prefix());
    let sending = async {
        let h = source
            .send(2, offset, prefix, &mut tx, &mut |n| {
                first_progress.get_or_insert(n);
            })
            .await;
        drop(tx);
        h
    };
    let mut noop = |_| {};
    let (hash, received) = tokio::join!(sending, incoming.receive(2, &mut rx, &mut noop));
    let hash = hash.unwrap();
    received.unwrap();
    assert_eq!(first_progress, Some(cut as u64));
    assert_eq!(hash, *blake3::hash(&body).as_bytes());
    let out = incoming.finish(hash).await.unwrap();
    assert_eq!(fs::read(out).unwrap(), body);
}

#[tokio::test]
async fn diverging_part_is_refused_then_restarted() {
    let tmp = TempDir::new("diverge");
    let src = tmp.0.join("a.bin");
    let body = data(100_000, 3);
    fs::write(&src, &body).unwrap();
    let root = tmp.0.join("r");
    fs::create_dir(&root).unwrap();
    // .part de outro arquivo com o mesmo nome.
    fs::write(root.join("a.bin.part"), data(5_000, 4)).unwrap();

    let mut incoming = Incoming::prepare(&root, "a.bin", body.len() as u64, 0)
        .await
        .unwrap();
    assert_eq!(incoming.offset(), 5_000);
    let mut source = Source::open(&src).await.unwrap();
    let mut sink = Vec::new();
    let err = source
        .send(
            1,
            incoming.offset(),
            incoming.prefix(),
            &mut sink,
            &mut |_| {},
        )
        .await
        .unwrap_err();
    assert!(matches!(err, XferError::ResumeMismatch));
    assert_eq!(err.code(), FileErrorCode::ResumeMismatch);
    assert!(sink.is_empty(), "nada pode ir para o stream");

    incoming.restart().await.unwrap();
    assert_eq!(incoming.offset(), 0);
    drop(incoming);
    let out = transfer(&src, &root, "a.bin").await.unwrap();
    assert_eq!(fs::read(out).unwrap(), body);
}

#[tokio::test]
async fn bigger_part_starts_over() {
    let tmp = TempDir::new("bigger");
    fs::write(tmp.0.join("x.part"), vec![1; 50]).unwrap();
    let incoming = Incoming::prepare(&tmp.0, "x", 10, 0).await.unwrap();
    assert_eq!(incoming.offset(), 0);
}

#[tokio::test]
async fn never_overwrites() {
    let tmp = TempDir::new("conflict");
    let src = tmp.0.join("nota.txt");
    fs::write(&src, b"novo").unwrap();
    let root = tmp.0.join("r");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("nota.txt"), b"antigo").unwrap();

    let out = transfer(&src, &root, "nota.txt").await.unwrap();
    assert_eq!(out.file_name().unwrap(), "nota (1).txt");
    assert_eq!(fs::read(root.join("nota.txt")).unwrap(), b"antigo");
    assert_eq!(fs::read(out).unwrap(), b"novo");
}

#[tokio::test]
async fn wrong_hash_discards_part() {
    let tmp = TempDir::new("hash");
    let mut incoming = Incoming::prepare(&tmp.0, "f", 3, 0).await.unwrap();
    let mut stream = DataHeader {
        xfer_id: 5,
        offset: 0,
    }
    .encode()
    .to_vec();
    stream.extend_from_slice(b"abc");
    incoming
        .receive(5, &mut stream.as_slice(), &mut |_| {})
        .await
        .unwrap();
    let part = incoming.part_path().to_owned();
    let err = incoming.finish([0; 32]).await.unwrap_err();
    assert!(matches!(err, XferError::HashMismatch));
    assert!(!part.exists());
    assert!(!tmp.0.join("f").exists());
}

#[tokio::test]
async fn stream_errors() {
    let tmp = TempDir::new("stream");
    let head = |xfer_id, offset| DataHeader { xfer_id, offset }.encode().to_vec();

    // Outro xfer_id, offset errado ou lixo no lugar do cabeçalho.
    for bad in [head(9, 0), head(1, 4), b"lixo lixo lixo!!".to_vec()] {
        let mut incoming = Incoming::prepare(&tmp.0, "s", 3, 0).await.unwrap();
        let err = incoming
            .receive(1, &mut bad.as_slice(), &mut |_| {})
            .await
            .unwrap_err();
        assert!(matches!(err, XferError::BadHeader), "{err:?}");
    }

    // Mais bytes que o oferecido.
    let mut long = head(1, 0);
    long.extend_from_slice(b"abcd");
    let mut incoming = Incoming::prepare(&tmp.0, "s", 3, 0).await.unwrap();
    let err = incoming
        .receive(1, &mut long.as_slice(), &mut |_| {})
        .await
        .unwrap_err();
    assert!(matches!(err, XferError::TooLong));
}

#[tokio::test]
async fn hostile_paths_are_refused() {
    let tmp = TempDir::new("hostile");
    for rel in [
        "../fora",
        "/etc/passwd",
        "C:/x",
        "a/../../b",
        "NUL",
        "x/CON.txt",
    ] {
        let err = Incoming::prepare(&tmp.0, rel, 1, 0).await.unwrap_err();
        assert!(matches!(err, XferError::Path(_)), "{rel}: {err:?}");
        assert_eq!(err.code(), FileErrorCode::BadPath);
    }
    assert!(fs::read_dir(&tmp.0).unwrap().next().is_none());
}

#[cfg(unix)]
#[tokio::test]
async fn symlink_out_of_root_is_refused() {
    let tmp = TempDir::new("link");
    let root = tmp.0.join("root");
    let outside = tmp.0.join("outside");
    fs::create_dir(&root).unwrap();
    fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("sai")).unwrap();
    let err = Incoming::prepare(&root, "sai/x", 1, 0).await.unwrap_err();
    assert!(matches!(
        err,
        XferError::Path(taildesk_files::PathError::EscapesRoot)
    ));
    // Um .part plantado como link para fora também não pode ser usado.
    std::os::unix::fs::symlink(outside.join("alvo"), root.join("y.part")).unwrap();
    let err = Incoming::prepare(&root, "y", 1, 0).await.unwrap_err();
    assert!(matches!(err, XferError::Path(_)), "{err:?}");
    assert!(!outside.join("alvo").exists());
}

#[test]
fn listing_dirs_first_and_pages() {
    let tmp = TempDir::new("list");
    fs::create_dir(tmp.0.join("zeta")).unwrap();
    fs::create_dir(tmp.0.join("Alfa")).unwrap();
    fs::write(tmp.0.join("b.txt"), b"12345").unwrap();
    fs::write(tmp.0.join("A.txt"), b"").unwrap();

    let all = list_dir(&tmp.0, "").unwrap();
    let names: Vec<_> = all.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["Alfa", "zeta", "A.txt", "b.txt"]);
    assert_eq!(all[0].kind, EntryKind::Dir);
    assert_eq!(all[3].size, 5);
    assert!(all[3].mtime_ms > 0);

    assert!(list_dir(&tmp.0, "zeta").unwrap().is_empty());
    assert!(list_dir(&tmp.0, "../").is_err());
    assert!(list_dir(&tmp.0, "nao-existe").is_err());

    let (first, more) = page(&all, 0);
    assert_eq!((first.len(), more), (4, false));
    let (rest, more) = page(&all, 3);
    assert_eq!((rest.len(), more), (1, false));
    assert!(page(&all, 99).0.is_empty());
}

#[cfg(unix)]
#[test]
fn listing_hides_links_out_of_root() {
    let tmp = TempDir::new("listlink");
    let root = tmp.0.join("root");
    fs::create_dir_all(root.join("dentro")).unwrap();
    std::os::unix::fs::symlink(&tmp.0, root.join("fora")).unwrap();
    std::os::unix::fs::symlink(root.join("dentro"), root.join("atalho")).unwrap();
    let names: Vec<_> = list_dir(&root, "")
        .unwrap()
        .into_iter()
        .map(|e| (e.name, e.kind))
        .collect();
    assert_eq!(
        names,
        [
            ("atalho".to_owned(), EntryKind::Dir),
            ("dentro".to_owned(), EntryKind::Dir)
        ]
    );
}

#[test]
fn walk_folder_for_offers() {
    let tmp = TempDir::new("walk");
    let dir = tmp.0.join("Fotos");
    fs::create_dir_all(dir.join("2026/out")).unwrap();
    fs::write(dir.join("capa.jpg"), b"1").unwrap();
    fs::write(dir.join("2026/out/a.jpg"), b"22").unwrap();
    fs::create_dir(dir.join("vazia")).unwrap();
    let w = walk(&dir).unwrap();
    let rels: Vec<_> = w
        .files
        .iter()
        .map(|f| (f.rel_path.as_str(), f.size))
        .collect();
    assert_eq!(rels, [("Fotos/2026/out/a.jpg", 2), ("Fotos/capa.jpg", 1)]);
    assert!(w.skipped.is_empty());
}

#[test]
fn listing_pages_fit_a_control_frame() {
    let long = |i: usize| Entry {
        name: format!("{i:04}-{}", "x".repeat(995)),
        kind: EntryKind::File,
        size: 1,
        mtime_ms: 1,
    };
    let entries: Vec<Entry> = (0..100).map(long).collect();
    let (first, more) = page(&entries, 0);
    assert!(more);
    assert_eq!(first.len(), limits::LISTING_BYTES / 1000);
    let total: usize = first.iter().map(|e| e.name.len()).sum();
    assert!(total <= limits::LISTING_BYTES);

    let short: Vec<Entry> = (0..1000)
        .map(|i| Entry {
            name: i.to_string(),
            ..long(0)
        })
        .collect();
    let (first, more) = page(&short, 0);
    assert_eq!((first.len(), more), (limits::LISTING_ENTRIES, true));

    let wire = FsEntry::from(&entries[0]);
    assert_eq!(wire.kind(), FsEntryKind::File);
    assert_eq!(wire.name, entries[0].name);
}
