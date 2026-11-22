//! Networking for MU* connections: TCP/TLS transport and an async session task that runs the
//! telnet protocol and hands decoded lines to the caller.

mod encoding;
mod session;
mod transport;

pub use encoding::Encoding;
pub use session::{SessionCommand, SessionEvent, SessionHandle, SessionOptions, spawn_session};
pub use transport::{ConnectOptions, Stream, connect};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not resolve {0}: {1}")]
    Resolve(String, std::io::Error),
    #[error("no addresses found for {0}")]
    NoAddresses(String),
    #[error("connection timed out")]
    Timeout,
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("TLS: {0}")]
    Tls(#[from] rustls::Error),
    #[error("invalid TLS server name: {0}")]
    ServerName(String),
}
