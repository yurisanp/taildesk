//! Ponte entre a área de transferência do sistema ([`Sistema`], de `taildesk-platform`) e a do
//! peer, numa task por sessão. As mensagens 70–73 do peer entram por [`Ponte::mensagem`]; o que
//! se copia e cola aqui chega pelo canal de [`DoSistema`].
//!
//! Conteúdo grande sai em pedaços pela fila do controle com `send` (espera a vez), então mouse
//! e teclado continuam passando entre um pedaço e outro.
//!
//! Arquivos copiados não vão pelas mensagens 70–73: a ponte serve os caminhos locais no
//! serviço de arquivos da sessão ([`ArquivosDaArea`]) e, quando alguém daqui cola, baixa o que
//! o peer copiou para uma pasta temporária e entrega ao sistema os caminhos baixados.

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use taildesk_files::service::{Files, Link};
use taildesk_proto::control::{ClipErrorCode, ClipFile, ClipFormat, Control, FileErrorCode, Msg};
use tokio::sync::{mpsc, oneshot};
use tokio::task::{JoinHandle, JoinSet};

use super::controle;
use crate::clipboard::{AreaTransferencia, Copiado, Evento};

/// O que a área de transferência do sistema avisa.
#[derive(Debug)]
pub enum DoSistema {
    /// Alguém copiou aqui (nunca a oferta do peer que nós mesmos pusemos).
    Copiou(Copiado),
    /// Alguém colou aqui a oferta `seq` do peer: responda com o conteúdo, ou `None` para colar
    /// vazio. Em [`ClipFormat::Files`] o conteúdo são os caminhos locais já baixados, um por
    /// linha (UTF-8).
    Colar {
        seq: u32,
        formato: ClipFormat,
        resposta: Resposta,
    },
}

/// A área de transferência deste computador.
pub trait Sistema: Send + 'static {
    /// Põe a oferta do peer na área de transferência daqui, sem o conteúdo (colar vira
    /// [`DoSistema::Colar`] com este `seq`). `copiado` vazio = esvaziar.
    fn oferecer(&mut self, seq: u32, copiado: &Copiado);
    /// Lê `formato` do que foi copiado aqui.
    fn ler(
        &mut self,
        formato: ClipFormat,
    ) -> impl Future<Output = Result<Vec<u8>, ClipErrorCode>> + Send + 'static;
}

/// Arquivos da área de transferência, pelo serviço de arquivos da sessão.
pub trait ArquivosDaArea: Send + Sync + 'static {
    /// Passa a servir ao peer o que foi copiado aqui (vazio = nada).
    fn servir(&self, itens: Vec<PathBuf>);
    /// Baixa o que o peer copiou para dentro de `destino`; o caminho local de cada item.
    fn baixar(&self, arquivos: Vec<ClipFile>, destino: PathBuf) -> Baixando;
}

/// Download de uma colagem de arquivos em andamento.
pub type Baixando = Pin<Box<dyn Future<Output = Result<Vec<PathBuf>, FileErrorCode>> + Send>>;

impl<L: Link> ArquivosDaArea for Files<L> {
    fn servir(&self, itens: Vec<PathBuf>) {
        self.serve_clipboard(itens);
    }

    fn baixar(&self, arquivos: Vec<ClipFile>, destino: PathBuf) -> Baixando {
        let me = self.clone();
        Box::pin(async move { me.paste_clipboard(&arquivos, &destino).await })
    }
}

/// Pasta nova para os arquivos de uma colagem (o sistema limpa a pasta temporária).
fn pasta_da_colagem() -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join("taildesk-colar").join(format!(
        "{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Resposta de uma colagem de arquivos: os caminhos locais, um por linha, em UTF-8.
fn caminhos(v: &[PathBuf]) -> Vec<u8> {
    v.iter()
        .map(|p| p.to_string_lossy())
        .collect::<Vec<_>>()
        .join("\n")
        .into_bytes()
}

/// Para quem não tem área de transferência: nunca é criada ([`Maquina::area`] devolve `None`).
///
/// [`Maquina::area`]: super::host::Maquina::area
#[derive(Debug)]
pub enum SemArea {}

impl Sistema for SemArea {
    fn oferecer(&mut self, _: u32, _: &Copiado) {
        match *self {}
    }

    fn ler(
        &mut self,
        _: ClipFormat,
    ) -> impl Future<Output = Result<Vec<u8>, ClipErrorCode>> + Send + 'static {
        match *self {}
        #[allow(unreachable_code)]
        std::future::ready(Err(ClipErrorCode::Unavailable))
    }
}

/// Mensagens do peer esperando a task. Os pedaços de um conteúdo grande chegam em rajada.
const FILA_PEER: usize = 256;

/// A área de transferência de uma sessão; soltar para tudo.
#[derive(Debug)]
pub struct Ponte {
    do_peer: mpsc::Sender<Msg>,
    tarefa: JoinHandle<()>,
}

impl Drop for Ponte {
    fn drop(&mut self) {
        self.tarefa.abort();
    }
}

impl Ponte {
    /// `permitido` = a sessão tem `Permissions.clipboard`; sem ela nada passa e os pedidos do
    /// peer voltam `DENIED`.
    pub fn iniciar<S: Sistema>(
        sistema: S,
        do_sistema: mpsc::Receiver<DoSistema>,
        saida: mpsc::Sender<Control>,
        permitido: bool,
    ) -> Ponte {
        Ponte::com_arquivos(sistema, do_sistema, saida, permitido, None)
    }

    /// Como [`Ponte::iniciar`], com arquivos copiados e colados pelo serviço de arquivos.
    pub fn com_arquivos<S: Sistema>(
        sistema: S,
        do_sistema: mpsc::Receiver<DoSistema>,
        saida: mpsc::Sender<Control>,
        permitido: bool,
        arquivos: Option<Arc<dyn ArquivosDaArea>>,
    ) -> Ponte {
        let (do_peer, rx) = mpsc::channel(FILA_PEER);
        let tarefa = tokio::spawn(rodar(sistema, do_sistema, rx, saida, permitido, arquivos));
        Ponte { do_peer, tarefa }
    }

    /// Uma mensagem 70–73 do peer.
    pub async fn mensagem(&self, msg: Msg) {
        let _ = self.do_peer.send(msg).await;
    }

    /// Fila das mensagens do peer, para quem as entrega de outra task (na ordem de chegada).
    pub fn entrada(&self) -> mpsc::Sender<Msg> {
        self.do_peer.clone()
    }
}

/// Resposta para quem não tem área de transferência (sem backend neste sistema).
pub fn sem_sistema(msg: &Msg) -> Option<Msg> {
    match msg {
        Msg::ClipboardRequest(r) => Some(Msg::ClipboardError(
            taildesk_proto::control::ClipboardError {
                seq: r.seq,
                format: r.format,
                code: ClipErrorCode::Unavailable.into(),
            },
        )),
        _ => None,
    }
}

/// Quem colou e espera o conteúdo.
type Resposta = oneshot::Sender<Option<Vec<u8>>>;

async fn rodar<S: Sistema>(
    mut sistema: S,
    mut do_sistema: mpsc::Receiver<DoSistema>,
    mut do_peer: mpsc::Receiver<Msg>,
    saida: mpsc::Sender<Control>,
    permitido: bool,
    arquivos: Option<Arc<dyn ArquivosDaArea>>,
) {
    let mut area = AreaTransferencia::default();
    // Arquivos: a lista da oferta do peer em vigor e a última colagem já baixada.
    let mut oferta_peer: Option<(u32, Vec<ClipFile>)> = None;
    let mut colado: Option<(u32, Vec<u8>)> = None;
    // Downloads de colagens; soltar a ponte cancela.
    let mut baixando = JoinSet::new();
    // Quem está esperando cada colagem pedida.
    let mut esperando: HashMap<(u32, ClipFormat), Vec<Resposta>> = HashMap::new();
    // Leituras do sistema prontas para virar pedaços: (seq, formato, conteúdo).
    let (lido_tx, mut lido_rx) = mpsc::channel(4);
    // Respostas saindo, uma de cada vez (pedaços de duas respostas não se misturam por gosto,
    // mas uma de cada vez deixa a fila do controle livre para o resto).
    let (resp_tx, mut resp_rx) = mpsc::channel::<crate::clipboard::Pedacos>(4);
    let envio = {
        let saida = saida.clone();
        tokio::spawn(async move {
            while let Some(pedacos) = resp_rx.recv().await {
                for m in pedacos {
                    if saida.send(controle(m)).await.is_err() {
                        return;
                    }
                }
            }
        })
    };
    let enviar = |m: Msg| {
        let saida = saida.clone();
        async move { saida.send(controle(m)).await.is_ok() }
    };

    loop {
        tokio::select! {
            ev = do_sistema.recv() => match ev {
                None => break,
                Some(_) if !permitido => {}
                Some(DoSistema::Copiou(c)) => {
                    if let Some(a) = &arquivos {
                        let tem = c.formatos.iter().any(|(f, _)| *f == ClipFormat::Files);
                        a.servir(if tem { c.locais.clone() } else { Vec::new() });
                    }
                    if !enviar(area.copiado(&c)).await {
                        break;
                    }
                }
                Some(DoSistema::Colar { seq, formato: ClipFormat::Files, resposta }) => {
                    let chave = (seq, ClipFormat::Files);
                    match (&colado, &oferta_peer, &arquivos) {
                        (Some((s, d)), _, _) if *s == seq => {
                            let _ = resposta.send(Some(d.clone()));
                        }
                        _ if esperando.contains_key(&chave) => {
                            esperando.entry(chave).or_default().push(resposta);
                        }
                        (_, Some((s, lista)), Some(a)) if *s == seq && !lista.is_empty() => {
                            esperando.insert(chave, vec![resposta]);
                            let download = a.baixar(lista.clone(), pasta_da_colagem());
                            baixando.spawn(async move {
                                (seq, download.await.ok().map(|v| caminhos(&v)))
                            });
                        }
                        _ => {
                            let _ = resposta.send(None);
                        }
                    }
                }
                Some(DoSistema::Colar { seq, formato, resposta }) => {
                    let chave = (seq, formato);
                    if let Some(lista) = esperando.get_mut(&chave) {
                        lista.push(resposta);
                    } else if let Some(m) = area.colar(seq, formato) {
                        esperando.insert(chave, vec![resposta]);
                        if !enviar(m).await {
                            break;
                        }
                    } else {
                        let _ = resposta.send(None);
                    }
                }
            },
            msg = do_peer.recv() => {
                let Some(msg) = msg else { break };
                let (evento, resposta) = area.receber(&msg, permitido);
                if let Some(r) = resposta
                    && !enviar(r).await
                {
                    break;
                }
                match evento {
                    None => {}
                    Some(Evento::Oferta { seq, copiado }) => {
                        oferta_peer = Some((seq, copiado.arquivos.clone()));
                        colado = None;
                        sistema.oferecer(seq, &copiado);
                    }
                    Some(Evento::Ler { seq, formato }) => {
                        let leitura = sistema.ler(formato);
                        let lido = lido_tx.clone();
                        tokio::spawn(async move {
                            let _ = lido.send((seq, formato, leitura.await)).await;
                        });
                    }
                    Some(Evento::Recebido { seq, formato, dados }) => {
                        let mut lista = esperando.remove(&(seq, formato)).unwrap_or_default();
                        if let Some(ultimo) = lista.pop() {
                            for r in lista {
                                let _ = r.send(Some(dados.clone()));
                            }
                            let _ = ultimo.send(Some(dados));
                        }
                    }
                    Some(Evento::Falhou { seq, formato, .. }) => {
                        for r in esperando.remove(&(seq, formato)).unwrap_or_default() {
                            let _ = r.send(None);
                        }
                    }
                }
            }
            Some(Ok((seq, dados))) = baixando.join_next() => {
                if let Some(d) = &dados
                    && oferta_peer.as_ref().is_some_and(|(s, _)| *s == seq)
                {
                    colado = Some((seq, d.clone()));
                }
                for r in esperando.remove(&(seq, ClipFormat::Files)).unwrap_or_default() {
                    let _ = r.send(dados.clone());
                }
            }
            Some((seq, formato, dados)) = lido_rx.recv() => {
                if resp_tx.send(area.responder(seq, formato, dados)).await.is_err() {
                    break;
                }
            }
        }
    }
    envio.abort();
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use taildesk_proto::control::{ClipboardRequest, limits};

    use super::*;

    /// Área de transferência em memória: o que se oferece fica anotado e `ler` devolve `texto`.
    #[derive(Clone, Default)]
    struct Falsa {
        ofertas: Arc<Mutex<Vec<(u32, Copiado)>>>,
        texto: Vec<u8>,
    }

    impl Sistema for Falsa {
        fn oferecer(&mut self, seq: u32, copiado: &Copiado) {
            self.ofertas.lock().unwrap().push((seq, copiado.clone()));
        }

        fn ler(
            &mut self,
            formato: ClipFormat,
        ) -> impl Future<Output = Result<Vec<u8>, ClipErrorCode>> + Send + 'static {
            let t = self.texto.clone();
            async move {
                if formato == ClipFormat::Text {
                    Ok(t)
                } else {
                    Err(ClipErrorCode::Unavailable)
                }
            }
        }
    }

    /// Dois lados ligados: o que um manda vai para o outro.
    struct Lado {
        ponte: Ponte,
        sistema: mpsc::Sender<DoSistema>,
        saida: mpsc::Receiver<Control>,
        falsa: Falsa,
    }

    fn lado(texto: &[u8], permitido: bool) -> Lado {
        let falsa = Falsa {
            texto: texto.to_vec(),
            ..Falsa::default()
        };
        let (sistema, do_sistema) = mpsc::channel(8);
        let (saida_tx, saida) = mpsc::channel(64);
        Lado {
            ponte: Ponte::iniciar(falsa.clone(), do_sistema, saida_tx, permitido),
            sistema,
            saida,
            falsa,
        }
    }

    /// Repassa tudo que `de` mandou para `para` até `de` ficar quieto.
    async fn repassar(de: &mut Lado, para: &Lado) -> usize {
        let mut n = 0;
        while let Ok(Some(c)) =
            tokio::time::timeout(std::time::Duration::from_millis(100), de.saida.recv()).await
        {
            para.ponte.mensagem(c.msg.unwrap()).await;
            n += 1;
        }
        n
    }

    #[tokio::test]
    async fn copiar_no_host_e_colar_no_controlador() {
        let conteudo = vec![b'x'; limits::CLIP_CHUNK * 2 + 10];
        let mut host = lado(&conteudo, true);
        let mut ctl = lado(b"", true);
        host.sistema
            .send(DoSistema::Copiou(Copiado {
                formatos: vec![(ClipFormat::Text, conteudo.len() as u64)],
                arquivos: Vec::new(),
                locais: Vec::new(),
            }))
            .await
            .unwrap();
        assert_eq!(repassar(&mut host, &ctl).await, 1);
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let (seq, _) = ctl.falsa.ofertas.lock().unwrap()[0].clone();

        // Duas colagens seguidas: um pedido só, as duas recebem.
        let (r1, c1) = oneshot::channel();
        let (r2, c2) = oneshot::channel();
        for resposta in [r1, r2] {
            ctl.sistema
                .send(DoSistema::Colar {
                    seq,
                    formato: ClipFormat::Text,
                    resposta,
                })
                .await
                .unwrap();
        }
        assert_eq!(repassar(&mut ctl, &host).await, 1);
        assert_eq!(repassar(&mut host, &ctl).await, 3);
        assert_eq!(c1.await.unwrap().unwrap(), conteudo);
        assert_eq!(c2.await.unwrap().unwrap(), conteudo);
    }

    #[tokio::test]
    async fn sem_permissao_nada_passa() {
        let mut host = lado(b"segredo", false);
        host.sistema
            .send(DoSistema::Copiou(Copiado {
                formatos: vec![(ClipFormat::Text, 7)],
                arquivos: Vec::new(),
                locais: Vec::new(),
            }))
            .await
            .unwrap();
        let ctl = lado(b"", true);
        assert_eq!(repassar(&mut host, &ctl).await, 0);
        host.ponte
            .mensagem(Msg::ClipboardRequest(ClipboardRequest {
                seq: 1,
                format: ClipFormat::Text.into(),
            }))
            .await;
        let c = host.saida.recv().await.unwrap();
        assert!(matches!(
            c.msg,
            Some(Msg::ClipboardError(e)) if e.code == ClipErrorCode::Denied as i32
        ));
    }

    /// Serviço de arquivos de mentira: anota o que serve e quantas vezes baixou.
    #[derive(Default)]
    struct ArquivosFalsos {
        servidos: Mutex<Vec<PathBuf>>,
        baixados: Mutex<usize>,
    }

    impl ArquivosDaArea for ArquivosFalsos {
        fn servir(&self, itens: Vec<PathBuf>) {
            *self.servidos.lock().unwrap() = itens;
        }

        fn baixar(&self, arquivos: Vec<ClipFile>, destino: PathBuf) -> Baixando {
            *self.baixados.lock().unwrap() += 1;
            Box::pin(async move { Ok(arquivos.iter().map(|f| destino.join(&f.name)).collect()) })
        }
    }

    #[tokio::test]
    async fn arquivos_sao_servidos_e_baixados_uma_vez() {
        let falsos = Arc::new(ArquivosFalsos::default());
        let (h_sis, h_rx) = mpsc::channel(8);
        let (h_saida_tx, mut h_saida) = mpsc::channel(64);
        let host = Ponte::com_arquivos(
            Falsa::default(),
            h_rx,
            h_saida_tx,
            true,
            Some(falsos.clone() as Arc<dyn ArquivosDaArea>),
        );
        let ctl_falsos = Arc::new(ArquivosFalsos::default());
        let (c_sis, c_rx) = mpsc::channel(8);
        let (c_saida_tx, _c_saida) = mpsc::channel(64);
        let ctl_falsa = Falsa::default();
        let ctl = Ponte::com_arquivos(
            ctl_falsa.clone(),
            c_rx,
            c_saida_tx,
            true,
            Some(ctl_falsos.clone() as Arc<dyn ArquivosDaArea>),
        );
        let arq = ClipFile {
            name: "a.txt".into(),
            size: 3,
            dir: false,
        };
        h_sis
            .send(DoSistema::Copiou(Copiado {
                formatos: vec![(ClipFormat::Files, 0)],
                arquivos: vec![arq.clone()],
                locais: vec![PathBuf::from("/home/yuri/a.txt")],
            }))
            .await
            .unwrap();
        let oferta = h_saida.recv().await.unwrap().msg.unwrap();
        assert_eq!(
            *falsos.servidos.lock().unwrap(),
            vec![PathBuf::from("/home/yuri/a.txt")]
        );
        ctl.mensagem(oferta).await;
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let (seq, _) = ctl_falsa.ofertas.lock().unwrap()[0].clone();
        for _ in 0..2 {
            let (r, c) = oneshot::channel();
            c_sis
                .send(DoSistema::Colar {
                    seq,
                    formato: ClipFormat::Files,
                    resposta: r,
                })
                .await
                .unwrap();
            let d = String::from_utf8(c.await.unwrap().unwrap()).unwrap();
            assert!(d.ends_with("a.txt") && !d.contains('\n'), "{d}");
        }
        assert_eq!(*ctl_falsos.baixados.lock().unwrap(), 1);
        drop(host);
    }

    #[tokio::test]
    async fn colar_oferta_velha_cola_vazio() {
        let ctl = lado(b"", true);
        let (r, c) = oneshot::channel();
        ctl.sistema
            .send(DoSistema::Colar {
                seq: 9,
                formato: ClipFormat::Text,
                resposta: r,
            })
            .await
            .unwrap();
        assert_eq!(c.await.unwrap(), None);
    }
}
