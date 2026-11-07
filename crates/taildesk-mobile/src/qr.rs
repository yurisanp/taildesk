//! Leitura do QR de pareamento (mockup 03). A câmera (CameraX, na casca Kotlin) entrega o plano Y
//! de cada quadro; a decodificação é em Rust (`rqrr`, MIT/Apache + ISC) para não depender do ML
//! Kit, que é proprietário (`android-detalhado.md` §4.3).

use taildesk_proto::pair::{PairOffer, PairUriError};

/// O que a leitura de um quadro achou.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Leitura {
    /// Nenhum QR no quadro (continue mandando quadros).
    Nada,
    /// Um QR do TailDesk válido.
    Oferta(PairOffer),
    /// Um QR, mas não do TailDesk ou de outra versão (a tela explica).
    Invalido(PairUriError),
}

/// Lê um quadro em tons de cinza (`stride` ≥ `largura`). Quadro estranho (tamanho que não bate)
/// vira [`Leitura::Nada`].
pub fn ler(largura: usize, altura: usize, stride: usize, luma: &[u8]) -> Leitura {
    if largura == 0
        || altura == 0
        || stride < largura
        || luma.len() < stride * (altura - 1) + largura
    {
        return Leitura::Nada;
    }
    let mut img =
        rqrr::PreparedImage::prepare_from_greyscale(largura, altura, |x, y| luma[y * stride + x]);
    let mut invalido = None;
    for grade in img.detect_grids() {
        let Ok((_, texto)) = grade.decode() else {
            continue;
        };
        match PairOffer::from_uri(&texto) {
            Ok(o) => return Leitura::Oferta(o),
            Err(e) => invalido = Some(e),
        }
    }
    invalido.map_or(Leitura::Nada, Leitura::Invalido)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// QR desenhado com `qrcode` (como o desktop faz), com margem e stride maior que a largura.
    fn quadro(texto: &str) -> (usize, usize, usize, Vec<u8>) {
        let code = qrcode::QrCode::new(texto.as_bytes()).unwrap();
        let n = code.width();
        let escala = 5;
        let lado = (n + 8) * escala;
        let stride = lado + 16;
        let mut luma = vec![255u8; stride * lado];
        for (i, cor) in code.to_colors().iter().enumerate() {
            if *cor == qrcode::Color::Dark {
                let (mx, my) = (i % n + 4, i / n + 4);
                for y in my * escala..(my + 1) * escala {
                    for x in mx * escala..(mx + 1) * escala {
                        luma[y * stride + x] = 0;
                    }
                }
            }
        }
        (lado, lado, stride, luma)
    }

    #[test]
    fn le_a_oferta_do_desktop() {
        let o = PairOffer {
            host_name: "desktop-casa".into(),
            addr: "100.64.0.1".parse().unwrap(),
            port: 7420,
            fingerprint: [7; 32],
            code: "K7P2QX".into(),
        };
        let (w, h, s, luma) = quadro(&o.to_uri());
        assert_eq!(ler(w, h, s, &luma), Leitura::Oferta(o));
    }

    #[test]
    fn qr_de_outra_coisa_e_quadro_vazio() {
        let (w, h, s, luma) = quadro("https://exemplo.com");
        assert_eq!(
            ler(w, h, s, &luma),
            Leitura::Invalido(PairUriError::NotTaildesk)
        );
        assert_eq!(ler(64, 64, 64, &vec![200; 64 * 64]), Leitura::Nada);
        assert_eq!(ler(64, 64, 64, &[0; 10]), Leitura::Nada);
        assert_eq!(ler(0, 0, 0, &[]), Leitura::Nada);
    }
}
