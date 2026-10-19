//! Configurações → "Este computador" (pedir permissão, dispositivos confiáveis, pasta de entrada)
//! e "Sessão" (codec e taxa de bits pedidos ao host, vídeo deste computador), mais as listas de
//! codec e de taxa que o seletor da janela da sessão também usa.

use std::sync::OnceLock;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use taildesk_codec::CodecVideo;
use taildesk_proto::control::Codec;
use taildesk_ui::JanelaPrincipal;

use crate::config::{self, ModoMonitores, PrefsMonitores, PrefsVideo};
use crate::{avisar, com_app, no_app};

/// Codecs do seletor, na ordem das listas (o índice 0 é o automático).
pub const CODECS: [(Codec, &str); 5] = [
    (Codec::Unspecified, "Automático"),
    (Codec::H264, "H.264"),
    (Codec::Hevc, "HEVC"),
    (Codec::Av1, "AV1"),
    (Codec::PyroWave, "PyroWave"),
];

/// Taxas do seletor em kbit/s (0 = automática). As altas são para o PyroWave e a LAN.
pub const TAXAS_KBPS: [u32; 7] = [0, 5_000, 10_000, 20_000, 50_000, 100_000, 300_000];

/// Índice de `codec` em [`CODECS`] (desconhecido = automático).
pub fn indice_codec(codec: Codec) -> usize {
    CODECS.iter().position(|(c, _)| *c == codec).unwrap_or(0)
}

/// Índice da taxa em [`TAXAS_KBPS`]: a mais próxima por baixo (0 = automática).
pub fn indice_taxa(kbps: u32) -> usize {
    if kbps == 0 {
        return 0;
    }
    TAXAS_KBPS
        .iter()
        .rposition(|&t| t != 0 && t <= kbps)
        .unwrap_or(1)
}

fn mbit(kbps: u32) -> String {
    format!("{} Mbit/s", kbps / 1000)
}

/// Rótulos das taxas para as Configurações ("Automática", "5 Mbit/s", …).
pub fn rotulos_taxa() -> Vec<SharedString> {
    TAXAS_KBPS
        .iter()
        .map(|&t| {
            if t == 0 {
                "Automática".into()
            } else {
                mbit(t).into()
            }
        })
        .collect()
}

/// Rótulos curtos para os botões da janela da sessão (a unidade vai no título).
pub fn rotulos_taxa_curtos() -> Vec<SharedString> {
    TAXAS_KBPS
        .iter()
        .map(|&t| {
            if t == 0 {
                "Auto".into()
            } else {
                (t / 1000).to_string().into()
            }
        })
        .collect()
}

/// Codec do protocolo de um codec que este computador tem.
pub fn codec_proto(c: CodecVideo) -> Codec {
    match c {
        CodecVideo::H264 => Codec::H264,
        CodecVideo::Hevc => Codec::Hevc,
        CodecVideo::Av1 => Codec::Av1,
        CodecVideo::PyroWave => Codec::PyroWave,
    }
}

fn nome_backend(b: &str) -> &str {
    match b {
        "openh264" => "CPU",
        "nvenc" => "NVENC",
        "nvdec" => "NVDEC",
        "vaapi" => "VA-API",
        "mf" => "Media Foundation",
        outro => outro,
    }
}

fn nome_codec(c: CodecVideo) -> &'static str {
    CODECS[indice_codec(codec_proto(c))].1
}

/// "Codifica H.264 (NVENC, CPU), HEVC (NVENC) · Decodifica H.264 (NVDEC, CPU)". Lista as GPUs
/// (alguns milissegundos), então roda fora da thread da interface e fica guardado.
fn video_local() -> String {
    static TEXTO: OnceLock<String> = OnceLock::new();
    TEXTO
        .get_or_init(|| {
            let lista = |opcoes: &dyn Fn(CodecVideo) -> Vec<taildesk_codec::Opcao>| {
                let partes: Vec<String> = [
                    CodecVideo::H264,
                    CodecVideo::Hevc,
                    CodecVideo::Av1,
                    CodecVideo::PyroWave,
                ]
                .into_iter()
                .filter_map(|c| {
                    let mut backends: Vec<&str> = Vec::new();
                    for o in opcoes(c) {
                        let b = nome_backend(o.backend);
                        if !backends.contains(&b) {
                            backends.push(b);
                        }
                    }
                    (!backends.is_empty())
                        .then(|| format!("{} ({})", nome_codec(c), backends.join(", ")))
                })
                .collect();
                if partes.is_empty() {
                    "nada".to_string()
                } else {
                    partes.join(", ")
                }
            };
            format!(
                "Codifica {} · Decodifica {}",
                lista(&taildesk_codec::opcoes_encoder),
                lista(&taildesk_codec::opcoes_decoder)
            )
        })
        .clone()
}

/// Mostra na janela o que está gravado e o que este computador sabe fazer.
pub fn mostrar(j: &JanelaPrincipal) {
    let c = config::ler();
    let settings = com_app(|app| {
        app.settings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    })
    .unwrap_or(c.host.clone());
    j.set_pedir_sempre(settings.ask_always);
    j.set_tem_senha(settings.password_hash.is_some());
    let nomes: Vec<SharedString> = settings
        .trusted
        .iter()
        .map(|t| t.name.as_str().into())
        .collect();
    j.set_confiaveis(ModelRc::new(VecModel::from(nomes)));
    j.set_pasta_entrada(
        pasta_entrada()
            .map(|p| p.display().to_string())
            .unwrap_or_default()
            .into(),
    );
    let codecs: Vec<SharedString> = CODECS.iter().map(|(_, n)| (*n).into()).collect();
    j.set_codecs_config(ModelRc::new(VecModel::from(codecs)));
    j.set_codec_config(indice_codec(c.video.codec) as i32);
    j.set_taxas_config(ModelRc::new(VecModel::from(rotulos_taxa())));
    j.set_taxa_config(indice_taxa(c.video.taxa_kbps) as i32);
    j.set_modo_monitores_config(
        ModoMonitores::TODOS
            .iter()
            .position(|m| *m == c.monitores.modo)
            .unwrap_or(0) as i32,
    );
    j.set_seguir_mouse_config(c.monitores.seguir_mouse);
    crate::historico::mostrar(j);
    let fraca = j.as_weak();
    std::thread::spawn(move || {
        let inicio = taildesk_platform::inicio::ligado();
        let texto = video_local();
        let teclado = crate::host::layout_local();
        let _ = fraca.upgrade_in_event_loop(move |j| {
            j.set_iniciar_com_sistema(inicio);
            j.set_video_local(texto.into());
            j.set_teclado_local(teclado.into());
        });
    });
}

/// `~/Downloads/TailDesk`, onde chegam os arquivos recebidos.
pub fn pasta_entrada() -> Option<std::path::PathBuf> {
    taildesk_platform::pasta_downloads().map(|d| d.join("TailDesk"))
}

fn gravar_monitores(f: impl FnOnce(&mut PrefsMonitores)) {
    let mut c = config::ler();
    f(&mut c.monitores);
    if let Err(e) = config::gravar(&c) {
        avisar(format!("Não foi possível gravar a configuração: {e}"));
    }
}

fn gravar_video(f: impl FnOnce(&mut PrefsVideo)) {
    let mut c = config::ler();
    f(&mut c.video);
    if let Err(e) = config::gravar(&c) {
        avisar(format!("Não foi possível gravar a configuração: {e}"));
    }
}

/// Grava o hash da senha de acesso (ou tira, com `None`) e mostra.
fn gravar_senha(hash: Option<String>) {
    no_app(move |app| {
        let tinha = {
            let mut s = app.settings.lock().unwrap_or_else(|e| e.into_inner());
            std::mem::replace(&mut s.password_hash, hash.clone()).is_some()
        };
        crate::host::gravar_settings(&app.settings);
        app.atualizar_configuracoes();
        avisar(match (hash.is_some(), tinha) {
            (true, false) => "Acesso com senha ligado.".into(),
            (true, true) => "Senha de acesso trocada.".into(),
            (false, _) => "Acesso com senha desligado.".into(),
        });
    });
}

/// Liga os callbacks das Configurações.
pub fn ligar(j: &JanelaPrincipal) {
    mostrar(j);
    crate::historico::ligar(j);
    let fraca = j.as_weak();
    j.on_definir_senha(move |nova, confirma| {
        let Some(j) = fraca.upgrade() else { return };
        if nova != confirma {
            j.set_erro_senha("As duas senhas não são iguais.".into());
            return;
        }
        j.set_erro_senha("".into());
        let fraca = fraca.clone();
        let nova = nova.to_string();
        // argon2 leva um tempo: fora da thread da interface.
        std::thread::spawn(move || {
            let r = taildesk_session::senha::gerar_hash(&nova);
            let _ = fraca.upgrade_in_event_loop(move |j| match r {
                Ok(h) => {
                    j.set_editando_senha(false);
                    gravar_senha(Some(h));
                }
                Err(e) => j.set_erro_senha(format!("{e}.").into()),
            });
        });
    });
    j.on_remover_senha(|| gravar_senha(None));
    let fraca = j.as_weak();
    j.on_iniciar_com_sistema_mudou(move |ligar| {
        let fraca = fraca.clone();
        // systemctl (ou o pedido de administrador no Windows) fora da thread da interface.
        std::thread::spawn(move || {
            use taildesk_platform::inicio;
            let r = if ligar {
                inicio::ligar()
            } else {
                inicio::desligar()
            };
            // O agente da tela de entrada (Windows) usa a identidade deste host.
            #[cfg(windows)]
            if r.is_ok() {
                if ligar {
                    if let Err(e) = crate::host::exportar_para_agente() {
                        eprintln!("taildesk: identidade para o agente da tela de entrada: {e}");
                    }
                } else {
                    crate::host::apagar_exportacao_do_agente();
                }
            }
            let agora = inicio::ligado();
            let _ = fraca.upgrade_in_event_loop(move |j| {
                j.set_iniciar_com_sistema(agora);
                match r {
                    Ok(()) if ligar => avisar(
                        "O TailDesk vai abrir na bandeja sempre que você entrar no computador."
                            .into(),
                    ),
                    Ok(()) => avisar("O TailDesk não abre mais sozinho.".into()),
                    Err(e) => avisar(format!(
                        "Não foi possível mudar o início com o sistema: {e}"
                    )),
                }
            });
        });
    });
    j.on_pedir_sempre_mudou(|v| {
        no_app(move |app| {
            app.settings
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .ask_always = v;
            crate::host::gravar_settings(&app.settings);
        });
    });
    j.on_revogar(|i| {
        no_app(move |app| {
            let nome = {
                let mut s = app.settings.lock().unwrap_or_else(|e| e.into_inner());
                let i = i as usize;
                (i < s.trusted.len()).then(|| s.trusted.remove(i).name)
            };
            crate::host::gravar_settings(&app.settings);
            app.atualizar_configuracoes();
            if let Some(n) = nome {
                avisar(format!("{n} vai precisar pedir acesso de novo."));
            }
        });
    });
    j.on_abrir_pasta_entrada(|| {
        let Some(dir) = pasta_entrada() else { return };
        com_app(|app| {
            app.rt.spawn(async move {
                let _ = tokio::fs::create_dir_all(&dir).await;
                if let Err(e) = taildesk_platform::arquivos::abrir_pasta(&dir).await {
                    avisar(format!("Não foi possível abrir {}: {e}", dir.display()));
                }
            });
        });
    });
    j.on_codec_config_mudou(|i| {
        let codec = CODECS.get(i as usize).map_or(Codec::Unspecified, |c| c.0);
        gravar_video(|v| v.codec = codec);
    });
    j.on_taxa_config_mudou(|i| {
        let kbps = TAXAS_KBPS.get(i as usize).copied().unwrap_or(0);
        gravar_video(|v| v.taxa_kbps = kbps);
    });
    j.on_modo_monitores_config_mudou(|i| {
        let modo = ModoMonitores::TODOS
            .get(i as usize)
            .copied()
            .unwrap_or_default();
        gravar_monitores(|m| m.modo = modo);
    });
    j.on_seguir_mouse_config_mudou(|v| gravar_monitores(|m| m.seguir_mouse = v));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indices_das_listas() {
        assert_eq!(indice_codec(Codec::Unspecified), 0);
        assert_eq!(indice_codec(Codec::PyroWave), 4);
        assert_eq!(indice_codec(Codec::Vp9), 0);
        assert_eq!(indice_taxa(0), 0);
        assert_eq!(indice_taxa(20_000), 3);
        // Taxa gravada fora da lista: a de baixo mais próxima.
        assert_eq!(indice_taxa(30_000), 3);
        assert_eq!(indice_taxa(1_000), 1);
        assert_eq!(indice_taxa(900_000), TAXAS_KBPS.len() - 1);
        assert_eq!(rotulos_taxa()[2], "10 Mbit/s");
        assert_eq!(rotulos_taxa_curtos()[0], "Auto");
    }
}
