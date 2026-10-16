//! Liga o pipeline de vídeo (`taildesk_media::video`) à conexão QUIC.

use bytes::Bytes;
use taildesk_media::video::SaidaDatagramas;

/// Envia os datagramas de vídeo pela conexão. Datagrama recusado (fila cheia, conexão fechada)
/// é descartado: o controlador percebe a perda e pede um quadro-chave.
#[derive(Debug, Clone)]
pub struct SaidaQuic(pub quinn::Connection);

impl SaidaDatagramas for SaidaQuic {
    fn max_datagrama(&self) -> Option<usize> {
        self.0.max_datagram_size()
    }

    fn enviar(&mut self, datagrama: &[u8]) {
        // O quinn pede `Bytes` próprio: uma cópia pequena por datagrama.
        let _ = self.0.send_datagram(Bytes::copy_from_slice(datagrama));
    }

    fn na_fila(&self) -> usize {
        crate::endpoint::DATAGRAM_BUFFER.saturating_sub(self.0.datagram_send_buffer_space())
    }
}
