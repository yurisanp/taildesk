//! `cargo run --release -p taildesk-codec --example medir_encode [largura altura]`: mede cada
//! encoder disponível nesta máquina (hardware e openh264) com um quadro de tela sintético, e
//! decodifica no openh264 para conferir.

use std::time::Instant;

use taildesk_codec::{ConfigEncoder, DecoderOpenH264, OrdemRgb, QuadroRgb, VideoDecoder};

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
    let n = 120u32;
    for opcao in taildesk_codec::opcoes_encoder() {
        let config = ConfigEncoder {
            bitrate_bps: 20_000_000,
            fps_max: 60.0,
        };
        let mut enc = match opcao.abrir_encoder(config) {
            Ok(e) => e,
            Err(e) => {
                println!("{} ({:?}): não abriu: {e}", opcao.backend, opcao.gpu);
                continue;
            }
        };
        let mut dec = DecoderOpenH264::novo().unwrap();
        let mut au = Vec::new();
        let (mut t_enc, mut t_dec, mut bytes, mut decodificados) = (0.0, 0.0, 0usize, 0);
        let mut pior = 0.0f64;
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
            enc.codificar(&q, i == 0, &mut au).unwrap();
            let dt = t.elapsed().as_secs_f64();
            t_enc += dt;
            pior = pior.max(dt);
            bytes += au.len();
            let t = Instant::now();
            if !au.is_empty() && dec.decodificar(&au, &mut |_| decodificados += 1).is_err() {
                println!("  erro no decode do quadro {i}");
            }
            t_dec += t.elapsed().as_secs_f64();
        }
        println!(
            "{:<8} {:<40} {l}x{a}: encode {:.1} ms (pior {:.1}), decode openh264 {:.1} ms, \
             {:.1} Mbit/s a 60 fps, {decodificados}/{n} decodificados",
            enc.nome(),
            enc.gpu().unwrap_or("CPU"),
            t_enc * 1000.0 / f64::from(n),
            pior * 1000.0,
            t_dec * 1000.0 / f64::from(n),
            bytes as f64 * 8.0 / f64::from(n) * 60.0 / 1e6,
        );
    }
}
