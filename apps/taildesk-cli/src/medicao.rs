//! Conta quadros e atualizações de cursor por monitor durante uma captura de diagnóstico
//! (`portal`, `captura-node`, `captura-dxgi`). Com `--video`, cada monitor também passa pelo
//! pipeline de vídeo inteiro (I420 → openh264 → datagramas → remontagem → decode) em loopback,
//! sem rede, para medir o custo do encode e a taxa gerada.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use taildesk_codec::{ConfigEncoder, DecoderOpenH264, EncoderOpenH264, OrdemRgb, QuadroRgb};
use taildesk_media::video::{ConfigEnvio, EnvioVideo, RecepcaoVideo, SaidaDatagramas};
use taildesk_platform::captura::{FormatoPixel, Quadro, Rotacao};

/// Bitrate do teste de vídeo (classe LAN do `14-selecao-adaptativa.md`).
const BITRATE: u32 = 20_000_000;

/// Datagramas direto para a recepção, com o tamanho típico da tailnet.
struct Loopback(Arc<Mutex<RecepcaoVideo>>);

impl SaidaDatagramas for Loopback {
    fn max_datagrama(&self) -> Option<usize> {
        Some(1200)
    }
    fn enviar(&mut self, d: &[u8]) {
        self.0.lock().unwrap().receber(d);
    }
}

/// Pipeline de vídeo de um monitor, em loopback.
struct Video {
    envio: EnvioVideo,
    _recepcao: Arc<Mutex<RecepcaoVideo>>,
    decodificados: Arc<AtomicU64>,
    pedidos_chave: Arc<AtomicU64>,
}

impl Video {
    fn novo(display_id: u32, stream_id: u8) -> Result<Self, String> {
        let decodificados = Arc::new(AtomicU64::new(0));
        let pedidos_chave = Arc::new(AtomicU64::new(0));
        let decoder = DecoderOpenH264::novo().map_err(|e| e.to_string())?;
        let recepcao = RecepcaoVideo::iniciar(
            display_id,
            stream_id,
            Box::new(decoder),
            Box::new({
                let d = decodificados.clone();
                move |_, _| {
                    d.fetch_add(1, Ordering::Relaxed);
                }
            }),
            Arc::new({
                let p = pedidos_chave.clone();
                move || {
                    p.fetch_add(1, Ordering::Relaxed);
                }
            }),
        )
        .map_err(|e| e.to_string())?;
        let recepcao = Arc::new(Mutex::new(recepcao));
        let encoder = EncoderOpenH264::novo(ConfigEncoder {
            bitrate_bps: BITRATE,
            fps_max: 60.0,
        })
        .map_err(|e| e.to_string())?;
        let envio = EnvioVideo::iniciar(
            ConfigEnvio {
                stream_id,
                fps_max: 60.0,
            },
            Box::new(encoder),
            Box::new(Loopback(recepcao.clone())),
        )
        .map_err(|e| e.to_string())?;
        Ok(Video {
            envio,
            _recepcao: recepcao,
            decodificados,
            pedidos_chave,
        })
    }

    fn imprimir(&self, segundos: f64) {
        let e = self.envio.estatisticas();
        let quadros = e.quadros.load(Ordering::Relaxed);
        let bytes = e.bytes.load(Ordering::Relaxed);
        let encode_ms = e.encode_us.load(Ordering::Relaxed) as f64 / 1000.0;
        println!(
            "  vídeo: {} codificados ({:.1}/s, {} chaves, {} pulados), {:.1} Mbit/s, \
             encode {:.1} ms/quadro, {} datagramas, {} decodificados, {} pedidos de chave, {} erros",
            quadros,
            quadros as f64 / segundos,
            e.chaves.load(Ordering::Relaxed),
            e.pulados.load(Ordering::Relaxed),
            bytes as f64 * 8.0 / segundos / 1e6,
            if quadros > 0 {
                encode_ms / quadros as f64
            } else {
                0.0
            },
            e.datagramas.load(Ordering::Relaxed),
            self.decodificados.load(Ordering::Relaxed),
            self.pedidos_chave.load(Ordering::Relaxed),
            e.erros.load(Ordering::Relaxed),
        );
    }
}

/// Quadros e último formato de um monitor.
#[derive(Clone, Copy, Debug, Default)]
struct Contagem {
    quadros: u32,
    tamanho: (u32, u32),
    formato: Option<FormatoPixel>,
    rotacao: Rotacao,
    cursores: u32,
    forma: Option<(u32, u32)>,
}

/// Medição de vários monitores; o callback pode ir para várias threads.
#[derive(Clone)]
pub struct Medidor {
    ids: Arc<[u32]>,
    contagem: Arc<Mutex<Vec<Contagem>>>,
    video: Arc<Vec<Video>>,
    inicio: Instant,
}

impl Medidor {
    /// `video` também codifica e decodifica cada monitor (ver o topo do módulo).
    pub fn novo(ids: &[u32], video: bool) -> Self {
        let video = if video {
            ids.iter()
                .enumerate()
                .filter_map(|(i, &id)| match Video::novo(id, i as u8) {
                    Ok(v) => Some(v),
                    Err(e) => {
                        eprintln!("vídeo do monitor {id}: {e}");
                        None
                    }
                })
                .collect()
        } else {
            Vec::new()
        };
        Medidor {
            ids: ids.into(),
            contagem: Arc::new(Mutex::new(vec![Contagem::default(); ids.len()])),
            video: Arc::new(video),
            inicio: Instant::now(),
        }
    }

    pub fn registrar(&self, q: &Quadro<'_>) {
        let Some(i) = self.ids.iter().position(|&d| d == q.display_id) else {
            return;
        };
        if let (Some(v), Some(img)) = (self.video.get(i), &q.imagem) {
            // A rotação fica para o pipeline do host; aqui só o custo do encode importa.
            v.envio.entregar(&QuadroRgb {
                largura: img.largura,
                altura: img.altura,
                stride: img.stride,
                ordem: match img.formato {
                    FormatoPixel::Bgrx | FormatoPixel::Bgra => OrdemRgb::Bgrx,
                    FormatoPixel::Rgbx | FormatoPixel::Rgba => OrdemRgb::Rgbx,
                },
                dados: img.dados,
            });
        }
        let mut c = self.contagem.lock().unwrap();
        let c = &mut c[i];
        if let Some(img) = &q.imagem {
            c.quadros += 1;
            c.tamanho = (img.largura, img.altura);
            c.formato = Some(img.formato);
            c.rotacao = img.rotacao;
        }
        if let Some(cur) = &q.cursor {
            c.cursores += 1;
            if let Some(f) = &cur.forma {
                c.forma = Some((f.largura, f.altura));
            }
        }
    }

    /// Imprime quadros por segundo, tamanho, formato e cursor de cada monitor.
    pub fn imprimir(&self) {
        let s = self.inicio.elapsed().as_secs_f64();
        for (i, (id, c)) in self
            .ids
            .iter()
            .zip(self.contagem.lock().unwrap().iter())
            .enumerate()
        {
            println!(
                "monitor {id}: {} quadros ({:.1}/s), {}x{} {:?} {:?}; cursor: {} atualizações, forma {:?}",
                c.quadros,
                f64::from(c.quadros) / s,
                c.tamanho.0,
                c.tamanho.1,
                c.formato,
                c.rotacao,
                c.cursores,
                c.forma
            );
            if let Some(v) = self.video.get(i) {
                v.imprimir(s);
            }
        }
    }
}
