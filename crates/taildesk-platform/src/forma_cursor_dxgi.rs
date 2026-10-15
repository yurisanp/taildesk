//! Converte a forma do ponteiro da Desktop Duplication (`GetFramePointerShape`) para BGRA.
//!
//! Lógica pura, fora do `cfg(windows)` para os testes rodarem em todo CI. O controlador desenha
//! o cursor por cima do vídeo sem ler a tela de baixo, então os modos "inverter" do Windows
//! (XOR) viram uma aproximação: preto opaco no monocromático, a própria cor no colorido com
//! máscara.
#![cfg_attr(not(windows), allow(dead_code))]

/// Lado máximo do cursor aceito (o mesmo limite do PipeWire); acima disso a forma é ignorada.
pub(crate) const CURSOR_MAX: u32 = 256;

/// `DXGI_OUTDUPL_POINTER_SHAPE_TYPE`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TipoForma {
    /// 1 bpp: máscara AND nas primeiras `altura / 2` linhas e XOR nas seguintes.
    Monocromatico,
    /// BGRA 32 bpp com alfa normal.
    Colorido,
    /// BGRA 32 bpp; o byte de alfa é máscara (0 = copia a cor, 0xFF = XOR com a tela).
    ColoridoMascarado,
}

impl TipoForma {
    /// Valores de `DXGI_OUTDUPL_POINTER_SHAPE_TYPE_*`.
    pub(crate) fn de(tipo: u32) -> Option<TipoForma> {
        Some(match tipo {
            1 => TipoForma::Monocromatico,
            2 => TipoForma::Colorido,
            4 => TipoForma::ColoridoMascarado,
            _ => return None,
        })
    }
}

/// Escreve a forma em `destino` como BGRA com `largura * 4` bytes por linha e devolve
/// `(largura, altura)` do cursor. `None` se o tamanho passa de [`CURSOR_MAX`] ou os buffers não
/// cabem.
pub(crate) fn converter(
    tipo: TipoForma,
    largura: u32,
    altura: u32,
    pitch: usize,
    origem: &[u8],
    destino: &mut [u8],
) -> Option<(u32, u32)> {
    let alt = if tipo == TipoForma::Monocromatico {
        altura / 2
    } else {
        altura
    };
    if largura == 0 || alt == 0 || largura > CURSOR_MAX || alt > CURSOR_MAX {
        return None;
    }
    let (l, a) = (largura as usize, alt as usize);
    let minimo_pitch = match tipo {
        TipoForma::Monocromatico => l.div_ceil(8),
        _ => l * 4,
    };
    if pitch < minimo_pitch
        || origem.len() < pitch * (altura as usize - 1) + minimo_pitch
        || destino.len() < l * a * 4
    {
        return None;
    }
    for y in 0..a {
        for x in 0..l {
            let px = match tipo {
                TipoForma::Monocromatico => {
                    let bit = 0x80u8 >> (x % 8);
                    let e = origem[y * pitch + x / 8] & bit != 0;
                    let ou = origem[(y + a) * pitch + x / 8] & bit != 0;
                    match (e, ou) {
                        (true, false) => [0, 0, 0, 0],
                        (false, false) => [0, 0, 0, 255],
                        (false, true) => [255, 255, 255, 255],
                        // Inverte a tela: sem a tela, preto opaco.
                        (true, true) => [0, 0, 0, 255],
                    }
                }
                TipoForma::Colorido => {
                    let i = y * pitch + x * 4;
                    [origem[i], origem[i + 1], origem[i + 2], origem[i + 3]]
                }
                TipoForma::ColoridoMascarado => {
                    let i = y * pitch + x * 4;
                    let [b, g, r, m] = [origem[i], origem[i + 1], origem[i + 2], origem[i + 3]];
                    if m == 0 {
                        [b, g, r, 255]
                    } else if b | g | r == 0 {
                        // XOR com preto não muda a tela: transparente.
                        [0, 0, 0, 0]
                    } else {
                        [b, g, r, 255]
                    }
                }
            };
            let o = (y * l + x) * 4;
            destino[o..o + 4].copy_from_slice(&px);
        }
    }
    Some((largura, alt))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monocromatico_usa_and_e_xor() {
        // 8x2 (pitch 1): bytes 0 e 1 são a máscara AND, 2 e 3 a XOR.
        // x0: AND 1 XOR 0 → transparente; x1: 0/0 → preto; x2: 0/1 → branco; x3: 1/1 → preto.
        let origem = [0b1001_0000u8, 0b1111_1111, 0b0011_0000, 0b0000_0000];
        let mut destino = [9u8; 8 * 2 * 4];
        let tam = converter(TipoForma::Monocromatico, 8, 4, 1, &origem, &mut destino);
        assert_eq!(tam, Some((8, 2)));
        assert_eq!(&destino[0..4], &[0, 0, 0, 0]);
        assert_eq!(&destino[4..8], &[0, 0, 0, 255]);
        assert_eq!(&destino[8..12], &[255, 255, 255, 255]);
        assert_eq!(&destino[12..16], &[0, 0, 0, 255]);
        // Segunda linha: AND todo 1, XOR todo 0 → transparente.
        assert!(destino[32..64].iter().all(|&b| b == 0));
    }

    #[test]
    fn colorido_respeita_pitch() {
        let mut origem = vec![0u8; 2 * 12];
        origem[..8].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        origem[12..20].copy_from_slice(&[9, 10, 11, 12, 13, 14, 15, 16]);
        let mut destino = [0u8; 16];
        assert_eq!(
            converter(TipoForma::Colorido, 2, 2, 12, &origem, &mut destino),
            Some((2, 2))
        );
        assert_eq!(
            destino,
            [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
        );
    }

    #[test]
    fn mascarado_vira_opaco_ou_transparente() {
        let origem = [10, 20, 30, 0, 0, 0, 0, 0xFF, 40, 50, 60, 0xFF];
        let mut destino = [0u8; 12];
        converter(
            TipoForma::ColoridoMascarado,
            3,
            1,
            12,
            &origem,
            &mut destino,
        )
        .unwrap();
        assert_eq!(destino, [10, 20, 30, 255, 0, 0, 0, 0, 40, 50, 60, 255]);
    }

    #[test]
    fn recusa_tamanhos_fora_do_limite() {
        let mut destino = vec![0u8; 4];
        assert_eq!(
            converter(TipoForma::Colorido, 0, 1, 4, &[0; 4], &mut destino),
            None
        );
        assert_eq!(
            converter(
                TipoForma::Colorido,
                CURSOR_MAX + 1,
                1,
                2000,
                &[0; 2000],
                &mut destino
            ),
            None
        );
        // Origem curta demais para o pitch informado.
        assert_eq!(
            converter(TipoForma::Colorido, 1, 2, 64, &[0; 8], &mut destino),
            None
        );
        // Destino curto demais.
        assert_eq!(
            converter(TipoForma::Colorido, 2, 1, 8, &[0; 8], &mut destino),
            None
        );
    }

    #[test]
    fn tipos_do_dxgi() {
        assert_eq!(TipoForma::de(1), Some(TipoForma::Monocromatico));
        assert_eq!(TipoForma::de(2), Some(TipoForma::Colorido));
        assert_eq!(TipoForma::de(4), Some(TipoForma::ColoridoMascarado));
        assert_eq!(TipoForma::de(3), None);
    }
}
