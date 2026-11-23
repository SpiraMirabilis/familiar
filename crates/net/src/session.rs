use std::net::SocketAddr;
use std::time::Duration;

use familiar_telnet::{Event as TelnetEvent, LineSplitter, Telnet, TelnetConfig, mtts, opt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::bouncer::{BouncerOptions, BouncerRequest, ClientMsg, Deframer, Frame, ServerMsg, SessionId, TelnetPrefs, WorldSpec};
use crate::{ConnectOptions, Encoding, Stream, connect};

#[derive(Debug, Clone)]
pub struct SessionOptions {
    pub connect: ConnectOptions,
    pub telnet: TelnetConfig,
    pub encoding: Encoding,
    /// Show an unterminated line as a prompt after this much silence. Covers servers that don't
    /// send GA/EOR.
    pub prompt_timeout: Option<Duration>,
    /// Connect to the world through this bouncer instead of directly.
    pub bouncer: Option<BouncerOptions>,
}

impl SessionOptions {
    pub fn new(connect: ConnectOptions) -> Self {
        Self {
            connect,
            telnet: TelnetConfig::default(),
            encoding: Encoding::default(),
            prompt_timeout: Some(Duration::from_millis(250)),
            bouncer: None,
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
    /// Only does something on a connection through a bouncer.
    Bouncer(BouncerRequest),
    /// Close the connection. Through a bouncer, this detaches: the bouncer stays connected to
    /// the world and keeps what arrives for next time.
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

    pub fn bouncer(&self, request: BouncerRequest) -> bool {
        self.send(SessionCommand::Bouncer(request))
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
            SessionCommand::Bouncer(_) => self.emit(SessionEvent::Notice("Not connected through a bouncer".into())),
            SessionCommand::Disconnect => {}
        }
    }
}

async fn run(
    opts: SessionOptions,
    mut commands: mpsc::UnboundedReceiver<SessionCommand>,
    events: &mpsc::UnboundedSender<SessionEvent>,
) -> Result<(), String> {
    let target = opts.bouncer.as_ref().map_or(&opts.connect, |b| &b.connect);
    let (mut stream, addr) = connect(target).await.map_err(|e| e.to_string())?;
    let mut bouncer = match &opts.bouncer {
        Some(b) => {
            let wait = opts.connect.timeout.max(Duration::from_secs(30));
            let link = tokio::time::timeout(wait, BouncerLink::open(&mut stream, &opts, b, events))
                .await
                .map_err(|_| "the bouncer didn't answer".to_string())??;
            Some(link)
        }
        None => None,
    };
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
                Ok(0) => {
                    if bouncer.is_some() {
                        return Err("the bouncer closed the connection".into());
                    }
                    break;
                }
                Ok(n) => {
                    match bouncer.as_mut() {
                        Some(link) => {
                            if !link.on_read(&buf[..n], &mut session)? {
                                break;
                            }
                        }
                        None => session.on_data(&buf[..n]),
                    }
                    partial_shown = false;
                }
                Err(e) => return Err(e.to_string()),
            },
            command = commands.recv() => match command {
                None | Some(SessionCommand::Disconnect) => {
                    if let Some(link) = bouncer.as_mut() {
                        link.send(&ClientMsg::Detach { session: link.session });
                        let _ = write_pending(&mut stream, &mut session.telnet, bouncer.as_mut()).await;
                    }
                    let _ = stream.shutdown().await;
                    break;
                }
                Some(SessionCommand::Bouncer(request)) if bouncer.is_some() => {
                    bouncer.as_mut().unwrap().request(request);
                }
                Some(command) => session.on_command(command),
            },
            _ = tokio::time::sleep(prompt_wait.unwrap_or_default()), if prompt_wait.is_some() => {
                let text = session.encoding.decode_partial(session.lines.partial());
                session.emit(SessionEvent::PartialLine(text));
                partial_shown = true;
            }
        }
        write_pending(&mut stream, &mut session.telnet, bouncer.as_mut()).await?;
    }
    Ok(())
}

async fn write_pending(stream: &mut Box<dyn Stream>, telnet: &mut Telnet, bouncer: Option<&mut BouncerLink>) -> Result<(), String> {
    let mut out = telnet.take_output();
    if let Some(link) = bouncer {
        if !out.is_empty() {
            link.out.extend(Frame::Data { session: link.session, bytes: out }.encode());
        }
        out = std::mem::take(&mut link.out);
    }
    if !out.is_empty() {
        stream.write_all(&out).await.map_err(|e| e.to_string())?;
        stream.flush().await.map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// The state of a connection through a bouncer, once attached to a session.
struct BouncerLink {
    session: SessionId,
    frames: Deframer,
    /// Encoded frames waiting to be written.
    out: Vec<u8>,
    next_req: u64,
    /// When the last replayed backlog chunk arrived at the bouncer, while replaying.
    backlog_at: Option<i64>,
}

/// A gap this long in the replayed backlog gets a timestamp.
const BACKLOG_GAP_MS: i64 = 5 * 60 * 1000;

impl BouncerLink {
    /// Logs in to the bouncer and attaches to (or creates) the session for this world and
    /// character.
    async fn open(
        stream: &mut Box<dyn Stream>,
        opts: &SessionOptions,
        bouncer: &BouncerOptions,
        events: &mpsc::UnboundedSender<SessionEvent>,
    ) -> Result<Self, String> {
        let mut frames = Deframer::default();
        match read_control(stream, &mut frames).await? {
            ServerMsg::Hello { protocol: 1, .. } => {}
            ServerMsg::Hello { protocol, server } => {
                return Err(format!("the bouncer ({server}) speaks protocol {protocol}, but this client only knows version 1"));
            }
            other => return Err(format!("the bouncer didn't say hello ({other:?})")),
        }
        let client = format!("{}/{}", opts.telnet.client_name, opts.telnet.client_version);
        let auth = ClientMsg::Auth { password: bouncer.password.clone(), client };
        stream.write_all(&Frame::control(&auth).encode()).await.map_err(|e| e.to_string())?;
        match read_control(stream, &mut frames).await? {
            ServerMsg::AuthOk => {}
            ServerMsg::Error { message } => return Err(format!("bouncer: {message}")),
            other => return Err(format!("unexpected reply from the bouncer: {other:?}")),
        }

        let t = &opts.telnet;
        let mut mtts = t.mtts;
        if opts.connect.tls {
            mtts |= familiar_telnet::mtts::SSL;
        }
        let telnet = TelnetPrefs {
            server_options: vec![opt::ECHO, opt::SGA, opt::EOR, opt::MSSP, opt::GMCP]
                .into_iter()
                .filter(|&o| o != opt::GMCP || t.enable_gmcp)
                .collect(),
            client_options: vec![opt::TTYPE, opt::NAWS],
            ttype: vec![t.client_name.to_uppercase(), t.terminal_type.clone(), format!("MTTS {mtts}")],
            naws: (80, 24),
        };
        let open = ClientMsg::Open {
            req: 1,
            world: WorldSpec {
                host: opts.connect.host.clone(),
                port: opts.connect.port,
                tls: opts.connect.tls,
                verify_cert: opts.connect.verify_certificate,
            },
            character: bouncer.character.clone(),
            login: bouncer.login.clone(),
            telnet,
        };
        stream.write_all(&Frame::control(&open).encode()).await.map_err(|e| e.to_string())?;
        stream.flush().await.map_err(|e| e.to_string())?;
        let info = loop {
            match read_control(stream, &mut frames).await? {
                ServerMsg::Attached { created, info } => {
                    let _ = events.send(SessionEvent::Notice(if created {
                        format!("Bouncer: started session #{} for {}; it connects and logs in from here on", info.session, info.character)
                    } else {
                        format!("Bouncer: resumed session #{} ({})", info.session, info.describe())
                    }));
                    break info;
                }
                ServerMsg::Error { message } => return Err(format!("bouncer: {message}")),
                _ => {}
            }
        };
        Ok(Self { session: info.session, frames, out: Vec::new(), next_req: 2, backlog_at: None })
    }

    fn send(&mut self, msg: &ClientMsg) {
        self.out.extend(Frame::control(msg).encode());
    }

    fn request(&mut self, request: BouncerRequest) {
        let req = self.next_req;
        self.next_req += 1;
        let session = self.session;
        self.send(&match request {
            BouncerRequest::List => ClientMsg::List { req },
            BouncerRequest::Reconnect => ClientMsg::Reconnect { req, session },
            BouncerRequest::Close => ClientMsg::Close { req, session },
        });
    }

    /// Handles bytes from the bouncer. Returns false when the session is over.
    fn on_read(&mut self, data: &[u8], session: &mut Session) -> Result<bool, String> {
        self.frames.push(data);
        while let Some(frame) = self.frames.next()? {
            match frame {
                Frame::Data { session: id, bytes } if id == self.session => session.on_data(&bytes),
                Frame::Backlog { session: id, ts_ms, bytes } if id == self.session => {
                    match self.backlog_at {
                        None => session.emit(SessionEvent::Notice(format!("Bouncer: what arrived while you were away, from {}:", time(ts_ms)))),
                        Some(last) if ts_ms - last >= BACKLOG_GAP_MS => session.emit(SessionEvent::Notice(format!("Bouncer: {}", time(ts_ms)))),
                        _ => {}
                    }
                    self.backlog_at = Some(ts_ms);
                    session.on_data(&bytes);
                }
                Frame::Data { .. } | Frame::Backlog { .. } => {}
                Frame::Control(json) => {
                    if !self.on_control(ServerMsg::parse(&json)?, session) {
                        return Ok(false);
                    }
                }
            }
        }
        Ok(true)
    }

    fn on_control(&mut self, msg: ServerMsg, session: &mut Session) -> bool {
        let notice = |text: String| session.emit(SessionEvent::Notice(text));
        match msg {
            ServerMsg::BacklogEnd { session: id, bytes, truncated } if id == self.session => {
                if self.backlog_at.take().is_some() || bytes > 0 {
                    let dropped = if truncated { "; older output was dropped" } else { "" };
                    notice(format!("Bouncer: end of what arrived while you were away ({}{dropped})", size(bytes)));
                }
            }
            ServerMsg::Notice { session: id, ts_ms, message } if id == self.session => match self.backlog_at {
                Some(_) => notice(format!("Bouncer, {}: {message}", time(ts_ms))),
                None => notice(format!("Bouncer: {message}")),
            },
            ServerMsg::Status { session: id, state, message } if id == self.session => {
                if let Some(message) = message {
                    notice(format!("Bouncer: {message}"));
                }
                // The bouncer doesn't reconnect while a client is attached, in case the user
                // quit, so end the session here; /reconnect asks it to connect again.
                if state == crate::bouncer::ConnState::Disconnected {
                    return false;
                }
            }
            ServerMsg::Closed { session: id, reason } | ServerMsg::Detached { session: id, reason } if id == self.session => {
                notice(format!("Bouncer: {reason}"));
                return false;
            }
            ServerMsg::Error { message } => notice(format!("Bouncer: {message}")),
            ServerMsg::Sessions { sessions } => {
                if sessions.is_empty() {
                    notice("Bouncer: no sessions".into());
                }
                for info in sessions {
                    let this = if info.session == self.session { " (this one)" } else { "" };
                    notice(format!("Bouncer: {}{this}", info.describe()));
                }
            }
            _ => {}
        }
        true
    }
}

/// Reads frames until a control message arrives. Only used before attaching, when there's no
/// session data yet.
async fn read_control(stream: &mut Box<dyn Stream>, frames: &mut Deframer) -> Result<ServerMsg, String> {
    let mut buf = [0u8; 4096];
    loop {
        while let Some(frame) = frames.next()? {
            if let Frame::Control(json) = frame {
                return ServerMsg::parse(&json);
            }
        }
        match stream.read(&mut buf).await {
            Ok(0) => return Err("the bouncer closed the connection".into()),
            Ok(n) => frames.push(&buf[..n]),
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// A bouncer timestamp in local time, with the date if it isn't today.
fn time(ts_ms: i64) -> String {
    use chrono::{Local, TimeZone};
    let Some(t) = Local.timestamp_millis_opt(ts_ms).single() else { return "an unknown time".into() };
    if t.date_naive() == Local::now().date_naive() { t.format("%H:%M").to_string() } else { t.format("%a %b %-d, %H:%M").to_string() }
}

fn size(bytes: u64) -> String {
    match bytes {
        0..1024 => format!("{bytes} bytes"),
        1024..1_048_576 => format!("{:.1} KB", bytes as f64 / 1024.0),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.0),
    }
}
