//! Teste de ponta a ponta do `bench-net` em localhost: QUIC real, TLS com fingerprint fixado e
//! sondagem pelos datagramas nos dois sentidos.

use std::net::{Ipv4Addr, SocketAddr};

use taildesk_proto::net::NetClass;
use taildesk_transport::bench::{self, BenchOptions};
use taildesk_transport::{Identity, Pin, client_endpoint, connect, server_endpoint};

async fn host() -> (quinn::Endpoint, SocketAddr, [u8; 32]) {
    let id = Identity::generate().unwrap();
    let server = server_endpoint((Ipv4Addr::LOCALHOST, 0).into(), &id).unwrap();
    let addr = server.local_addr().unwrap();
    (server, addr, id.fingerprint())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bench_over_loopback_measures_both_directions() {
    let (server, addr, fp) = host().await;
    let serving = tokio::spawn(async move {
        let conn = server.accept().await.unwrap().await.unwrap();
        bench::serve(&conn).await.unwrap();
        // Espera o cliente fechar, para não derrubar a conexão antes do fim.
        conn.closed().await;
    });

    let client = client_endpoint(addr).unwrap();
    let conn = connect(&client, addr, Pin::Sha256(fp)).await.unwrap();
    let opts = BenchOptions {
        pings: 5,
        step_ms: 50,
        max_kbps: Some(20_000),
        ..BenchOptions::default()
    };
    let out = bench::run(&conn, opts).await.unwrap();
    conn.close(0u32.into(), b"fim");
    serving.await.unwrap();

    assert_eq!(out.rtt.len(), 5);
    assert_eq!(out.down.steps.len(), 2);
    assert_eq!(out.up.steps.len(), 2);
    let down = out.down_estimate.unwrap();
    let up = out.up_estimate.unwrap();
    // Loopback aguenta 20 Mbit/s com folga; a medida varia com a máquina do CI.
    assert!(down.kbps >= 2_000, "download {down:?}");
    assert!(up.kbps >= 2_000, "upload {up:?}");
    assert!(out.max_datagram >= 1_000);
    assert!(out.class >= NetClass::Slow, "{:?}", out.class);
}

#[tokio::test]
async fn wrong_fingerprint_is_refused() {
    let (server, addr, fp) = host().await;
    tokio::spawn(async move {
        if let Some(incoming) = server.accept().await {
            let _ = incoming.await;
        }
    });
    let mut wrong = fp;
    wrong[0] ^= 0xff;
    let client = client_endpoint(addr).unwrap();
    assert!(connect(&client, addr, Pin::Sha256(wrong)).await.is_err());
}

#[tokio::test]
async fn first_contact_reports_the_fingerprint() {
    let (server, addr, fp) = host().await;
    tokio::spawn(async move {
        if let Some(incoming) = server.accept().await
            && let Ok(conn) = incoming.await
        {
            conn.closed().await;
        }
    });
    let client = client_endpoint(addr).unwrap();
    let conn = connect(&client, addr, Pin::FirstContact).await.unwrap();
    assert_eq!(taildesk_transport::peer_fingerprint(&conn), Some(fp));
    conn.close(0u32.into(), b"fim");
}
