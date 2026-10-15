//! `cargo run --release -p taildesk-codec --example medir_encode [largura altura]`: mede a
//! conversão BGRx → I420 e o encode openh264 de um quadro de tela sintético.

use std::time::Instant;

use taildesk_codec::cor::I420;
use taildesk_codec::{ConfigEncoder, EncoderOpenH264, OrdemRgb, QuadroRgb, VideoEncoder};

fn main() {
    let args: Vec<u32> = std::env::args()
        .skip(1)
        .filter_map(|a| a.parse().ok())
        .collect();
    let (l, a) = (
        *args.first().unwrap_or(&1920),
        *args.get(1).unwrap_or(&1200),
    );
    let mut img = vec![0u8; (l * a * 4) as usize];
    let mut enc = EncoderOpenH264::novo(ConfigEncoder {
        bitrate_bps: 20_000_000,
        fps_max: 60.0,
    })
    .unwrap();
    let mut yuv = I420::default();
    let mut au = Vec::new();
    let (mut t_cor, mut t_enc, n) = (0.0, 0.0, 60);
    for i in 0..n {
        // Tela "de texto": fundo claro, faixas escuras que rolam 4 px por quadro.
        for (p, px) in img.chunks_exact_mut(4).enumerate() {
            let (x, y) = (p as u32 % l, p as u32 / l + i * 4);
            let v = if (y / 3) % 7 == 0 && (x / 2) % 5 != 0 {
                30
            } else {
                240
            };
            px.copy_from_slice(&[v, v, v, 0]);
        }
        let q = QuadroRgb {
            largura: l,
            altura: a,
            stride: l as usize * 4,
            ordem: OrdemRgb::Bgrx,
            dados: &img,
        };
        let t = Instant::now();
        yuv.preparar(l, a);
        yuv.converter(&q);
        t_cor += t.elapsed().as_secs_f64();
        let t = Instant::now();
        enc.codificar(&q, false, &mut au).unwrap();
        t_enc += t.elapsed().as_secs_f64();
    }
    println!(
        "{l}x{a}: conversão {:.1} ms, conversão+encode {:.1} ms por quadro",
        t_cor * 1000.0 / n as f64,
        t_enc * 1000.0 / n as f64
    );
}
