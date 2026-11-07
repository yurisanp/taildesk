//! Som do computador no celular (MF-70, `15-audio.md`): o botão 🔊 da barra pede o som; o host
//! manda Opus em datagramas `kind=1`; o celular decodifica e toca pelo AAudio.
//!
//! Desligado (o padrão) = nenhum `StartAudio`, e o host não manda nada (`15` §4.3). Desligar
//! fecha a saída de som do aparelho na hora.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use taildesk_media::audio::RecepcaoAudio;
use taildesk_proto::control::{
    AudioPrefs, AudioProfile, AudioSelected, AudioStopReason, AudioStopped, Msg, StartAudio,
    StopAudio,
};

use crate::textos::Idioma;

/// `stream_id` do som do sistema (o único stream de áudio por sessão).
pub const STREAM_SOM: u8 = taildesk_proto::media::AUDIO_STREAM_MIN;

/// Onde a task dos datagramas entrega o áudio: `None` com o som desligado (o que chegar em voo
/// é descartado).
pub type Destino = Arc<Mutex<Option<(u8, RecepcaoAudio)>>>;

fn travar(d: &Destino) -> MutexGuard<'_, Option<(u8, RecepcaoAudio)>> {
    d.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Entrega um datagrama de áudio (`stream_id` no byte 1) à recepção, se o som está ligado.
pub fn receber(destino: &Destino, stream_id: u8, datagrama: &[u8]) {
    if let Some((s, r)) = travar(destino).as_mut()
        && *s == stream_id
    {
        r.receber(datagrama);
    }
}

/// O botão de som e o que ele pede ao host.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Som {
    /// A pessoa quer ouvir (fica entre as sessões).
    pub ligado: bool,
    /// A sessão tem `Permissions.audio`.
    pub permitido: bool,
    ao_vivo: bool,
}

impl Som {
    /// A sessão ficou ao vivo: pede o som se já estava ligado.
    pub fn ao_vivo(&mut self, permitido: bool, out: &mut Vec<Msg>) {
        self.ao_vivo = true;
        self.permitido = permitido;
        if self.ligado && permitido {
            out.push(Msg::StartAudio(start()));
        }
    }

    /// A sessão acabou.
    pub fn fim(&mut self) {
        self.ao_vivo = false;
        self.permitido = false;
    }

    /// Toque no botão. Devolve se ficou ligado; desligado, a saída do aparelho deve fechar.
    pub fn alternar(&mut self, out: &mut Vec<Msg>) -> bool {
        self.definir(!self.ligado, out);
        self.ligado
    }

    pub fn definir(&mut self, ligar: bool, out: &mut Vec<Msg>) {
        if ligar == self.ligado {
            return;
        }
        self.ligado = ligar;
        if !self.ao_vivo || !self.permitido {
            return;
        }
        out.push(if ligar {
            Msg::StartAudio(start())
        } else {
            Msg::StopAudio(StopAudio {
                stream_id: u32::from(STREAM_SOM),
            })
        });
    }

    /// `AudioSelected`: o `stream_id` para abrir a saída, se o som ainda está ligado.
    pub fn escolhido(&self, a: &AudioSelected) -> Option<u8> {
        let s = u8::try_from(a.stream_id).ok()?;
        (self.ligado && self.ao_vivo && s >= STREAM_SOM).then_some(s)
    }

    /// `AudioStopped`: o host parou. Por motivo do host, o botão desliga e há um aviso.
    pub fn parou(&mut self, a: &AudioStopped, idioma: Idioma, nome: &str) -> Option<String> {
        let motivo = match AudioStopReason::try_from(a.reason) {
            Ok(AudioStopReason::ByHost) => idioma.t("desligou o som.", "turned the sound off."),
            Ok(AudioStopReason::NoPermission) => {
                idioma.t("não permite ouvir o som.", "does not allow sound.")
            }
            Ok(AudioStopReason::DeviceError) => idioma.t(
                "não conseguiu capturar o som.",
                "could not capture the sound.",
            ),
            _ => return None,
        };
        self.ligado = false;
        Some(format!("{nome} {motivo}"))
    }
}

fn start() -> StartAudio {
    StartAudio {
        stream_id: u32::from(STREAM_SOM),
        prefs: Some(AudioPrefs {
            profile: AudioProfile::Auto as i32,
            max_kbps: 0,
            channels: 2,
        }),
    }
}

/// Saída de som aberta; soltar fecha o AAudio e desliga o [`Destino`].
pub struct Saida {
    destino: Destino,
    #[cfg(target_os = "android")]
    _stream: taildesk_platform::som::StreamSom,
}

impl std::fmt::Debug for Saida {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Saida").finish_non_exhaustive()
    }
}

impl Saida {
    /// Abre a saída de som para o `stream_id`. Bloqueia enquanto o dispositivo abre.
    pub fn abrir(stream_id: u8, destino: Destino) -> Result<Saida, String> {
        use taildesk_media::audio::{CANAIS, Reproducao, TAXA, fila};
        let (prod, cons) = fila(1000);
        let recepcao = RecepcaoAudio::nova(stream_id, prod).map_err(|e| e.to_string())?;
        let mut reproducao = Reproducao::nova(cons);
        let mut preencher = move |buf: &mut [f32], taxa: u32, canais: usize| {
            if taxa == TAXA && canais == CANAIS {
                reproducao.preencher(buf);
            } else {
                buf.fill(0.0);
            }
        };
        #[cfg(target_os = "android")]
        let stream = taildesk_platform::som::tocar(move |buf, f| {
            preencher(buf, f.taxa, usize::from(f.canais));
        })?;
        #[cfg(not(target_os = "android"))]
        {
            // Fora do Android não há saída: os testes só conferem o caminho até aqui.
            let mut vazio = [0.0f32; 4];
            preencher(&mut vazio, TAXA, CANAIS);
        }
        *travar(&destino) = Some((stream_id, recepcao));
        Ok(Saida {
            destino,
            #[cfg(target_os = "android")]
            _stream: stream,
        })
    }
}

impl Drop for Saida {
    fn drop(&mut self) {
        *travar(&self.destino) = None;
    }
}

/// O Hello anuncia `Feature::Audio` só onde há saída de som.
pub const TEM_SAIDA: bool = cfg!(target_os = "android");

#[cfg(test)]
mod tests {
    use super::*;

    fn tipos(out: &[Msg]) -> Vec<&'static str> {
        out.iter()
            .map(|m| match m {
                Msg::StartAudio(_) => "start",
                Msg::StopAudio(_) => "stop",
                _ => "?",
            })
            .collect()
    }

    #[test]
    fn desligado_nao_pede_nada() {
        let mut s = Som::default();
        let mut out = Vec::new();
        s.ao_vivo(true, &mut out);
        assert!(out.is_empty(), "sem botão, nenhum StartAudio");
        assert!(s.alternar(&mut out));
        assert!(
            s.escolhido(&AudioSelected {
                stream_id: u32::from(STREAM_SOM),
                ..Default::default()
            })
            .is_some()
        );
        assert!(!s.alternar(&mut out));
        assert_eq!(tipos(&out), ["start", "stop"]);
    }

    #[test]
    fn ligado_antes_pede_ao_ficar_ao_vivo_e_respeita_permissao() {
        let mut s = Som::default();
        let mut out = Vec::new();
        s.alternar(&mut out);
        assert!(out.is_empty(), "fora da sessão só guarda");
        s.ao_vivo(false, &mut out);
        assert!(out.is_empty(), "sem Permissions.audio não pede");
        s.fim();
        s.ao_vivo(true, &mut out);
        assert_eq!(tipos(&out), ["start"]);
        // O pedido vai pelo fio.
        let mut b = Vec::new();
        taildesk_proto::control::encode_frame(&out[0].clone().into(), &mut b);
        assert!(taildesk_proto::control::decode_frame(&b).unwrap().is_some());
    }

    #[test]
    fn host_parou_desliga_o_botao() {
        let mut s = Som::default();
        let mut out = Vec::new();
        s.ao_vivo(true, &mut out);
        s.alternar(&mut out);
        let aviso = s.parou(
            &AudioStopped {
                stream_id: u32::from(STREAM_SOM),
                reason: AudioStopReason::ByHost as i32,
            },
            Idioma::Pt,
            "pc-mae",
        );
        assert_eq!(aviso.as_deref(), Some("pc-mae desligou o som."));
        assert!(!s.ligado);
        assert!(
            s.escolhido(&AudioSelected {
                stream_id: u32::from(STREAM_SOM),
                ..Default::default()
            })
            .is_none()
        );
    }

    #[test]
    fn saida_liga_e_desliga_o_destino() {
        let destino = Destino::default();
        let saida = Saida::abrir(STREAM_SOM, destino.clone()).unwrap();
        assert!(travar(&destino).is_some());
        // Lixo e stream errado não quebram.
        receber(&destino, STREAM_SOM, &[1, 2, 3]);
        receber(&destino, STREAM_SOM + 1, &[0; 32]);
        drop(saida);
        assert!(travar(&destino).is_none());
    }
}
