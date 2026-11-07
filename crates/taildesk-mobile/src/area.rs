//! Área de transferência de texto na sessão do celular (M3, `requisitos.md` §5: "Clipboard
//! texto"), sobre a mesma lógica do desktop (`taildesk_session::clipboard`).
//!
//! O protocolo é preguiçoso: quem copia anuncia e o outro lado só pede quando alguém cola. O
//! Android não avisa quando uma pessoa cola em outro app, então o celular faz assim:
//! - texto copiado no computador: o celular pede na hora e põe na área de transferência do
//!   Android (só texto, até [`LIMITE_TEXTO`]);
//! - texto copiado no celular: ao abrir a sessão e sempre que o app volta à frente (o Android só
//!   deixa ler com o app em foco), se mudou, o celular anuncia; o computador pede ao colar.
//!
//! O que veio do computador não volta para ele como oferta nova (sem laço).

use taildesk_proto::control::{ClipErrorCode, ClipFormat, Msg};
use taildesk_session::clipboard::{AreaTransferencia, Copiado, Evento};

/// Maior texto que o celular busca sozinho do computador.
pub const LIMITE_TEXTO: u64 = 1024 * 1024;

/// Estado da área de transferência de uma sessão.
#[derive(Debug, Default)]
pub struct AreaCelular {
    area: AreaTransferencia,
    /// A sessão tem `Permissions.clipboard`.
    permitido: bool,
    /// O que anunciamos (para responder quando o computador colar).
    meu: Option<String>,
    /// O último texto que trocamos (de lá ou daqui), para não anunciar de volta.
    ultimo: Option<String>,
}

/// O que o app faz depois de uma mensagem do host.
#[derive(Debug, Default, PartialEq)]
pub struct Saida {
    pub enviar: Vec<Msg>,
    /// Texto para pôr na área de transferência do Android.
    pub copiar: Option<String>,
}

impl AreaCelular {
    pub fn new(permitido: bool) -> AreaCelular {
        AreaCelular {
            permitido,
            ..AreaCelular::default()
        }
    }

    /// Texto que está agora na área de transferência do Android. Devolve a oferta para o host
    /// se é novo (nem o último daqui, nem o que veio de lá).
    pub fn texto_local(&mut self, texto: &str) -> Option<Msg> {
        if !self.permitido || texto.is_empty() || self.ultimo.as_deref() == Some(texto) {
            return None;
        }
        let oferta = self.area.copiado(&Copiado {
            formatos: vec![(ClipFormat::Text, texto.len() as u64)],
            ..Copiado::default()
        });
        self.meu = Some(texto.to_owned());
        self.ultimo = Some(texto.to_owned());
        Some(oferta)
    }

    /// Mensagem de área de transferência do host (`ClipboardOffer`/`Request`/`Data`/`Error`).
    pub fn do_host(&mut self, msg: &Msg) -> Saida {
        let mut s = Saida::default();
        let (ev, resposta) = self.area.receber(msg, self.permitido);
        s.enviar.extend(resposta);
        match ev {
            Some(Evento::Oferta { seq, copiado }) => {
                let texto = copiado
                    .formatos
                    .iter()
                    .find(|(f, _)| *f == ClipFormat::Text)
                    .map(|(_, n)| *n);
                // Tamanho desconhecido (0) também vale: o limite do protocolo segura o resto.
                if let Some(n) = texto
                    && n <= LIMITE_TEXTO
                {
                    s.enviar.extend(self.area.colar(seq, ClipFormat::Text));
                }
            }
            Some(Evento::Ler { seq, formato }) => {
                let dados = match (&self.meu, formato) {
                    (Some(t), ClipFormat::Text) => Ok(t.clone().into_bytes()),
                    _ => Err(ClipErrorCode::Unavailable),
                };
                s.enviar.extend(self.area.responder(seq, formato, dados));
            }
            Some(Evento::Recebido {
                formato: ClipFormat::Text,
                dados,
                ..
            }) => {
                let t = String::from_utf8_lossy(&dados).into_owned();
                if !t.is_empty() && u64::try_from(t.len()).is_ok_and(|n| n <= LIMITE_TEXTO) {
                    self.ultimo = Some(t.clone());
                    s.copiar = Some(t);
                }
            }
            Some(Evento::Recebido { .. } | Evento::Falhou { .. }) | None => {}
        }
        s
    }
}

/// Mensagem que vai para a [`AreaCelular`].
pub fn e_da_area(m: &Msg) -> bool {
    matches!(
        m,
        Msg::ClipboardOffer(_)
            | Msg::ClipboardRequest(_)
            | Msg::ClipboardData(_)
            | Msg::ClipboardError(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Leva as mensagens pelo fio, como na sessão.
    fn fio(m: Msg) -> Msg {
        let mut b = Vec::new();
        taildesk_proto::control::encode_frame(&m.into(), &mut b);
        let (c, _) = taildesk_proto::control::decode_frame(&b).unwrap().unwrap();
        c.msg.unwrap()
    }

    /// Entrega ao "computador" (outra `AreaTransferencia`) e devolve as respostas dele.
    fn no_pc(pc: &mut AreaTransferencia, msgs: Vec<Msg>, conteudo: &str) -> Vec<Msg> {
        let mut volta = Vec::new();
        for m in msgs {
            let (ev, r) = pc.receber(&fio(m), true);
            volta.extend(r);
            if let Some(Evento::Ler { seq, formato }) = ev {
                volta.extend(pc.responder(seq, formato, Ok(conteudo.as_bytes().to_vec())));
            }
        }
        volta
    }

    #[test]
    fn texto_do_computador_vai_para_o_android() {
        let mut cel = AreaCelular::new(true);
        let mut pc = AreaTransferencia::default();
        let oferta = pc.copiado(&Copiado {
            formatos: vec![(ClipFormat::Text, 5), (ClipFormat::Html, 20)],
            ..Copiado::default()
        });
        let s = cel.do_host(&fio(oferta));
        assert!(matches!(&s.enviar[..], [Msg::ClipboardRequest(_)]));
        let mut copiado = None;
        for m in no_pc(&mut pc, s.enviar, "olá!") {
            let s = cel.do_host(&fio(m));
            copiado = copiado.or(s.copiar);
        }
        assert_eq!(copiado.as_deref(), Some("olá!"));
        // O que veio de lá não volta como oferta.
        assert!(cel.texto_local("olá!").is_none());
    }

    #[test]
    fn texto_do_celular_e_anunciado_e_entregue() {
        let mut cel = AreaCelular::new(true);
        let mut pc = AreaTransferencia::default();
        let oferta = cel.texto_local("senha do wifi").unwrap();
        // Mesmo texto de novo (o app voltou à frente): nada.
        assert!(cel.texto_local("senha do wifi").is_none());
        let (ev, _) = pc.receber(&fio(oferta), true);
        let Some(Evento::Oferta { seq, .. }) = ev else {
            panic!("{ev:?}")
        };
        let pedido = pc.colar(seq, ClipFormat::Text).unwrap();
        let s = cel.do_host(&fio(pedido));
        let mut recebido = None;
        for m in s.enviar {
            if let (Some(Evento::Recebido { dados, .. }), _) = pc.receber(&fio(m), true) {
                recebido = Some(String::from_utf8(dados).unwrap());
            }
        }
        assert_eq!(recebido.as_deref(), Some("senha do wifi"));
    }

    #[test]
    fn sem_permissao_e_grande_demais() {
        let mut cel = AreaCelular::new(false);
        assert!(cel.texto_local("x").is_none());
        let mut pc = AreaTransferencia::default();
        let s = cel.do_host(&fio(pc.copiado(&Copiado {
            formatos: vec![(ClipFormat::Text, 3)],
            ..Copiado::default()
        })));
        assert!(s.enviar.is_empty() && s.copiar.is_none());

        let mut cel = AreaCelular::new(true);
        let s = cel.do_host(&fio(pc.copiado(&Copiado {
            formatos: vec![(ClipFormat::Text, LIMITE_TEXTO + 1)],
            ..Copiado::default()
        })));
        assert!(s.enviar.is_empty());
        // Imagem só: o celular não busca.
        let s = cel.do_host(&fio(pc.copiado(&Copiado {
            formatos: vec![(ClipFormat::Png, 10)],
            ..Copiado::default()
        })));
        assert!(s.enviar.is_empty());
        assert!(cel.texto_local("").is_none());
    }
}
