//! TLS 1.3 do QUIC: certificado autoassinado por instalação e verificação pelo SHA-256 do
//! certificado (`06-arquitetura.md` §6, camada 2). Sem CA: quem conecta fixa o *fingerprint*.

use std::fmt;
use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::{CertificateError, DigitallySignedStruct, SignatureScheme};

/// Nome usado no SNI e no certificado. A autenticação é pelo *fingerprint*, não pelo nome.
pub const SERVER_NAME: &str = "taildesk";

/// SHA-256 do certificado (DER).
pub type Fingerprint = [u8; 32];

pub fn fingerprint(cert: &CertificateDer<'_>) -> Fingerprint {
    let d = ring::digest::digest(&ring::digest::SHA256, cert.as_ref());
    let mut out = [0; 32];
    out.copy_from_slice(d.as_ref());
    out
}

/// Fingerprint em hexadecimal minúsculo (64 caracteres).
pub fn to_hex(fp: &Fingerprint) -> String {
    fp.iter().map(|b| format!("{b:02x}")).collect()
}

/// Lê um fingerprint em hexadecimal (aceita `:` entre os bytes).
pub fn from_hex(s: &str) -> Option<Fingerprint> {
    let clean: Vec<u8> = s.bytes().filter(|&b| b != b':').collect();
    if clean.len() != 64 {
        return None;
    }
    let mut out = [0; 32];
    for (i, pair) in clean.chunks_exact(2).enumerate() {
        let hex = std::str::from_utf8(pair).ok()?;
        out[i] = u8::from_str_radix(hex, 16).ok()?;
    }
    Some(out)
}

/// Certificado e chave do servidor. A chave nunca é logada (`Debug` mostra só o fingerprint).
pub struct Identity {
    cert: CertificateDer<'static>,
    key: PrivatePkcs8KeyDer<'static>,
}

impl fmt::Debug for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Identity")
            .field("fingerprint", &to_hex(&self.fingerprint()))
            .finish_non_exhaustive()
    }
}

impl Identity {
    /// Gera uma identidade nova (ECDSA P-256). A persistência no `keyring` fica para a sessão.
    pub fn generate() -> Result<Identity, rcgen::Error> {
        let ck = rcgen::generate_simple_self_signed(vec![SERVER_NAME.to_owned()])?;
        Ok(Identity {
            cert: ck.cert.der().clone(),
            key: PrivatePkcs8KeyDer::from(ck.signing_key.serialize_der()),
        })
    }

    pub fn fingerprint(&self) -> Fingerprint {
        fingerprint(&self.cert)
    }

    /// Certificado e chave em bytes, para guardar no `keyring` (nunca em arquivo nem em log):
    /// `"TDI1"`, tamanho do certificado (u32 LE), certificado DER, chave PKCS#8 DER.
    pub fn to_secret(&self) -> Vec<u8> {
        let cert = self.cert.as_ref();
        let key = self.key.secret_pkcs8_der();
        let mut out = Vec::with_capacity(8 + cert.len() + key.len());
        out.extend_from_slice(Self::MAGIC);
        out.extend_from_slice(&(cert.len() as u32).to_le_bytes());
        out.extend_from_slice(cert);
        out.extend_from_slice(key);
        out
    }

    /// Lê o que [`Identity::to_secret`] gravou. `None` se o formato não bate ou a chave não
    /// corresponde a um certificado utilizável.
    pub fn from_secret(bytes: &[u8]) -> Option<Identity> {
        let rest = bytes.strip_prefix(Self::MAGIC)?;
        let (len, rest) = rest.split_first_chunk::<4>()?;
        let len = u32::from_le_bytes(*len) as usize;
        if len > rest.len() || len > Self::MAX_CERT {
            return None;
        }
        let (cert, key) = rest.split_at(len);
        let id = Identity {
            cert: CertificateDer::from(cert.to_vec()),
            key: PrivatePkcs8KeyDer::from(key.to_vec()),
        };
        // Confere que chave e certificado formam uma configuração válida.
        id.server_config().ok()?;
        Some(id)
    }

    const MAGIC: &[u8; 4] = b"TDI1";
    const MAX_CERT: usize = 16 * 1024;

    pub(crate) fn server_config(&self) -> Result<rustls::ServerConfig, rustls::Error> {
        let mut cfg = rustls::ServerConfig::builder_with_provider(provider())
            .with_protocol_versions(&[&rustls::version::TLS13])?
            .with_no_client_auth()
            .with_single_cert(
                vec![self.cert.clone()],
                PrivateKeyDer::Pkcs8(self.key.clone_key()),
            )?;
        cfg.alpn_protocols = vec![taildesk_proto::ALPN_V1.to_vec()];
        Ok(cfg)
    }
}

/// Como o cliente confere o certificado do servidor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pin {
    /// Só aceita este fingerprint.
    Sha256(Fingerprint),
    /// Aceita qualquer certificado. Só para o `bench-net` com `--inseguro`, que não troca
    /// nada além de dados sintéticos; nunca para sessões.
    Insecure,
    /// Primeiro contato (TOFU, `06-arquitetura.md` §6): aceita o certificado e quem chama lê
    /// [`peer_fingerprint`] e o grava ligado ao `node_stable_id` vindo de `Overlay::identify()`.
    /// Da próxima vez conecta com [`Pin::Sha256`].
    FirstContact,
}

/// Fingerprint do certificado que o servidor apresentou nesta conexão.
pub fn peer_fingerprint(conn: &quinn::Connection) -> Option<Fingerprint> {
    let certs = conn
        .peer_identity()?
        .downcast::<Vec<CertificateDer<'static>>>()
        .ok()?;
    certs.first().map(fingerprint)
}

pub(crate) fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

pub(crate) fn client_config(pin: Pin) -> Result<rustls::ClientConfig, rustls::Error> {
    let provider = provider();
    let mut cfg = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinnedVerifier { pin, provider }))
        .with_no_client_auth();
    cfg.alpn_protocols = vec![taildesk_proto::ALPN_V1.to_vec()];
    Ok(cfg)
}

#[derive(Debug)]
struct PinnedVerifier {
    pin: Pin,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        match self.pin {
            Pin::Insecure | Pin::FirstContact => Ok(ServerCertVerified::assertion()),
            Pin::Sha256(expected) if fingerprint(end_entity) == expected => {
                Ok(ServerCertVerified::assertion())
            }
            Pin::Sha256(_) => Err(rustls::Error::InvalidCertificate(
                CertificateError::ApplicationVerificationFailure,
            )),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip_and_colons() {
        let fp: Fingerprint = std::array::from_fn(|i| i as u8 * 7);
        let hex = to_hex(&fp);
        assert_eq!(hex.len(), 64);
        assert_eq!(from_hex(&hex), Some(fp));
        let colons: Vec<String> = fp.iter().map(|b| format!("{b:02X}")).collect();
        assert_eq!(from_hex(&colons.join(":")), Some(fp));
        assert_eq!(from_hex("abc"), None);
        assert_eq!(from_hex(&"zz".repeat(32)), None);
    }

    #[test]
    fn identity_secret_round_trip() {
        let id = Identity::generate().unwrap();
        let bytes = id.to_secret();
        let back = Identity::from_secret(&bytes).unwrap();
        assert_eq!(back.fingerprint(), id.fingerprint());
        assert!(Identity::from_secret(&bytes[..bytes.len() - 1]).is_none());
        assert!(Identity::from_secret(b"TDI1").is_none());
        let mut errado = bytes.clone();
        errado[0] = b'X';
        assert!(Identity::from_secret(&errado).is_none());
        // Chave de outra identidade não serve com este certificado.
        let outra = Identity::generate().unwrap().to_secret();
        let cert_len = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
        let outra_len = u32::from_le_bytes(outra[4..8].try_into().unwrap()) as usize;
        let mut misturada = bytes[..8 + cert_len].to_vec();
        misturada.extend_from_slice(&outra[8 + outra_len..]);
        assert!(Identity::from_secret(&misturada).is_none());
    }

    #[test]
    fn identity_debug_hides_key() {
        let id = Identity::generate().unwrap();
        let dbg = format!("{id:?}");
        assert!(dbg.contains(&to_hex(&id.fingerprint())));
        assert!(!dbg.contains("key"));
    }
}
