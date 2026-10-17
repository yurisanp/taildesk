//! Som do sistema (`15-audio.md` §3): captura do que toca no computador (host) e reprodução
//! (controlador).
//!
//! - Windows: WASAPI pelo `cpal`. Abrir o dispositivo de **saída** padrão como entrada liga o
//!   loopback.
//! - Linux: PipeWire direto (a mesma `libpipewire` da captura de tela). Um stream com
//!   `stream.capture.sink` ouve o sink padrão e segue a troca de saída; não depende do portal.
//!   Sem PipeWire não há captura.
//!
//! Cada stream vive numa thread própria, e soltar o valor fecha o dispositivo: com o áudio
//! desligado nada fica aberto (`15` §4).

use std::sync::mpsc;
use std::thread::JoinHandle;

/// Formato que o dispositivo entrega ou espera.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FormatoSom {
    pub taxa: u32,
    pub canais: u16,
}

/// Um stream de som aberto; soltar fecha.
pub struct StreamSom {
    parar: Option<Box<dyn FnOnce() + Send + Sync>>,
    thread: Option<JoinHandle<()>>,
    pub formato: FormatoSom,
    /// Nome do dispositivo, para o diagnóstico.
    pub dispositivo: String,
}

impl std::fmt::Debug for StreamSom {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamSom")
            .field("formato", &self.formato)
            .field("dispositivo", &self.dispositivo)
            .finish_non_exhaustive()
    }
}

impl Drop for StreamSom {
    fn drop(&mut self) {
        if let Some(parar) = self.parar.take() {
            parar();
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(windows)]
pub use wasapi::{capturar, tocar};

#[cfg(windows)]
mod wasapi {
    use super::*;
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{Device, SampleFormat, StreamConfig};

    fn erro(e: impl std::fmt::Display) -> String {
        e.to_string()
    }

    /// Dispositivo cujo som do sistema é capturado.
    fn fonte_do_sistema() -> Result<(Device, StreamConfig, SampleFormat), String> {
        let host = cpal::default_host();
        let d = host
            .default_output_device()
            .ok_or("nenhuma saída de som no Windows")?;
        let c = d.default_output_config().map_err(erro)?;
        Ok((d, c.config(), c.sample_format()))
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
                parar: Some(Box::new(move || drop(parar))),
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
}

#[cfg(target_os = "linux")]
pub use pipewire_som::{capturar, tocar};

#[cfg(target_os = "linux")]
mod pipewire_som {
    use super::*;
    use pipewire as pw;
    use pw::spa;
    use pw::spa::pod::Pod;

    /// O PipeWire converte para o que for pedido: sempre 48 kHz estéreo f32.
    const FORMATO: FormatoSom = FormatoSom {
        taxa: 48_000,
        canais: 2,
    };
    /// Quadros por ciclo (10 ms): sem `requested` (PipeWire ≥ 0.3.49), a saída escreve só isto
    /// por buffer para não acumular atraso.
    const QUANTUM: usize = 480;
    const BYTES_QUADRO: usize = 4 * FORMATO.canais as usize;

    /// Abre a captura do som do sistema (o monitor do sink padrão). `ao_som` roda na thread do
    /// PipeWire: só copia.
    pub fn capturar(ao_som: impl FnMut(&[f32]) + Send + 'static) -> Result<StreamSom, String> {
        abrir(
            true,
            "Som do sistema (PipeWire)",
            Processo::Captura(Box::new(ao_som)),
        )
    }

    /// Abre a saída de som padrão. `preencher(buf, formato)` roda na thread do PipeWire.
    pub fn tocar(
        preencher: impl FnMut(&mut [f32], FormatoSom) + Send + 'static,
    ) -> Result<StreamSom, String> {
        abrir(
            false,
            "Saída padrão (PipeWire)",
            Processo::Saida(Box::new(preencher)),
        )
    }

    type Captura = Box<dyn FnMut(&[f32]) + Send>;
    type Saida = Box<dyn FnMut(&mut [f32], FormatoSom) + Send>;

    enum Processo {
        Captura(Captura),
        Saida(Saida),
    }

    fn abrir(captura: bool, nome: &str, processo: Processo) -> Result<StreamSom, String> {
        let (pronto_tx, pronto_rx) = mpsc::sync_channel::<Result<(), String>>(1);
        let (parar_tx, parar_rx) = pw::channel::channel::<()>();
        let thread = std::thread::Builder::new()
            .name("taildesk-som".into())
            .spawn(move || {
                if let Err(e) = laco(captura, processo, parar_rx, &pronto_tx) {
                    let _ = pronto_tx.send(Err(format!("PipeWire: {e}")));
                }
            })
            .map_err(|e| e.to_string())?;
        match pronto_rx.recv() {
            Ok(Ok(())) => Ok(StreamSom {
                parar: Some(Box::new(move || {
                    let _ = parar_tx.send(());
                })),
                thread: Some(thread),
                formato: FORMATO,
                dispositivo: nome.to_owned(),
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

    fn laco(
        captura: bool,
        processo: Processo,
        parar: pw::channel::Receiver<()>,
        pronto: &mpsc::SyncSender<Result<(), String>>,
    ) -> Result<(), pw::Error> {
        pw::init();
        let main_loop = pw::main_loop::MainLoopRc::new(None)?;
        let context = pw::context::ContextRc::new(&main_loop, None)?;
        let core = context.connect_rc(None)?;
        let mut props = pw::properties::properties! {
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CATEGORY => if captura { "Capture" } else { "Playback" },
            *pw::keys::MEDIA_ROLE => "Music",
            *pw::keys::AUDIO_CHANNELS => "2",
            *pw::keys::NODE_LATENCY => "480/48000",
            *pw::keys::APP_NAME => "TailDesk",
        };
        if captura {
            props.insert(*pw::keys::STREAM_CAPTURE_SINK, "true");
        }
        let nome = if captura {
            "taildesk-som-captura"
        } else {
            "taildesk-som"
        };
        let stream = pw::stream::StreamRc::new(core, nome, props)?;
        // Buffer reaproveitado entre ciclos (até 1 s), sem alocar no caminho do som.
        let buf: Vec<f32> = Vec::with_capacity(48_000 * 2);
        let _ouvinte = stream
            .add_local_listener_with_user_data((processo, buf))
            .process(|stream, (processo, buf)| {
                let Some(mut b) = stream.dequeue_buffer() else {
                    return;
                };
                let Some(d) = b.datas_mut().first_mut() else {
                    return;
                };
                match processo {
                    Processo::Captura(cb) => {
                        let (ini, tam) = (d.chunk().offset() as usize, d.chunk().size() as usize);
                        let Some(bytes) = d.data() else {
                            return;
                        };
                        let fim = ini.saturating_add(tam).min(bytes.len());
                        let bytes = &bytes[ini.min(fim)..fim];
                        let n = (bytes.len() / 4).min(buf.capacity());
                        buf.clear();
                        buf.extend(
                            bytes[..n * 4]
                                .chunks_exact(4)
                                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])),
                        );
                        cb(buf);
                    }
                    Processo::Saida(cb) => {
                        let quadros = match d.data() {
                            Some(bytes) => {
                                let quadros = (bytes.len() / BYTES_QUADRO).min(QUANTUM);
                                buf.clear();
                                buf.resize(quadros * FORMATO.canais as usize, 0.0);
                                cb(buf, FORMATO);
                                for (c, s) in bytes.chunks_exact_mut(4).zip(buf.iter()) {
                                    c.copy_from_slice(&s.to_le_bytes());
                                }
                                quadros
                            }
                            None => 0,
                        };
                        let chunk = d.chunk_mut();
                        *chunk.offset_mut() = 0;
                        *chunk.stride_mut() = BYTES_QUADRO as _;
                        *chunk.size_mut() = (quadros * BYTES_QUADRO) as _;
                    }
                }
            })
            .register()?;

        let mut info = spa::param::audio::AudioInfoRaw::new();
        info.set_format(spa::param::audio::AudioFormat::F32LE);
        info.set_rate(FORMATO.taxa);
        info.set_channels(u32::from(FORMATO.canais));
        let mut posicao = [0; spa::param::audio::MAX_CHANNELS];
        posicao[0] = spa::sys::SPA_AUDIO_CHANNEL_FL;
        posicao[1] = spa::sys::SPA_AUDIO_CHANNEL_FR;
        info.set_position(posicao);
        let params = formato_pod(info);
        let mut params = [Pod::from_bytes(&params).expect("pod serializado por nós")];
        let direcao = if captura {
            spa::utils::Direction::Input
        } else {
            spa::utils::Direction::Output
        };
        stream.connect(
            direcao,
            None,
            pw::stream::StreamFlags::AUTOCONNECT
                | pw::stream::StreamFlags::MAP_BUFFERS
                | pw::stream::StreamFlags::RT_PROCESS,
            &mut params,
        )?;

        let laco_fraco = main_loop.downgrade();
        let _rx = parar.attach(main_loop.loop_(), move |()| {
            if let Some(l) = laco_fraco.upgrade() {
                l.quit();
            }
        });
        let _ = pronto.send(Ok(()));
        main_loop.run();
        let _ = stream.disconnect();
        Ok(())
    }

    fn formato_pod(info: spa::param::audio::AudioInfoRaw) -> Vec<u8> {
        spa::pod::serialize::PodSerializer::serialize(
            std::io::Cursor::new(Vec::new()),
            &spa::pod::Value::Object(spa::pod::Object {
                type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
                id: spa::param::ParamType::EnumFormat.as_raw(),
                properties: info.into(),
            }),
        )
        .expect("pod de formato")
        .0
        .into_inner()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn formato_serializa() {
            let mut info = spa::param::audio::AudioInfoRaw::new();
            info.set_format(spa::param::audio::AudioFormat::F32LE);
            info.set_rate(FORMATO.taxa);
            info.set_channels(2);
            assert!(Pod::from_bytes(&formato_pod(info)).is_some());
        }
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn capturar(_: impl FnMut(&[f32]) + Send + 'static) -> Result<StreamSom, String> {
    Err("captura de som não suportada neste sistema".into())
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn tocar(_: impl FnMut(&mut [f32], FormatoSom) + Send + 'static) -> Result<StreamSom, String> {
    Err("saída de som não suportada neste sistema".into())
}
