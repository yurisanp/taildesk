//! `cargo run --release -p taildesk-codec --example medir_encode [largura altura]`: mede cada
//! encoder disponível nesta máquina (hardware e openh264) com um quadro de tela sintético, e cada
//! decoder com o que cada encoder gerou.

use std::time::Instant;

use taildesk_codec::{ConfigEncoder, OrdemRgb, QuadroRgb};

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
    let decoders = taildesk_codec::opcoes_decoder();
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
        let mut aus = Vec::new();
        let (mut t_enc, mut pior) = (0.0, 0.0f64);
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
            let mut au = Vec::new();
            let t = Instant::now();
            enc.codificar(&q, i == 0, &mut au).unwrap();
            let dt = t.elapsed().as_secs_f64();
            t_enc += dt;
            pior = pior.max(dt);
            if !au.is_empty() {
                aus.push(au);
            }
        }
        let bytes: usize = aus.iter().map(Vec::len).sum();
        println!(
            "{:<8} {:<36} {l}x{a}: encode {:.1} ms (pior {:.1}), {:.1} Mbit/s a 60 fps",
            enc.nome(),
            enc.gpu().unwrap_or("CPU"),
            t_enc * 1000.0 / f64::from(n),
            pior * 1000.0,
            bytes as f64 * 8.0 / f64::from(n) * 60.0 / 1e6,
        );
        for d in &decoders {
            let Ok(mut dec) = d.abrir_decoder() else {
                continue;
            };
            let (mut t_dec, mut ok) = (0.0, 0);
            for au in &aus {
                let t = Instant::now();
                if dec.decodificar(au, &mut |_| ok += 1).is_err() {
                    println!("  erro no decode");
                }
                t_dec += t.elapsed().as_secs_f64();
            }
            println!(
                "    decode {:<8} {:<36} {:.1} ms, {ok}/{} quadros",
                dec.nome(),
                dec.gpu().unwrap_or("CPU"),
                t_dec * 1000.0 / aus.len().max(1) as f64,
                aus.len()
            );
        }
    }
}
