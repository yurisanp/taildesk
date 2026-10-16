//! Área de transferência compartilhada (fase 4), igual nos dois lados da sessão.
//!
//! Quem copia anuncia os formatos ([`AreaTransferencia::copiado`] → `ClipboardOffer`); o outro
//! lado põe a oferta no sistema sem o conteúdo e só pede quando alguém cola
//! ([`AreaTransferencia::colar`] → `ClipboardRequest`). Quem recebe o pedido lê do sistema e
//! responde em pedaços ([`AreaTransferencia::responder`]), e quem pediu remonta e entrega
//! ([`Evento::Recebido`]). Lógica pura: o código de SO fica em `taildesk-platform` e o laço de
//! rede em [`crate::conexao`].
//!
//! Para não entrar em laço, só se anuncia o que alguém copiou aqui: a cópia que veio do peer
//! (a nossa oferta no sistema) não volta como `ClipboardOffer`. Isso é do backend de SO, que
//! sabe quando o dono da área de transferência é o próprio TailDesk.

use std::collections::HashMap;

use taildesk_proto::control::{
    ClipErrorCode, ClipFile, ClipFormat, ClipboardData, ClipboardError, ClipboardOffer,
    ClipboardRequest, Msg, limits,
};

/// O que foi copiado aqui: formatos com o tamanho, se conhecido (0 = não sei), e os arquivos.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Copiado {
    pub formatos: Vec<(ClipFormat, u64)>,
    pub arquivos: Vec<ClipFile>,
    /// Caminho local de cada item de `arquivos` (vazio no que veio do peer).
    pub locais: Vec<std::path::PathBuf>,
}

/// O que a sessão precisa fazer depois de uma mensagem do peer.
#[derive(Clone, Debug, PartialEq)]
pub enum Evento {
    /// O peer copiou algo: ponha esta oferta na área de transferência daqui (sem o conteúdo).
    /// Lista vazia = o peer esvaziou a dele.
    Oferta { seq: u32, copiado: Copiado },
    /// O peer colou o que copiamos: leia `formato` do sistema e chame
    /// [`AreaTransferencia::responder`] com o mesmo `seq`.
    Ler { seq: u32, formato: ClipFormat },
    /// Chegou tudo do que alguém daqui colou.
    Recebido {
        seq: u32,
        formato: ClipFormat,
        dados: Vec<u8>,
    },
    /// O colar não vai acontecer (o peer copiou outra coisa, não tem permissão, era grande
    /// demais, o app de lá fechou ou os pedaços vieram errados).
    Falhou {
        seq: u32,
        formato: ClipFormat,
        codigo: ClipErrorCode,
    },
}

#[derive(Debug, Default)]
struct Remontagem {
    total: Option<u64>,
    dados: Vec<u8>,
}

/// Estado da área de transferência de uma sessão.
#[derive(Debug, Default)]
pub struct AreaTransferencia {
    /// `seq` da última oferta nossa (0 = nenhuma ainda).
    meu_seq: u32,
    /// Formatos da nossa oferta em vigor, para recusar pedido do que não oferecemos.
    meus_formatos: Vec<ClipFormat>,
    /// `seq` da oferta do peer em vigor.
    seq_peer: Option<u32>,
    /// Colagens pedidas e ainda não completas.
    pedidos: HashMap<(u32, ClipFormat), Remontagem>,
}

fn formato(v: i32) -> ClipFormat {
    ClipFormat::try_from(v).unwrap_or(ClipFormat::Unspecified)
}

fn erro(seq: u32, formato: ClipFormat, codigo: ClipErrorCode) -> Msg {
    Msg::ClipboardError(ClipboardError {
        seq,
        format: formato.into(),
        code: codigo.into(),
    })
}

impl AreaTransferencia {
    /// Alguém copiou aqui: a oferta para mandar ao peer. Pedidos de ofertas anteriores passam a
    /// receber `GONE`.
    pub fn copiado(&mut self, c: &Copiado) -> Msg {
        self.meu_seq = self.meu_seq.wrapping_add(1).max(1);
        let formatos: Vec<(ClipFormat, u64)> = c
            .formatos
            .iter()
            .copied()
            .filter(|(f, _)| *f != ClipFormat::Unspecified)
            .take(limits::CLIP_FORMATS)
            .collect();
        self.meus_formatos = formatos.iter().map(|(f, _)| *f).collect();
        let arquivos = if self.meus_formatos.contains(&ClipFormat::Files) {
            c.arquivos
                .iter()
                .take(limits::CLIP_FILES)
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        Msg::ClipboardOffer(ClipboardOffer {
            seq: self.meu_seq,
            formats: formatos.iter().map(|(f, _)| (*f).into()).collect(),
            sizes: formatos.iter().map(|(_, n)| *n).collect(),
            files: arquivos,
        })
    }

    /// `seq` da oferta nossa em vigor (para a raiz de arquivos da área de transferência).
    pub fn meu_seq(&self) -> u32 {
        self.meu_seq
    }

    /// Alguém daqui colou o que o peer copiou. `None` se a oferta `seq` já não vale (o peer
    /// copiou outra coisa): cole vazio. Pedir de novo o mesmo formato enquanto o primeiro não
    /// chegou não manda outro pedido.
    pub fn colar(&mut self, seq: u32, formato: ClipFormat) -> Option<Msg> {
        if self.seq_peer != Some(seq) || formato == ClipFormat::Unspecified {
            return None;
        }
        if self.pedidos.contains_key(&(seq, formato)) {
            return None;
        }
        self.pedidos.insert((seq, formato), Remontagem::default());
        Some(Msg::ClipboardRequest(ClipboardRequest {
            seq,
            format: formato.into(),
        }))
    }

    /// Resposta a um [`Evento::Ler`]: o conteúdo em pedaços, ou o erro. Oferta já trocada
    /// responde `GONE`.
    pub fn responder(
        &self,
        seq: u32,
        formato: ClipFormat,
        dados: Result<Vec<u8>, ClipErrorCode>,
    ) -> Pedacos {
        let resultado = if seq != self.meu_seq || !self.meus_formatos.contains(&formato) {
            Err(ClipErrorCode::Gone)
        } else {
            dados.and_then(|d| {
                if d.len() as u64 > limits::CLIP_TOTAL {
                    Err(ClipErrorCode::TooBig)
                } else {
                    Ok(d)
                }
            })
        };
        Pedacos {
            seq,
            formato,
            resultado,
            offset: 0,
            fim: false,
        }
    }

    /// Mensagem de área de transferência vinda do peer. `permitido` = a sessão tem
    /// `Permissions.clipboard`; sem ela os pedidos voltam `DENIED` e as ofertas são ignoradas.
    /// Devolve o evento para a sessão e, se for o caso, a resposta imediata ao peer.
    pub fn receber(&mut self, msg: &Msg, permitido: bool) -> (Option<Evento>, Option<Msg>) {
        match msg {
            Msg::ClipboardOffer(o) if permitido => {
                // Colagens pendentes da oferta anterior continuam: o peer responde cada pedido
                // (o conteúdo ou `GONE`), na ordem do stream de controle.
                self.seq_peer = Some(o.seq);
                let formatos = o
                    .formats
                    .iter()
                    .enumerate()
                    .map(|(i, &f)| (formato(f), o.sizes.get(i).copied().unwrap_or(0)))
                    .filter(|(f, _)| *f != ClipFormat::Unspecified)
                    .collect();
                (
                    Some(Evento::Oferta {
                        seq: o.seq,
                        copiado: Copiado {
                            formatos,
                            arquivos: o.files.clone(),
                            locais: Vec::new(),
                        },
                    }),
                    None,
                )
            }
            Msg::ClipboardRequest(r) => {
                let f = formato(r.format);
                if !permitido {
                    return (None, Some(erro(r.seq, f, ClipErrorCode::Denied)));
                }
                if r.seq != self.meu_seq || !self.meus_formatos.contains(&f) {
                    return (None, Some(erro(r.seq, f, ClipErrorCode::Gone)));
                }
                (
                    Some(Evento::Ler {
                        seq: r.seq,
                        formato: f,
                    }),
                    None,
                )
            }
            Msg::ClipboardData(d) => (self.pedaco(d), None),
            Msg::ClipboardError(e) => {
                let f = formato(e.format);
                self.pedidos.remove(&(e.seq, f)).map_or((None, None), |_| {
                    (
                        Some(Evento::Falhou {
                            seq: e.seq,
                            formato: f,
                            codigo: ClipErrorCode::try_from(e.code)
                                .unwrap_or(ClipErrorCode::Unavailable),
                        }),
                        None,
                    )
                })
            }
            _ => (None, None),
        }
    }

    fn pedaco(&mut self, d: &ClipboardData) -> Option<Evento> {
        let chave = (d.seq, formato(d.format));
        // Pedaço de algo que não pedimos (ou que já desistimos): ignora.
        let r = self.pedidos.get_mut(&chave)?;
        let consistente = r.total.is_none_or(|t| t == d.total)
            && d.offset == r.dados.len() as u64
            && d.total <= limits::CLIP_TOTAL
            && d.offset + d.data.len() as u64 <= d.total;
        if !consistente {
            self.pedidos.remove(&chave);
            return Some(Evento::Falhou {
                seq: chave.0,
                formato: chave.1,
                codigo: ClipErrorCode::Unavailable,
            });
        }
        if r.total.is_none() {
            r.total = Some(d.total);
            r.dados.reserve_exact(d.total as usize);
        }
        r.dados.extend_from_slice(&d.data);
        if (r.dados.len() as u64) < d.total {
            return None;
        }
        let r = self.pedidos.remove(&chave)?;
        Some(Evento::Recebido {
            seq: chave.0,
            formato: chave.1,
            dados: r.dados,
        })
    }
}

/// Os `ClipboardData` de uma resposta, um pedaço por vez (ou um `ClipboardError`).
#[derive(Debug)]
pub struct Pedacos {
    seq: u32,
    formato: ClipFormat,
    resultado: Result<Vec<u8>, ClipErrorCode>,
    offset: usize,
    fim: bool,
}

impl Iterator for Pedacos {
    type Item = Msg;

    fn next(&mut self) -> Option<Msg> {
        if self.fim {
            return None;
        }
        let dados = match &self.resultado {
            Ok(d) => d,
            Err(c) => {
                self.fim = true;
                return Some(erro(self.seq, self.formato, *c));
            }
        };
        let fim = (self.offset + limits::CLIP_CHUNK).min(dados.len());
        let msg = Msg::ClipboardData(ClipboardData {
            seq: self.seq,
            format: self.formato.into(),
            offset: self.offset as u64,
            total: dados.len() as u64,
            data: dados[self.offset..fim].to_vec(),
        });
        self.offset = fim;
        self.fim = fim == dados.len();
        Some(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texto(n: u64) -> Copiado {
        Copiado {
            formatos: vec![(ClipFormat::Text, n)],
            arquivos: Vec::new(),
            locais: Vec::new(),
        }
    }

    /// Leva as mensagens de `de` para `para` até não sobrar nada; devolve os eventos de `para`.
    fn entregar(
        para: &mut AreaTransferencia,
        msgs: impl IntoIterator<Item = Msg>,
    ) -> (Vec<Evento>, Vec<Msg>) {
        let (mut ev, mut resp) = (Vec::new(), Vec::new());
        for m in msgs {
            // Ida e volta pelo fio, como na sessão.
            let mut fio = Vec::new();
            taildesk_proto::control::encode_frame(&m.into(), &mut fio);
            let (c, _) = taildesk_proto::control::decode_frame(&fio)
                .unwrap()
                .unwrap();
            let (e, r) = para.receber(&c.msg.unwrap(), true);
            ev.extend(e);
            resp.extend(r);
        }
        (ev, resp)
    }

    #[test]
    fn copiar_e_colar_texto_grande_em_pedacos() {
        let (mut host, mut ctl) = (AreaTransferencia::default(), AreaTransferencia::default());
        let conteudo: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        let oferta = host.copiado(&texto(conteudo.len() as u64));
        let (ev, _) = entregar(&mut ctl, [oferta]);
        let Evento::Oferta { seq, copiado } = &ev[0] else {
            panic!("{ev:?}")
        };
        assert_eq!(copiado.formatos, vec![(ClipFormat::Text, 100_000)]);

        let pedido = ctl.colar(*seq, ClipFormat::Text).unwrap();
        // Colar de novo antes de chegar não repete o pedido.
        assert!(ctl.colar(*seq, ClipFormat::Text).is_none());
        let (ev, _) = entregar(&mut host, [pedido]);
        assert_eq!(
            ev,
            vec![Evento::Ler {
                seq: *seq,
                formato: ClipFormat::Text
            }]
        );
        let pedacos: Vec<Msg> = host
            .responder(*seq, ClipFormat::Text, Ok(conteudo.clone()))
            .collect();
        assert_eq!(pedacos.len(), 4);
        let (ev, _) = entregar(&mut ctl, pedacos);
        assert_eq!(
            ev,
            vec![Evento::Recebido {
                seq: *seq,
                formato: ClipFormat::Text,
                dados: conteudo
            }]
        );
    }

    #[test]
    fn conteudo_vazio_e_um_pedaco_so() {
        let mut host = AreaTransferencia::default();
        let Msg::ClipboardOffer(o) = host.copiado(&texto(0)) else {
            panic!()
        };
        let p: Vec<Msg> = host
            .responder(o.seq, ClipFormat::Text, Ok(Vec::new()))
            .collect();
        assert_eq!(p.len(), 1);
        let mut ctl = AreaTransferencia::default();
        entregar(&mut ctl, [Msg::ClipboardOffer(o.clone())]);
        ctl.colar(o.seq, ClipFormat::Text).unwrap();
        let (ev, _) = entregar(&mut ctl, p);
        assert!(matches!(&ev[0], Evento::Recebido { dados, .. } if dados.is_empty()));
    }

    #[test]
    fn oferta_trocada_responde_gone() {
        let (mut host, mut ctl) = (AreaTransferencia::default(), AreaTransferencia::default());
        let primeira = host.copiado(&texto(5));
        entregar(&mut ctl, [primeira]);
        let pedido = ctl.colar(1, ClipFormat::Text).unwrap();
        // Alguém no host copiou outra coisa antes do pedido chegar.
        host.copiado(&texto(9));
        let (ev, resp) = entregar(&mut host, [pedido]);
        assert!(ev.is_empty());
        let (ev, _) = entregar(&mut ctl, resp);
        assert_eq!(
            ev,
            vec![Evento::Falhou {
                seq: 1,
                formato: ClipFormat::Text,
                codigo: ClipErrorCode::Gone
            }]
        );
        // Formato que não foi oferecido também.
        let (_, resp) = entregar(
            &mut host,
            [Msg::ClipboardRequest(ClipboardRequest {
                seq: 2,
                format: ClipFormat::Png.into(),
            })],
        );
        assert!(matches!(&resp[0], Msg::ClipboardError(e) if e.code == ClipErrorCode::Gone as i32));
        // E colar uma oferta velha nem sai daqui.
        assert!(ctl.colar(7, ClipFormat::Text).is_none());
    }

    #[test]
    fn sem_permissao_pedido_e_recusado_e_oferta_ignorada() {
        let mut host = AreaTransferencia::default();
        let Msg::ClipboardOffer(o) = host.copiado(&texto(5)) else {
            panic!()
        };
        let pedido = Msg::ClipboardRequest(ClipboardRequest {
            seq: o.seq,
            format: ClipFormat::Text.into(),
        });
        let (ev, resp) = host.receber(&pedido, false);
        assert!(ev.is_none());
        assert!(
            matches!(resp, Some(Msg::ClipboardError(e)) if e.code == ClipErrorCode::Denied as i32)
        );
        let mut ctl = AreaTransferencia::default();
        assert_eq!(ctl.receber(&Msg::ClipboardOffer(o), false), (None, None));
    }

    #[test]
    fn pedaco_fora_de_ordem_ou_nao_pedido() {
        let (mut host, mut ctl) = (AreaTransferencia::default(), AreaTransferencia::default());
        entregar(&mut ctl, [host.copiado(&texto(70_000))]);
        // Sem pedido, pedaço é ignorado.
        let mut p: Vec<Msg> = host
            .responder(1, ClipFormat::Text, Ok(vec![1; 70_000]))
            .collect();
        assert_eq!(entregar(&mut ctl, p.clone()).0, Vec::new());
        ctl.colar(1, ClipFormat::Text).unwrap();
        p.swap(0, 1);
        let (ev, _) = entregar(&mut ctl, p);
        assert!(matches!(
            &ev[0],
            Evento::Falhou {
                codigo: ClipErrorCode::Unavailable,
                ..
            }
        ));
    }

    #[test]
    fn grande_demais_e_too_big() {
        let mut host = AreaTransferencia::default();
        host.copiado(&texto(0));
        let p: Vec<Msg> = host
            .responder(
                1,
                ClipFormat::Text,
                Ok(vec![0; limits::CLIP_TOTAL as usize + 1]),
            )
            .collect();
        assert!(
            matches!(&p[..], [Msg::ClipboardError(e)] if e.code == ClipErrorCode::TooBig as i32)
        );
    }

    #[test]
    fn arquivos_vao_so_com_o_formato() {
        let mut a = AreaTransferencia::default();
        let arq = ClipFile {
            name: "a.txt".into(),
            size: 3,
            dir: false,
        };
        let Msg::ClipboardOffer(o) = a.copiado(&Copiado {
            formatos: vec![(ClipFormat::Text, 3)],
            arquivos: vec![arq.clone()],
            locais: Vec::new(),
        }) else {
            panic!()
        };
        assert!(o.files.is_empty());
        let Msg::ClipboardOffer(o) = a.copiado(&Copiado {
            formatos: vec![(ClipFormat::Files, 0), (ClipFormat::Text, 5)],
            arquivos: vec![arq.clone()],
            locais: Vec::new(),
        }) else {
            panic!()
        };
        assert_eq!(o.files, vec![arq]);
        assert_eq!(o.sizes, vec![0, 5]);
    }
}
