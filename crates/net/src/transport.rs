use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::WebPkiSupportedAlgorithms;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
use rustls_platform_verifier::BuilderVerifierExt;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

use crate::Error;

#[derive(Debug, Clone)]
pub struct ConnectOptions {
    pub host: String,
    pub port: u16,
    pub tls: bool,
    /// Check the server certificate. Many MU*s use self-signed certificates, so this can be off.
    pub verify_certificate: bool,
    pub force_ipv4: bool,
    pub timeout: Duration,
}

impl ConnectOptions {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
            tls: false,
            verify_certificate: true,
            force_ipv4: false,
            timeout: Duration::from_secs(15),
        }
    }
}

pub trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

/// Resolves the host and connects to each address in turn, then does the TLS handshake if
/// asked. Returns the stream and the address it connected to.
pub async fn connect(opts: &ConnectOptions) -> Result<(Box<dyn Stream>, SocketAddr), Error> {
    let target = format!("{}:{}", opts.host, opts.port);
    let deadline = Instant::now() + opts.timeout;
    let remaining = || deadline.saturating_duration_since(Instant::now());
    let addrs: Vec<SocketAddr> = tokio::time::timeout(remaining(), tokio::net::lookup_host((opts.host.as_str(), opts.port)))
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(|e| Error::Resolve(target.clone(), e))?
        .filter(|a| !opts.force_ipv4 || a.is_ipv4())
        .collect();
    if addrs.is_empty() {
        return Err(Error::NoAddresses(target));
    }

    let mut last_error = Error::NoAddresses(target);
    let count = addrs.len();
    for (idx, addr) in addrs.into_iter().enumerate() {
        let left = remaining();
        if left.is_zero() {
            return Err(Error::Timeout);
        }
        // Divide remaining time across candidate addresses so a blackholed IPv6 address
        // doesn't consume the entire budget before trying IPv4.
        let per_addr = left.div_f64((count - idx) as f64).max(Duration::from_millis(250)).min(left);
        match tokio::time::timeout(per_addr, TcpStream::connect(addr)).await {
            Ok(Ok(tcp)) => {
                let _ = tcp.set_nodelay(true);
                if !opts.tls {
                    return Ok((Box::new(tcp), addr));
                }
                let tls_wait = remaining().max(Duration::from_secs(5));
                let stream = tokio::time::timeout(tls_wait, tls_handshake(tcp, opts))
                    .await
                    .map_err(|_| Error::Timeout)??;
                return Ok((stream, addr));
            }
            Ok(Err(e)) => last_error = Error::Io(e),
            Err(_) => last_error = Error::Timeout,
        }
    }
    Err(last_error)
}

fn tls_config(verify_certificate: bool) -> Result<Arc<ClientConfig>, rustls::Error> {
    if verify_certificate {
        static VERIFIED: OnceLock<Result<Arc<ClientConfig>, String>> = OnceLock::new();
        let cached = VERIFIED.get_or_init(|| {
            let provider = Arc::new(rustls::crypto::ring::default_provider());
            let config = ClientConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .map_err(|e| e.to_string())?
                .with_platform_verifier()
                .map_err(|e| e.to_string())?
                .with_no_client_auth();
            Ok(Arc::new(config))
        });
        match cached {
            Ok(cfg) => Ok(Arc::clone(cfg)),
            Err(msg) => Err(rustls::Error::General(msg.clone())),
        }
    } else {
        static INSECURE: OnceLock<Result<Arc<ClientConfig>, String>> = OnceLock::new();
        let cached = INSECURE.get_or_init(|| {
            let provider = Arc::new(rustls::crypto::ring::default_provider());
            let config = ClientConfig::builder_with_provider(provider.clone())
                .with_safe_default_protocol_versions()
                .map_err(|e| e.to_string())?
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(NoVerification(provider.signature_verification_algorithms)))
                .with_no_client_auth();
            Ok(Arc::new(config))
        });
        match cached {
            Ok(cfg) => Ok(Arc::clone(cfg)),
            Err(msg) => Err(rustls::Error::General(msg.clone())),
        }
    }
}

async fn tls_handshake(tcp: TcpStream, opts: &ConnectOptions) -> Result<Box<dyn Stream>, Error> {
    let config = tls_config(opts.verify_certificate)?;
    let name = ServerName::try_from(opts.host.clone()).map_err(|_| Error::ServerName(opts.host.clone()))?;
    let stream = TlsConnector::from(config).connect(name, tcp).await?;
    Ok(Box::new(stream))
}

/// Accepts any certificate. Only used when the user turns verification off for a server.
#[derive(Debug)]
struct NoVerification(WebPkiSupportedAlgorithms);

impl ServerCertVerifier for NoVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.supported_schemes()
    }
}
