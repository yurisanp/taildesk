//! Pipeline de vídeo por monitor (`13-multimonitor.md` §3): um [`EnvioVideo`] por monitor no host
//! e uma [`RecepcaoVideo`] por monitor no controlador, ligados por datagramas QUIC.
//!
//! Sem pacing próprio por enquanto: o quinn espaça os datagramas pelo controle de congestionamento
//! e a seleção adaptativa ajusta o bitrate. O pacer por prioridade entra na fase 4.

pub mod envio;
pub mod fragmentos;
pub mod recepcao;

pub use envio::{ConfigEnvio, EnvioVideo, EstatisticasEnvio, SaidaDatagramas};
pub use fragmentos::{Fragmentador, Remontador, Remontagem};
pub use recepcao::{AoQuadro, PedirChave, RecepcaoVideo};

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use taildesk_codec::{
        ConfigEncoder, DecoderOpenH264, EncoderOpenH264, FormatoYuv, OrdemRgb, QuadroRgb,
    };

    use super::*;

    /// Entrega cada datagrama direto à recepção, perdendo os de índice em `perder`.
    struct Loopback {
        rx: Arc<Mutex<RecepcaoVideo>>,
        n: u64,
        perder: Vec<u64>,
    }

    impl SaidaDatagramas for Loopback {
        fn max_datagrama(&self) -> Option<usize> {
            Some(1200)
        }
        fn enviar(&mut self, d: &[u8]) {
            self.n += 1;
            if !self.perder.contains(&self.n) {
                self.rx.lock().unwrap().receber(d);
            }
        }
    }

    fn esperar(cond: impl Fn() -> bool) -> bool {
        let fim = Instant::now() + Duration::from_secs(10);
        while Instant::now() < fim {
            if cond() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }

    fn rodar(perder: Vec<u64>) -> (Arc<AtomicU64>, Arc<AtomicU64>, u64) {
        let (l, a) = (320u32, 192u32);
        let decodificados = Arc::new(AtomicU64::new(0));
        let pedidos = Arc::new(AtomicU64::new(0));
        let rx = RecepcaoVideo::iniciar(
            9,
            2,
            Box::new(DecoderOpenH264::novo().unwrap()),
            Box::new({
                let d = decodificados.clone();
                move |display, q| {
                    assert_eq!(display, 9);
                    assert_eq!((q.largura, q.altura, q.formato), (l, a, FormatoYuv::I420));
                    d.fetch_add(1, Ordering::Relaxed);
                }
            }),
            Arc::new({
                let p = pedidos.clone();
                move || {
                    p.fetch_add(1, Ordering::Relaxed);
                }
            }),
        )
        .unwrap();
        let rx = Arc::new(Mutex::new(rx));
        let envio = EnvioVideo::iniciar(
            ConfigEnvio {
                stream_id: 2,
                fps_max: 1000.0,
            },
            Box::new(
                EncoderOpenH264::novo(ConfigEncoder {
                    bitrate_bps: 4_000_000,
                    fps_max: 60.0,
                })
                .unwrap(),
            ),
            Box::new(Loopback {
                rx: rx.clone(),
                n: 0,
                perder,
            }),
        )
        .unwrap();
        let mut img = vec![0u8; (l * a * 4) as usize];
        for i in 0..20u32 {
            for (p, px) in img.chunks_exact_mut(4).enumerate() {
                let v = ((p as u32 + i * 7) % 251) as u8;
                px.copy_from_slice(&[v, v.wrapping_mul(3), v ^ 0x55, 0]);
            }
            envio.entregar(&QuadroRgb {
                largura: l,
                altura: a,
                stride: l as usize * 4,
                ordem: OrdemRgb::Bgrx,
                dados: &img,
            });
            let q = envio.estatisticas().quadros.load(Ordering::Relaxed)
                + envio.estatisticas().pulados.load(Ordering::Relaxed);
            assert!(esperar(|| {
                envio.estatisticas().quadros.load(Ordering::Relaxed)
                    + envio.estatisticas().pulados.load(Ordering::Relaxed)
                    > q
            }));
            if pedidos.load(Ordering::Relaxed) > 0 {
                envio.pedir_chave();
            }
        }
        let enviados = envio.estatisticas().quadros.load(Ordering::Relaxed);
        drop(envio);
        drop(rx);
        (decodificados, pedidos, enviados)
    }

    #[test]
    fn host_e_controlador_de_ponta_a_ponta() {
        let (decodificados, pedidos, enviados) = rodar(Vec::new());
        assert!(enviados >= 15, "enviados {enviados}");
        assert_eq!(decodificados.load(Ordering::Relaxed), enviados);
        assert_eq!(pedidos.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn perda_pede_chave_e_o_video_volta() {
        // Cerca de 2 datagramas por quadro: o 10º cai num delta do começo.
        let (decodificados, pedidos, enviados) = rodar(vec![10]);
        assert!(pedidos.load(Ordering::Relaxed) >= 1);
        let d = decodificados.load(Ordering::Relaxed);
        assert!(d >= 5 && d < enviados, "decodificados {d} de {enviados}");
    }
}
