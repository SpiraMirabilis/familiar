//! Runs a session against a scripted fake MUD on localhost.

use std::time::Duration;

use flate2::{Compress, Compression, FlushCompress};
use mu_net::{ConnectOptions, Encoding, SessionEvent, SessionOptions, spawn_session};
use mu_telnet::{cmd::*, opt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::UnboundedReceiver;

async fn next_event(rx: &mut UnboundedReceiver<SessionEvent>) -> SessionEvent {
    tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("timed out").expect("closed")
}

/// Reads from the client until `needle` shows up, and returns everything read.
async fn read_until(sock: &mut TcpStream, needle: &[u8]) -> Vec<u8> {
    let mut got = Vec::new();
    let mut buf = [0u8; 1024];
    while !got.windows(needle.len()).any(|w| w == needle) {
        let n = tokio::time::timeout(Duration::from_secs(5), sock.read(&mut buf)).await.unwrap().unwrap();
        assert!(n > 0, "client closed; got {got:?}");
        got.extend_from_slice(&buf[..n]);
    }
    got
}

#[tokio::test]
async fn full_session() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let server = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.write_all(&[IAC, WILL, opt::GMCP, IAC, DO, opt::NAWS, IAC, WILL, opt::MCCP2]).await.unwrap();
        let got = read_until(&mut sock, b"Core.Hello").await;
        assert!(got.windows(3).any(|w| w == [IAC, WILL, opt::NAWS]));

        sock.write_all(b"Welcome!\r\n\x1b[31mred\x1b[0m line\r\nName: ").await.unwrap();
        sock.write_all(&[IAC, GA]).await.unwrap();
        read_until(&mut sock, b"bob\r\n").await;

        let mut gmcp = vec![IAC, SB, opt::GMCP];
        gmcp.extend_from_slice(br#"Char.Vitals {"hp":5}"#);
        gmcp.extend_from_slice(&[IAC, SE]);
        sock.write_all(&gmcp).await.unwrap();

        // Start compression and send the rest compressed, in a single write.
        let mut out = vec![IAC, SB, opt::MCCP2, IAC, SE];
        let mut z = Compress::new(Compression::default(), true);
        let mut compressed = Vec::with_capacity(256);
        z.compress_vec("caf\u{e9} compressed\r\nHP> ".as_bytes(), &mut compressed, FlushCompress::Sync).unwrap();
        out.extend_from_slice(&compressed);
        sock.write_all(&out).await.unwrap();
        // No GA, so the client should time out and show "HP> " as a partial line.
        read_until(&mut sock, b"quit\r\n").await;
    });

    let mut opts = SessionOptions::new(ConnectOptions::new("127.0.0.1", port));
    opts.encoding = Encoding::Utf8;
    opts.prompt_timeout = Some(Duration::from_millis(50));
    let (handle, mut rx) = spawn_session(opts);
    handle.set_window_size(100, 40);

    assert!(matches!(next_event(&mut rx).await, SessionEvent::Connected { .. }));
    assert_eq!(next_event(&mut rx).await, SessionEvent::Line("Welcome!".into()));
    assert_eq!(next_event(&mut rx).await, SessionEvent::Line("\x1b[31mred\x1b[0m line".into()));
    assert_eq!(next_event(&mut rx).await, SessionEvent::Prompt("Name: ".into()));
    handle.send_line("bob");
    assert_eq!(
        next_event(&mut rx).await,
        SessionEvent::Gmcp { package: "Char.Vitals".into(), data: r#"{"hp":5}"#.into() }
    );
    assert_eq!(next_event(&mut rx).await, SessionEvent::Compression(true));
    assert_eq!(next_event(&mut rx).await, SessionEvent::Line("café compressed".into()));
    assert_eq!(next_event(&mut rx).await, SessionEvent::PartialLine("HP> ".into()));
    handle.send_line("quit");

    server.await.unwrap();
    assert_eq!(next_event(&mut rx).await, SessionEvent::Disconnected { error: None });
}

#[tokio::test]
async fn connection_refused() {
    // Bind and drop to get a port nothing listens on.
    let port = TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap().port();
    let (_handle, mut rx) = spawn_session(SessionOptions::new(ConnectOptions::new("127.0.0.1", port)));
    match next_event(&mut rx).await {
        SessionEvent::Disconnected { error: Some(e) } => assert!(e.to_lowercase().contains("refused"), "{e}"),
        other => panic!("unexpected {other:?}"),
    }
}
