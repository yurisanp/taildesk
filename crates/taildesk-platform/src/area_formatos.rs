//! Conversões dos formatos da área de transferência do Windows, em código puro (testado em
//! qualquer sistema):
//!
//! - `CF_DIB`/`CF_DIBV5` ↔ PNG (o protocolo só leva PNG; muitos apps do Windows só põem DIB);
//! - `"HTML Format"` (CF_HTML, com o cabeçalho de offsets) ↔ fragmento HTML;
//! - `CF_UNICODETEXT` (UTF-16 com `\r\n` e `\0` no fim) ↔ UTF-8 com `\n`.

use std::io::Cursor;

/// Maior imagem aceita de um DIB (lados), para não alocar o que um app mal-feito anunciar.
const LADO_MAX: u32 = 16_384;

const BI_RGB: u32 = 0;
const BI_BITFIELDS: u32 = 3;

fn u32_em(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

fn i32_em(b: &[u8], i: usize) -> Option<i32> {
    Some(i32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

fn u16_em(b: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(i..i + 2)?.try_into().ok()?))
}

/// Imagem RGBA 8 bits, linha a linha de cima para baixo.
#[derive(Debug, PartialEq, Eq)]
pub struct Rgba {
    pub largura: u32,
    pub altura: u32,
    pub pixels: Vec<u8>,
}

/// Lê um DIB (`BITMAPINFOHEADER`, `BITMAPV4HEADER` ou `BITMAPV5HEADER` + pixels) de 24 ou 32
/// bits. 32 bits sem alfa nenhum (o comum em `BI_RGB`) vira opaco.
pub fn dib_para_rgba(dib: &[u8]) -> Option<Rgba> {
    let tam_cab = u32_em(dib, 0)? as usize;
    if tam_cab < 40 {
        return None;
    }
    let largura = i32_em(dib, 4)?;
    let altura_c = i32_em(dib, 8)?;
    let bits = u16_em(dib, 14)?;
    let compressao = u32_em(dib, 16)?;
    let cores_usadas = u32_em(dib, 32)? as usize;
    if largura <= 0 || altura_c == 0 || largura as u32 > LADO_MAX {
        return None;
    }
    let (largura, altura) = (largura as u32, altura_c.unsigned_abs());
    if altura > LADO_MAX || !matches!(bits, 24 | 32) {
        return None;
    }
    // Máscaras: no cabeçalho V4/V5, ou logo depois do de 40 bytes com BI_BITFIELDS.
    let (mut r, mut g, mut b, mut a) = (0x00ff_0000, 0x0000_ff00, 0x0000_00ff, 0);
    let mut inicio = tam_cab + cores_usadas * 4;
    match compressao {
        BI_RGB => {}
        BI_BITFIELDS if bits == 32 => {
            let base = if tam_cab >= 52 { 40 } else { tam_cab };
            r = u32_em(dib, base)?;
            g = u32_em(dib, base + 4)?;
            b = u32_em(dib, base + 8)?;
            if tam_cab >= 56 {
                a = u32_em(dib, 52)?;
            }
            if tam_cab == 40 {
                inicio += 12;
            }
        }
        _ => return None,
    }
    if tam_cab >= 56 && compressao == BI_RGB && bits == 32 {
        a = u32_em(dib, 52).unwrap_or(0);
    }
    let passo = ((largura as usize * bits as usize).div_ceil(32)) * 4;
    let dados = dib.get(inicio..inicio + passo * altura as usize)?;
    let canal = |p: u32, m: u32| -> u8 {
        if m == 0 {
            return 0;
        }
        ((p & m) >> m.trailing_zeros()) as u8
    };
    let mut pixels = Vec::with_capacity(largura as usize * altura as usize * 4);
    let mut algum_alfa = false;
    for y in 0..altura as usize {
        // Altura positiva = de baixo para cima.
        let linha = if altura_c > 0 {
            altura as usize - 1 - y
        } else {
            y
        };
        let l = &dados[linha * passo..linha * passo + passo];
        for x in 0..largura as usize {
            if bits == 24 {
                let p = &l[x * 3..x * 3 + 3];
                pixels.extend_from_slice(&[p[2], p[1], p[0], 255]);
            } else {
                let p = u32::from_le_bytes(l[x * 4..x * 4 + 4].try_into().ok()?);
                let alfa = if a == 0 { 0 } else { canal(p, a) };
                algum_alfa |= alfa != 0;
                pixels.extend_from_slice(&[canal(p, r), canal(p, g), canal(p, b), alfa]);
            }
        }
    }
    if bits == 32 && !algum_alfa {
        for p in pixels.chunks_exact_mut(4) {
            p[3] = 255;
        }
    }
    Some(Rgba {
        largura,
        altura,
        pixels,
    })
}

/// DIB de 32 bits de baixo para cima (`BITMAPINFOHEADER`, `BI_RGB`), o que todo app lê.
pub fn rgba_para_dib(img: &Rgba) -> Vec<u8> {
    let (w, h) = (img.largura as usize, img.altura as usize);
    let mut out = Vec::with_capacity(40 + w * h * 4);
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(img.largura as i32).to_le_bytes());
    out.extend_from_slice(&(img.altura as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&BI_RGB.to_le_bytes());
    out.extend_from_slice(&((w * h * 4) as u32).to_le_bytes());
    out.extend_from_slice(&[0; 16]);
    for y in (0..h).rev() {
        for p in img.pixels[y * w * 4..(y + 1) * w * 4].chunks_exact(4) {
            out.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
        }
    }
    out
}

/// RGBA → PNG.
pub fn rgba_para_png(img: &Rgba) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut e = png::Encoder::new(&mut out, img.largura, img.altura);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    e.set_compression(png::Compression::Fast);
    let mut w = e.write_header().ok()?;
    w.write_image_data(&img.pixels).ok()?;
    w.finish().ok()?;
    Some(out)
}

/// PNG → RGBA (cinza, paleta e 16 bits viram RGBA 8 bits).
pub fn png_para_rgba(png_bytes: &[u8]) -> Option<Rgba> {
    let mut d = png::Decoder::new(Cursor::new(png_bytes));
    d.set_transformations(
        png::Transformations::EXPAND | png::Transformations::ALPHA | png::Transformations::STRIP_16,
    );
    let mut r = d.read_info().ok()?;
    let (w, h) = (r.info().width, r.info().height);
    if w == 0 || h == 0 || w > LADO_MAX || h > LADO_MAX {
        return None;
    }
    let mut buf = vec![0; r.output_buffer_size()?];
    let info = r.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    let pixels = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::GrayscaleAlpha => buf
            .chunks_exact(2)
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        _ => return None,
    };
    Some(Rgba {
        largura: w,
        altura: h,
        pixels,
    })
}

/// O fragmento de um `"HTML Format"` (os offsets são em bytes do UTF-8).
pub fn cf_html_para_fragmento(cf: &[u8]) -> Option<Vec<u8>> {
    let cabecalho = String::from_utf8_lossy(&cf[..cf.len().min(1024)]);
    let campo = |nome: &str| -> Option<usize> {
        cabecalho.lines().find_map(|l| {
            l.trim()
                .strip_prefix(nome)?
                .trim()
                .parse::<i64>()
                .ok()
                .filter(|&v| v >= 0)
                .map(|v| v as usize)
        })
    };
    let (ini, fim) = (campo("StartFragment:")?, campo("EndFragment:")?);
    let fim = fim.min(cf.len());
    (ini <= fim).then(|| cf[ini..fim].to_vec())
}

/// Embrulha um fragmento HTML no `"HTML Format"` do Windows.
pub fn fragmento_para_cf_html(fragmento: &[u8]) -> Vec<u8> {
    const MODELO: &str = "Version:0.9\r\nStartHTML:0000000000\r\nEndHTML:0000000000\r\n\
                          StartFragment:0000000000\r\nEndFragment:0000000000\r\n";
    let antes = b"<html><body>\r\n<!--StartFragment-->";
    let depois = b"<!--EndFragment-->\r\n</body></html>";
    let inicio_html = MODELO.len();
    let inicio_frag = inicio_html + antes.len();
    let fim_frag = inicio_frag + fragmento.len();
    let fim_html = fim_frag + depois.len();
    let mut out = format!(
        "Version:0.9\r\nStartHTML:{inicio_html:010}\r\nEndHTML:{fim_html:010}\r\n\
         StartFragment:{inicio_frag:010}\r\nEndFragment:{fim_frag:010}\r\n"
    )
    .into_bytes();
    out.extend_from_slice(antes);
    out.extend_from_slice(fragmento);
    out.extend_from_slice(depois);
    out
}

/// `CF_UNICODETEXT` (UTF-16LE em u16, até o primeiro `\0`) → UTF-8 com `\n`.
pub fn utf16_para_texto(u: &[u16]) -> Vec<u8> {
    let fim = u.iter().position(|&c| c == 0).unwrap_or(u.len());
    String::from_utf16_lossy(&u[..fim])
        .replace("\r\n", "\n")
        .into_bytes()
}

/// UTF-8 → `CF_UNICODETEXT` com `\r\n` e o `\0` final.
pub fn texto_para_utf16(t: &[u8]) -> Vec<u16> {
    let s = String::from_utf8_lossy(t).replace("\r\n", "\n");
    let mut u: Vec<u16> = s.replace('\n', "\r\n").encode_utf16().collect();
    u.push(0);
    u
}

#[cfg(test)]
mod tests {
    use super::*;

    fn imagem() -> Rgba {
        // 3×2: cores diferentes e um pixel meio transparente.
        let pixels = vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 128, //
            10, 20, 30, 255, 40, 50, 60, 255, 70, 80, 90, 255,
        ];
        Rgba {
            largura: 3,
            altura: 2,
            pixels,
        }
    }

    #[test]
    fn dib_ida_e_volta() {
        let dib = rgba_para_dib(&imagem());
        // BI_RGB 32 bits não tem máscara de alfa: lido de volta fica opaco.
        let mut opaca = imagem();
        for p in opaca.pixels.chunks_exact_mut(4) {
            p[3] = 255;
        }
        assert_eq!(dib_para_rgba(&dib), Some(opaca));
    }

    #[test]
    fn dib_24_bits_de_cima_para_baixo() {
        // 2×1, altura negativa, linha com preenchimento até 4 bytes.
        let mut dib = Vec::new();
        dib.extend_from_slice(&40u32.to_le_bytes());
        dib.extend_from_slice(&2i32.to_le_bytes());
        dib.extend_from_slice(&(-1i32).to_le_bytes());
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&24u16.to_le_bytes());
        dib.extend_from_slice(&[0; 24]);
        dib.extend_from_slice(&[1, 2, 3, 4, 5, 6, 0, 0]);
        let img = dib_para_rgba(&dib).unwrap();
        assert_eq!(img.pixels, vec![3, 2, 1, 255, 6, 5, 4, 255]);
    }

    #[test]
    fn dib_v5_com_alfa() {
        // BITMAPV5HEADER (124 bytes), BI_BITFIELDS com máscara de alfa, 1×1.
        let mut dib = vec![0u8; 124];
        dib[0..4].copy_from_slice(&124u32.to_le_bytes());
        dib[4..8].copy_from_slice(&1i32.to_le_bytes());
        dib[8..12].copy_from_slice(&1i32.to_le_bytes());
        dib[12..14].copy_from_slice(&1u16.to_le_bytes());
        dib[14..16].copy_from_slice(&32u16.to_le_bytes());
        dib[16..20].copy_from_slice(&BI_BITFIELDS.to_le_bytes());
        dib[40..44].copy_from_slice(&0x00ff_0000u32.to_le_bytes());
        dib[44..48].copy_from_slice(&0x0000_ff00u32.to_le_bytes());
        dib[48..52].copy_from_slice(&0x0000_00ffu32.to_le_bytes());
        dib[52..56].copy_from_slice(&0xff00_0000u32.to_le_bytes());
        dib.extend_from_slice(&[30, 20, 10, 64]);
        assert_eq!(dib_para_rgba(&dib).unwrap().pixels, vec![10, 20, 30, 64]);
    }

    #[test]
    fn dib_ruim_e_recusado() {
        assert_eq!(dib_para_rgba(&[0; 10]), None);
        let mut dib = rgba_para_dib(&imagem());
        dib.truncate(50);
        assert_eq!(dib_para_rgba(&dib), None);
    }

    #[test]
    fn png_ida_e_volta() {
        let img = imagem();
        let png = rgba_para_png(&img).unwrap();
        assert_eq!(&png[1..4], b"PNG");
        assert_eq!(png_para_rgba(&png).unwrap(), img);
        assert_eq!(png_para_rgba(b"nada"), None);
    }

    #[test]
    fn cf_html_ida_e_volta() {
        let frag = "<b>olá</b> mundo".as_bytes();
        let cf = fragmento_para_cf_html(frag);
        assert_eq!(cf_html_para_fragmento(&cf).unwrap(), frag);
        let texto = String::from_utf8(cf).unwrap();
        assert!(texto.starts_with("Version:0.9\r\nStartHTML:0000000"));
        assert_eq!(cf_html_para_fragmento(b"lixo"), None);
    }

    #[test]
    fn texto_utf16() {
        let u = texto_para_utf16("a\nção".as_bytes());
        assert_eq!(*u.last().unwrap(), 0);
        assert_eq!(&u[..3], &[u16::from(b'a'), 13, 10]);
        assert_eq!(utf16_para_texto(&u), "a\nção".as_bytes());
    }
}
