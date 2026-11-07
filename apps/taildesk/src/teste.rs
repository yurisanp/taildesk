//! "Testar conexão" no painel do dispositivo (`14-selecao-adaptativa.md` §7): pede o teste ao
//! TailDesk do outro lado (`TestRequest`), mede com o `bench` e mostra as linhas no painel.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use slint::{ModelRc, VecModel, Weak};
use taildesk_proto::control::{CloseReason, TestError};
use taildesk_session::conexao::PORTA;
use taildesk_session::conexao::servico::{ErroServico, testar};
use taildesk_session::teste::{self, Resultado};
use taildesk_transport::bench::BenchOptions;
use taildesk_transport::{Pin, client_endpoint};
use taildesk_ui::{InfoTeste, JanelaPrincipal, LinhaTeste};

use crate::config;

/// A sondagem leva uns 15 s; o host corta em 60 s.
const ESPERA: Duration = Duration::from_secs(75);

/// Linhas para quando o teste não deu certo.
fn falha(e: &ErroServico) -> (&'static str, String) {
    let texto = match e {
        ErroServico::TesteRecusado(TestError::Disabled) => {
            "o outro computador não aceita testes (Configurações → Permitir testes de conexão)"
                .to_owned()
        }
        ErroServico::TesteRecusado(_) => "espere 10 s e teste de novo".to_owned(),
        ErroServico::Recusado(CloseReason::ProtocolError) => {
            "o TailDesk do outro computador é antigo: atualize para testar".to_owned()
        }
        ErroServico::Recusado(_) => "o outro computador recusou".to_owned(),
        outro => outro.to_string(),
    };
    ("Teste de conexão", texto)
}

fn info(ip: &str, testando: bool, linhas: &[(&str, String)], relatorio: String) -> InfoTeste {
    let linhas: Vec<LinhaTeste> = linhas
        .iter()
        .map(|(r, v)| LinhaTeste {
            rotulo: (*r).into(),
            valor: v.as_str().into(),
        })
        .collect();
    InfoTeste {
        ip: ip.into(),
        testando,
        linhas: ModelRc::new(VecModel::from(linhas)),
        relatorio: relatorio.into(),
    }
}

/// Começa o teste até `ip` (o `id` acha o certificado já conhecido) e atualiza a janela no fim.
pub fn iniciar(rt: &tokio::runtime::Handle, janela: Weak<JanelaPrincipal>, id: String, ip: String) {
    let Ok(addr) = ip.parse::<IpAddr>() else {
        return;
    };
    if let Some(j) = janela.upgrade() {
        j.set_teste(info(&ip, true, &[], String::new()));
    }
    let pin = config::ler()
        .hosts
        .get(&id)
        .copied()
        .map_or(Pin::FirstContact, Pin::Sha256);
    rt.spawn(async move {
        let remoto = SocketAddr::new(addr, PORTA);
        let r: Result<Resultado, ErroServico> = match client_endpoint(remoto) {
            Ok(endpoint) => {
                let r = tokio::time::timeout(
                    ESPERA,
                    testar(&endpoint, remoto, pin, BenchOptions::default()),
                )
                .await
                .unwrap_or_else(|_| Err(ErroServico::Rede(std::io::ErrorKind::TimedOut.into())));
                endpoint.close(0u32.into(), b"fim");
                r
            }
            Err(e) => Err(ErroServico::Rede(e)),
        };
        let _ = janela.upgrade_in_event_loop(move |j| {
            let novo = match &r {
                Ok(r) => info(
                    &ip,
                    false,
                    &teste::linhas(r, false),
                    teste::relatorio(r, false),
                ),
                Err(e) => info(&ip, false, &[falha(e)], String::new()),
            };
            j.set_teste(novo);
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn textos_de_falha() {
        let (_, t) = falha(&ErroServico::TesteRecusado(TestError::Disabled));
        assert!(t.contains("Permitir testes de conexão"));
        let (_, t) = falha(&ErroServico::TesteRecusado(TestError::RateLimited));
        assert_eq!(t, "espere 10 s e teste de novo");
        let (_, t) = falha(&ErroServico::Recusado(CloseReason::ProtocolError));
        assert!(t.contains("antigo"));
    }
}
