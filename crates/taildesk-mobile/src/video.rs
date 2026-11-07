//! Quadro decodificado → imagem da tela (`mobile/arquitetura.md` §5).
//!
//! Caminho do M1: o decoder (MediaCodec) entrega os planos YUV na thread de decode, que os
//! converte para RGBA com aritmética inteira e guarda na [`Caixa`] (latest-wins, sem fila: a
//! interface só pega o mais novo). Custa uma conversão na CPU por quadro; o caminho sem cópia
//! (`AHardwareBuffer` → textura do wgpu) é o gate do S6 em `mobile/m0-resultados.md`.
//!
//! Sem alocação depois do primeiro quadro de cada tamanho (regra do hot path de mídia): o buffer
//! de saída é reaproveitado e trocado com o da interface.

use std::sync::{Mutex, MutexGuard, PoisonError};

use taildesk_codec::{Faixa, FormatoYuv, Matriz, QuadroYuv};

/// Coeficientes em ponto fixo (×2^14) de YCbCr → RGB.
#[derive(Clone, Copy, Debug)]
struct Coef {
    y: i32,
    y0: i32,
    rv: i32,
    gu: i32,
    gv: i32,
    bu: i32,
}

fn coef(m: Matriz, f: Faixa) -> Coef {
    const K: f32 = 16_384.0;
    let (kr_v, kg_u, kg_v, kb_u) = match m {
        Matriz::Bt601 => (1.402, -0.344_136, -0.714_136, 1.772),
        Matriz::Bt709 => (1.5748, -0.187_324, -0.468_124, 1.8556),
    };
    let (sy, sc, y0) = match f {
        Faixa::Limitada => (255.0 / 219.0, 255.0 / 224.0, 16),
        Faixa::Cheia => (1.0, 1.0, 0),
    };
    let i = |x: f32| (x * K).round() as i32;
    Coef {
        y: i(sy),
        y0,
        rv: i(kr_v * sc),
        gu: i(kg_u * sc),
        gv: i(kg_v * sc),
        bu: i(kb_u * sc),
    }
}

#[inline]
fn pixel(c: &Coef, y: u8, u: u8, v: u8) -> [u8; 4] {
    let y = (i32::from(y) - c.y0) * c.y;
    let (u, v) = (i32::from(u) - 128, i32::from(v) - 128);
    let r = (y + c.rv * v + 8_192) >> 14;
    let g = (y + c.gu * u + c.gv * v + 8_192) >> 14;
    let b = (y + c.bu * u + 8_192) >> 14;
    [
        r.clamp(0, 255) as u8,
        g.clamp(0, 255) as u8,
        b.clamp(0, 255) as u8,
        255,
    ]
}

/// Converte para RGBA (4 bytes por pixel, linhas de `largura * 4`) em `saida`, que cresce se
/// precisar. Planos menores que o tamanho anunciado não estouram: o resto fica preto.
pub fn para_rgba(q: &QuadroYuv<'_>, saida: &mut Vec<u8>) {
    let (w, h) = (q.largura as usize, q.altura as usize);
    saida.resize(w * h * 4, 0);
    let c = coef(q.matriz, q.faixa);
    let [py, p1, p2] = q.planos;
    for (yy, linha) in saida.chunks_exact_mut(w * 4).enumerate() {
        let ly = py.dados.get(yy * py.passo as usize..).unwrap_or_default();
        let lc = yy / 2;
        let l1 = p1.dados.get(lc * p1.passo as usize..).unwrap_or_default();
        let l2 = p2.dados.get(lc * p2.passo as usize..).unwrap_or_default();
        for (x, px) in linha.chunks_exact_mut(4).enumerate() {
            let y = ly.get(x).copied().unwrap_or(16);
            let (u, v) = match q.formato {
                FormatoYuv::Nv12 => (
                    l1.get(x & !1).copied().unwrap_or(128),
                    l1.get((x & !1) + 1).copied().unwrap_or(128),
                ),
                FormatoYuv::I420 => (
                    l1.get(x / 2).copied().unwrap_or(128),
                    l2.get(x / 2).copied().unwrap_or(128),
                ),
            };
            px.copy_from_slice(&pixel(&c, y, u, v));
        }
    }
}

/// Último quadro pronto para a interface.
#[derive(Debug, Default)]
pub struct Caixa {
    inner: Mutex<Quadro>,
}

/// Um quadro RGBA.
#[derive(Debug, Default)]
pub struct Quadro {
    pub largura: u32,
    pub altura: u32,
    pub rgba: Vec<u8>,
    /// Chegou um quadro novo desde a última vez que a interface pegou.
    pub novo: bool,
}

impl Caixa {
    fn travar(&self) -> MutexGuard<'_, Quadro> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Thread de decode: converte e guarda. `true` se a interface ainda não sabia de um quadro
    /// novo (aí vale acordá-la; se já sabia, ela pega este mesmo).
    pub fn guardar(&self, q: &QuadroYuv<'_>, rascunho: &mut Vec<u8>) -> bool {
        para_rgba(q, rascunho);
        let mut g = self.travar();
        std::mem::swap(&mut g.rgba, rascunho);
        g.largura = q.largura;
        g.altura = q.altura;
        !std::mem::replace(&mut g.novo, true)
    }

    /// Thread da interface: troca o quadro mais novo pelo buffer `local` (sem copiar).
    pub fn pegar(&self, local: &mut Quadro) -> bool {
        let mut g = self.travar();
        if !g.novo {
            return false;
        }
        g.novo = false;
        std::mem::swap(&mut g.rgba, &mut local.rgba);
        local.largura = g.largura;
        local.altura = g.altura;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use taildesk_codec::PlanoYuv;

    fn quadro<'a>(
        fmt: FormatoYuv,
        y: &'a [u8],
        c1: &'a [u8],
        c2: &'a [u8],
        passo_c: u32,
    ) -> QuadroYuv<'a> {
        QuadroYuv {
            largura: 2,
            altura: 2,
            formato: fmt,
            planos: [
                PlanoYuv { dados: y, passo: 2 },
                PlanoYuv {
                    dados: c1,
                    passo: passo_c,
                },
                PlanoYuv {
                    dados: c2,
                    passo: passo_c,
                },
            ],
            matriz: Matriz::Bt709,
            faixa: Faixa::Limitada,
        }
    }

    #[test]
    fn cores_conhecidas_bt709() {
        let c = coef(Matriz::Bt709, Faixa::Limitada);
        assert_eq!(pixel(&c, 16, 128, 128), [0, 0, 0, 255]);
        assert_eq!(pixel(&c, 235, 128, 128), [255, 255, 255, 255]);
        assert_eq!(pixel(&c, 126, 128, 128)[..3], [128, 128, 128]);
        // Vermelho BT.709 limitado: Y=63, Cb=102, Cr=240.
        let r = pixel(&c, 63, 102, 240);
        assert!(r[0] >= 250 && r[1] <= 5 && r[2] <= 5, "{r:?}");
    }

    #[test]
    fn nv12_e_i420_dao_o_mesmo() {
        let y = [235, 16, 126, 63];
        let nv12 = quadro(FormatoYuv::Nv12, &y, &[102, 240], &[], 2);
        let i420 = quadro(FormatoYuv::I420, &y, &[102], &[240], 1);
        let (mut a, mut b) = (Vec::new(), Vec::new());
        para_rgba(&nv12, &mut a);
        para_rgba(&i420, &mut b);
        assert_eq!(a, b);
        assert_eq!(a.len(), 16);
    }

    #[test]
    fn plano_curto_nao_estoura() {
        let q = quadro(FormatoYuv::Nv12, &[235], &[], &[], 2);
        let mut out = Vec::new();
        para_rgba(&q, &mut out);
        assert_eq!(&out[..4], &[255, 255, 255, 255]);
        assert_eq!(&out[4..8], &[0, 0, 0, 255]);
    }

    #[test]
    fn caixa_fica_com_o_mais_novo_sem_copiar() {
        let caixa = Caixa::default();
        let y = [16u8; 4];
        let q = quadro(FormatoYuv::I420, &y, &[128], &[128], 1);
        let mut rascunho = Vec::new();
        assert!(caixa.guardar(&q, &mut rascunho));
        // O segundo antes da interface pegar não acorda de novo.
        assert!(!caixa.guardar(&q, &mut rascunho));
        let mut local = Quadro::default();
        assert!(caixa.pegar(&mut local));
        assert_eq!((local.largura, local.altura, local.rgba.len()), (2, 2, 16));
        assert!(!caixa.pegar(&mut local));
    }
}
