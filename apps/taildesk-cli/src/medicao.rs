//! Conta quadros e atualizações de cursor por monitor durante uma captura de diagnóstico
//! (`portal`, `captura-node`, `captura-dxgi`).

use std::sync::{Arc, Mutex};
use std::time::Instant;

use taildesk_platform::captura::{FormatoPixel, Quadro, Rotacao};

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
    inicio: Instant,
}

impl Medidor {
    pub fn novo(ids: &[u32]) -> Self {
        Medidor {
            ids: ids.into(),
            contagem: Arc::new(Mutex::new(vec![Contagem::default(); ids.len()])),
            inicio: Instant::now(),
        }
    }

    pub fn registrar(&self, q: &Quadro<'_>) {
        let Some(i) = self.ids.iter().position(|&d| d == q.display_id) else {
            return;
        };
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
        for (id, c) in self.ids.iter().zip(self.contagem.lock().unwrap().iter()) {
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
        }
    }
}
