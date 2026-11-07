//! QR do pareamento com o celular (Configurações → Celulares).

use qrcode::{EcLevel, QrCode};

/// Pixels por módulo do QR (o Slint ainda escala com `image-rendering: pixelated`).
const ESCALA: usize = 6;
/// Margem branca em módulos (a norma pede 4).
const MARGEM: usize = 4;

/// Imagem em tons de cinza (0 = preto, 255 = branco), quadrada, `lado` × `lado`.
#[derive(Debug, PartialEq, Eq)]
pub struct ImagemQr {
    pub lado: usize,
    pub pixels: Vec<u8>,
}

/// Desenha `texto` num QR com correção média (aguenta reflexo na tela do monitor).
pub fn gerar(texto: &str) -> Option<ImagemQr> {
    let code = QrCode::with_error_correction_level(texto.as_bytes(), EcLevel::M).ok()?;
    let n = code.width();
    let lado = (n + 2 * MARGEM) * ESCALA;
    let mut pixels = vec![255u8; lado * lado];
    for (i, cor) in code.to_colors().iter().enumerate() {
        if *cor == qrcode::Color::Dark {
            let (mx, my) = (i % n + MARGEM, i / n + MARGEM);
            for y in my * ESCALA..(my + 1) * ESCALA {
                pixels[y * lado + mx * ESCALA..y * lado + (mx + 1) * ESCALA].fill(0);
            }
        }
    }
    Some(ImagemQr { lado, pixels })
}

/// Para o Slint.
pub fn imagem(qr: &ImagemQr) -> slint::Image {
    let mut buf =
        slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(qr.lado as u32, qr.lado as u32);
    for (d, &v) in buf.make_mut_slice().iter_mut().zip(&qr.pixels) {
        *d = slint::Rgba8Pixel::new(v, v, v, 255);
    }
    slint::Image::from_rgba8(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use taildesk_proto::pair::PairOffer;

    #[test]
    fn qr_volta_a_mesma_oferta() {
        let oferta = PairOffer {
            host_name: "notebook-yuri".into(),
            addr: "100.101.102.103".parse().unwrap(),
            port: 7420,
            fingerprint: [0x3c; 32],
            code: "K7P2QX".into(),
        };
        let qr = gerar(&oferta.to_uri()).unwrap();
        let mut img = rqrr::PreparedImage::prepare_from_greyscale(qr.lado, qr.lado, |x, y| {
            qr.pixels[y * qr.lado + x]
        });
        let grades = img.detect_grids();
        assert_eq!(grades.len(), 1);
        let (_, texto) = grades[0].decode().unwrap();
        assert_eq!(PairOffer::from_uri(&texto).unwrap(), oferta);
    }
}
