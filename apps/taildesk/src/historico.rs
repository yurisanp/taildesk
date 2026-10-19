//! Configurações → Histórico de acessos: os últimos eventos do registro de acessos
//! (`taildesk_session::acessos`) e o registro de arquivos, na pasta de registros do sistema.

use std::sync::{Arc, OnceLock};

use slint::{ComponentHandle, ModelRc, VecModel};
use taildesk_files::audit::AuditLog;
use taildesk_session::acessos::{Acesso, Evento, RegistroAcessos, Via};
use taildesk_ui::{JanelaPrincipal, LinhaAcesso};

use crate::{avisar, com_app};

/// Quantos eventos as Configurações mostram.
const MOSTRAR: usize = 15;

/// O registro de acessos do app (um só, para a escrita do host e a leitura da tela).
pub fn registro() -> Option<Arc<RegistroAcessos>> {
    static R: OnceLock<Option<Arc<RegistroAcessos>>> = OnceLock::new();
    R.get_or_init(|| {
        taildesk_platform::pasta_registros()
            .map(|d| Arc::new(RegistroAcessos::new(d.join("acessos.jsonl"))))
    })
    .clone()
}

/// Registro das transferências de arquivo (quem, o quê, tamanho, hash).
pub fn auditoria_arquivos() -> Option<AuditLog> {
    taildesk_platform::pasta_registros().map(|d| AuditLog::new(d.join("arquivos.jsonl")))
}

fn duracao(s: u64) -> String {
    match s {
        0..60 => format!("{s} s"),
        60..3600 => format!("{} min", s / 60),
        _ => format!("{} h {:02} min", s / 3600, (s % 3600) / 60),
    }
}

fn motivo(m: &str) -> &str {
    match m {
        "por_controlador" => "encerrada por quem acessou",
        "por_host" => "encerrada neste computador",
        "recusado_no_dialogo" => "pedido recusado",
        "sem_resposta" => "ninguém respondeu ao pedido",
        "nao_permitido" => "este computador não aceita conexões dele",
        "senha_errada" => "senha errada",
        "tentativas_demais" => "bloqueado por senhas erradas demais",
        "versao_diferente" => "versão do TailDesk incompatível",
        "erro_de_protocolo" => "erro de protocolo",
        "fora_da_tailnet" => "fora da sua tailnet",
        "queda" => "a conexão caiu",
        outro => outro,
    }
}

fn permissoes(p: &[String]) -> String {
    p.iter()
        .map(|n| match n.as_str() {
            "ver" => "ver a tela",
            "controlar" => "controlar",
            "arquivos" => "arquivos",
            "area" => "área de transferência",
            "som" => "som",
            outro => outro,
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Texto de um evento ("Entrou com a senha · ver a tela, controlar").
fn texto(a: &Acesso) -> String {
    match a.evento {
        Evento::Entrou => {
            let como = match a.via {
                Some(Via::Dialogo) => "Entrou com o pedido aceito",
                Some(Via::Confiavel) => "Entrou como dispositivo lembrado",
                Some(Via::Senha) => "Entrou com a senha",
                None => "Entrou",
            };
            let mut t = como.to_owned();
            if a.tela_de_entrada {
                t.push_str(" pela tela de entrada do Windows");
            }
            if !a.permissoes.is_empty() {
                t.push_str(&format!(" · {}", permissoes(&a.permissoes)));
            }
            t
        }
        Evento::Saiu => {
            let mut t = match a.duracao_s {
                Some(s) => format!("Saiu depois de {}", duracao(s)),
                None => "Saiu".to_owned(),
            };
            if let Some(m) = &a.motivo {
                t.push_str(&format!(" ({})", motivo(m)));
            }
            t
        }
        Evento::Recusado => match &a.motivo {
            Some(m) => format!("Não entrou: {}", motivo(m)),
            None => "Não entrou".to_owned(),
        },
    }
}

fn alerta(a: &Acesso) -> bool {
    a.evento == Evento::Recusado
        && matches!(
            a.motivo.as_deref(),
            Some("senha_errada" | "tentativas_demais" | "fora_da_tailnet")
        )
}

fn quando(ts_ms: i64) -> String {
    use chrono::{Local, TimeZone};
    match Local.timestamp_millis_opt(ts_ms) {
        chrono::LocalResult::Single(t) | chrono::LocalResult::Ambiguous(t, _) => {
            t.format("%d/%m %H:%M").to_string()
        }
        chrono::LocalResult::None => String::new(),
    }
}

fn linha(a: &Acesso) -> LinhaAcesso {
    LinhaAcesso {
        quando: quando(a.ts_ms).into(),
        quem: a.quem().into(),
        texto: texto(a).into(),
        alerta: alerta(a),
    }
}

/// Lê o registro fora da thread da interface e mostra na janela.
pub fn mostrar(j: &JanelaPrincipal) {
    let fraca = j.as_weak();
    std::thread::spawn(move || {
        let ultimos = registro()
            .and_then(|r| {
                r.ultimos(MOSTRAR)
                    .inspect_err(|e| eprintln!("taildesk: histórico de acessos: {e}"))
                    .ok()
            })
            .unwrap_or_default();
        let _ = fraca.upgrade_in_event_loop(move |j| {
            let linhas: Vec<LinhaAcesso> = ultimos.iter().map(linha).collect();
            j.set_acessos(ModelRc::new(VecModel::from(linhas)));
        });
    });
}

pub fn ligar(j: &JanelaPrincipal) {
    j.on_abrir_registros(|| {
        let Some(dir) = taildesk_platform::pasta_registros() else {
            return;
        };
        com_app(|app| {
            app.rt.spawn(async move {
                let _ = tokio::fs::create_dir_all(&dir).await;
                if let Err(e) = taildesk_platform::arquivos::abrir_pasta(&dir).await {
                    avisar(format!("Não foi possível abrir {}: {e}", dir.display()));
                }
            });
        });
    });
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::time::Duration;

    use taildesk_overlay::PeerIdentity;
    use taildesk_proto::control::Permissions;

    use super::*;

    fn a(evento: Evento) -> Acesso {
        let peer = PeerIdentity {
            node_stable_id: "n".into(),
            hostname: "notebook".into(),
            user_login: Some("yuri@github".into()),
            tags: Vec::new(),
            app_caps: Vec::new(),
        };
        let end: SocketAddr = "100.64.0.1:1".parse().unwrap();
        Acesso::novo(evento, Some(&peer), end)
    }

    #[test]
    fn textos() {
        let entrou = a(Evento::Entrou)
            .com_via(Some(Via::Senha))
            .com_permissoes(Permissions {
                view: true,
                control: true,
                ..Permissions::default()
            });
        assert_eq!(texto(&entrou), "Entrou com a senha · ver a tela, controlar");
        let na_entrada = Acesso {
            tela_de_entrada: true,
            ..entrou.clone()
        };
        assert_eq!(
            texto(&na_entrada),
            "Entrou com a senha pela tela de entrada do Windows · ver a tela, controlar"
        );
        let saiu = a(Evento::Saiu)
            .com_motivo("por_host")
            .com_duracao(Duration::from_secs(3725));
        assert_eq!(
            texto(&saiu),
            "Saiu depois de 1 h 02 min (encerrada neste computador)"
        );
        let errou = a(Evento::Recusado).com_motivo("senha_errada");
        assert_eq!(texto(&errou), "Não entrou: senha errada");
        assert!(alerta(&errou));
        assert!(!alerta(
            &a(Evento::Recusado).com_motivo("recusado_no_dialogo")
        ));
        assert_eq!(duracao(59), "59 s");
        assert_eq!(duracao(61), "1 min");
        assert_eq!(quando(0).len(), "01/01 00:00".len());
    }
}
