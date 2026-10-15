//! Access unit ↔ datagramas de vídeo (`07-protocolo.md` §4).
//!
//! O host corta o access unit em fragmentos de mesmo tamanho (o último pode ser menor), cada um
//! com o header de 16 bytes. O controlador remonta o quadro e só entrega quadros completos e em
//! ordem; depois de uma perda descarta os deltas até o próximo quadro-chave.

use taildesk_proto::media::{Flags, HEADER_LEN, Kind, MediaHeader, seq_newer};

/// Corta access units em datagramas, reaproveitando um buffer.
#[derive(Debug)]
pub struct Fragmentador {
    stream_id: u8,
    frame_seq: u16,
    buf: Vec<u8>,
}

impl Fragmentador {
    pub fn novo(stream_id: u8) -> Self {
        Fragmentador {
            stream_id,
            frame_seq: 0,
            buf: Vec::new(),
        }
    }

    /// Chama `enviar` com cada datagrama do access unit. `max_datagrama` é o
    /// `max_datagram_size()` atual da conexão. Devolve `false` sem enviar nada se o quadro não
    /// cabe em `u16::MAX` fragmentos ou o datagrama não comporta o header.
    pub fn fragmentar(
        &mut self,
        au: &[u8],
        chave: bool,
        pts_us: u32,
        max_datagrama: usize,
        mut enviar: impl FnMut(&[u8]),
    ) -> bool {
        let Some(carga) = max_datagrama.checked_sub(HEADER_LEN).filter(|&c| c > 0) else {
            return false;
        };
        let Ok(total) = u16::try_from(au.len().div_ceil(carga).max(1)) else {
            return false;
        };
        let seq = self.frame_seq;
        self.frame_seq = seq.wrapping_add(1);
        let mut flags = Flags::empty();
        if chave {
            flags = flags.with(Flags::KEY);
        }
        for idx in 0..total {
            let ini = idx as usize * carga;
            let pedaco = &au[ini..(ini + carga).min(au.len())];
            let mut f = flags;
            if idx + 1 == total {
                f = f.with(Flags::LAST);
            }
            self.buf.clear();
            self.buf.resize(HEADER_LEN, 0);
            MediaHeader {
                kind: Kind::Video,
                flags: f,
                stream_id: self.stream_id,
                frame_seq: seq,
                frag_idx: idx,
                frag_count: total,
                pts_us,
                fec_group: 0,
                codec_aux: 0,
            }
            .write(&mut self.buf);
            self.buf.extend_from_slice(pedaco);
            enviar(&self.buf);
        }
        true
    }
}

/// O que aconteceu com um datagrama recebido.
#[derive(Debug, PartialEq, Eq)]
pub enum Remontagem {
    /// Fragmento guardado; o quadro ainda não está completo.
    Parcial,
    /// Quadro completo, decodificável: os bytes estão em [`Remontador::quadro`].
    Completo { chave: bool, pts_us: u32 },
    /// Quadro perdido ou descartado: o controlador deve pedir um quadro-chave.
    PedirChave,
    /// Datagrama atrasado, repetido, de outro stream ou malformado: ignorado.
    Ignorado,
}

/// Limite de memória de um quadro em remontagem (64 MiB cobrem um IDR 4K com folga).
const MAX_QUADRO: usize = 64 << 20;

/// Remonta os quadros de um stream de vídeo.
#[derive(Debug)]
pub struct Remontador {
    stream_id: u8,
    /// Quadro em remontagem.
    seq: Option<u16>,
    total: u16,
    recebidos: u16,
    marcados: Vec<bool>,
    chave: bool,
    pts_us: u32,
    /// Tamanho dos fragmentos não finais; `None` até chegar um.
    carga: Option<usize>,
    dados: Vec<u8>,
    /// Último fragmento, guardado à parte enquanto a carga não é conhecida.
    ultimo: Vec<u8>,
    tem_ultimo: bool,
    /// Último quadro entregue (ou descartado de propósito).
    entregue: Option<u16>,
    /// Depois de uma perda, só um quadro-chave volta a entregar.
    esperando_chave: bool,
    tamanho_quadro: usize,
}

impl Remontador {
    pub fn novo(stream_id: u8) -> Self {
        Remontador {
            stream_id,
            seq: None,
            total: 0,
            recebidos: 0,
            marcados: Vec::new(),
            chave: false,
            pts_us: 0,
            carga: None,
            dados: Vec::new(),
            ultimo: Vec::new(),
            tem_ultimo: false,
            entregue: None,
            esperando_chave: true,
            tamanho_quadro: 0,
        }
    }

    /// O access unit do último [`Remontagem::Completo`].
    pub fn quadro(&self) -> &[u8] {
        &self.dados[..self.tamanho_quadro]
    }

    /// Volta a esperar um quadro-chave (o decoder falhou, o monitor foi reiniciado).
    pub fn esperar_chave(&mut self) {
        self.esperando_chave = true;
        self.seq = None;
    }

    pub fn receber(&mut self, datagrama: &[u8]) -> Remontagem {
        let Ok((h, carga)) = MediaHeader::parse(datagrama) else {
            return Remontagem::Ignorado;
        };
        if h.kind != Kind::Video || h.stream_id != self.stream_id {
            return Remontagem::Ignorado;
        }
        if self.entregue.is_some_and(|e| !seq_newer(h.frame_seq, e)) {
            return Remontagem::Ignorado;
        }
        let mut perdeu = false;
        match self.seq {
            Some(s) if s == h.frame_seq => {
                if h.frag_count != self.total {
                    return Remontagem::Ignorado;
                }
            }
            Some(s) if !seq_newer(h.frame_seq, s) => return Remontagem::Ignorado,
            atual => {
                // Quadro novo: o anterior, se incompleto, se perdeu.
                perdeu = atual.is_some();
                self.comecar(&h);
            }
        }
        let idx = h.frag_idx as usize;
        if self.marcados[idx] {
            return if perdeu {
                Remontagem::PedirChave
            } else {
                Remontagem::Ignorado
            };
        }
        let ultimo = h.frag_idx + 1 == h.frag_count;
        if !self.guardar(idx, ultimo, carga) {
            self.seq = None;
            self.esperando_chave = true;
            return Remontagem::PedirChave;
        }
        self.marcados[idx] = true;
        self.recebidos += 1;
        if self.recebidos < self.total {
            return if perdeu {
                self.esperando_chave = true;
                Remontagem::PedirChave
            } else {
                Remontagem::Parcial
            };
        }
        self.completar(perdeu)
    }

    fn comecar(&mut self, h: &MediaHeader) {
        self.seq = Some(h.frame_seq);
        self.total = h.frag_count;
        self.recebidos = 0;
        self.marcados.clear();
        self.marcados.resize(h.frag_count as usize, false);
        self.chave = h.flags.contains(Flags::KEY);
        self.pts_us = h.pts_us;
        self.carga = None;
        self.tem_ultimo = false;
        self.tamanho_quadro = 0;
    }

    /// Copia o fragmento para o lugar dele; `false` se ele contradiz os anteriores.
    fn guardar(&mut self, idx: usize, ultimo: bool, carga: &[u8]) -> bool {
        if ultimo && self.total > 1 {
            if self.carga.is_some_and(|c| carga.len() > c) {
                return false;
            }
            self.ultimo.clear();
            self.ultimo.extend_from_slice(carga);
            self.tem_ultimo = true;
            return true;
        }
        match self.carga {
            Some(c) if c != carga.len() => return false,
            Some(_) => {}
            None => {
                if carga.is_empty() && self.total > 1 {
                    return false;
                }
                if self.tem_ultimo && self.ultimo.len() > carga.len() {
                    return false;
                }
                let Some(n) = carga.len().checked_mul(self.total as usize) else {
                    return false;
                };
                if n > MAX_QUADRO {
                    return false;
                }
                self.carga = Some(carga.len());
                if self.dados.len() < n {
                    self.dados.resize(n, 0);
                }
            }
        }
        let c = self.carga.unwrap_or(0);
        self.dados[idx * c..idx * c + carga.len()].copy_from_slice(carga);
        true
    }

    fn completar(&mut self, perdeu: bool) -> Remontagem {
        let seq = self.seq.take().unwrap_or(0);
        let c = self.carga.unwrap_or(0);
        let fim = (self.total as usize - 1) * c;
        if self.total == 1 {
            // Quadro de um fragmento só: veio por `guardar` como não último.
            self.tamanho_quadro = c;
        } else {
            let n = fim + self.ultimo.len();
            if self.dados.len() < n {
                self.dados.resize(n, 0);
            }
            self.dados[fim..n].copy_from_slice(&self.ultimo);
            self.tamanho_quadro = n;
        }
        let pulou = self.entregue.is_some_and(|e| seq != e.wrapping_add(1));
        self.entregue = Some(seq);
        if self.chave {
            self.esperando_chave = false;
        } else if perdeu || pulou {
            self.esperando_chave = true;
        }
        if self.esperando_chave {
            return Remontagem::PedirChave;
        }
        Remontagem::Completo {
            chave: self.chave,
            pts_us: self.pts_us,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn datagramas(f: &mut Fragmentador, au: &[u8], chave: bool, max: usize) -> Vec<Vec<u8>> {
        let mut v = Vec::new();
        assert!(f.fragmentar(au, chave, 7, max, |d| v.push(d.to_vec())));
        v
    }

    fn au(n: usize, semente: u8) -> Vec<u8> {
        (0..n)
            .map(|i| (i as u8).wrapping_mul(31) ^ semente)
            .collect()
    }

    #[test]
    fn ida_e_volta_em_qualquer_ordem() {
        let mut f = Fragmentador::novo(3);
        let mut r = Remontador::novo(3);
        for (n, max) in [(1, 100), (84, 100), (85, 100), (5000, 1200), (10_000, 17)] {
            let a = au(n, n as u8);
            let mut ds = datagramas(&mut f, &a, true, max);
            assert!(ds.iter().all(|d| d.len() <= max));
            ds.reverse();
            let mut fim = None;
            for d in &ds {
                fim = Some(r.receber(d));
            }
            assert_eq!(
                fim,
                Some(Remontagem::Completo {
                    chave: true,
                    pts_us: 7
                })
            );
            assert_eq!(r.quadro(), &a[..]);
        }
    }

    #[test]
    fn espera_chave_no_inicio_e_depois_de_perda() {
        let mut f = Fragmentador::novo(0);
        let mut r = Remontador::novo(0);
        // Delta antes de qualquer chave: descartado.
        for d in datagramas(&mut f, &au(300, 1), false, 116) {
            r.receber(&d);
        }
        assert_eq!(r.quadro().len(), 300);
        let ok = |r: &mut Remontador, ds: &[Vec<u8>]| ds.iter().map(|d| r.receber(d)).last();
        let chave = datagramas(&mut f, &au(300, 2), true, 116);
        assert!(matches!(
            ok(&mut r, &chave),
            Some(Remontagem::Completo { chave: true, .. })
        ));
        let delta = datagramas(&mut f, &au(300, 3), false, 116);
        assert!(matches!(
            ok(&mut r, &delta),
            Some(Remontagem::Completo { chave: false, .. })
        ));
        // Perde um fragmento do próximo delta; o seguinte denuncia a perda.
        let mut perdido = datagramas(&mut f, &au(300, 4), false, 116);
        perdido.remove(1);
        assert_eq!(ok(&mut r, &perdido), Some(Remontagem::Parcial));
        let delta = datagramas(&mut f, &au(300, 5), false, 116);
        assert_eq!(r.receber(&delta[0]), Remontagem::PedirChave);
        assert_eq!(ok(&mut r, &delta[1..]), Some(Remontagem::PedirChave));
        let chave = datagramas(&mut f, &au(300, 6), true, 116);
        assert!(matches!(
            ok(&mut r, &chave),
            Some(Remontagem::Completo { chave: true, .. })
        ));
        assert_eq!(r.quadro(), &au(300, 6)[..]);
    }

    #[test]
    fn quadro_inteiro_perdido_vira_pedido_de_chave() {
        let mut f = Fragmentador::novo(0);
        let mut r = Remontador::novo(0);
        for d in datagramas(&mut f, &au(50, 1), true, 1200) {
            r.receber(&d);
        }
        let _sumiu = datagramas(&mut f, &au(50, 2), false, 1200);
        let d = datagramas(&mut f, &au(50, 3), false, 1200);
        assert_eq!(r.receber(&d[0]), Remontagem::PedirChave);
    }

    #[test]
    fn ignora_atrasado_repetido_e_outro_stream() {
        let mut f = Fragmentador::novo(1);
        let mut r = Remontador::novo(1);
        let a = datagramas(&mut f, &au(50, 1), true, 1200);
        let b = datagramas(&mut f, &au(50, 2), false, 1200);
        assert!(matches!(r.receber(&a[0]), Remontagem::Completo { .. }));
        assert!(matches!(r.receber(&b[0]), Remontagem::Completo { .. }));
        assert_eq!(r.receber(&a[0]), Remontagem::Ignorado);
        assert_eq!(r.receber(&b[0]), Remontagem::Ignorado);
        let mut outro = Fragmentador::novo(2);
        assert_eq!(
            r.receber(&datagramas(&mut outro, &au(5, 0), true, 1200)[0]),
            Remontagem::Ignorado
        );
        assert_eq!(r.receber(&[0; 3]), Remontagem::Ignorado);
    }

    #[test]
    fn fragmentos_incoerentes_pedem_chave() {
        let mut f = Fragmentador::novo(0);
        let mut r = Remontador::novo(0);
        let mut ds = datagramas(&mut f, &au(300, 1), true, 116);
        ds[1].pop();
        assert_eq!(r.receber(&ds[0]), Remontagem::Parcial);
        assert_eq!(r.receber(&ds[1]), Remontagem::PedirChave);
    }

    #[test]
    fn fragmentador_recusa_datagrama_pequeno_demais() {
        let mut f = Fragmentador::novo(0);
        assert!(!f.fragmentar(&[1, 2, 3], true, 0, HEADER_LEN, |_| unreachable!()));
    }
}
