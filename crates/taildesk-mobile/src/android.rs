//! Entrada do app no Android e a ponte JNI com a casca Kotlin (`apps/android`).
//!
//! - Rust → Kotlin: métodos da `MainActivity` (`abrirTailscale`, `lerQr`, `copiar`…), chamados
//!   pelo objeto da Activity que o `android-activity` entrega (funciona de qualquer thread
//!   anexada à JVM, sem procurar classe pelo nome).
//! - Kotlin → Rust: funções nativas do objeto `com.santech.taildesk.Ponte` (`nativeSinais`,
//!   `nativeQr`, `nativeLerQuadro`, `nativeArquivosEscolhidos`, `nativeDownloadPublicado`,
//!   `nativeCompartilhar`, `nativeTeclaFisica`, `nativeEncerrar`).
//!   Chegam na thread de Java e vão para a thread da interface por [`no_app`].
//!
//! Formato das mensagens: texto com campos separados por tabulação, para manter a ponte pequena.

use std::path::PathBuf;

use jni::objects::{JClass, JObject, JString, JValue};
use jni::sys::{jint, jstring};
use jni::{EnvUnowned, JavaVM, jni_sig, jni_str};
use slint::ComponentHandle;
use slint::android::android_activity::{AndroidApp, MainEvent, PollEvent};
use taildesk_overlay::mobile::SinaisRede;

use crate::app::{App, Plataforma, no_app};
use crate::controle::Aparelho;
use crate::ui::AppCelular;

/// A Activity e a JVM, para chamar a casca Kotlin.
struct Android {
    app: AndroidApp,
}

impl Android {
    fn chamar(&self, metodo: &'static jni::strings::JNIStr, texto: Option<&str>) {
        // SAFETY: o ponteiro vem do `android-activity` e vale enquanto o app existir.
        let vm = unsafe { JavaVM::from_raw(self.app.vm_as_ptr().cast()) };
        let atividade = self.app.activity_as_ptr();
        let r = vm.attach_current_thread(|env| -> jni::errors::Result<()> {
            // SAFETY: referência global da Activity mantida pelo `android-activity`.
            let act = unsafe { JObject::from_raw(env, atividade.cast()) };
            match texto {
                Some(t) => {
                    let s = env.new_string(t)?;
                    env.call_method(
                        &act,
                        metodo,
                        jni_sig!("(Ljava/lang/String;)V"),
                        &[JValue::Object(&s)],
                    )?;
                }
                None => {
                    env.call_method(&act, metodo, jni_sig!("()V"), &[])?;
                }
            }
            Ok(())
        });
        if let Err(e) = r {
            crate::registrar!("taildesk: JNI {metodo:?}: {e}");
        }
    }

    /// `MainActivity.aparelho()`: modelo, tablet, largura, altura, dpi, densidade, idioma do
    /// sistema, versão do app (separados por tabulação).
    fn texto_do_aparelho(&self) -> String {
        self.texto(jni_str!("aparelho"))
    }

    /// `MainActivity.modoInicial()`: `demo`, `exemplo` ou vazio.
    fn modo_inicial(&self) -> String {
        self.texto(jni_str!("modoInicial"))
    }

    /// Chama um método `String metodo()` da Activity.
    fn texto(&self, metodo: &'static jni::strings::JNIStr) -> String {
        // SAFETY: como em `chamar`.
        let vm = unsafe { JavaVM::from_raw(self.app.vm_as_ptr().cast()) };
        let atividade = self.app.activity_as_ptr();
        vm.attach_current_thread(|env| -> jni::errors::Result<String> {
            // SAFETY: como em `chamar`.
            let act = unsafe { JObject::from_raw(env, atividade.cast()) };
            let v = env
                .call_method(&act, metodo, jni_sig!("()Ljava/lang/String;"), &[])?
                .l()?;
            let s = env.cast_local::<JString>(v)?;
            Ok(s.to_string())
        })
        .unwrap_or_default()
    }
}

fn aparelho_de(texto: &str) -> (Aparelho, f32, String) {
    let c: Vec<&str> = texto.split('\t').collect();
    let n = |i: usize| c.get(i).and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
    let a = Aparelho {
        modelo: c.first().copied().unwrap_or("Android").to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        tablet: c.get(1) == Some(&"1"),
        largura_px: n(2).max(1),
        altura_px: n(3).max(1),
        dpi: n(4),
        decode_hw: true,
        idioma_teclado: c.get(6).copied().unwrap_or("pt-BR").to_owned(),
        versao_app: c
            .get(7)
            .copied()
            .unwrap_or(env!("CARGO_PKG_VERSION"))
            .to_owned(),
    };
    let densidade = c.get(5).and_then(|v| v.parse().ok()).unwrap_or(1.0);
    (
        a,
        densidade,
        c.get(6).copied().unwrap_or("pt-BR").to_owned(),
    )
}

struct PlataformaAndroid {
    android: Android,
    aparelho: Aparelho,
    densidade: f32,
    idioma: String,
    pasta: Option<PathBuf>,
}

impl Plataforma for PlataformaAndroid {
    fn abrir_tailscale(&self) {
        self.android.chamar(jni_str!("abrirTailscale"), None);
    }
    fn instalar_tailscale(&self) {
        self.android.chamar(jni_str!("instalarTailscale"), None);
    }
    fn ler_qr(&self) {
        self.android.chamar(jni_str!("lerQr"), None);
    }
    fn copiar(&self, texto: &str) {
        self.android.chamar(jni_str!("copiar"), Some(texto));
    }
    fn abrir_link(&self, url: &str) {
        self.android.chamar(jni_str!("abrirLink"), Some(url));
    }
    fn otimizacao_bateria(&self) {
        self.android.chamar(jni_str!("otimizacaoBateria"), None);
    }
    fn pasta_dados(&self) -> Option<PathBuf> {
        self.pasta.clone()
    }
    fn ler_area(&self) -> Option<String> {
        Some(self.android.texto(jni_str!("lerAreaTransferencia"))).filter(|t| !t.is_empty())
    }
    fn escolher_arquivos(&self) {
        self.android.chamar(jni_str!("escolherArquivos"), None);
    }
    fn teclas_fisicas(&self, ligar: bool) {
        self.android.chamar(
            jni_str!("teclasFisicas"),
            Some(if ligar { "1" } else { "0" }),
        );
    }
    fn ouvindo(&self, nome: Option<&str>) {
        self.android
            .chamar(jni_str!("ouvindo"), Some(nome.unwrap_or("")));
    }
    fn tem_teclado_fisico(&self) -> bool {
        self.android.texto(jni_str!("temTecladoFisico")) == "1"
    }
    fn publicar_download(&self, caminho: &std::path::Path, nome: &str) -> bool {
        let Some(c) = caminho.to_str() else {
            return false;
        };
        self.android
            .chamar(jni_str!("publicarDownload"), Some(&format!("{c}\t{nome}")));
        true
    }
    fn aparelho(&self) -> Aparelho {
        self.aparelho.clone()
    }
    fn idioma(&self) -> String {
        self.idioma.clone()
    }
    fn densidade(&self) -> f32 {
        self.densidade
    }
}

/// Lê `instalado\tversão\tvpn\ttarifada\tips` (ips separados por vírgula) da casca Kotlin.
pub fn sinais_de(texto: &str) -> SinaisRede {
    let c: Vec<&str> = texto.split('\t').collect();
    let sim = |i: usize| c.get(i) == Some(&"1");
    SinaisRede {
        instalado: sim(0),
        versao: c.get(1).filter(|v| !v.is_empty()).map(|v| (*v).to_owned()),
        rede_vpn: sim(2),
        tarifada: sim(3),
        enderecos: c
            .get(4)
            .map(|ips| {
                ips.split(',')
                    .filter_map(|i| i.trim().parse().ok())
                    .collect()
            })
            .unwrap_or_default(),
        dns_tailnet: None,
    }
}

/// Entrada do `NativeActivity` (via `android-activity`).
#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_tag("taildesk")
            .with_max_level(log::LevelFilter::Info),
    );
    let android = Android { app: app.clone() };
    let (aparelho, densidade, idioma) = aparelho_de(&android.texto_do_aparelho());
    let plataforma_idioma_pt = idioma.to_ascii_lowercase().starts_with("pt");
    let pasta = app.internal_data_path();
    let plataforma = PlataformaAndroid {
        android,
        aparelho,
        densidade,
        idioma,
        pasta,
    };
    if let Err(e) = slint::android::init_with_event_listener(app.clone(), |ev| match ev {
        PollEvent::Main(MainEvent::Pause) | PollEvent::Main(MainEvent::LostFocus) => {
            no_app(|a| a.segundo_plano());
        }
        PollEvent::Main(MainEvent::Resume { .. }) | PollEvent::Main(MainEvent::GainedFocus) => {
            no_app(|a| a.frente());
        }
        _ => {}
    }) {
        crate::registrar!("taildesk: Slint no Android: {e}");
        return;
    }
    let ui = match AppCelular::new() {
        Ok(ui) => ui,
        Err(e) => {
            crate::registrar!("taildesk: janela: {e}");
            return;
        }
    };
    // As telas são escritas em pt-BR; fora do português, inglês (o contexto das traduções nasce
    // com a primeira janela).
    if !plataforma_idioma_pt {
        let _ = slint::select_bundled_translation("en");
    }
    let modo = Android { app: app.clone() }.modo_inicial();
    let app_rust = App::new(&ui, Box::new(plataforma));
    match modo.as_str() {
        "demo" => app_rust.demonstracao(),
        "s1" => app_rust.medir_s1(),
        "exemplo" => {
            app_rust.preencher_exemplo();
            ui.set_tela(crate::ui::Tela::Dispositivos);
        }
        _ => {}
    }
    // A casca Kotlin manda os sinais da rede assim que o Rust está pronto.
    Android { app }.chamar(jni_str!("rustPronto"), None);
    if let Err(e) = ui.run() {
        crate::registrar!("taildesk: {e}");
    }
}

/// `Ponte.nativeSinais(texto)`: estado da rede mudou (`NetworkCallback`).
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_santech_taildesk_Ponte_nativeSinais<'c>(
    mut env: EnvUnowned<'c>,
    _classe: JClass<'c>,
    texto: JString<'c>,
) {
    env.with_env(|_env| -> jni::errors::Result<()> {
        let sinais = sinais_de(&texto.to_string());
        no_app(move |a| a.sinais(sinais));
        Ok(())
    })
    .resolve::<jni::errors::LogErrorAndDefault>();
}

/// `Ponte.nativeQr(texto)`: a câmera leu um QR.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_santech_taildesk_Ponte_nativeQr<'c>(
    mut env: EnvUnowned<'c>,
    _classe: JClass<'c>,
    texto: JString<'c>,
) {
    env.with_env(|_env| -> jni::errors::Result<()> {
        let t = texto.to_string();
        no_app(move |a| a.qr_lido(&t));
        Ok(())
    })
    .resolve::<jni::errors::LogErrorAndDefault>();
}

/// `Ponte.nativeArquivosEscolhidos(texto)`: o seletor do sistema devolveu arquivos, já
/// copiados para o cache do app (`caminho\tnome\ttamanho` por linha).
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_santech_taildesk_Ponte_nativeArquivosEscolhidos<'c>(
    mut env: EnvUnowned<'c>,
    _classe: JClass<'c>,
    texto: JString<'c>,
) {
    env.with_env(|_env| -> jni::errors::Result<()> {
        let t = texto.to_string();
        no_app(move |a| a.arquivos_escolhidos(&t));
        Ok(())
    })
    .resolve::<jni::errors::LogErrorAndDefault>();
}

/// `Ponte.nativeTeclaFisica(texto)`: tecla de um teclado físico na sessão, já com o layout do
/// aparelho aplicado (formato em [`crate::fisico`]).
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_santech_taildesk_Ponte_nativeTeclaFisica<'c>(
    mut env: EnvUnowned<'c>,
    _classe: JClass<'c>,
    texto: JString<'c>,
) {
    env.with_env(|_env| -> jni::errors::Result<()> {
        let t = texto.to_string();
        no_app(move |a| a.tecla_fisica(&t));
        Ok(())
    })
    .resolve::<jni::errors::LogErrorAndDefault>();
}

/// `Ponte.nativeEncerrar()`: "Encerrar" na notificação do som em segundo plano (MF-72).
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_santech_taildesk_Ponte_nativeEncerrar<'c>(
    mut env: EnvUnowned<'c>,
    _classe: JClass<'c>,
) {
    env.with_env(|_env| -> jni::errors::Result<()> {
        no_app(|a| a.encerrar_de_fora());
        Ok(())
    })
    .resolve::<jni::errors::LogErrorAndDefault>();
}

/// `Ponte.nativeCompartilhar(texto)`: arquivos do menu de compartilhar do sistema, já no cache
/// (`caminho\tnome\ttamanho` por linha; MF-63).
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_santech_taildesk_Ponte_nativeCompartilhar<'c>(
    mut env: EnvUnowned<'c>,
    _classe: JClass<'c>,
    texto: JString<'c>,
) {
    env.with_env(|_env| -> jni::errors::Result<()> {
        let t = texto.to_string();
        no_app(move |a| a.compartilhar(&t));
        Ok(())
    })
    .resolve::<jni::errors::LogErrorAndDefault>();
}

/// `Ponte.nativeDownloadPublicado(texto)`: o download foi (ou não) para `Download/TailDesk`
/// (`ok\tnome` ou `erro\tnome`).
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_santech_taildesk_Ponte_nativeDownloadPublicado<'c>(
    mut env: EnvUnowned<'c>,
    _classe: JClass<'c>,
    texto: JString<'c>,
) {
    env.with_env(|_env| -> jni::errors::Result<()> {
        let t = texto.to_string();
        no_app(move |a| a.download_publicado(&t));
        Ok(())
    })
    .resolve::<jni::errors::LogErrorAndDefault>();
}

/// `Ponte.nativeLerQuadro(largura, altura, stride, luma)`: decodifica um quadro da câmera.
/// Devolve o texto do QR do TailDesk, ou `null` se não achou (a câmera manda o próximo).
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_santech_taildesk_Ponte_nativeLerQuadro<'c>(
    mut env: EnvUnowned<'c>,
    _classe: JClass<'c>,
    largura: jint,
    altura: jint,
    stride: jint,
    luma: jni::objects::JByteArray<'c>,
) -> jstring {
    env.with_env(|env| -> jni::errors::Result<jstring> {
        let bytes = env.convert_byte_array(&luma)?;
        let (w, h, s) = (
            largura.max(0) as usize,
            altura.max(0) as usize,
            stride.max(0) as usize,
        );
        match crate::qr::ler(w, h, s, &bytes) {
            crate::qr::Leitura::Oferta(o) => Ok(env.new_string(o.to_uri())?.into_raw()),
            crate::qr::Leitura::Invalido(_) | crate::qr::Leitura::Nada => Ok(std::ptr::null_mut()),
        }
    })
    .resolve::<jni::errors::LogErrorAndDefault>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sinais_da_casca_kotlin() {
        let s = sinais_de("1\t1.104.1\t1\t0\t192.168.0.10, 100.64.0.9,lixo");
        assert!(s.instalado && s.rede_vpn && !s.tarifada);
        assert_eq!(s.versao.as_deref(), Some("1.104.1"));
        assert_eq!(s.enderecos.len(), 2);
        assert!(!sinais_de("").instalado);
    }
}
