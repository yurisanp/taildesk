//! Vídeo de ponta a ponta em localhost: encoder openh264 → datagramas QUIC reais → remontagem →
//! decoder, como entre host e controlador.

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use taildesk_codec::{ConfigEncoder, DecoderOpenH264, EncoderOpenH264, OrdemRgb, QuadroRgb};
use taildesk_media::video::{ConfigEnvio, EnvioVideo, RecepcaoVideo};
use taildesk_transport::{Identity, Pin, SaidaQuic, client_endpoint, connect, server_endpoint};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn video_sobre_quic_chega_decodificado() {
    let id = Identity::generate().unwrap();
    let server = server_endpoint((Ipv4Addr::LOCALHOST, 0).into(), &id).unwrap();
    let addr = server.local_addr().unwrap();
    let aceitar = tokio::spawn(async move { server.accept().await.unwrap().await.unwrap() });
    let client = client_endpoint(addr).unwrap();
    let controlador = connect(&client, addr, Pin::Sha256(id.fingerprint()))
        .await
        .unwrap();
    let host = aceitar.await.unwrap();

    let (l, a) = (640u32, 360u32);
    let decodificados = Arc::new(AtomicU64::new(0));
    // Como o `RequestKeyframe` pelo controle: o laço de envio repassa ao encoder.
    let pedir_chave = Arc::new(AtomicBool::new(false));
    let mut rx = RecepcaoVideo::iniciar(
        1,
        0,
        Box::new(DecoderOpenH264::novo().unwrap()),
        Box::new({
            let d = decodificados.clone();
            move |_, q| {
                assert_eq!((q.largura, q.altura), (l, a));
                d.fetch_add(1, Ordering::Relaxed);
            }
        }),
        Arc::new({
            let p = pedir_chave.clone();
            move || p.store(true, Ordering::Relaxed)
        }),
    )
    .unwrap();
    let recebendo = tokio::spawn({
        let c = controlador.clone();
        async move {
            while let Ok(d) = c.read_datagram().await {
                rx.receber(&d);
            }
        }
    });

    let envio = EnvioVideo::iniciar(
        ConfigEnvio {
            stream_id: 0,
            fps_max: 60.0,
        },
        Box::new(
            EncoderOpenH264::novo(ConfigEncoder {
                bitrate_bps: 8_000_000,
                fps_max: 60.0,
            })
            .unwrap(),
        ),
        Box::new(SaidaQuic(host.clone())),
    )
    .unwrap();
    let mut img = vec![0u8; (l * a * 4) as usize];
    for i in 0..30u32 {
        for (p, px) in img.chunks_exact_mut(4).enumerate() {
            let (x, y) = (p as u32 % l, p as u32 / l);
            px.copy_from_slice(&[(x + i * 3) as u8, y as u8, (x ^ y) as u8, 0]);
        }
        envio.entregar(&QuadroRgb {
            largura: l,
            altura: a,
            stride: l as usize * 4,
            ordem: OrdemRgb::Bgrx,
            dados: &img,
        });
        if pedir_chave.swap(false, Ordering::Relaxed) {
            envio.pedir_chave();
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let enviados = envio.estatisticas().quadros.load(Ordering::Relaxed);
    let datagramas = envio.estatisticas().datagramas.load(Ordering::Relaxed);
    drop(envio);
    controlador.close(0u32.into(), b"fim");
    let _ = recebendo.await;
    let d = decodificados.load(Ordering::Relaxed);
    assert!(enviados >= 20, "enviados {enviados}");
    assert!(
        datagramas > enviados,
        "um quadro 640x360 precisa de vários datagramas"
    );
    // Loopback quase nunca perde; se perder, o quadro-chave pedido recupera o vídeo.
    assert!(d * 4 >= enviados * 3, "decodificados {d} de {enviados}");
}
