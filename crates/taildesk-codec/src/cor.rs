//! BGRx/RGBx → I420 (BT.709, faixa limitada), o formato que o openh264 recebe.
//!
//! Inteiros de 8 bits de fração; cada bloco 2×2 dá 4 Y e um par U/V da média dos 4 pixels.
//! Largura e altura ímpares perdem a última coluna/linha (o H.264 4:2:0 pede tamanhos pares).

use crate::{OrdemRgb, QuadroRgb};

/// Imagem I420 com buffers próprios, reaproveitados entre quadros.
#[derive(Debug, Default)]
pub struct I420 {
    pub largura: u32,
    pub altura: u32,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

impl I420 {
    /// Ajusta o tamanho (par) e os buffers; só aloca quando cresce.
    pub fn preparar(&mut self, largura: u32, altura: u32) {
        self.largura = largura & !1;
        self.altura = altura & !1;
        let (l, a) = (self.largura as usize, self.altura as usize);
        self.y.resize(l * a, 0);
        self.u.resize(l / 2 * (a / 2), 0);
        self.v.resize(l / 2 * (a / 2), 0);
    }

    /// Converte `q` (que precisa caber nos buffers: chame [`I420::preparar`] antes).
    pub fn converter(&mut self, q: &QuadroRgb<'_>) {
        // Índices constantes por ordem: o laço interno vetoriza.
        match q.ordem {
            OrdemRgb::Bgrx => self.converter_em::<2, 1, 0>(q),
            OrdemRgb::Rgbx => self.converter_em::<0, 1, 2>(q),
        }
    }

    fn converter_em<const R: usize, const G: usize, const B: usize>(&mut self, q: &QuadroRgb<'_>) {
        let l = self.largura as usize;
        let meia = l / 2;
        let linhas_y = self.y.chunks_exact_mut(2 * l);
        let linhas_uv = self
            .u
            .chunks_exact_mut(meia)
            .zip(self.v.chunks_exact_mut(meia));
        for (by, (ys, (us, vs))) in linhas_y.zip(linhas_uv).enumerate() {
            let l0 = &q.dados[2 * by * q.stride..][..l * 4];
            let l1 = &q.dados[(2 * by + 1) * q.stride..][..l * 4];
            let (y0, y1) = ys.split_at_mut(l);
            let pares = l0
                .chunks_exact(8)
                .zip(l1.chunks_exact(8))
                .zip(y0.chunks_exact_mut(2).zip(y1.chunks_exact_mut(2)))
                .zip(us.iter_mut().zip(vs.iter_mut()));
            for (((a, b), (ya, yb)), (u, v)) in pares {
                let px = |p: &[u8], i: usize| {
                    (
                        i32::from(p[i * 4 + R]),
                        i32::from(p[i * 4 + G]),
                        i32::from(p[i * 4 + B]),
                    )
                };
                let (p0, p1, p2, p3) = (px(a, 0), px(a, 1), px(b, 0), px(b, 1));
                ya[0] = luma(p0.0, p0.1, p0.2);
                ya[1] = luma(p1.0, p1.1, p1.2);
                yb[0] = luma(p2.0, p2.1, p2.2);
                yb[1] = luma(p3.0, p3.1, p3.2);
                let media = |s: i32| (s + 2) >> 2;
                (*u, *v) = croma(
                    media(p0.0 + p1.0 + p2.0 + p3.0),
                    media(p0.1 + p1.1 + p2.1 + p3.1),
                    media(p0.2 + p1.2 + p2.2 + p3.2),
                );
            }
        }
    }
}

fn luma(r: i32, g: i32, b: i32) -> u8 {
    (16 + ((47 * r + 157 * g + 16 * b + 128) >> 8)) as u8
}

fn croma(r: i32, g: i32, b: i32) -> (u8, u8) {
    let u = 128 + ((-26 * r - 86 * g + 112 * b + 128) >> 8);
    let v = 128 + ((112 * r - 102 * g - 10 * b + 128) >> 8);
    (u.clamp(16, 240) as u8, v.clamp(16, 240) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cor(r: u8, g: u8, b: u8) -> (u8, u8, u8) {
        let px = [b, g, r, 0];
        let dados: Vec<u8> = px.iter().copied().cycle().take(4 * 4).collect();
        let q = QuadroRgb {
            largura: 2,
            altura: 2,
            stride: 8,
            ordem: OrdemRgb::Bgrx,
            dados: &dados,
        };
        let mut i = I420::default();
        i.preparar(2, 2);
        i.converter(&q);
        assert!(i.y.iter().all(|&y| y == i.y[0]));
        (i.y[0], i.u[0], i.v[0])
    }

    #[test]
    fn cores_de_referencia_bt709() {
        assert_eq!(cor(255, 255, 255), (235, 128, 128));
        assert_eq!(cor(0, 0, 0), (16, 128, 128));
        assert_eq!(cor(255, 0, 0), (63, 102, 240));
        let (y, u, v) = cor(0, 0, 255);
        assert_eq!((y, u), (32, 240));
        assert!(v.abs_diff(118) <= 1);
    }

    #[test]
    fn respeita_stride_ordem_e_tamanho_impar() {
        // 3x3 RGBx com stride 16: só o bloco 2x2 de cima à esquerda entra.
        let mut dados = vec![0u8; 16 * 3];
        for y in 0..3 {
            for x in 0..3 {
                dados[y * 16 + x * 4] = 255; // R
            }
        }
        let q = QuadroRgb {
            largura: 3,
            altura: 3,
            stride: 16,
            ordem: OrdemRgb::Rgbx,
            dados: &dados,
        };
        let mut i = I420::default();
        i.preparar(3, 3);
        assert_eq!((i.largura, i.altura, i.y.len(), i.u.len()), (2, 2, 4, 1));
        i.converter(&q);
        assert_eq!((i.y[0], i.u[0], i.v[0]), (63, 102, 240));
    }

    #[test]
    fn preparar_reaproveita_buffers() {
        let mut i = I420::default();
        i.preparar(64, 32);
        let p = i.y.as_ptr();
        i.preparar(32, 16);
        assert_eq!(i.y.as_ptr(), p);
        assert_eq!(i.y.len(), 32 * 16);
    }
}
