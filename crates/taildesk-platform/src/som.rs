//! Som do sistema pelo `cpal` (`15-audio.md` §3): captura do que toca no computador (host) e
//! reprodução (controlador).
//!
//! - Windows: WASAPI. Abrir o dispositivo de **saída** padrão como entrada liga o loopback.
//! - Linux: PipeWire. O dispositivo `sink_default` do cpal captura o que toca no sink padrão
//!   (e segue a troca de saída); não depende do portal. Sem PipeWire não há captura.
//!
//! Cada stream vive numa thread própria (o `Stream` do cpal não é `Send` em todo backend), e
//! soltar o valor fecha o dispositivo: com o áudio desligado nada fica aberto (`15` §4).

use std::sync::mpsc;
use std::thread::JoinHandle;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, StreamConfig};

/// Formato que o dispositivo entrega ou espera.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FormatoSom {
    pub taxa: u32,
    pub canais: u16,
}

/// Um stream de som aberto; soltar fecha.
#[derive(Debug)]
pub struct StreamSom {
    parar: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
    pub formato: FormatoSom,
    /// Nome do dispositivo, para o diagnóstico.
    pub dispositivo: String,
}

impl Drop for StreamSom {
    fn drop(&mut self) {
        self.parar.take();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn erro(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Dispositivo cujo som do sistema é capturado.
fn fonte_do_sistema() -> Result<(Device, StreamConfig, SampleFormat), String> {
    #[cfg(windows)]
    {
        let host = cpal::default_host();
        let d = host
            .default_output_device()
            .ok_or("nenhuma saída de som no Windows")?;
        let c = d.default_output_config().map_err(erro)?;
        Ok((d, c.config(), c.sample_format()))
    }
    #[cfg(target_os = "linux")]
    {
        let host = cpal::host_from_id(cpal::HostId::PipeWire)
            .map_err(|e| format!("PipeWire indisponível ({e}); sem captura do som do sistema"))?;
        let d = host
            .devices()
            .map_err(erro)?
            .find(|d| d.id().is_ok_and(|id| id.id() == "sink_default"))
            .ok_or("o PipeWire não tem saída de som padrão")?;
        let c = d.default_input_config().map_err(erro)?;
        let mut config = c.config();
        // O PipeWire converte para o que for pedido: já em 48 kHz estéreo, sem reamostrar.
        config.sample_rate = 48_000;
        config.channels = 2;
        Ok((d, config, SampleFormat::F32))
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        Err("captura de som não suportada neste sistema".into())
    }
}

/// Converte para f32 e entrega.
fn para_f32<'a, T: cpal::Sample>(dados: &[T], buf: &'a mut Vec<f32>) -> &'a [f32]
where
    f32: cpal::FromSample<T>,
{
    buf.clear();
    buf.extend(dados.iter().map(|&s| s.to_sample::<f32>()));
    buf
}

/// Abre a captura do som do sistema. `ao_som` roda na thread de áudio do SO: só copia.
pub fn capturar(ao_som: impl FnMut(&[f32]) + Send + 'static) -> Result<StreamSom, String> {
    let (pronto_tx, pronto_rx) = mpsc::channel();
    let (parar_tx, parar_rx) = mpsc::channel::<()>();
    let thread = std::thread::Builder::new()
        .name("taildesk-som-captura".into())
        .spawn(move || {
            let aberto = (|| {
                let (d, config, formato) = fonte_do_sistema()?;
                let nome = d
                    .description()
                    .map(|n| n.name().to_owned())
                    .unwrap_or_default();
                let fmt = FormatoSom {
                    taxa: config.sample_rate,
                    canais: config.channels,
                };
                let falha = |e: cpal::Error| eprintln!("taildesk: captura de som: {e}");
                let mut cb = ao_som;
                let stream = match formato {
                    SampleFormat::I16 => {
                        let mut buf = Vec::new();
                        d.build_input_stream(
                            config,
                            move |dados: &[i16], _: &_| cb(para_f32(dados, &mut buf)),
                            falha,
                            None,
                        )
                    }
                    _ => d.build_input_stream(
                        config,
                        move |dados: &[f32], _: &_| cb(dados),
                        falha,
                        None,
                    ),
                }
                .map_err(erro)?;
                stream.play().map_err(erro)?;
                Ok::<_, String>((stream, fmt, nome))
            })();
            match aberto {
                Ok((stream, fmt, nome)) => {
                    let _ = pronto_tx.send(Ok((fmt, nome)));
                    // Até soltar o `StreamSom` (o remetente cai).
                    let _ = parar_rx.recv();
                    drop(stream);
                }
                Err(e) => {
                    let _ = pronto_tx.send(Err(e));
                }
            }
        })
        .map_err(erro)?;
    terminar(pronto_rx, parar_tx, thread)
}

fn terminar(
    pronto: mpsc::Receiver<Result<(FormatoSom, String), String>>,
    parar: mpsc::Sender<()>,
    thread: JoinHandle<()>,
) -> Result<StreamSom, String> {
    match pronto.recv() {
        Ok(Ok((formato, dispositivo))) => Ok(StreamSom {
            parar: Some(parar),
            thread: Some(thread),
            formato,
            dispositivo,
        }),
        Ok(Err(e)) => {
            let _ = thread.join();
            Err(e)
        }
        Err(_) => {
            let _ = thread.join();
            Err("a thread de som terminou sem abrir".into())
        }
    }
}

/// Abre a saída de som padrão. `preencher(buf, formato)` roda na thread de áudio do SO e
/// preenche o buffer intercalado (f32) no formato do dispositivo.
pub fn tocar(
    preencher: impl FnMut(&mut [f32], FormatoSom) + Send + 'static,
) -> Result<StreamSom, String> {
    let (pronto_tx, pronto_rx) = mpsc::channel();
    let (parar_tx, parar_rx) = mpsc::channel::<()>();
    let thread = std::thread::Builder::new()
        .name("taildesk-som-saida".into())
        .spawn(move || {
            let aberto = (|| {
                let host = cpal::default_host();
                let d = host.default_output_device().ok_or("nenhuma saída de som")?;
                let c = d.default_output_config().map_err(erro)?;
                let mut config = c.config();
                // 48 kHz estéreo poupa a reamostragem quando o dispositivo aceita (PipeWire
                // converte sempre; o WASAPI compartilhado converte por AUTOCONVERTPCM).
                config.sample_rate = 48_000;
                config.channels = 2;
                let nome = d
                    .description()
                    .map(|n| n.name().to_owned())
                    .unwrap_or_default();
                let falha = |e: cpal::Error| eprintln!("taildesk: saída de som: {e}");
                let mut cb = preencher;
                let fmt = FormatoSom {
                    taxa: config.sample_rate,
                    canais: config.channels,
                };
                let stream = match c.sample_format() {
                    SampleFormat::I16 => {
                        let mut buf = Vec::new();
                        d.build_output_stream(
                            config,
                            move |dados: &mut [i16], _: &_| {
                                buf.resize(dados.len(), 0.0);
                                cb(&mut buf, fmt);
                                for (d, s) in dados.iter_mut().zip(&buf) {
                                    *d = cpal::Sample::from_sample(*s);
                                }
                            },
                            falha,
                            None,
                        )
                    }
                    _ => d.build_output_stream(
                        config,
                        move |dados: &mut [f32], _: &_| cb(dados, fmt),
                        falha,
                        None,
                    ),
                }
                .map_err(erro)?;
                stream.play().map_err(erro)?;
                Ok::<_, String>((stream, fmt, nome))
            })();
            match aberto {
                Ok((stream, fmt, nome)) => {
                    let _ = pronto_tx.send(Ok((fmt, nome)));
                    let _ = parar_rx.recv();
                    drop(stream);
                }
                Err(e) => {
                    let _ = pronto_tx.send(Err(e));
                }
            }
        })
        .map_err(erro)?;
    terminar(pronto_rx, parar_tx, thread)
}
