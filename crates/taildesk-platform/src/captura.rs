//! Tipos comuns da captura de tela: o que cada backend (PipeWire no Linux, DXGI no Windows)
//! entrega por quadro, emprestado e sem alocar.

/// Ordem dos bytes de cada pixel na memória.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormatoPixel {
    /// B, G, R, (ignorado). O mais comum no KDE e no GNOME.
    Bgrx,
    Rgbx,
    Bgra,
    Rgba,
}

/// Quanto a imagem está girada em relação ao que aparece na tela (sentido horário). O DXGI
/// entrega monitores girados na orientação do painel; quem consome gira de volta.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Rotacao {
    #[default]
    Nenhuma,
    Graus90,
    Graus180,
    Graus270,
}

/// O que chegou de um monitor; vale só dentro do callback.
#[derive(Debug)]
pub struct Quadro<'a> {
    pub display_id: u32,
    /// Imagem nova; `None` quando só o cursor mudou.
    pub imagem: Option<Imagem<'a>>,
    /// Cursor, quando o sistema mandou dado novo. O vídeo sai sem cursor (modo metadado do
    /// portal, DXGI sem desenhar o ponteiro): quem consome envia posição e forma separadas para o
    /// controlador desenhar.
    pub cursor: Option<Cursor<'a>>,
}

/// Imagem do monitor, emprestada do buffer mapeado (PipeWire) ou da textura de staging (DXGI).
#[derive(Debug)]
pub struct Imagem<'a> {
    pub largura: u32,
    pub altura: u32,
    /// Bytes por linha (pode ser maior que `largura * 4`).
    pub stride: usize,
    pub formato: FormatoPixel,
    pub rotacao: Rotacao,
    /// `altura` linhas de `stride` bytes; a última pode ter só `largura * 4`.
    pub dados: &'a [u8],
}

/// Posição do cursor no monitor (pixels do stream) e, quando mudou, a forma.
#[derive(Debug)]
pub struct Cursor<'a> {
    /// `false` = cursor escondido (fora deste monitor ou sem imagem).
    pub visivel: bool,
    pub x: i32,
    pub y: i32,
    pub forma: Option<FormaCursor<'a>>,
}

/// Bitmap do cursor; o ponto (`hotspot_x`, `hotspot_y`) do bitmap fica em (`x`, `y`).
#[derive(Debug)]
pub struct FormaCursor<'a> {
    pub largura: u32,
    pub altura: u32,
    pub stride: usize,
    pub hotspot_x: i32,
    pub hotspot_y: i32,
    pub formato: FormatoPixel,
    pub dados: &'a [u8],
}

impl Imagem<'_> {
    /// Uma linha da imagem, só com os pixels visíveis.
    pub fn linha(&self, y: u32) -> &[u8] {
        let ini = y as usize * self.stride;
        &self.dados[ini..ini + self.largura as usize * 4]
    }
}
