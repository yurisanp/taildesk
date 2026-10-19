//! Registro de acessos do host (`10-plano-windows-linux.md` §5, fase 5 "auditoria"): uma linha
//! JSON por evento de sessão, com quem (identidade da tailnet, nunca o `Hello`), de onde, como
//! entrou (diálogo, dispositivo lembrado ou senha), com que permissões, quanto durou e por que
//! acabou. Fica ao lado do registro de arquivos (`taildesk_files::audit`).
//!
//! O arquivo só cresce por acréscimo; passando de [`TAMANHO_MAXIMO`] vira `<nome>.1` (o `.1`
//! anterior some) e um novo começa. Nada aqui é segredo: senhas nunca entram no registro.

use std::io::{self, BufRead, Write as _};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use taildesk_overlay::PeerIdentity;
use taildesk_proto::control::{CloseReason, Permissions};

/// Passando disto, o arquivo é girado.
pub const TAMANHO_MAXIMO: u64 = 4 * 1024 * 1024;
/// Linhas maiores que isto na leitura são ignoradas (arquivo corrompido ou adulterado).
const LINHA_MAXIMA: usize = 8 * 1024;

/// O que aconteceu.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Evento {
    /// A sessão começou (depois do consentimento, se houve).
    Entrou,
    /// Não entrou: fora da tailnet, recusado, senha errada, sem resposta...
    Recusado,
    /// Uma sessão que tinha entrado acabou.
    Saiu,
}

/// Como o acesso foi liberado.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Via {
    /// Alguém clicou "Permitir" no diálogo do host.
    Dialogo,
    /// Dispositivo lembrado ("Lembrar deste dispositivo").
    Confiavel,
    /// Senha de acesso certa (sem ninguém na frente).
    Senha,
}

/// Uma linha do registro.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Acesso {
    /// Milissegundos desde 1970 (UTC).
    pub ts_ms: i64,
    pub evento: Evento,
    /// Login do usuário na tailnet; vazio em nó com tag ou fora da tailnet.
    #[serde(default)]
    pub usuario: String,
    /// Nome da máquina na tailnet.
    #[serde(default)]
    pub maquina: String,
    #[serde(default)]
    pub node_stable_id: String,
    /// Endereço de onde veio a conexão.
    #[serde(default)]
    pub endereco: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<Via>,
    /// Permissões em vigor: `ver`, `controlar`, `arquivos`, `area`, `som`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub permissoes: Vec<String>,
    /// Motivo do fim ou da recusa (`por_controlador`, `senha_errada`, `queda`...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub motivo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duracao_s: Option<u64>,
    /// Atendido pelo agente do serviço do Windows na tela de entrada (ninguém logado).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tela_de_entrada: bool,
}

fn agora_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Nomes das permissões, na ordem da faixa.
pub fn nomes_permissoes(p: Permissions) -> Vec<String> {
    [
        (p.view, "ver"),
        (p.control, "controlar"),
        (p.files, "arquivos"),
        (p.clipboard, "area"),
        (p.audio, "som"),
    ]
    .into_iter()
    .filter(|(sim, _)| *sim)
    .map(|(_, n)| n.to_owned())
    .collect()
}

/// Nome do motivo no registro.
pub fn nome_motivo(r: CloseReason) -> &'static str {
    match r {
        CloseReason::Unspecified => "sem_motivo",
        CloseReason::ByController => "por_controlador",
        CloseReason::ByHost => "por_host",
        CloseReason::ConsentDenied => "recusado_no_dialogo",
        CloseReason::VersionMismatch => "versao_diferente",
        CloseReason::ProtocolError => "erro_de_protocolo",
        CloseReason::ConsentTimeout => "sem_resposta",
        CloseReason::NotAllowed => "nao_permitido",
        CloseReason::BadPassword => "senha_errada",
        CloseReason::TooManyAttempts => "tentativas_demais",
    }
}

/// A conexão caiu (sem `Close`).
pub const MOTIVO_QUEDA: &str = "queda";
/// O `tailscaled` não soube dizer quem é (fora da tailnet).
pub const MOTIVO_FORA_DA_TAILNET: &str = "fora_da_tailnet";

impl Acesso {
    /// Linha com a identidade de `peer` (ou só o endereço, se `None`).
    pub fn novo(evento: Evento, peer: Option<&PeerIdentity>, endereco: SocketAddr) -> Self {
        Acesso {
            ts_ms: agora_ms(),
            evento,
            usuario: peer.and_then(|p| p.user_login.clone()).unwrap_or_default(),
            maquina: peer.map(|p| p.hostname.clone()).unwrap_or_default(),
            node_stable_id: peer.map(|p| p.node_stable_id.clone()).unwrap_or_default(),
            endereco: endereco.ip().to_string(),
            via: None,
            permissoes: Vec::new(),
            motivo: None,
            duracao_s: None,
            tela_de_entrada: false,
        }
    }

    pub fn com_via(mut self, via: Option<Via>) -> Self {
        self.via = via;
        self
    }

    pub fn com_permissoes(mut self, p: Permissions) -> Self {
        self.permissoes = nomes_permissoes(p);
        self
    }

    pub fn com_motivo(mut self, motivo: &str) -> Self {
        self.motivo = Some(motivo.to_owned());
        self
    }

    pub fn com_duracao(mut self, d: Duration) -> Self {
        self.duracao_s = Some(d.as_secs());
        self
    }

    /// Quem, para mostrar: "yuri@github (notebook)" ou só a máquina.
    pub fn quem(&self) -> String {
        match (self.usuario.is_empty(), self.maquina.is_empty()) {
            (false, false) => format!("{} ({})", self.usuario, self.maquina),
            (false, true) => self.usuario.clone(),
            (true, false) => self.maquina.clone(),
            (true, true) => self.endereco.clone(),
        }
    }
}

/// Arquivo de registro (`acessos.jsonl`).
#[derive(Debug)]
pub struct RegistroAcessos {
    caminho: PathBuf,
    maximo: u64,
    /// Serializa escrita e giro entre sessões simultâneas.
    trava: Mutex<()>,
}

impl RegistroAcessos {
    pub fn new(caminho: impl Into<PathBuf>) -> Self {
        RegistroAcessos {
            caminho: caminho.into(),
            maximo: TAMANHO_MAXIMO,
            trava: Mutex::new(()),
        }
    }

    /// Para testes: gira com `maximo` bytes.
    pub fn com_maximo(mut self, maximo: u64) -> Self {
        self.maximo = maximo;
        self
    }

    pub fn caminho(&self) -> &Path {
        &self.caminho
    }

    fn girado(&self) -> PathBuf {
        let mut nome = self.caminho.file_name().unwrap_or_default().to_owned();
        nome.push(".1");
        self.caminho.with_file_name(nome)
    }

    /// Acrescenta uma linha (cria pasta e arquivo se preciso, gira se passou do tamanho).
    pub fn registrar(&self, a: &Acesso) -> io::Result<()> {
        let mut linha = serde_json::to_string(a).map_err(io::Error::other)?;
        linha.push('\n');
        let _t = self.trava.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(dir) = self.caminho.parent() {
            std::fs::create_dir_all(dir)?;
        }
        if std::fs::metadata(&self.caminho).is_ok_and(|m| m.len() >= self.maximo) {
            std::fs::rename(&self.caminho, self.girado())?;
        }
        let mut f = abrir_para_acrescentar(&self.caminho)?;
        f.write_all(linha.as_bytes())
    }

    /// Os `n` eventos mais recentes, do mais novo para o mais antigo. Linhas ilegíveis são
    /// puladas.
    pub fn ultimos(&self, n: usize) -> io::Result<Vec<Acesso>> {
        let mut todos = Vec::new();
        for caminho in [self.girado(), self.caminho.clone()] {
            let f = match std::fs::File::open(&caminho) {
                Ok(f) => f,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e),
            };
            for linha in io::BufReader::new(f).split(b'\n') {
                let linha = linha?;
                if linha.len() > LINHA_MAXIMA {
                    continue;
                }
                if let Ok(a) = serde_json::from_slice::<Acesso>(&linha) {
                    todos.push(a);
                }
            }
        }
        let inicio = todos.len().saturating_sub(n);
        let mut r = todos.split_off(inicio);
        r.reverse();
        Ok(r)
    }
}

/// Só o dono lê e escreve (o registro diz quem acessou este computador).
fn abrir_para_acrescentar(caminho: &Path) -> io::Result<std::fs::File> {
    let mut o = std::fs::OpenOptions::new();
    o.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(caminho)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer() -> PeerIdentity {
        PeerIdentity {
            node_stable_id: "nX".into(),
            hostname: "notebook".into(),
            user_login: Some("yuri@github".into()),
            tags: Vec::new(),
            app_caps: Vec::new(),
        }
    }

    fn end() -> SocketAddr {
        "100.64.0.7:41641".parse().unwrap()
    }

    #[test]
    fn linha_ida_e_volta() {
        let a = Acesso::novo(Evento::Entrou, Some(&peer()), end())
            .com_via(Some(Via::Senha))
            .com_permissoes(Permissions {
                view: true,
                control: true,
                files: false,
                clipboard: true,
                audio: false,
            });
        let s = serde_json::to_string(&a).unwrap();
        assert!(s.contains("\"evento\":\"entrou\""));
        assert!(s.contains("\"via\":\"senha\""));
        assert!(s.contains("[\"ver\",\"controlar\",\"area\"]"));
        assert!(!s.contains("motivo"));
        assert!(!s.contains("tela_de_entrada"));
        assert_eq!(serde_json::from_str::<Acesso>(&s).unwrap(), a);
        let login = Acesso {
            tela_de_entrada: true,
            ..a.clone()
        };
        let s = serde_json::to_string(&login).unwrap();
        assert!(s.contains("\"tela_de_entrada\":true"));
        assert_eq!(serde_json::from_str::<Acesso>(&s).unwrap(), login);
        assert_eq!(a.quem(), "yuri@github (notebook)");
        assert_eq!(a.endereco, "100.64.0.7");
        let fora = Acesso::novo(Evento::Recusado, None, end()).com_motivo(MOTIVO_FORA_DA_TAILNET);
        assert_eq!(fora.quem(), "100.64.0.7");
    }

    #[test]
    fn grava_le_e_gira() {
        let dir = std::env::temp_dir().join(format!("taildesk-acessos-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let r = RegistroAcessos::new(dir.join("sub/acessos.jsonl")).com_maximo(600);
        for i in 0..10u64 {
            let a = Acesso::novo(Evento::Saiu, Some(&peer()), end())
                .com_motivo(nome_motivo(CloseReason::ByController))
                .com_duracao(Duration::from_secs(i));
            r.registrar(&a).unwrap();
        }
        // Lixo no meio não derruba a leitura.
        std::fs::OpenOptions::new()
            .append(true)
            .open(r.caminho())
            .unwrap()
            .write_all(b"{nao e json\n")
            .unwrap();
        let ultimos = r.ultimos(3).unwrap();
        assert_eq!(
            ultimos.iter().map(|a| a.duracao_s).collect::<Vec<_>>(),
            vec![Some(9), Some(8), Some(7)]
        );
        assert!(dir.join("sub/acessos.jsonl.1").exists());
        assert!(std::fs::metadata(r.caminho()).unwrap().len() < 600 + 300);
        // Lê também o girado, sem passar de `n`.
        assert!(r.ultimos(100).unwrap().len() <= 10);
        assert!(r.ultimos(100).unwrap().len() >= 3);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let modo = std::fs::metadata(r.caminho()).unwrap().permissions().mode();
            assert_eq!(modo & 0o777, 0o600);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
