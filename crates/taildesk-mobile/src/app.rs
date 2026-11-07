//! Liga as telas ([`crate::ui::AppCelular`]) ao estado do app e à rede.
//!
//! Roda na thread da interface (a do Slint); a rede roda num runtime tokio à parte e volta pela
//! fila de eventos do Slint (`invoke_from_event_loop`). O que depende do Android (abrir o app do
//! Tailscale, câmera, área de transferência, links) passa pela [`Plataforma`], que a casca Kotlin
//! implementa por JNI (`crate::android`) e os testes e capturas implementam sem fazer nada.

mod folha;

use std::cell::RefCell;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use taildesk_keyboard::Mods;
use taildesk_keyboard::virtual_kb::{TeclaFaixa, Trava};
use taildesk_overlay::MobileVpnOverlay;
use taildesk_overlay::mobile::{EstadoTailscale, SinaisRede};
use taildesk_proto::control::{HelloAck, Msg};
use taildesk_proto::pair::{PairOffer, normalize_code};
use taildesk_session::conexao::controlador::EventoControlador;
use taildesk_session::{ControllerState, EndReason};
use taildesk_transport::Pin;

use crate::ciclo::{AoVoltar, Ciclo, Hud, indicador_teclado};
use crate::controle::{Aparelho, Controle, hello};
use crate::demo::DemoHost;
use crate::dispositivos::{Dispositivo, Grupo, Lista, Situacao};
use crate::gestos::{Fase, Modo, Toque};
use crate::rede::{Qualidade, prefs};
use crate::sessao::{AbrirDecoder, SessaoRede, decoder_do_sistema};
use crate::textos::Idioma;
use crate::ui::{
    AppCelular, EstadoTs, Etapa, GrupoDispositivos, ItemDispositivo, LinhaTeste, Tela,
};
use crate::video::Quadro;
use crate::zoom::Retangulo;

/// Porta do `taildesk-cli bench-net --ouvir` para o spike S3 (a 7420 é do app desktop).
pub const PORTA_BENCH: u16 = 7421;

/// O que só o Android sabe fazer.
pub trait Plataforma: 'static {
    fn abrir_tailscale(&self) {}
    fn instalar_tailscale(&self) {}
    /// Abre a câmera; o resultado volta por [`App::qr_lido`].
    fn ler_qr(&self) {}
    fn copiar(&self, _texto: &str) {}
    fn abrir_link(&self, _url: &str) {}
    fn otimizacao_bateria(&self) {}
    /// Pasta privada do app (lista de dispositivos, configurações).
    fn pasta_dados(&self) -> Option<PathBuf> {
        None
    }
    fn aparelho(&self) -> Aparelho;
    /// Tag do idioma do sistema ("pt-BR").
    fn idioma(&self) -> String {
        "pt-BR".into()
    }
    /// Pixels físicos por pixel lógico.
    fn densidade(&self) -> f32 {
        1.0
    }
    /// Abre o seletor de arquivos do sistema (SAF); o resultado volta por
    /// [`App::arquivos_escolhidos`], já copiado para uma pasta do app.
    fn escolher_arquivos(&self) {}
    /// Passa um arquivo baixado para a pasta pública (`Download/TailDesk`). `true` = a
    /// plataforma cuidou dele e avisa por [`App::download_publicado`]; `false` = ele fica onde
    /// está.
    fn publicar_download(&self, _caminho: &Path, _nome: &str) -> bool {
        false
    }
}

/// Plataforma que não faz nada (testes, capturas, janela de desenvolvimento).
#[derive(Debug, Clone)]
pub struct SemPlataforma(pub Aparelho);

impl Plataforma for SemPlataforma {
    fn aparelho(&self) -> Aparelho {
        self.0.clone()
    }
    fn idioma(&self) -> String {
        self.0.idioma_teclado.clone()
    }
}

/// Preferências guardadas (Configurações, MF-100).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    pub modo_toque: Modo,
    pub qualidade: Qualidade,
    pub economizar_em_movel: bool,
    pub onboarding_feito: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            modo_toque: Modo::Touchpad,
            qualidade: Qualidade::Automatica,
            economizar_em_movel: true,
            onboarding_feito: false,
        }
    }
}

impl Config {
    pub fn para_texto(&self) -> String {
        format!(
            "modo\t{}\nqualidade\t{}\neconomizar\t{}\nonboarding\t{}\n",
            match self.modo_toque {
                Modo::Touchpad => "touchpad",
                Modo::ToqueDireto => "direto",
            },
            match self.qualidade {
                Qualidade::Automatica => "auto",
                Qualidade::Nitidez => "nitidez",
                Qualidade::Fluidez => "fluidez",
                Qualidade::EconomizarDados => "economia",
            },
            u8::from(self.economizar_em_movel),
            u8::from(self.onboarding_feito),
        )
    }

    pub fn de_texto(t: &str) -> Config {
        let mut c = Config::default();
        for l in t.lines() {
            match l.split_once('\t') {
                Some(("modo", "direto")) => c.modo_toque = Modo::ToqueDireto,
                Some(("qualidade", q)) => {
                    c.qualidade = match q {
                        "nitidez" => Qualidade::Nitidez,
                        "fluidez" => Qualidade::Fluidez,
                        "economia" => Qualidade::EconomizarDados,
                        _ => Qualidade::Automatica,
                    }
                }
                Some(("economizar", v)) => c.economizar_em_movel = v != "0",
                Some(("onboarding", v)) => c.onboarding_feito = v == "1",
                _ => {}
            }
        }
        c
    }
}

/// Sessão em andamento (de verdade ou de demonstração).
enum Alvo {
    Rede(Option<SessaoRede>),
    Demo(DemoHost),
}

struct Estado {
    plat: Box<dyn Plataforma>,
    idioma: Idioma,
    overlay: MobileVpnOverlay,
    lista: Lista,
    config: Config,
    rt: tokio::runtime::Runtime,
    endpoint: Option<quinn::Endpoint>,
    abrir_decoder: AbrirDecoder,
    /// Host da conexão ou sessão atual.
    host: Option<Dispositivo>,
    alvo: Option<Alvo>,
    ack: Option<HelloAck>,
    controle: Option<Controle>,
    ciclo: Ciclo,
    hud: Hud,
    hud_visivel: bool,
    area: Retangulo,
    quadro: Quadro,
    quadros_no_segundo: u32,
    inicio: Instant,
    /// Muda a cada conexão: eventos de uma conexão cancelada são ignorados.
    geracao: u64,
    prazo_consentimento: Option<Instant>,
    detalhe: Option<IpAddr>,
    busca: String,
    teste: Option<IpAddr>,
    /// Tela presa (lista de exemplo dos testes e capturas): o estado do Tailscale não a troca.
    tela_fixa: bool,
    /// Folha de arquivos da sessão.
    arquivos: crate::arquivos::Folha,
    ultimo_desenho_arquivos: Option<Instant>,
}

/// O app inteiro (um por processo, na thread da interface).
pub struct App {
    ui: slint::Weak<AppCelular>,
    estado: RefCell<Estado>,
    relogio: slint::Timer,
    relogio_lento: slint::Timer,
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App").finish_non_exhaustive()
    }
}

thread_local! {
    static APP: RefCell<Option<Rc<App>>> = const { RefCell::new(None) };
}

/// Roda `f` com o app, na thread da interface (de qualquer thread).
pub fn no_app(f: impl FnOnce(&Rc<App>) + Send + 'static) {
    let _ = slint::invoke_from_event_loop(move || {
        APP.with_borrow(|a| {
            if let Some(a) = a {
                f(a);
            }
        });
    });
}

fn agora_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

fn trava(t: Trava) -> i32 {
    match t {
        Trava::Solta => 0,
        Trava::UmaVez => 1,
        Trava::Travada => 2,
    }
}

fn tecla_da_faixa(nome: &str) -> Option<TeclaFaixa> {
    Some(match nome {
        "esc" => TeclaFaixa::Esc,
        "tab" => TeclaFaixa::Tab,
        "enter" => TeclaFaixa::Enter,
        "del" => TeclaFaixa::Delete,
        "esquerda" => TeclaFaixa::Esquerda,
        "direita" => TeclaFaixa::Direita,
        "cima" => TeclaFaixa::Cima,
        "baixo" => TeclaFaixa::Baixo,
        "home" => TeclaFaixa::Home,
        "end" => TeclaFaixa::End,
        "pgup" => TeclaFaixa::PageUp,
        "pgdn" => TeclaFaixa::PageDown,
        f if f.starts_with('f') => TeclaFaixa::F(f[1..].parse().ok()?),
        _ => return None,
    })
}

fn modificador(nome: &str) -> Mods {
    match nome {
        "ctrl" => Mods::CTRL,
        "alt" => Mods::ALT,
        "meta" => Mods::META,
        _ => Mods::SHIFT,
    }
}

impl Estado {
    fn guardar(&self) {
        let Some(p) = self.plat.pasta_dados() else {
            return;
        };
        let _ = std::fs::create_dir_all(&p);
        let _ = std::fs::write(p.join("dispositivos"), self.lista.para_texto());
        let _ = std::fs::write(p.join("config"), self.config.para_texto());
    }

    fn t_ms(&self) -> u64 {
        u64::try_from(self.inicio.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn item(&self, d: &Dispositivo) -> ItemDispositivo {
        let i = self.idioma;
        let os = match d.os.as_str() {
            "windows" => "Windows",
            "linux" => "Linux",
            "macOS" => "macOS",
            "android" => "Android",
            o => o,
        };
        let estado = match d.situacao {
            Situacao::ComTaildesk | Situacao::Online => "online",
            Situacao::SemTaildesk => i.t("sem TailDesk aberto", "TailDesk not running"),
            Situacao::Offline => "offline",
            Situacao::Desconhecida => i.t("estado desconhecido", "unknown"),
        };
        ItemDispositivo {
            nome: d.nome.as_str().into(),
            detalhe: if os.is_empty() {
                estado.into()
            } else {
                format!("{estado} · {os}").into()
            },
            situacao: match d.situacao {
                Situacao::ComTaildesk => 0,
                Situacao::SemTaildesk => 1,
                Situacao::Online => 2,
                Situacao::Offline => 3,
                Situacao::Desconhecida => 4,
            },
            favorito: d.favorito,
            conectavel: d.conectavel(),
            ip: d.ip.to_string().into(),
            os: d.os.as_str().into(),
        }
    }

    fn prefs(&self) -> Option<taildesk_proto::control::QualityPrefs> {
        prefs(
            self.config.qualidade,
            self.overlay.sinais().tarifada,
            self.config.economizar_em_movel,
        )
    }
}

impl App {
    /// Cria o app e liga a janela. Guarde o `Rc` enquanto a janela existir.
    pub fn new(ui: &AppCelular, plat: Box<dyn Plataforma>) -> Rc<App> {
        let idioma = Idioma::do_sistema(&plat.idioma());
        let pasta = plat.pasta_dados();
        let ler = |nome: &str| {
            pasta
                .as_ref()
                .and_then(|p| std::fs::read_to_string(p.join(nome)).ok())
                .unwrap_or_default()
        };
        let lista = Lista::de_texto(&ler("dispositivos"));
        let config = Config::de_texto(&ler("config"));
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("taildesk-rede")
            .enable_all()
            .build()
            .expect("runtime tokio");
        let endpoint = {
            let _g = rt.enter();
            taildesk_transport::client_endpoint("0.0.0.0:0".parse().expect("endereço"))
                .map_err(|e| crate::registrar!("taildesk: endpoint QUIC: {e}"))
                .ok()
        };
        let versao = plat.aparelho().versao_app;
        let app = Rc::new(App {
            ui: ui.as_weak(),
            estado: RefCell::new(Estado {
                plat,
                idioma,
                overlay: MobileVpnOverlay::default(),
                lista,
                config,
                rt,
                endpoint,
                abrir_decoder: decoder_do_sistema(),
                host: None,
                alvo: None,
                ack: None,
                controle: None,
                ciclo: Ciclo::default(),
                hud: Hud::default(),
                hud_visivel: false,
                area: Retangulo {
                    x: 0.0,
                    y: 0.0,
                    w: 915.0,
                    h: 412.0,
                },
                quadro: Quadro::default(),
                quadros_no_segundo: 0,
                inicio: Instant::now(),
                geracao: 0,
                prazo_consentimento: None,
                detalhe: None,
                busca: String::new(),
                teste: None,
                tela_fixa: false,
                arquivos: crate::arquivos::Folha::new(),
                ultimo_desenho_arquivos: None,
            }),
            relogio: slint::Timer::default(),
            relogio_lento: slint::Timer::default(),
        });
        APP.with_borrow_mut(|a| *a = Some(app.clone()));
        ui.set_versao_app(versao.as_str().into());
        app.ligar(ui);
        app.mostrar_config();
        app.mostrar_lista();
        app
    }

    fn ui(&self) -> Option<AppCelular> {
        self.ui.upgrade()
    }

    fn toast(&self, texto: impl Into<SharedString>) {
        if let Some(ui) = self.ui() {
            ui.set_toast(texto.into());
        }
    }

    fn t<'a>(&self, pt: &'a str, en: &'a str) -> &'a str {
        self.estado.borrow().idioma.t(pt, en)
    }

    // -----------------------------------------------------------------------------------------
    // Tailscale (onboarding)

    /// Novos sinais da rede (casca Kotlin, a cada `NetworkCallback`).
    pub fn sinais(self: &Rc<Self>, sinais: SinaisRede) {
        let versao = sinais.versao.clone().unwrap_or_default();
        let c = self.estado.borrow().overlay.definir_sinais(sinais);
        self.mostrar_ts(c.estado, &versao, c.versao_antiga);
        if c.estado == EstadoTailscale::Conectado {
            // "Conectado" exige que o DNS da tailnet responda deste processo.
            let overlay = self.estado.borrow().overlay.clone();
            self.estado.borrow().rt.spawn(async move {
                overlay.testar_dns().await;
                let c = overlay.classificacao();
                let versao = overlay.sinais().versao.unwrap_or_default();
                no_app(move |a| a.mostrar_ts(c.estado, &versao, c.versao_antiga));
            });
        }
        if let Some(ui) = self.ui() {
            ui.set_tarifada(self.estado.borrow().overlay.sinais().tarifada);
        }
    }

    fn mostrar_ts(self: &Rc<Self>, e: EstadoTailscale, versao: &str, antiga: bool) {
        let Some(ui) = self.ui() else { return };
        ui.set_estado_ts(match e {
            EstadoTailscale::NaoInstalado => EstadoTs::NaoInstalado,
            EstadoTailscale::Desligado => EstadoTs::Desligado,
            EstadoTailscale::OutraVpn => EstadoTs::OutraVpn,
            EstadoTailscale::ForaDoTunel => EstadoTs::ForaDoTunel,
            EstadoTailscale::Conectado => EstadoTs::Conectado,
        });
        ui.set_versao_ts(versao.into());
        ui.set_versao_ts_antiga(antiga);
        if self.estado.borrow().tela_fixa {
            return;
        }
        // Quem já passou pelo onboarding vai direto à lista quando o Tailscale está bem.
        let feito = self.estado.borrow().config.onboarding_feito;
        if ui.get_tela() == Tela::Onboarding && feito && e == EstadoTailscale::Conectado {
            ui.set_tela(Tela::Dispositivos);
            self.atualizar_lista();
        }
        if e != EstadoTailscale::Conectado
            && matches!(ui.get_tela(), Tela::Dispositivos | Tela::Encontrar)
        {
            ui.set_tela(Tela::Onboarding);
        }
    }

    fn continuar_onboarding(self: &Rc<Self>) {
        let tem_pareado = {
            let mut e = self.estado.borrow_mut();
            e.config.onboarding_feito = true;
            e.guardar();
            e.lista.pareados().next().is_some()
        };
        if let Some(ui) = self.ui() {
            ui.set_tela(if tem_pareado {
                Tela::Dispositivos
            } else {
                Tela::Encontrar
            });
        }
        self.atualizar_lista();
    }

    // -----------------------------------------------------------------------------------------
    // Pareamento

    /// Texto lido pela câmera (QrScanActivity → JNI).
    pub fn qr_lido(self: &Rc<Self>, texto: &str) {
        match PairOffer::from_uri(texto) {
            Ok(o) => self.parear(o),
            Err(e) => self.erro_pareamento(&format!(
                "{} ({e})",
                self.t(
                    "Esse QR não é de um computador com o TailDesk",
                    "That QR code is not from a computer running TailDesk"
                )
            )),
        }
    }

    fn erro_pareamento(&self, texto: &str) {
        if let Some(ui) = self.ui() {
            ui.set_pareando(false);
            ui.set_erro_pareamento(texto.into());
            if ui.get_tela() != Tela::Encontrar {
                ui.set_tela(Tela::Encontrar);
            }
        }
    }

    fn texto_pair(&self, r: &taildesk_proto::control::PairResult) -> String {
        use taildesk_proto::control::PairError;
        match r.error() {
            PairError::BadCode => self.t(
                "Código errado ou já usado. Gere outro no computador.",
                "Wrong or already used code. Make a new one on the computer.",
            ),
            PairError::Expired => self.t(
                "O código venceu (vale 5 minutos). Gere outro no computador.",
                "The code expired (it lasts 5 minutes). Make a new one on the computer.",
            ),
            PairError::TooManyAttempts => self.t(
                "Códigos errados demais. Espere alguns minutos.",
                "Too many wrong codes. Wait a few minutes.",
            ),
            PairError::Disabled => self.t(
                "Esse computador não aceita celulares (Configurações → Celulares).",
                "That computer does not accept phones (Settings → Phones).",
            ),
            PairError::Unspecified => self.t(
                "O computador recusou o pareamento.",
                "The computer refused to pair.",
            ),
        }
        .to_owned()
    }

    fn parear(self: &Rc<Self>, oferta: PairOffer) {
        let (endpoint, aparelho) = {
            let e = self.estado.borrow();
            (e.endpoint.clone(), e.plat.aparelho().modelo)
        };
        let Some(endpoint) = endpoint else {
            return self.erro_pareamento(self.t("Sem rede.", "No network."));
        };
        if let Some(ui) = self.ui() {
            ui.set_pareando(true);
            ui.set_erro_pareamento("".into());
        }
        self.estado.borrow().rt.spawn(async move {
            let r = taildesk_session::conexao::servico::parear(&endpoint, &oferta, &aparelho).await;
            no_app(move |a| match r {
                Ok(p) if p.ok => a.pareou(&oferta.host_name, oferta.addr, oferta.fingerprint),
                Ok(p) => a.erro_pareamento(&a.texto_pair(&p)),
                Err(e) => a.erro_pareamento(&format!(
                    "{}: {e}",
                    a.t(
                        "Não deu para falar com o computador",
                        "Could not reach the computer"
                    )
                )),
            });
        });
    }

    fn parear_codigo(self: &Rc<Self>, nome: &str, codigo: &str) {
        let Some(codigo) = normalize_code(codigo) else {
            return self.erro_pareamento(self.t(
                "O código tem 6 letras e números (ex.: K7P-2QX).",
                "The code has 6 letters and digits (e.g. K7P-2QX).",
            ));
        };
        let nome = nome.trim().to_owned();
        let (endpoint, aparelho) = {
            let e = self.estado.borrow();
            (e.endpoint.clone(), e.plat.aparelho().modelo)
        };
        let Some(endpoint) = endpoint else { return };
        if let Some(ui) = self.ui() {
            ui.set_pareando(true);
            ui.set_erro_pareamento("".into());
        }
        self.estado.borrow().rt.spawn(async move {
            // Nome MagicDNS resolve pelo DNS do sistema (o Tailscale cuida); IP vai direto.
            let destino = match nome.parse::<IpAddr>() {
                Ok(ip) => Some(SocketAddr::new(ip, taildesk_proto::DEFAULT_PORT)),
                Err(_) => tokio::net::lookup_host((nome.as_str(), taildesk_proto::DEFAULT_PORT))
                    .await
                    .ok()
                    .and_then(|mut i| i.next()),
            };
            let Some(destino) = destino else {
                no_app(move |a| {
                    a.erro_pareamento(a.t(
                        "Não achei esse computador na tailnet.",
                        "Could not find that computer in the tailnet.",
                    ))
                });
                return;
            };
            let r = taildesk_session::conexao::servico::parear_com_codigo(
                &endpoint, destino, &codigo, &aparelho,
            )
            .await;
            no_app(move |a| match r {
                Ok((p, fp)) if p.ok => a.pareou(&nome, destino.ip(), fp),
                Ok((p, _)) => a.erro_pareamento(&a.texto_pair(&p)),
                Err(e) => a.erro_pareamento(&format!(
                    "{}: {e}",
                    a.t(
                        "Não deu para falar com o computador",
                        "Could not reach the computer"
                    )
                )),
            });
        });
    }

    fn pareou(self: &Rc<Self>, nome: &str, ip: IpAddr, fp: [u8; 32]) {
        {
            let mut e = self.estado.borrow_mut();
            e.lista.pareou(nome, ip, fp);
            e.overlay.nomear(ip, nome);
            e.config.onboarding_feito = true;
            e.guardar();
        }
        if let Some(ui) = self.ui() {
            ui.set_pareando(false);
            ui.set_tela(Tela::Dispositivos);
        }
        self.toast(format!("{} {nome}", self.t("Pareado com", "Paired with")));
        self.atualizar_lista();
    }

    // -----------------------------------------------------------------------------------------
    // Dispositivos

    fn mostrar_lista(&self) {
        let Some(ui) = self.ui() else { return };
        let e = self.estado.borrow();
        let i = e.idioma;
        let grupos: Vec<GrupoDispositivos> = e
            .lista
            .agrupar(&e.busca)
            .into_iter()
            .map(|(g, itens)| {
                let via = itens
                    .iter()
                    .find(|d| !d.via.is_empty())
                    .map(|d| d.via.clone())
                    .unwrap_or_default();
                let titulo = match g {
                    Grupo::Favoritos => i.t("FAVORITOS", "FAVORITES").to_owned(),
                    Grupo::Recentes => i.t("RECENTES", "RECENT").to_owned(),
                    Grupo::NaTailnet if via.is_empty() => {
                        i.t("NA TAILNET", "IN THE TAILNET").to_owned()
                    }
                    Grupo::NaTailnet => format!(
                        "{} ({} {})",
                        i.t("NA TAILNET", "IN THE TAILNET"),
                        i.t("VIA", "VIA"),
                        via.to_uppercase()
                    ),
                };
                GrupoDispositivos {
                    titulo: titulo.into(),
                    itens: ModelRc::new(VecModel::from(
                        itens.iter().map(|d| e.item(d)).collect::<Vec<_>>(),
                    )),
                }
            })
            .collect();
        ui.set_lista_vazia(e.lista.itens().is_empty());
        ui.set_grupos(ModelRc::new(VecModel::from(grupos)));
        let pareados: Vec<SharedString> =
            e.lista.pareados().map(|d| d.nome.as_str().into()).collect();
        ui.set_pareados(ModelRc::new(VecModel::from(pareados)));
    }

    /// Pede a lista aos desktops pareados e sonda os favoritos (MF-11/12/14).
    fn atualizar_lista(self: &Rc<Self>) {
        let (endpoint, fontes, sondar, meu_ip) = {
            let e = self.estado.borrow();
            let fontes: Vec<(String, IpAddr, [u8; 32])> = e
                .lista
                .pareados()
                .filter_map(|d| Some((d.nome.clone(), d.ip, d.fingerprint?)))
                .collect();
            let sondar: Vec<(IpAddr, Pin)> = e
                .lista
                .itens()
                .iter()
                .filter(|d| d.favorito || d.usado_ms > 0)
                .map(|d| (d.ip, d.fingerprint.map_or(Pin::FirstContact, Pin::Sha256)))
                .collect();
            (
                e.endpoint.clone(),
                fontes,
                sondar,
                e.overlay.sinais().ip_tailnet(),
            )
        };
        let Some(endpoint) = endpoint else { return };
        if let Some(ui) = self.ui() {
            ui.set_atualizando(true);
        }
        self.estado.borrow().rt.spawn(async move {
            for (nome, ip, fp) in fontes {
                let addr = SocketAddr::new(ip, taildesk_proto::DEFAULT_PORT);
                let r = tokio::time::timeout(
                    Duration::from_secs(8),
                    taildesk_session::conexao::servico::pedir_lista(
                        &endpoint,
                        addr,
                        Pin::Sha256(fp),
                    ),
                )
                .await;
                if let Ok(Ok(entradas)) = r {
                    no_app(move |a| {
                        a.estado
                            .borrow_mut()
                            .lista
                            .mesclar(&nome, &entradas, meu_ip);
                        a.estado.borrow_mut().lista.sondado(ip, true);
                        a.mostrar_lista();
                    });
                }
            }
            for (ip, pin) in sondar {
                let addr = SocketAddr::new(ip, taildesk_proto::DEFAULT_PORT);
                let ok = matches!(
                    tokio::time::timeout(
                        Duration::from_secs(3),
                        taildesk_transport::connect(&endpoint, addr, pin)
                    )
                    .await,
                    Ok(Ok(_))
                );
                no_app(move |a| a.estado.borrow_mut().lista.sondado(ip, ok));
            }
            no_app(|a| {
                a.estado.borrow().guardar();
                a.mostrar_lista();
                if let Some(ui) = a.ui() {
                    ui.set_atualizando(false);
                }
            });
        });
    }

    fn dispositivo(&self, item: &ItemDispositivo) -> Option<Dispositivo> {
        let ip: IpAddr = item.ip.parse().ok()?;
        self.estado.borrow().lista.por_ip(ip).cloned()
    }

    // -----------------------------------------------------------------------------------------
    // Conexão e sessão

    fn etapas(&self, atual: usize, falhou: bool) {
        let Some(ui) = self.ui() else { return };
        let nome = ui.get_conectando_nome();
        let textos = [
            format!("{} {nome}…", self.t("Encontrando", "Finding")),
            format!(
                "{} {nome} {}",
                self.t("Esperando", "Waiting for"),
                self.t("aceitar…", "to accept…")
            ),
            self.t("Abrindo a imagem…", "Opening the screen…")
                .to_owned(),
        ];
        let etapas: Vec<Etapa> = textos
            .iter()
            .enumerate()
            .map(|(i, t)| Etapa {
                texto: t.as_str().into(),
                estado: if i < atual {
                    2
                } else if i == atual {
                    if falhou { 3 } else { 1 }
                } else {
                    0
                },
            })
            .collect();
        ui.set_etapas(ModelRc::new(VecModel::from(etapas)));
    }

    fn conectar(self: &Rc<Self>, d: Dispositivo) {
        let Some(ui) = self.ui() else { return };
        ui.set_conectando_nome(d.nome.as_str().into());
        ui.set_erro_conexao("".into());
        ui.set_restante_consentimento(0);
        ui.set_tela(Tela::Conectando);
        self.etapas(0, false);
        let (endpoint, h, abrir, geracao) = {
            let mut e = self.estado.borrow_mut();
            e.geracao += 1;
            e.host = Some(d.clone());
            e.ack = None;
            (
                e.endpoint.clone(),
                hello(&e.plat.aparelho()),
                e.abrir_decoder.clone(),
                e.geracao,
            )
        };
        let Some(endpoint) = endpoint else { return };
        let pin = d.fingerprint.map_or(Pin::FirstContact, Pin::Sha256);
        let addr = SocketAddr::new(d.ip, taildesk_proto::DEFAULT_PORT);
        let nome = d.nome.clone();
        let acordar: Arc<dyn Fn() + Send + Sync> = Arc::new(|| no_app(|a| a.novo_quadro()));
        self.estado.borrow().rt.spawn(async move {
            let r = SessaoRede::conectar(&endpoint, addr, pin, h, &nome, abrir, acordar).await;
            match r {
                Ok((sessao, mut eventos)) => {
                    no_app(move |a| a.conectou(geracao, sessao));
                    while let Some(ev) = eventos.recv().await {
                        no_app(move |a| a.evento(geracao, ev));
                    }
                }
                Err(e) => {
                    let texto = e.to_string();
                    no_app(move |a| a.falhou(geracao, &texto));
                }
            }
        });
    }

    fn conectou(&self, geracao: u64, sessao: SessaoRede) {
        let mut e = self.estado.borrow_mut();
        if e.geracao != geracao {
            drop(e);
            self.estado.borrow().rt.spawn(sessao.encerrar());
            return;
        }
        let fp = sessao.fingerprint();
        if let Some(h) = &e.host {
            let ip = h.ip;
            e.lista.usou(ip, agora_ms(), fp);
            e.guardar();
        }
        e.alvo = Some(Alvo::Rede(Some(sessao)));
    }

    fn falhou(&self, geracao: u64, texto: &str) {
        if self.estado.borrow().geracao != geracao {
            return;
        }
        let msg = if texto.contains("certificado") || texto.contains("certificate") {
            self.t(
                "O certificado desse computador mudou. Se foi reinstalado, esqueça e pareie de novo.",
                "This computer's certificate changed. If it was reinstalled, forget it and pair again.",
            )
            .to_owned()
        } else {
            format!(
                "{} ({texto})",
                self.t(
                    "Não deu para conectar. O TailDesk está aberto nesse computador?",
                    "Could not connect. Is TailDesk open on that computer?"
                )
            )
        };
        if let Some(ui) = self.ui() {
            ui.set_erro_conexao(msg.into());
            ui.set_tela(Tela::Conectando);
        }
        self.etapas(0, true);
        self.fim_da_sessao();
    }

    fn texto_fim(&self, r: EndReason) -> Option<&'static str> {
        Some(match r {
            EndReason::Refused => self.t(
                "A pessoa no computador recusou.",
                "The person at the computer refused.",
            ),
            EndReason::NoAnswer => self.t(
                "Ninguém respondeu no computador em 60 s.",
                "Nobody answered at the computer within 60 s.",
            ),
            EndReason::NotAllowed => self.t(
                "Esse computador não aceita conexões deste celular.",
                "That computer does not accept connections from this phone.",
            ),
            EndReason::VersionMismatch => self.t(
                "Versões do TailDesk diferentes: atualize os dois.",
                "Different TailDesk versions: update both.",
            ),
            EndReason::ByHost => self.t(
                "O computador encerrou a sessão.",
                "The computer ended the session.",
            ),
            EndReason::Lost => self.t("A conexão caiu.", "The connection dropped."),
            EndReason::ProtocolError => self.t("Erro de protocolo.", "Protocol error."),
            EndReason::BadPassword | EndReason::TooManyAttempts => {
                self.t("Acesso recusado.", "Access refused.")
            }
            EndReason::ByUser => return None,
        })
    }

    fn evento(self: &Rc<Self>, geracao: u64, ev: EventoControlador) {
        if self.estado.borrow().geracao != geracao {
            return;
        }
        let Some(ui) = self.ui() else { return };
        match ev {
            EventoControlador::Estado(ControllerState::Connecting) => self.etapas(0, false),
            EventoControlador::Estado(ControllerState::AwaitingConsent { .. }) => {
                self.estado.borrow_mut().prazo_consentimento =
                    Some(Instant::now() + taildesk_session::CONSENT_TIMEOUT);
                self.etapas(1, false);
            }
            EventoControlador::Estado(ControllerState::Live { .. }) => {
                self.estado.borrow_mut().prazo_consentimento = None;
                self.etapas(2, false);
                self.abrir_sessao();
            }
            EventoControlador::Estado(ControllerState::Ended(r)) => {
                let em_sessao = ui.get_tela() == Tela::Sessao;
                if r == EndReason::Lost
                    && em_sessao
                    && self.estado.borrow().ciclo.em_segundo_plano()
                {
                    // Caiu com o app fora: reconecta ao voltar (MF-49).
                    self.estado.borrow_mut().ciclo.conexao_caiu();
                    return;
                }
                if let Some(t) = self.texto_fim(r) {
                    if em_sessao {
                        self.toast(t);
                        ui.set_tela(Tela::Dispositivos);
                    } else {
                        ui.set_erro_conexao(t.into());
                        let passo = if self.estado.borrow().prazo_consentimento.is_some() {
                            1
                        } else {
                            0
                        };
                        self.etapas(passo, true);
                    }
                } else {
                    ui.set_tela(Tela::Dispositivos);
                }
                self.fim_da_sessao();
            }
            EventoControlador::Ack(ack) => {
                let mut e = self.estado.borrow_mut();
                e.ack = Some(*ack);
            }
            EventoControlador::Mensagem(m) => self.do_host(*m),
            EventoControlador::Arquivos(ev) => self.evento_arquivos(&ev),
            EventoControlador::Forma(_) => {}
        }
    }

    fn abrir_sessao(self: &Rc<Self>) {
        let Some(ui) = self.ui() else { return };
        let mut out = Vec::new();
        {
            let mut e = self.estado.borrow_mut();
            let densidade = e.plat.densidade();
            let mut c = Controle::new(e.config.modo_toque, densidade, e.area);
            c.prefs = e.prefs();
            let (monitores, layout) = match (&e.alvo, &e.ack) {
                (Some(Alvo::Demo(_)), _) => (crate::demo::monitores(), "br-abnt2".to_owned()),
                (_, Some(a)) => {
                    c.atencao_segura = a.secure_attention;
                    (
                        a.displays.clone(),
                        a.keyboard
                            .as_ref()
                            .map(|k| k.layout_tag.clone())
                            .unwrap_or_default(),
                    )
                }
                _ => (Vec::new(), String::new()),
            };
            c.monitores_mudaram(&monitores, &mut out);
            ui.set_modo_toque(i32::from(e.config.modo_toque == Modo::ToqueDireto));
            ui.set_indicador_teclado(indicador_teclado(&layout, e.idioma).into());
            e.controle = Some(c);
            e.ciclo = Ciclo::default();
            e.hud = Hud::default();
        }
        ui.set_tem_quadro(false);
        ui.set_barra_aberta(true);
        ui.set_teclado_aberto(false);
        ui.set_tela(Tela::Sessao);
        self.enviar(out);
        self.mostrar_sessao();
        let fraco = Rc::downgrade(self);
        self.relogio.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(16),
            move || {
                if let Some(a) = fraco.upgrade() {
                    a.tick();
                }
            },
        );
        let fraco = Rc::downgrade(self);
        self.relogio_lento.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(250),
            move || {
                if let Some(a) = fraco.upgrade() {
                    a.tick_lento();
                }
            },
        );
    }

    fn fim_da_sessao(&self) {
        self.relogio.stop();
        self.relogio_lento.stop();
        let mut e = self.estado.borrow_mut();
        e.controle = None;
        e.prazo_consentimento = None;
        if let Some(Alvo::Rede(Some(s))) = e.alvo.take() {
            e.rt.spawn(s.encerrar());
        }
        let sobras = e.arquivos.fechar();
        drop(e);
        folha::apagar(sobras);
        if let Some(ui) = self.ui() {
            ui.set_arquivos_aberto(false);
        }
        self.desenhar_arquivos();
    }

    fn encerrar(self: &Rc<Self>) {
        let mut out = Vec::new();
        if let Some(c) = self.estado.borrow_mut().controle.as_mut() {
            c.soltar_tudo(&mut out);
        }
        self.enviar(out);
        self.estado.borrow_mut().geracao += 1;
        self.fim_da_sessao();
        if let Some(ui) = self.ui() {
            ui.set_tela(Tela::Dispositivos);
        }
        self.mostrar_lista();
    }

    /// Mensagens do host durante a sessão.
    fn do_host(&self, m: Msg) {
        let mut out = Vec::new();
        {
            let mut e = self.estado.borrow_mut();
            let e = &mut *e;
            match &m {
                Msg::CodecSelected(c) => {
                    if let Some(Alvo::Rede(Some(s))) = &e.alvo
                        && let Err(err) = s.codec_escolhido(c)
                    {
                        drop(err);
                        e.hud.decode_hw = false;
                    } else {
                        e.hud.decode_hw = cfg!(target_os = "android");
                    }
                }
                Msg::VideoStopped(v) => {
                    if let Some(Alvo::Rede(Some(s))) = &e.alvo {
                        s.video_parou(v.display_id);
                    }
                }
                Msg::DisplaysChanged(d) => {
                    if let Some(c) = e.controle.as_mut() {
                        c.monitores_mudaram(&d.displays, &mut out);
                    }
                }
                Msg::CursorPos(p) => {
                    if let Some(c) = e.controle.as_mut() {
                        c.cursor_do_host(p);
                    }
                }
                Msg::PathInfo(p) => {
                    if let Some(h) = &e.host {
                        e.overlay.informar_caminho(
                            h.ip,
                            taildesk_overlay::PathInfo {
                                kind: p.path_kind(),
                                rtt: (p.rtt_ms > 0)
                                    .then(|| Duration::from_millis(u64::from(p.rtt_ms))),
                                derp_region: (!p.relay_region.is_empty())
                                    .then(|| p.relay_region.clone()),
                                ..Default::default()
                            },
                        );
                    }
                    e.hud.caminho = Some(p.clone());
                    if e.hud.mostrar_sozinho() {
                        e.hud_visivel = true;
                    }
                }
                _ => {}
            }
        }
        self.enviar(out);
        self.mostrar_sessao();
    }

    /// Manda ao host (ou ao host de demonstração).
    fn enviar(&self, msgs: Vec<Msg>) {
        if msgs.is_empty() {
            return;
        }
        let mut voltas = Vec::new();
        {
            let mut e = self.estado.borrow_mut();
            match e.alvo.as_mut() {
                Some(Alvo::Rede(Some(s))) => {
                    for m in msgs {
                        s.enviar(m);
                    }
                }
                Some(Alvo::Demo(d)) => {
                    for m in &msgs {
                        voltas.extend(d.receber(m));
                    }
                }
                _ => {}
            }
        }
        for v in voltas {
            if let Msg::CursorPos(p) = &v
                && let Some(c) = self.estado.borrow_mut().controle.as_mut()
            {
                c.cursor_do_host(p);
            }
        }
        self.mostrar_sessao();
    }

    fn com_controle(&self, f: impl FnOnce(&mut Controle, &mut Vec<Msg>)) {
        let mut out = Vec::new();
        if let Some(c) = self.estado.borrow_mut().controle.as_mut() {
            f(c, &mut out);
        }
        self.enviar(out);
    }

    fn novo_quadro(&self) {
        let mut e = self.estado.borrow_mut();
        let e = &mut *e;
        let Some(Alvo::Rede(Some(s))) = &e.alvo else {
            return;
        };
        if !s.caixa.pegar(&mut e.quadro) {
            return;
        }
        e.quadros_no_segundo += 1;
        let q = &e.quadro;
        if q.largura == 0 || q.rgba.len() < (q.largura * q.altura * 4) as usize {
            return;
        }
        let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
            &q.rgba, q.largura, q.altura,
        );
        if let Some(ui) = self.ui() {
            ui.set_quadro(slint::Image::from_rgba8(buf));
            ui.set_tem_quadro(true);
        }
    }

    /// Atualiza a tela da sessão (posição da imagem, cursor, travas, monitores, HUD).
    fn mostrar_sessao(&self) {
        let Some(ui) = self.ui() else { return };
        let e = self.estado.borrow();
        let Some(c) = &e.controle else { return };
        let img = c.vista.imagem();
        ui.set_img_x(img.x);
        ui.set_img_y(img.y);
        ui.set_img_w(img.w);
        ui.set_img_h(img.h);
        let (cx, cy) = c.vista.fracao_para_tela(c.cursor().0, c.cursor().1);
        ui.set_cursor_x(cx);
        ui.set_cursor_y(cy);
        ui.set_cursor_visivel(true);
        ui.set_trava_ctrl(trava(c.teclado.trava(Mods::CTRL)));
        ui.set_trava_alt(trava(c.teclado.trava(Mods::ALT)));
        ui.set_trava_meta(trava(c.teclado.trava(Mods::META)));
        ui.set_trava_shift(trava(c.teclado.trava(Mods::SHIFT)));
        let (i, n) = c.monitores.posicao();
        ui.set_monitor_texto(format!("{i}/{n}").into());
        ui.set_varios_monitores(n > 1);
        let nomes: Vec<SharedString> = c
            .monitores
            .lista()
            .iter()
            .enumerate()
            .map(|(k, d)| {
                if d.name.is_empty() {
                    format!("{} {}", e.idioma.t("Monitor", "Display"), k + 1).into()
                } else {
                    d.name.as_str().into()
                }
            })
            .collect();
        ui.set_monitores(ModelRc::new(VecModel::from(nomes)));
        ui.set_monitor_atual(i.saturating_sub(1) as i32);
        ui.set_hud_visivel(e.hud_visivel);
        ui.set_hud(e.hud.linha(e.idioma).into());
        if let Some(Alvo::Demo(d)) = &e.alvo {
            ui.set_demo_monitor(d.monitor as i32);
            ui.set_demo_texto(d.texto.as_str().into());
            let reg: Vec<SharedString> = d.registro.iter().map(|r| r.as_str().into()).collect();
            ui.set_demo_registro(ModelRc::new(VecModel::from(reg)));
        }
    }

    fn tick(&self) {
        let t = self.estado.borrow().t_ms();
        self.com_controle(|c, out| c.tick(t, out));
        let encerrar = self.estado.borrow_mut().ciclo.tick(t);
        if encerrar {
            self.estado.borrow_mut().geracao += 1;
            self.fim_da_sessao();
            if let Some(ui) = self.ui() {
                ui.set_tela(Tela::Dispositivos);
            }
        }
    }

    fn tick_lento(&self) {
        let (relatorios, restante) = {
            let mut e = self.estado.borrow_mut();
            let e = &mut *e;
            let mut rel = Vec::new();
            if let Some(Alvo::Rede(Some(s))) = &e.alvo {
                rel = s.relatorios();
                e.hud.rtt_ms = s.rtt_ms();
                if let Some(Msg::ReceiverReport(r)) = rel.first() {
                    e.hud.decode_us = r.decode_us;
                    e.hud.perda = r.loss_ppm as f32 / 1_000_000.0;
                }
            }
            (
                rel,
                e.prazo_consentimento
                    .map(|p| p.saturating_duration_since(Instant::now())),
            )
        };
        self.enviar(relatorios);
        if let Some(r) = restante
            && let Some(ui) = self.ui()
        {
            ui.set_restante_consentimento(r.as_secs() as i32);
        }
        // FPS do HUD a cada segundo.
        let mut e = self.estado.borrow_mut();
        if e.t_ms() % 1000 < 250 {
            e.hud.fps = e.quadros_no_segundo as f32;
            e.quadros_no_segundo = 0;
        }
    }

    /// Modo demonstração (§8.4): sessão com o host de mentira.
    pub fn demonstracao(self: &Rc<Self>) {
        {
            let mut e = self.estado.borrow_mut();
            e.geracao += 1;
            e.host = None;
            e.alvo = Some(Alvo::Demo(DemoHost::default()));
            e.hud = Hud {
                rtt_ms: 18,
                caminho: Some(taildesk_proto::control::PathInfo {
                    kind: taildesk_proto::net::PathKind::DirectLan as u32,
                    ..Default::default()
                }),
                fps: 60.0,
                decode_hw: true,
                perda: 0.0,
                decode_us: 0,
            };
        }
        if let Some(ui) = self.ui() {
            ui.set_conectando_nome(crate::demo::NOME.into());
            ui.set_demo(true);
        }
        self.abrir_sessao();
        let mut e = self.estado.borrow_mut();
        e.hud = Hud {
            rtt_ms: 18,
            caminho: Some(taildesk_proto::control::PathInfo {
                kind: taildesk_proto::net::PathKind::DirectLan as u32,
                ..Default::default()
            }),
            fps: 60.0,
            decode_hw: true,
            perda: 0.0,
            decode_us: 0,
        };
    }

    /// Spike S1 (`m0-resultados.md`): sessão de demonstração com uma imagem 1920×1080 trocada a
    /// cada quadro (o caminho do vídeo do M1) e a taxa de quadros desenhados no HUD.
    pub fn medir_s1(self: &Rc<Self>) {
        self.demonstracao();
        let Some(ui) = self.ui() else { return };
        ui.set_demo(false);
        ui.set_tem_quadro(true);
        self.estado.borrow_mut().hud_visivel = true;
        let quadros = Rc::new(std::cell::Cell::new(0u32));
        {
            let quadros = quadros.clone();
            let _ = ui.window().set_rendering_notifier(move |estado, _| {
                if matches!(estado, slint::RenderingState::AfterRendering) {
                    quadros.set(quadros.get() + 1);
                }
            });
        }
        let (w, h) = (1920u32, 1080u32);
        let mut base = vec![0u8; (w * h * 4) as usize];
        for (i, px) in base.chunks_exact_mut(4).enumerate() {
            let (x, y) = (i as u32 % w, i as u32 / w);
            px.copy_from_slice(&[(x * 255 / w) as u8, (y * 255 / h) as u8, 160, 255]);
        }
        let deslocamento = std::cell::Cell::new(0usize);
        let fraco = Rc::downgrade(self);
        let inicio = std::cell::Cell::new(Instant::now());
        self.relogio_lento.stop();
        self.relogio.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(16),
            move || {
                let Some(a) = fraco.upgrade() else { return };
                let Some(ui) = a.ui() else { return };
                // Gira a imagem 8 px por quadro: muda tudo sem custo de gerar outra.
                let d = (deslocamento.get() + 32) % base.len();
                deslocamento.set(d);
                let mut buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(w, h);
                let dst = buf.make_mut_bytes();
                dst[..base.len() - d].copy_from_slice(&base[d..]);
                dst[base.len() - d..].copy_from_slice(&base[..d]);
                ui.set_quadro(slint::Image::from_rgba8(buf));
                let t = inicio.get().elapsed();
                if t >= Duration::from_secs(1) {
                    let fps = quadros.replace(0) as f32 / t.as_secs_f32();
                    inicio.set(Instant::now());
                    let texto = format!("S1 · {fps:.0} fps · 1920×1080");
                    crate::registrar!("taildesk: {texto}");
                    ui.set_hud(texto.into());
                }
            },
        );
    }

    // -----------------------------------------------------------------------------------------
    // Ciclo de vida (casca Kotlin / android-activity)

    pub fn segundo_plano(&self) {
        let t = self.estado.borrow().t_ms();
        let msgs = {
            let mut e = self.estado.borrow_mut();
            let e = &mut *e;
            match e.controle.as_mut() {
                Some(c) => e.ciclo.segundo_plano(c, t),
                None => Vec::new(),
            }
        };
        self.enviar(msgs);
    }

    pub fn frente(self: &Rc<Self>) {
        let r = {
            let e = self.estado.borrow();
            match &e.controle {
                Some(c) => {
                    let mut ciclo = e.ciclo;
                    let r = ciclo.frente(c);
                    drop(e);
                    self.estado.borrow_mut().ciclo = ciclo;
                    r
                }
                None => return,
            }
        };
        match r {
            AoVoltar::Enviar(m) => self.enviar(m),
            AoVoltar::Reconectar => {
                if let Some(d) = self.estado.borrow().host.clone() {
                    self.fim_da_sessao();
                    self.conectar(d);
                }
            }
            AoVoltar::Nada => {}
        }
    }

    // -----------------------------------------------------------------------------------------
    // Configurações e teste

    fn mostrar_config(&self) {
        let Some(ui) = self.ui() else { return };
        let c = self.estado.borrow().config;
        ui.set_modo_toque_padrao(i32::from(c.modo_toque == Modo::ToqueDireto));
        ui.set_qualidade(match c.qualidade {
            Qualidade::Automatica => 0,
            Qualidade::Nitidez => 1,
            Qualidade::Fluidez => 2,
            Qualidade::EconomizarDados => 3,
        });
        ui.set_economizar_em_movel(c.economizar_em_movel);
    }

    fn testar(self: &Rc<Self>, d: Dispositivo) {
        let Some(ui) = self.ui() else { return };
        ui.set_teste_nome(format!("{} · {}", d.nome, d.ip).into());
        ui.set_testando(true);
        ui.set_tela(Tela::Teste);
        self.estado.borrow_mut().teste = Some(d.ip);
        let Some(endpoint) = self.estado.borrow().endpoint.clone() else {
            return;
        };
        let pin = d.fingerprint.map_or(Pin::FirstContact, Pin::Sha256);
        let addr = SocketAddr::new(d.ip, taildesk_proto::DEFAULT_PORT);
        let hw = cfg!(target_os = "android");
        self.estado.borrow().rt.spawn(async move {
            let t0 = Instant::now();
            let r = tokio::time::timeout(
                Duration::from_secs(5),
                taildesk_transport::connect(&endpoint, addr, pin),
            )
            .await;
            let linhas = match r {
                Ok(Ok(conn)) => {
                    let handshake = t0.elapsed();
                    let rtt = conn.rtt();
                    let mtu = conn.max_datagram_size().unwrap_or(0);
                    conn.close(0u32.into(), b"teste");
                    vec![
                        ("TailDesk", "ok".to_owned()),
                        ("rtt", format!("{} ms", rtt.as_millis())),
                        ("handshake", format!("{} ms", handshake.as_millis())),
                        ("mtu", format!("{mtu} B")),
                        ("hw", (if hw { "MediaCodec" } else { "—" }).to_owned()),
                    ]
                }
                Ok(Err(e)) => vec![("TailDesk", e.to_string())],
                Err(_) => vec![("TailDesk", "timeout".to_owned())],
            };
            let mut linhas = linhas;
            // Spike S3 (m0-resultados.md): banda pela VPN contra `taildesk-cli bench-net
            // --ouvir <ip>:7421` no computador. Sem o servidor, a linha só não aparece.
            let banco = SocketAddr::new(addr.ip(), PORTA_BENCH);
            if let Ok(Ok(conn)) = tokio::time::timeout(
                Duration::from_secs(3),
                taildesk_transport::connect(&endpoint, banco, Pin::Insecure),
            )
            .await
                && let Ok(Ok(r)) = tokio::time::timeout(
                    Duration::from_secs(30),
                    taildesk_transport::bench::run(
                        &conn,
                        taildesk_transport::bench::BenchOptions {
                            max_kbps: Some(100_000),
                            ..Default::default()
                        },
                    ),
                )
                .await
            {
                let mbps = |e: Option<taildesk_media::measure::Estimate>| {
                    e.map_or("—".to_owned(), |e| {
                        format!(
                            "{}{:.1} Mbit/s",
                            if e.saturated { "" } else { "≥ " },
                            e.kbps as f32 / 1000.0
                        )
                    })
                };
                linhas.push(("down", mbps(r.down_estimate)));
                linhas.push(("up", mbps(r.up_estimate)));
                linhas.push(("mtu", format!("{} B", r.max_datagram)));
                linhas.push(("classe", format!("{:?}", r.class)));
            }
            no_app(move |a| {
                let i = a.estado.borrow().idioma;
                let rotulo = |k: &str| match k {
                    "rtt" => i.t("Latência (ida e volta)", "Latency (round trip)"),
                    "handshake" => i.t("Tempo para conectar", "Time to connect"),
                    "mtu" => i.t("Maior datagrama", "Largest datagram"),
                    "hw" => i.t("Decode por hardware", "Hardware decode"),
                    "down" => i.t("Banda para o celular", "Bandwidth to the phone"),
                    "up" => i.t("Banda do celular", "Bandwidth from the phone"),
                    "classe" => i.t("Classe de rede", "Network class"),
                    _ => i.t("Porta do TailDesk", "TailDesk port"),
                };
                let modelo: Vec<LinhaTeste> = linhas
                    .iter()
                    .map(|(k, v)| LinhaTeste {
                        rotulo: rotulo(k).into(),
                        valor: v.as_str().into(),
                    })
                    .collect();
                if let Some(ui) = a.ui() {
                    ui.set_teste_linhas(ModelRc::new(VecModel::from(modelo)));
                    ui.set_testando(false);
                }
            });
        });
    }

    /// Lista e rede de exemplo (capturas da loja e testes de tela), como no mockup 04.
    #[doc(hidden)]
    pub fn preencher_exemplo(self: &Rc<Self>) {
        use taildesk_proto::control::{PeerEntry, TaildeskPresence};
        let ip = |s: &str| s.parse::<IpAddr>().expect("ip de exemplo");
        let entrada = |n: &str, i: &str, os: &str, online, td: TaildeskPresence| PeerEntry {
            name: n.into(),
            dns_name: format!("{n}.tail1234.ts.net"),
            ip: i.into(),
            os: os.into(),
            online,
            taildesk: td.into(),
        };
        {
            let mut e = self.estado.borrow_mut();
            e.lista.pareou("desktop-casa", ip("100.64.0.1"), [1; 32]);
            let mut srv = Dispositivo::novo("servidor-homelab", ip("100.64.0.2"));
            srv.favorito = true;
            srv.os = "linux".into();
            srv.situacao = Situacao::ComTaildesk;
            e.lista.guardar(srv);
            e.lista.mesclar(
                "desktop-casa",
                &[
                    entrada(
                        "desktop-casa",
                        "100.64.0.1",
                        "windows",
                        true,
                        TaildeskPresence::Yes,
                    ),
                    entrada(
                        "servidor-homelab",
                        "100.64.0.2",
                        "linux",
                        true,
                        TaildeskPresence::Yes,
                    ),
                    entrada(
                        "pc-mae",
                        "100.64.0.3",
                        "windows",
                        true,
                        TaildeskPresence::Yes,
                    ),
                    entrada(
                        "notebook-trabalho",
                        "100.64.0.4",
                        "linux",
                        true,
                        TaildeskPresence::No,
                    ),
                    entrada(
                        "fedora-kde",
                        "100.64.0.5",
                        "linux",
                        false,
                        TaildeskPresence::Unknown,
                    ),
                ],
                None,
            );
            e.config.onboarding_feito = true;
            e.tela_fixa = true;
        }
        self.mostrar_lista();
    }

    // -----------------------------------------------------------------------------------------
    // Callbacks

    fn ligar(self: &Rc<Self>, ui: &AppCelular) {
        macro_rules! com {
            ($f:expr) => {{
                let fraco = Rc::downgrade(self);
                move || {
                    if let Some(a) = fraco.upgrade() {
                        #[allow(clippy::redundant_closure_call)]
                        ($f)(&a);
                    }
                }
            }};
        }
        macro_rules! com1 {
            ($f:expr) => {{
                let fraco = Rc::downgrade(self);
                move |x| {
                    if let Some(a) = fraco.upgrade() {
                        #[allow(clippy::redundant_closure_call)]
                        ($f)(&a, x);
                    }
                }
            }};
        }
        ui.on_abrir_arquivos(com!(|a: &Rc<App>| a.abrir_arquivos()));
        ui.on_arquivo_tocado(com1!(|a: &Rc<App>, i: i32| a.arquivo_tocado(i)));
        ui.on_arquivos_voltar(com!(|a: &Rc<App>| a.arquivos_voltar()));
        ui.on_enviar_arquivos(com!(|a: &Rc<App>| a
            .estado
            .borrow()
            .plat
            .escolher_arquivos()));
        ui.on_cancelar_transferencia(com1!(|a: &Rc<App>, id: i32| a.cancelar_transferencia(id)));
        ui.on_responder_arquivo(com1!(|a: &Rc<App>, sim: bool| a.responder_arquivo(sim)));
        ui.on_abrir_tailscale(com!(|a: &Rc<App>| a.estado.borrow().plat.abrir_tailscale()));
        ui.on_instalar_tailscale(com!(|a: &Rc<App>| a
            .estado
            .borrow()
            .plat
            .instalar_tailscale()));
        ui.on_verificar_tailscale(com!(|a: &Rc<App>| {
            let s = a.estado.borrow().overlay.sinais();
            a.sinais(s);
        }));
        ui.on_demonstracao(com!(|a: &Rc<App>| a.demonstracao()));
        ui.on_continuar_onboarding(com!(|a: &Rc<App>| a.continuar_onboarding()));
        ui.on_ler_qr(com!(|a: &Rc<App>| a.estado.borrow().plat.ler_qr()));
        {
            let fraco = Rc::downgrade(self);
            ui.on_parear_codigo(move |n, c| {
                if let Some(a) = fraco.upgrade() {
                    a.parear_codigo(&n, &c);
                }
            });
        }
        ui.on_pular_pareamento(com!(|a: &Rc<App>| {
            a.estado.borrow_mut().config.onboarding_feito = true;
            a.estado.borrow().guardar();
            if let Some(ui) = a.ui() {
                ui.set_tela(Tela::Dispositivos);
            }
        }));
        ui.on_conectar(com1!(|a: &Rc<App>, item: ItemDispositivo| {
            if let Some(d) = a.dispositivo(&item) {
                if let Some(ui) = a.ui() {
                    ui.set_demo(false);
                }
                a.conectar(d);
            }
        }));
        ui.on_abrir_detalhe(com1!(|a: &Rc<App>, item: ItemDispositivo| {
            let via = a.dispositivo(&item).map(|d| d.via).unwrap_or_default();
            a.estado.borrow_mut().detalhe = item.ip.parse().ok();
            if let Some(ui) = a.ui() {
                ui.set_detalhe(item);
                ui.set_detalhe_via(via.into());
                ui.set_mostrando_detalhe(true);
            }
        }));
        ui.on_atualizar_lista(com!(|a: &Rc<App>| a.atualizar_lista()));
        ui.on_busca_mudou(com1!(|a: &Rc<App>, t: SharedString| {
            a.estado.borrow_mut().busca = t.to_string();
            a.mostrar_lista();
        }));
        ui.on_testar(com1!(|a: &Rc<App>, item: ItemDispositivo| {
            if let Some(d) = a.dispositivo(&item) {
                a.testar(d);
            }
        }));
        ui.on_testar_de_novo(com!(|a: &Rc<App>| {
            let ip = a.estado.borrow().teste;
            let d = ip.and_then(|ip| a.estado.borrow().lista.por_ip(ip).cloned());
            if let Some(d) = d {
                a.testar(d);
            }
        }));
        ui.on_voltar(com!(|a: &Rc<App>| {
            if let Some(ui) = a.ui() {
                ui.set_tela(Tela::Dispositivos);
            }
        }));
        ui.on_copiar(com1!(|a: &Rc<App>, item: ItemDispositivo| {
            let d = a.dispositivo(&item);
            let texto = d
                .map(|d| {
                    if d.dns.is_empty() {
                        d.ip.to_string()
                    } else {
                        d.dns
                    }
                })
                .unwrap_or_else(|| item.ip.to_string());
            a.estado.borrow().plat.copiar(&texto);
            a.toast(format!("{} {texto}", a.t("Copiado:", "Copied:")));
        }));
        ui.on_favoritar(com1!(|a: &Rc<App>, item: ItemDispositivo| {
            if let Ok(ip) = item.ip.parse() {
                {
                    let mut e = a.estado.borrow_mut();
                    e.lista.favoritar(ip, !item.favorito);
                    e.guardar();
                }
                a.mostrar_lista();
                if let Some(ui) = a.ui() {
                    let mut i = item.clone();
                    i.favorito = !item.favorito;
                    ui.set_detalhe(i);
                }
            }
        }));
        ui.on_esquecer(com1!(|a: &Rc<App>, item: ItemDispositivo| {
            if let Ok(ip) = item.ip.parse() {
                {
                    let mut e = a.estado.borrow_mut();
                    e.lista.esquecer(ip);
                    e.guardar();
                }
                a.mostrar_lista();
            }
        }));
        ui.on_cancelar_conexao(com!(|a: &Rc<App>| a.encerrar()));
        ui.on_tentar_de_novo(com!(|a: &Rc<App>| {
            let d = a.estado.borrow().host.clone();
            if let Some(d) = d {
                a.conectar(d);
            }
        }));

        // Sessão
        ui.on_area_mudou({
            let fraco = Rc::downgrade(self);
            move |w, h| {
                if let Some(a) = fraco.upgrade() {
                    let area = Retangulo {
                        x: 0.0,
                        y: 0.0,
                        w,
                        h,
                    };
                    a.estado.borrow_mut().area = area;
                    if let Some(c) = a.estado.borrow_mut().controle.as_mut() {
                        c.area_mudou(area);
                    }
                    a.mostrar_sessao();
                }
            }
        });
        ui.on_toque({
            let fraco = Rc::downgrade(self);
            move |k, x, y| {
                if let Some(a) = fraco.upgrade() {
                    let fase = match k {
                        0 => Fase::Desceu,
                        1 => Fase::Moveu,
                        2 => Fase::Subiu,
                        _ => Fase::Cancelado,
                    };
                    let t_ms = a.estado.borrow().t_ms();
                    a.com_controle(|c, out| {
                        c.toque(
                            Toque {
                                id: 0,
                                fase,
                                x,
                                y,
                                t_ms,
                            },
                            out,
                        );
                        if fase == Fase::Subiu {
                            c.gestos.fim_de_toque_apos_pinca();
                        }
                    });
                }
            }
        });
        ui.on_pinca_comecou({
            let fraco = Rc::downgrade(self);
            move |x, y| {
                if let Some(a) = fraco.upgrade() {
                    let t = a.estado.borrow().t_ms();
                    a.com_controle(|c, _| c.gestos.pinca_comecou(t, x, y));
                }
            }
        });
        ui.on_pinca({
            let fraco = Rc::downgrade(self);
            move |s, x, y| {
                if let Some(a) = fraco.upgrade() {
                    let t = a.estado.borrow().t_ms();
                    a.com_controle(|c, out| {
                        let mut acoes = Vec::new();
                        c.gestos.pinca(t, s, x, y, &mut acoes);
                        c.aplicar(acoes, out);
                    });
                }
            }
        });
        ui.on_pinca_acabou(com1!(|a: &Rc<App>, cancelado: bool| {
            let t = a.estado.borrow().t_ms();
            a.com_controle(|c, out| {
                let mut acoes = Vec::new();
                c.gestos.pinca_acabou(t, cancelado, &mut acoes);
                c.aplicar(acoes, out);
            });
        }));
        ui.on_trocar_monitor(com1!(|a: &Rc<App>, passo: i32| {
            a.com_controle(|c, out| {
                let prefs = c.prefs;
                c.monitores.trocar(passo, prefs, out);
                c.ajustar_vista();
            });
        }));
        ui.on_ir_monitor(com1!(|a: &Rc<App>, i: i32| {
            a.com_controle(|c, out| {
                let id = c.monitores.lista().get(i as usize).map(|d| d.id);
                if let Some(id) = id {
                    let prefs = c.prefs;
                    c.monitores.ir_para(id, prefs, out);
                    c.ajustar_vista();
                }
            });
        }));
        ui.on_alternar_modo(com!(|a: &Rc<App>| {
            let modo = {
                let mut e = a.estado.borrow_mut();
                let Some(c) = e.controle.as_mut() else { return };
                c.gestos.modo = match c.gestos.modo {
                    Modo::Touchpad => Modo::ToqueDireto,
                    Modo::ToqueDireto => Modo::Touchpad,
                };
                c.gestos.modo
            };
            if let Some(ui) = a.ui() {
                ui.set_modo_toque(i32::from(modo == Modo::ToqueDireto));
            }
        }));
        ui.on_alternar_hud(com!(|a: &Rc<App>| {
            let v = !a.estado.borrow().hud_visivel;
            a.estado.borrow_mut().hud_visivel = v;
            a.mostrar_sessao();
        }));
        ui.on_encerrar(com!(|a: &Rc<App>| a.encerrar()));
        ui.on_tecla(com1!(|a: &Rc<App>, nome: SharedString| {
            if let Some(t) = tecla_da_faixa(&nome) {
                a.com_controle(|c, out| c.tecla(t, out));
            }
        }));
        ui.on_modificador(com1!(|a: &Rc<App>, nome: SharedString| {
            a.com_controle(|c, _| c.modificador(modificador(&nome)));
        }));
        ui.on_modificador_segurado(com1!(|a: &Rc<App>, nome: SharedString| {
            a.com_controle(|c, _| c.travar_modificador(modificador(&nome)));
        }));
        ui.on_ctrl_alt_del(com!(
            |a: &Rc<App>| a.com_controle(|c, out| c.ctrl_alt_del(out))
        ));
        ui.on_texto(com1!(|a: &Rc<App>, t: SharedString| {
            // O campo traz o sentinela na frente (ver `sessao.slint`).
            let novo = t.strip_prefix("  ").unwrap_or(&t).to_owned();
            if !novo.is_empty() {
                a.com_controle(|c, out| c.texto(&novo, out));
            }
        }));
        ui.on_apagar(com1!(|a: &Rc<App>, n: i32| {
            a.com_controle(|c, out| c.apagar(n.max(0) as usize, out));
        }));

        // Configurações
        ui.on_modo_toque_padrao_mudou(com1!(|a: &Rc<App>, i: i32| {
            let mut e = a.estado.borrow_mut();
            e.config.modo_toque = if i == 1 {
                Modo::ToqueDireto
            } else {
                Modo::Touchpad
            };
            e.guardar();
        }));
        ui.on_qualidade_mudou(com1!(|a: &Rc<App>, i: i32| {
            let mut e = a.estado.borrow_mut();
            e.config.qualidade = match i {
                1 => Qualidade::Nitidez,
                2 => Qualidade::Fluidez,
                3 => Qualidade::EconomizarDados,
                _ => Qualidade::Automatica,
            };
            e.guardar();
        }));
        ui.on_economizar_mudou(com1!(|a: &Rc<App>, v: bool| {
            let mut e = a.estado.borrow_mut();
            e.config.economizar_em_movel = v;
            e.guardar();
        }));
        ui.on_remover_pareado(com1!(|a: &Rc<App>, i: i32| {
            let ip = a
                .estado
                .borrow()
                .lista
                .pareados()
                .nth(i as usize)
                .map(|d| d.ip);
            if let Some(ip) = ip {
                let mut e = a.estado.borrow_mut();
                e.lista.esquecer(ip);
                e.guardar();
            }
            a.mostrar_lista();
        }));
        ui.on_abrir_link(com1!(|a: &Rc<App>, url: SharedString| a
            .estado
            .borrow()
            .plat
            .abrir_link(&url)));
        ui.on_otimizacao_bateria(com!(|a: &Rc<App>| a
            .estado
            .borrow()
            .plat
            .otimizacao_bateria()));
    }
}

/// Itens de um modelo (para os testes e as capturas).
pub fn itens<T: Clone + 'static>(m: &ModelRc<T>) -> Vec<T> {
    m.iter().collect()
}
