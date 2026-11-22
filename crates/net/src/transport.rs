use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

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
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((opts.host.as_str(), opts.port))
        .await
        .map_err(|e| Error::Resolve(target.clone(), e))?
        .filter(|a| !opts.force_ipv4 || a.is_ipv4())
        .collect();
    if addrs.is_empty() {
        return Err(Error::NoAddresses(target));
    }

    let mut last_error = Error::NoAddresses(target);
    for addr in addrs {
        match tokio::time::timeout(opts.timeout, TcpStream::connect(addr)).await {
            Ok(Ok(tcp)) => {
                tcp.set_nodelay(true)?;
                if !opts.tls {
                    return Ok((Box::new(tcp), addr));
                }
                let stream = tokio::time::timeout(opts.timeout, tls_handshake(tcp, opts))
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

async fn tls_handshake(tcp: TcpStream, opts: &ConnectOptions) -> Result<Box<dyn Stream>, Error> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = ClientConfig::builder_with_provider(provider.clone()).with_safe_default_protocol_versions()?;
    let config = if opts.verify_certificate {
        builder.with_platform_verifier()?.with_no_client_auth()
    } else {
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerification(provider.signature_verification_algorithms)))
            .with_no_client_auth()
    };
    let name = ServerName::try_from(opts.host.clone()).map_err(|_| Error::ServerName(opts.host.clone()))?;
    let stream = TlsConnector::from(Arc::new(config)).connect(name, tcp).await?;
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
