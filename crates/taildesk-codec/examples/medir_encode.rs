//! `cargo run --release -p taildesk-codec --example medir_encode [largura altura]`: mede cada
//! encoder disponível nesta máquina (hardware e openh264) com um quadro de tela sintético, e cada
//! decoder com o que cada encoder gerou, com o PSNR do luma. `BPS=...` muda a taxa (padrão 20
//! Mbit/s); `TAILDESK_ENCODER`/`TAILDESK_DECODER` escolhem os backends.

use std::time::Instant;

use taildesk_codec::cor::I420;
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
    let bps: u32 = std::env::var("BPS")
        .ok()
        .and_then(|b| b.parse().ok())
        .unwrap_or(20_000_000);
    let codecs = taildesk_codec::codecs_para_decodificar();
    for opcao in taildesk_codec::candidatos_encoder(&codecs) {
        let config = ConfigEncoder {
            bitrate_bps: bps,
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
        // Luma de referência de cada quadro, para o PSNR.
        let mut refs = Vec::new();
        let (mut t_enc, mut pior) = (0.0, 0.0f64);
        for i in 0..n {
            // Tela "de texto": fundo claro, "letras" de 1-2 px que rolam 4 px por quadro e uma
            // faixa colorida em gradiente.
            for (p, px) in img.chunks_exact_mut(4).enumerate() {
                let (x, y) = (p as u32 % l, p as u32 / l + i * 4);
                let h = (x.wrapping_mul(2654435761) ^ (y / 2).wrapping_mul(40503)) >> 27;
                let v = if (y / 3) % 7 < 4 && (x / 2) % 6 != 0 && h % 3 != 0 {
                    30
                } else {
                    240
                };
                if (p as u32 / l) < a / 4 {
                    px.copy_from_slice(&[(x * 255 / l) as u8, (y % 256) as u8, 128, 0]);
                } else {
                    px.copy_from_slice(&[v, v, v, 0]);
                }
            }
            let q = QuadroRgb {
                largura: l,
                altura: a,
                stride: l as usize * 4,
                ordem: OrdemRgb::Bgrx,
                dados: &img,
            };
            let mut r = I420::default();
            r.preparar(l, a);
            r.converter(&q);
            let mut au = Vec::new();
            let t = Instant::now();
            enc.codificar(&q, i == 0, &mut au).unwrap();
            let dt = t.elapsed().as_secs_f64();
            t_enc += dt;
            pior = pior.max(dt);
            if !au.is_empty() {
                aus.push(au);
                refs.push(r.y);
            }
        }
        let bytes: usize = aus.iter().map(Vec::len).sum();
        let decoders = taildesk_codec::opcoes_decoder(enc.codec());
        println!(
            "{:<5} {:<8} {:<36} {l}x{a}: encode {:.1} ms (pior {:.1}), {:.1} Mbit/s a 60 fps",
            taildesk_codec::nome_codec(enc.codec()),
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
            let (mut t_dec, mut ok, mut erro2, mut amostras) = (0.0, 0, 0.0f64, 0u64);
            for (au, r) in aus.iter().zip(&refs) {
                let t = Instant::now();
                let mut psnr = |q: &taildesk_codec::QuadroYuv<'_>| {
                    ok += 1;
                    let p = q.planos[0];
                    for y in (0..q.altura as usize).step_by(3) {
                        for x in (0..q.largura as usize).step_by(3) {
                            let d = f64::from(p.dados[y * p.passo as usize + x])
                                - f64::from(r[y * (l as usize & !1) + x]);
                            erro2 += d * d;
                            amostras += 1;
                        }
                    }
                };
                if dec.decodificar(au, &mut psnr).is_err() {
                    println!("  erro no decode");
                }
                t_dec += t.elapsed().as_secs_f64();
            }
            let mse = erro2 / amostras.max(1) as f64;
            println!(
                "    decode {:<8} {:<36} {:.1} ms (com o PSNR), {ok}/{} quadros, PSNR Y {:.1} dB",
                dec.nome(),
                dec.gpu().unwrap_or("CPU"),
                t_dec * 1000.0 / aus.len().max(1) as f64,
                aus.len(),
                10.0 * (255.0f64 * 255.0 / mse.max(1e-9)).log10()
            );
        }
    }
}
