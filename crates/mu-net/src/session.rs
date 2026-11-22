use std::net::SocketAddr;
use std::time::Duration;

use mu_telnet::{Event as TelnetEvent, LineSplitter, Telnet, TelnetConfig, mtts};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::{ConnectOptions, Encoding, Stream, connect};

#[derive(Debug, Clone)]
pub struct SessionOptions {
    pub connect: ConnectOptions,
    pub telnet: TelnetConfig,
    pub encoding: Encoding,
    /// Show an unterminated line as a prompt after this much silence. Covers servers that don't
    /// send GA/EOR.
    pub prompt_timeout: Option<Duration>,
}

impl SessionOptions {
    pub fn new(connect: ConnectOptions) -> Self {
        Self {
            connect,
            telnet: TelnetConfig::default(),
            encoding: Encoding::default(),
            prompt_timeout: Some(Duration::from_millis(250)),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SessionEvent {
    Connected { addr: SocketAddr },
    /// A complete line, decoded but with ANSI codes still in it.
    Line(String),
    /// A prompt ended by GA/EOR.
    Prompt(String),
    /// Text with no newline yet that has sat for `prompt_timeout`. Usually a prompt. The full
    /// line still arrives later as `Line` if the server finishes it, so the UI should replace,
    /// not append, a shown partial line.
    PartialLine(String),
    Gmcp { package: String, data: String },
    Mssp(Vec<(String, Vec<String>)>),
    /// Server-side echo is on, so local echo should be off (usually a password prompt).
    ServerEcho(bool),
    EncodingChanged(Encoding),
    Compression(bool),
    /// Informational message about the connection, for the user.
    Notice(String),
    /// The connection is closed. `error` is None for a normal close.
    Disconnected { error: Option<String> },
}

#[derive(Debug, Clone)]
pub enum SessionCommand {
    SendLine(String),
    SendGmcp { package: String, data: Option<String> },
    WindowSize { width: u16, height: u16 },
    Disconnect,
}

#[derive(Debug, Clone)]
pub struct SessionHandle {
    tx: mpsc::UnboundedSender<SessionCommand>,
}

impl SessionHandle {
    /// A handle that isn't attached to a connection. Commands arrive on the returned receiver.
    /// Useful for tests and for UIs that want to inspect what would be sent.
    pub fn channel() -> (Self, mpsc::UnboundedReceiver<SessionCommand>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx }, rx)
    }

    /// Returns false if the session has already ended.
    pub fn send(&self, command: SessionCommand) -> bool {
        self.tx.send(command).is_ok()
    }

    pub fn send_line(&self, line: impl Into<String>) -> bool {
        self.send(SessionCommand::SendLine(line.into()))
    }

    pub fn send_gmcp(&self, package: impl Into<String>, data: Option<String>) -> bool {
        self.send(SessionCommand::SendGmcp { package: package.into(), data })
    }

    pub fn set_window_size(&self, width: u16, height: u16) -> bool {
        self.send(SessionCommand::WindowSize { width, height })
    }

    pub fn disconnect(&self) -> bool {
        self.send(SessionCommand::Disconnect)
    }

    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
}

/// Starts a connection on the current tokio runtime. Events arrive on the returned receiver,
/// ending with [`SessionEvent::Disconnected`]. The receiver can be polled from any executor.
pub fn spawn_session(mut opts: SessionOptions) -> (SessionHandle, mpsc::UnboundedReceiver<SessionEvent>) {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    if opts.connect.tls {
        opts.telnet.mtts |= mtts::SSL;
    }
    tokio::spawn(async move {
        let error = run(opts, cmd_rx, &event_tx).await.err();
        let _ = event_tx.send(SessionEvent::Disconnected { error });
    });
    (SessionHandle { tx: cmd_tx }, event_rx)
}

struct Session<'a> {
    telnet: Telnet,
    lines: LineSplitter,
    encoding: Encoding,
    events: &'a mpsc::UnboundedSender<SessionEvent>,
}

impl Session<'_> {
    fn emit(&self, event: SessionEvent) {
        let _ = self.events.send(event);
    }

    fn on_data(&mut self, data: &[u8]) {
        for event in self.telnet.feed(data) {
            match event {
                TelnetEvent::Data(bytes) => {
                    for line in self.lines.push(&bytes) {
                        self.emit(SessionEvent::Line(self.encoding.decode(&line.bytes)));
                    }
                }
                TelnetEvent::Prompt => {
                    if let Some(line) = self.lines.take_prompt() {
                        self.emit(SessionEvent::Prompt(self.encoding.decode(&line.bytes)));
                    }
                }
                TelnetEvent::Gmcp { package, data } => self.emit(SessionEvent::Gmcp { package, data }),
                TelnetEvent::Mssp(vars) => self.emit(SessionEvent::Mssp(vars)),
                TelnetEvent::ServerEcho(on) => self.emit(SessionEvent::ServerEcho(on)),
                TelnetEvent::CharsetChanged(name) => match Encoding::from_name(&name) {
                    Some(encoding) => {
                        self.encoding = encoding;
                        self.emit(SessionEvent::EncodingChanged(encoding));
                    }
                    None => self.emit(SessionEvent::Notice(format!("Server chose unknown charset {name}"))),
                },
                TelnetEvent::CompressionStarted => self.emit(SessionEvent::Compression(true)),
                TelnetEvent::CompressionEnded => self.emit(SessionEvent::Compression(false)),
                TelnetEvent::CompressionError(e) => {
                    self.emit(SessionEvent::Notice(format!("MCCP decompression failed: {e}")));
                    self.emit(SessionEvent::Compression(false));
                }
                TelnetEvent::OptionChanged { .. } | TelnetEvent::Subnegotiation { .. } => {}
            }
        }
    }

    fn on_command(&mut self, command: SessionCommand) {
        match command {
            SessionCommand::SendLine(line) => {
                let bytes = self.encoding.encode(&line);
                self.telnet.send_line(&bytes);
            }
            SessionCommand::SendGmcp { package, data } => {
                if !self.telnet.send_gmcp(&package, data.as_deref()) {
                    self.emit(SessionEvent::Notice(format!("GMCP is not enabled; {package} not sent")));
                }
            }
            SessionCommand::WindowSize { width, height } => self.telnet.set_window_size(width, height),
            SessionCommand::Disconnect => {}
        }
    }
}

async fn run(
    opts: SessionOptions,
    mut commands: mpsc::UnboundedReceiver<SessionCommand>,
    events: &mpsc::UnboundedSender<SessionEvent>,
) -> Result<(), String> {
    let (mut stream, addr) = connect(&opts.connect).await.map_err(|e| e.to_string())?;
    let _ = events.send(SessionEvent::Connected { addr });

    let mut session = Session {
        telnet: Telnet::new(opts.telnet.clone()),
        lines: LineSplitter::new(),
        encoding: opts.encoding,
        events,
    };
    let mut buf = vec![0u8; 16384];
    let mut partial_shown = false;

    loop {
        let prompt_wait = match opts.prompt_timeout {
            Some(t) if !partial_shown && !session.lines.partial().is_empty() => Some(t),
            _ => None,
        };
        tokio::select! {
            read = stream.read(&mut buf) => match read {
                Ok(0) => break,
                Ok(n) => {
                    session.on_data(&buf[..n]);
                    partial_shown = false;
                }
                Err(e) => return Err(e.to_string()),
            },
            command = commands.recv() => match command {
                None | Some(SessionCommand::Disconnect) => {
                    let _ = stream.shutdown().await;
                    break;
                }
                Some(command) => session.on_command(command),
            },
            _ = tokio::time::sleep(prompt_wait.unwrap_or_default()), if prompt_wait.is_some() => {
                let text = session.encoding.decode_partial(session.lines.partial());
                session.emit(SessionEvent::PartialLine(text));
                partial_shown = true;
            }
        }
        write_pending(&mut stream, &mut session.telnet).await?;
    }
    Ok(())
}

async fn write_pending(stream: &mut Box<dyn Stream>, telnet: &mut Telnet) -> Result<(), String> {
    let out = telnet.take_output();
    if !out.is_empty() {
        stream.write_all(&out).await.map_err(|e| e.to_string())?;
        stream.flush().await.map_err(|e| e.to_string())?;
    }
    Ok(())
}
