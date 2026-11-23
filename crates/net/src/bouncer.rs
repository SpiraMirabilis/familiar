//! The client side of the mbnc bouncer protocol (version 1).
//!
//! The bouncer holds the connection to the world and multiplexes characters over one framed
//! TCP/TLS connection. Each frame is `length: u32 BE`, `type: u8`, payload. CONTROL frames carry
//! JSON; DATA and BACKLOG frames carry a session id and the world's raw telnet stream, which goes
//! through the same telnet parser as a direct connection. This module opens one bouncer
//! connection per character, so it only ever attaches to one session.

use serde::{Deserialize, Serialize};

use crate::ConnectOptions;

const FRAME_CONTROL: u8 = 0x01;
const FRAME_DATA: u8 = 0x02;
const FRAME_BACKLOG: u8 = 0x03;
const MAX_FRAME_LEN: usize = 16 * 1024 * 1024;

pub type SessionId = u32;

/// How to reach the bouncer, and what to ask it for.
#[derive(Debug, Clone)]
pub struct BouncerOptions {
    /// The bouncer itself. The world to connect to is [`crate::SessionOptions::connect`].
    pub connect: ConnectOptions,
    pub password: String,
    /// Part of the bouncer's session key, with the world's host and port. Must not be empty.
    pub character: String,
    /// Run by the bouncer on every connection to the world, including reconnects while the
    /// client is away. None keeps the script the bouncer already has.
    pub login: Option<Vec<LoginStep>>,
}

/// One step of the bouncer's login script: wait for `expect` (if any), then `delay_ms`, then send.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginStep {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect: Option<String>,
    pub send: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay_ms: Option<u64>,
}

/// Requests a user can make of the bouncer about the current session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BouncerRequest {
    /// Show the sessions the bouncer holds.
    List,
    /// Drop the world connection and connect again, logging in.
    Reconnect,
    /// Disconnect from the world and make the bouncer forget the session and its backlog.
    Close,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Frame {
    Control(Vec<u8>),
    Data { session: SessionId, bytes: Vec<u8> },
    Backlog { session: SessionId, ts_ms: i64, bytes: Vec<u8> },
}

impl Frame {
    pub(crate) fn control(msg: &ClientMsg) -> Self {
        Frame::Control(serde_json::to_vec(msg).expect("client messages serialize"))
    }

    pub(crate) fn encode(&self) -> Vec<u8> {
        let (ty, header, body): (u8, Vec<u8>, &[u8]) = match self {
            Frame::Control(json) => (FRAME_CONTROL, Vec::new(), json),
            Frame::Data { session, bytes } => (FRAME_DATA, session.to_be_bytes().to_vec(), bytes),
            Frame::Backlog { session, ts_ms, bytes } => {
                let mut h = session.to_be_bytes().to_vec();
                h.extend_from_slice(&ts_ms.to_be_bytes());
                (FRAME_BACKLOG, h, bytes)
            }
        };
        let len = 1 + header.len() + body.len();
        let mut out = Vec::with_capacity(4 + len);
        out.extend_from_slice(&(len as u32).to_be_bytes());
        out.push(ty);
        out.extend_from_slice(&header);
        out.extend_from_slice(body);
        out
    }
}

/// Collects bytes from the socket and splits them into frames. Reading into this rather than
/// awaiting whole frames keeps the read cancel-safe inside `select!`.
#[derive(Default)]
pub(crate) struct Deframer {
    buf: Vec<u8>,
}

impl Deframer {
    pub(crate) fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// The next complete frame, if there is one.
    pub(crate) fn next(&mut self) -> Result<Option<Frame>, String> {
        if self.buf.len() < 4 {
            return Ok(None);
        }
        let len = u32::from_be_bytes(self.buf[..4].try_into().unwrap()) as usize;
        if len == 0 || len > MAX_FRAME_LEN {
            return Err(format!("bouncer sent a bad frame length ({len})"));
        }
        if self.buf.len() < 4 + len {
            return Ok(None);
        }
        let body: Vec<u8> = self.buf.drain(..4 + len).skip(4).collect();
        let (ty, p) = (body[0], &body[1..]);
        let short = || "bouncer sent a truncated frame".to_string();
        let frame = match ty {
            FRAME_CONTROL => Frame::Control(p.to_vec()),
            FRAME_DATA => {
                let session = u32::from_be_bytes(p.get(..4).ok_or_else(short)?.try_into().unwrap());
                Frame::Data { session, bytes: p[4..].to_vec() }
            }
            FRAME_BACKLOG => {
                let head = p.get(..12).ok_or_else(short)?;
                Frame::Backlog {
                    session: u32::from_be_bytes(head[..4].try_into().unwrap()),
                    ts_ms: i64::from_be_bytes(head[4..12].try_into().unwrap()),
                    bytes: p[12..].to_vec(),
                }
            }
            t => return Err(format!("bouncer sent an unknown frame type 0x{t:02x}")),
        };
        Ok(Some(frame))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct WorldSpec {
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub verify_cert: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct TelnetPrefs {
    pub server_options: Vec<u8>,
    pub client_options: Vec<u8>,
    pub ttype: Vec<String>,
    pub naws: (u16, u16),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ClientMsg {
    Auth {
        password: String,
        client: String,
    },
    List {
        req: u64,
    },
    Open {
        req: u64,
        world: WorldSpec,
        character: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        login: Option<Vec<LoginStep>>,
        telnet: TelnetPrefs,
    },
    Detach {
        session: SessionId,
    },
    Reconnect {
        req: u64,
        session: SessionId,
    },
    Close {
        req: u64,
        session: SessionId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ConnState {
    Disconnected,
    Connecting,
    Connected,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(crate) struct SessionInfo {
    pub session: SessionId,
    pub host: String,
    pub port: u16,
    pub character: String,
    pub state: ConnState,
    pub attached: u32,
    pub backlog_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ServerMsg {
    Hello {
        #[serde(default)]
        server: String,
        protocol: u32,
    },
    AuthOk,
    Pong {},
    Ok {},
    Error {
        message: String,
    },
    Sessions {
        sessions: Vec<SessionInfo>,
    },
    Attached {
        created: bool,
        info: SessionInfo,
    },
    BacklogEnd {
        session: SessionId,
        bytes: u64,
        truncated: bool,
    },
    Status {
        session: SessionId,
        state: ConnState,
        #[serde(default)]
        message: Option<String>,
    },
    Notice {
        session: SessionId,
        ts_ms: i64,
        message: String,
    },
    Closed {
        session: SessionId,
        reason: String,
    },
    Detached {
        session: SessionId,
        reason: String,
    },
    /// A message type from a newer bouncer.
    #[serde(other)]
    Unknown,
}

impl ServerMsg {
    pub(crate) fn parse(json: &[u8]) -> Result<Self, String> {
        serde_json::from_slice(json).map_err(|e| format!("bouncer sent a bad control message: {e}"))
    }
}

impl SessionInfo {
    pub(crate) fn describe(&self) -> String {
        let state = match self.state {
            ConnState::Disconnected => "disconnected",
            ConnState::Connecting => "connecting",
            ConnState::Connected => "connected",
        };
        let mut s = format!("#{} {} on {}:{}, {state}", self.session, self.character, self.host, self.port);
        if self.attached > 0 {
            s.push_str(&format!(", {} attached", self.attached));
        }
        if self.backlog_bytes > 0 {
            s.push_str(&format!(", {} bytes waiting", self.backlog_bytes));
        }
        s
    }
}

/// Turns a familiar login (one command per line, `/delay TIME command` allowed) into a bouncer
/// login script. `parse_delay` reads TIME. Lines that are other client commands can't run on
/// the bouncer and are returned separately, for the client to run itself.
pub fn login_script(login: &str, parse_delay: impl Fn(&str) -> Option<std::time::Duration>) -> (Vec<LoginStep>, Vec<String>) {
    // Familiar's /delays all start counting at connect, while the bouncer's delays count from
    // the previous step, so sort by when each line goes out and send the differences.
    let mut timed: Vec<(u64, String)> = Vec::new();
    let mut local = Vec::new();
    for line in login.lines().filter(|l| !l.is_empty()) {
        if let Some(literal) = line.strip_prefix("//") {
            timed.push((0, format!("/{literal}")));
        } else if let Some(command) = line.strip_prefix('/') {
            let delayed = command.split_once(char::is_whitespace).and_then(|(name, rest)| {
                if !name.eq_ignore_ascii_case("delay") {
                    return None;
                }
                let (time, cmd) = rest.trim_start().split_once(char::is_whitespace)?;
                let wait = parse_delay(time)?;
                let cmd = cmd.trim();
                let cmd = cmd.strip_prefix('"').and_then(|c| c.strip_suffix('"')).unwrap_or(cmd).replace("\\\"", "\"");
                (!cmd.starts_with('/') || cmd.starts_with("//")).then(|| (wait.as_millis() as u64, cmd.strip_prefix('/').map_or(cmd.clone(), str::to_string)))
            });
            match delayed {
                Some(step) => timed.push(step),
                None => local.push(line.to_string()),
            }
        } else {
            timed.push((0, line.to_string()));
        }
    }
    timed.sort_by_key(|(at, _)| *at);
    let mut previous = 0;
    let steps = timed
        .into_iter()
        .map(|(at, send)| {
            let delay = at - previous;
            previous = at;
            LoginStep { expect: None, send, delay_ms: (delay > 0).then_some(delay) }
        })
        .collect();
    (steps, local)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn frames_round_trip() {
        let frames = [
            Frame::control(&ClientMsg::List { req: 7 }),
            Frame::Data { session: 3, bytes: b"look\r\n".to_vec() },
            Frame::Backlog { session: 9, ts_ms: -5, bytes: vec![255, 249] },
        ];
        let mut d = Deframer::default();
        let wire: Vec<u8> = frames.iter().flat_map(Frame::encode).collect();
        // Byte by byte, as a slow socket might deliver it.
        let mut got = Vec::new();
        for b in wire {
            d.push(&[b]);
            while let Some(f) = d.next().unwrap() {
                got.push(f);
            }
        }
        assert_eq!(got, frames);
    }

    #[test]
    fn bad_frames_are_errors() {
        let mut d = Deframer::default();
        d.push(&[0, 0, 0, 0]);
        assert!(d.next().is_err());
        let mut d = Deframer::default();
        d.push(&[0, 0, 0, 1, 9]);
        assert!(d.next().is_err());
    }

    #[test]
    fn server_messages_parse() {
        let m = ServerMsg::parse(br#"{"type":"attached","req":1,"created":false,"info":{"session":4,"host":"h","port":1,"tls":false,"character":"Bob","state":"connected","attached":1,"backlog_bytes":10}}"#).unwrap();
        assert!(matches!(m, ServerMsg::Attached { created: false, info } if info.session == 4));
        assert_eq!(ServerMsg::parse(br#"{"type":"ok","req":3}"#).unwrap(), ServerMsg::Ok {});
        assert_eq!(ServerMsg::parse(br#"{"type":"something_new","x":1}"#).unwrap(), ServerMsg::Unknown);
    }

    #[test]
    fn open_json() {
        let open = ClientMsg::Open {
            req: 1,
            world: WorldSpec { host: "moo".into(), port: 7777, tls: false, verify_cert: true },
            character: "Bob".into(),
            login: None,
            telnet: TelnetPrefs { server_options: vec![1], client_options: vec![31], ttype: vec!["X".into()], naws: (80, 24) },
        };
        let json: serde_json::Value = serde_json::to_value(&open).unwrap();
        assert_eq!(json["type"], "open");
        assert_eq!(json["world"]["verify_cert"], true);
        assert!(json.get("login").is_none());
        assert_eq!(json["telnet"]["naws"], serde_json::json!([80, 24]));
    }

    fn secs(s: &str) -> Option<Duration> {
        s.strip_suffix('s')?.parse().ok().map(Duration::from_secs)
    }

    #[test]
    fn login_scripts() {
        let (steps, local) = login_script("connect bob pw\nlook", secs);
        assert_eq!(steps.iter().map(|s| (s.send.as_str(), s.delay_ms)).collect::<Vec<_>>(), [("connect bob pw", None), ("look", None)]);
        assert!(local.is_empty());

        let (steps, local) = login_script("/delay 3s \"connect bob pw\"\n/delay 5s look\nwho\n/echo hi\n//slash", secs);
        assert_eq!(
            steps.iter().map(|s| (s.send.as_str(), s.delay_ms)).collect::<Vec<_>>(),
            [("who", None), ("/slash", None), ("connect bob pw", Some(3000)), ("look", Some(2000))]
        );
        assert_eq!(local, ["/echo hi"]);
    }
}
