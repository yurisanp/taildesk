//! Redução da imagem antes do encoder, para o laço lento ([`crate::escada`]): cada pixel de saída
//! é a média do retângulo da origem que ele cobre (4 bytes por pixel; o 4º byte vai junto, mas o
//! encoder ignora). Não aloca depois do primeiro quadro de cada tamanho.

/// Buffers da redução de um envio.
#[derive(Debug, Default)]
pub struct Reducao {
    /// Coluna inicial e final (exclusiva) da origem para cada coluna da saída.
    colunas: Vec<(usize, usize)>,
    /// (largura da origem, largura da saída) de `colunas`.
    chave: (u32, u32),
    /// Somas de uma linha da saída.
    soma: Vec<u32>,
    saida: Vec<u8>,
}

/// Pixels da origem cobertos pelo pixel `i` da saída: `[início, fim)`, nunca vazio.
fn intervalo(i: u32, origem: u32, destino: u32) -> (usize, usize) {
    let a = u64::from(i) * u64::from(origem) / u64::from(destino);
    let b = (u64::from(i) + 1) * u64::from(origem) / u64::from(destino);
    (a as usize, b.max(a + 1).min(u64::from(origem)) as usize)
}

impl Reducao {
    /// Reduz `origem` (`ol`×`oa` pixels, linhas de `ol * 4` bytes) para `dl`×`da`, com
    /// `dl <= ol` e `da <= oa`. Devolve a imagem reduzida (linhas de `dl * 4` bytes).
    pub fn reduzir(&mut self, origem: &[u8], (ol, oa): (u32, u32), (dl, da): (u32, u32)) -> &[u8] {
        let (dl, da) = (dl.clamp(1, ol.max(1)), da.clamp(1, oa.max(1)));
        let linha = ol as usize * 4;
        if origem.len() < linha * oa as usize {
            self.saida.clear();
            return &self.saida;
        }
        if self.chave != (ol, dl) {
            self.colunas.clear();
            self.colunas.extend((0..dl).map(|x| intervalo(x, ol, dl)));
            self.chave = (ol, dl);
        }
        let saida_linha = dl as usize * 4;
        self.saida.resize(saida_linha * da as usize, 0);
        self.soma.resize(saida_linha, 0);
        for (y, dst) in self.saida.chunks_exact_mut(saida_linha).enumerate() {
            let (y0, y1) = intervalo(y as u32, oa, da);
            self.soma.fill(0);
            for src in origem[y0 * linha..y1 * linha].chunks_exact(linha) {
                for (s, &(x0, x1)) in self.soma.chunks_exact_mut(4).zip(&self.colunas) {
                    for p in src[x0 * 4..x1 * 4].chunks_exact(4) {
                        s[0] += u32::from(p[0]);
                        s[1] += u32::from(p[1]);
                        s[2] += u32::from(p[2]);
                        s[3] += u32::from(p[3]);
                    }
                }
            }
            let linhas = (y1 - y0) as u32;
            for ((d, s), &(x0, x1)) in dst
                .chunks_exact_mut(4)
                .zip(self.soma.chunks_exact(4))
                .zip(&self.colunas)
            {
                let n = linhas * (x1 - x0) as u32;
                for k in 0..4 {
                    d[k] = ((s[k] + n / 2) / n) as u8;
                }
            }
        }
        &self.saida
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intervalos_cobrem_a_origem_sem_buraco() {
        for (o, d) in [(1920, 1280), (1080, 720), (2560, 1728), (7, 3), (5, 5)] {
            let mut fim = 0;
            for i in 0..d {
                let (a, b) = intervalo(i, o, d);
                assert_eq!(a, fim, "{o}→{d} em {i}");
                assert!(b > a);
                fim = b;
            }
            assert_eq!(fim, o as usize);
        }
    }

    #[test]
    fn metade_e_a_media_de_cada_quadrado() {
        // 4×2 → 2×1: cada pixel de saída é a média de 2×2.
        #[rustfmt::skip]
        let origem = [
            0, 0, 0, 0,      10, 20, 30, 0,   100, 100, 100, 0,  200, 200, 200, 0,
            20, 40, 60, 0,   30, 40, 50, 0,   100, 100, 100, 0,  201, 201, 201, 0,
        ];
        let mut r = Reducao::default();
        let s = r.reduzir(&origem, (4, 2), (2, 1)).to_vec();
        assert_eq!(s, [15, 25, 35, 0, 150, 150, 150, 0]);
        // Mesmo tamanho de novo: reaproveita os buffers e dá o mesmo resultado.
        assert_eq!(r.reduzir(&origem, (4, 2), (2, 1)), s);
    }

    #[test]
    fn cor_lisa_continua_lisa_em_escala_quebrada() {
        let (ol, oa) = (1365u32, 767u32);
        let origem: Vec<u8> = [9u8, 99, 199, 255].repeat((ol * oa) as usize);
        let mut r = Reducao::default();
        let s = r.reduzir(&origem, (ol, oa), (960, 540));
        assert_eq!(s.len(), 960 * 540 * 4);
        assert!(s.chunks_exact(4).all(|p| p == [9, 99, 199, 255]));
    }

    #[test]
    fn origem_curta_nao_entra_em_panico() {
        let mut r = Reducao::default();
        assert!(r.reduzir(&[0; 8], (4, 4), (2, 2)).is_empty());
    }
}
