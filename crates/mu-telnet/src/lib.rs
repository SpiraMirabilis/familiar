//! Telnet protocol state machine for MU* clients.
//!
//! Feed raw socket bytes into [`Telnet::feed`]. It returns application events (text, prompts,
//! GMCP messages, option changes) and queues any negotiation replies, which you collect with
//! [`Telnet::take_output`] and write back to the socket.
//!
//! MCCP2 is handled here, because compression can begin in the middle of a read: every byte
//! after `IAC SB COMPRESS2 IAC SE` is compressed, including the rest of the same buffer.

mod lines;

pub use lines::{Line, LineSplitter};

use flate2::{Decompress, FlushDecompress, Status};

pub mod cmd {
    pub const SE: u8 = 240;
    pub const NOP: u8 = 241;
    pub const GA: u8 = 249;
    pub const SB: u8 = 250;
    pub const WILL: u8 = 251;
    pub const WONT: u8 = 252;
    pub const DO: u8 = 253;
    pub const DONT: u8 = 254;
    pub const IAC: u8 = 255;
    pub const EOR: u8 = 239;
}

pub mod opt {
    pub const BINARY: u8 = 0;
    pub const ECHO: u8 = 1;
    pub const SGA: u8 = 3;
    pub const TTYPE: u8 = 24;
    pub const EOR: u8 = 25;
    pub const NAWS: u8 = 31;
    pub const CHARSET: u8 = 42;
    pub const MSSP: u8 = 70;
    pub const MCCP2: u8 = 86;
    pub const GMCP: u8 = 201;
}

const TTYPE_IS: u8 = 0;
const TTYPE_SEND: u8 = 1;
const CHARSET_REQUEST: u8 = 1;
const CHARSET_ACCEPTED: u8 = 2;
const CHARSET_REJECTED: u8 = 3;
const MSSP_VAR: u8 = 1;
const MSSP_VAL: u8 = 2;

/// MTTS capability bits, see <https://tintin.mudhalla.net/protocols/mtts/>.
pub mod mtts {
    pub const ANSI: u32 = 1;
    pub const UTF8: u32 = 4;
    pub const COLORS_256: u32 = 8;
    pub const TRUECOLOR: u32 = 256;
    pub const MSLP: u32 = 1024;
    pub const SSL: u32 = 2048;
}

#[derive(Debug, Clone)]
pub struct TelnetConfig {
    /// First TTYPE reply, the client name.
    pub client_name: String,
    pub client_version: String,
    /// Second TTYPE reply.
    pub terminal_type: String,
    pub mtts: u32,
    pub enable_mccp2: bool,
    pub enable_gmcp: bool,
    /// Package list sent with `Core.Supports.Set` when GMCP turns on.
    pub gmcp_supports: Vec<String>,
    /// Charsets we accept from a CHARSET REQUEST, in order of preference.
    pub charsets: Vec<String>,
}

impl Default for TelnetConfig {
    fn default() -> Self {
        Self {
            client_name: "MU".into(),
            client_version: env!("CARGO_PKG_VERSION").into(),
            terminal_type: "XTERM-256COLOR".into(),
            mtts: mtts::ANSI | mtts::UTF8 | mtts::COLORS_256 | mtts::TRUECOLOR | mtts::MSLP,
            enable_mccp2: true,
            enable_gmcp: true,
            gmcp_supports: Vec::new(),
            charsets: vec!["UTF-8".into()],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Application data, still in the server's encoding. Contains no telnet commands.
    Data(Vec<u8>),
    /// The server sent GA or EOR, so the text since the last newline is a prompt.
    Prompt,
    Gmcp { package: String, data: String },
    /// Server sent the MSSP variables.
    Mssp(Vec<(String, Vec<String>)>),
    /// Unhandled subnegotiation, passed through for logging.
    Subnegotiation { option: u8, data: Vec<u8> },
    /// The server switched echo on (true) or off. Server-side echo usually means a password prompt.
    ServerEcho(bool),
    /// An option was turned on or off. `local` is true for options we perform (WILL/WONT).
    OptionChanged { option: u8, local: bool, enabled: bool },
    /// The server accepted one of our charsets.
    CharsetChanged(String),
    CompressionStarted,
    CompressionEnded,
    /// The compressed stream was corrupt. Compression is switched off.
    CompressionError(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Data,
    Iac,
    Will,
    Wont,
    Do,
    Dont,
    Sb,
    SbData,
    SbIac,
}

pub struct Telnet {
    config: TelnetConfig,
    state: State,
    sb_option: u8,
    sb_data: Vec<u8>,
    data: Vec<u8>,
    events: Vec<Event>,
    out: Vec<u8>,
    /// Indexed by option code.
    local: [bool; 256],
    remote: [bool; 256],
    inflate: Option<Decompress>,
    ttype_index: usize,
    window_size: Option<(u16, u16)>,
}

impl Telnet {
    pub fn new(config: TelnetConfig) -> Self {
        Self {
            config,
            state: State::Data,
            sb_option: 0,
            sb_data: Vec::new(),
            data: Vec::new(),
            events: Vec::new(),
            out: Vec::new(),
            local: [false; 256],
            remote: [false; 256],
            inflate: None,
            ttype_index: 0,
            window_size: None,
        }
    }

    pub fn config(&self) -> &TelnetConfig {
        &self.config
    }

    pub fn is_local_enabled(&self, option: u8) -> bool {
        self.local[option as usize]
    }

    pub fn is_remote_enabled(&self, option: u8) -> bool {
        self.remote[option as usize]
    }

    pub fn is_compressed(&self) -> bool {
        self.inflate.is_some()
    }

    /// Bytes that must be written to the server (negotiation replies, NAWS, GMCP).
    pub fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.out)
    }

    /// Feeds raw bytes from the socket and returns the events they produced.
    pub fn feed(&mut self, mut input: &[u8]) -> Vec<Event> {
        while !input.is_empty() {
            if self.inflate.is_some() {
                input = self.feed_compressed(input);
            } else {
                let used = self.parse(input);
                input = &input[used..];
            }
        }
        self.flush_data();
        std::mem::take(&mut self.events)
    }

    /// Inflates as much of `input` as possible and returns whatever comes after the end of the
    /// compressed stream, if it ended.
    fn feed_compressed<'a>(&mut self, input: &'a [u8]) -> &'a [u8] {
        let mut buf = [0u8; 16384];
        let mut input = input;
        loop {
            let z = self.inflate.as_mut().unwrap();
            let before_in = z.total_in();
            let before_out = z.total_out();
            let result = z.decompress(input, &mut buf, FlushDecompress::None);
            let consumed = (z.total_in() - before_in) as usize;
            let produced = (z.total_out() - before_out) as usize;
            input = &input[consumed..];

            // Parse the inflated bytes. If they turn compression off again (they shouldn't), the
            // remaining inflated bytes are still plain telnet, so parse them all.
            let mut plain = &buf[..produced];
            while !plain.is_empty() {
                let used = self.parse(plain);
                plain = &plain[used..];
            }

            match result {
                Ok(Status::StreamEnd) => {
                    self.inflate = None;
                    self.flush_data();
                    self.events.push(Event::CompressionEnded);
                    return input;
                }
                Ok(_) => {
                    if input.is_empty() && produced < buf.len() {
                        return input;
                    }
                    if consumed == 0 && produced == 0 {
                        return &input[input.len()..];
                    }
                }
                Err(e) => {
                    self.inflate = None;
                    self.remote[opt::MCCP2 as usize] = false;
                    self.flush_data();
                    self.events.push(Event::CompressionError(e.to_string()));
                    return &input[input.len()..];
                }
            }
        }
    }

    /// Parses uncompressed telnet bytes. Returns how many were used. That is fewer than
    /// `input.len()` only when compression started partway through.
    fn parse(&mut self, input: &[u8]) -> usize {
        for (i, &b) in input.iter().enumerate() {
            match self.state {
                State::Data => {
                    if b == cmd::IAC {
                        self.state = State::Iac;
                    } else {
                        self.data.push(b);
                    }
                }
                State::Iac => {
                    self.state = State::Data;
                    match b {
                        cmd::IAC => self.data.push(cmd::IAC),
                        cmd::WILL => self.state = State::Will,
                        cmd::WONT => self.state = State::Wont,
                        cmd::DO => self.state = State::Do,
                        cmd::DONT => self.state = State::Dont,
                        cmd::SB => self.state = State::Sb,
                        cmd::GA | cmd::EOR => {
                            self.flush_data();
                            self.events.push(Event::Prompt);
                        }
                        _ => {} // NOP, AYT, etc.
                    }
                }
                State::Will => {
                    self.state = State::Data;
                    self.on_will(b);
                }
                State::Wont => {
                    self.state = State::Data;
                    self.on_wont(b);
                }
                State::Do => {
                    self.state = State::Data;
                    self.on_do(b);
                }
                State::Dont => {
                    self.state = State::Data;
                    self.on_dont(b);
                }
                State::Sb => {
                    self.sb_option = b;
                    self.sb_data.clear();
                    self.state = State::SbData;
                }
                State::SbData => {
                    if b == cmd::IAC {
                        self.state = State::SbIac;
                    } else {
                        self.sb_data.push(b);
                    }
                }
                State::SbIac => match b {
                    cmd::IAC => {
                        self.sb_data.push(cmd::IAC);
                        self.state = State::SbData;
                    }
                    cmd::SE => {
                        self.state = State::Data;
                        if self.on_subnegotiation() {
                            return i + 1;
                        }
                    }
                    _ => {
                        // Malformed subnegotiation. Drop it and stay in sync.
                        self.state = State::Data;
                    }
                },
            }
        }
        input.len()
    }

    fn flush_data(&mut self) {
        if !self.data.is_empty() {
            self.events.push(Event::Data(std::mem::take(&mut self.data)));
        }
    }

    fn wants_remote(&self, option: u8) -> bool {
        match option {
            opt::BINARY | opt::ECHO | opt::SGA | opt::EOR | opt::MSSP | opt::CHARSET => true,
            opt::MCCP2 => self.config.enable_mccp2,
            opt::GMCP => self.config.enable_gmcp,
            _ => false,
        }
    }

    fn wants_local(&self, option: u8) -> bool {
        matches!(option, opt::BINARY | opt::TTYPE | opt::NAWS | opt::CHARSET)
    }

    fn send_cmd(&mut self, command: u8, option: u8) {
        self.out.extend_from_slice(&[cmd::IAC, command, option]);
    }

    fn on_will(&mut self, option: u8) {
        if self.remote[option as usize] {
            return;
        }
        if self.wants_remote(option) {
            self.remote[option as usize] = true;
            self.send_cmd(cmd::DO, option);
            self.remote_enabled(option);
        } else {
            self.send_cmd(cmd::DONT, option);
        }
    }

    fn on_wont(&mut self, option: u8) {
        if !self.remote[option as usize] {
            return;
        }
        self.remote[option as usize] = false;
        self.send_cmd(cmd::DONT, option);
        self.flush_data();
        if option == opt::ECHO {
            self.events.push(Event::ServerEcho(false));
        }
        self.events.push(Event::OptionChanged { option, local: false, enabled: false });
    }

    fn on_do(&mut self, option: u8) {
        if self.local[option as usize] {
            return;
        }
        if self.wants_local(option) {
            self.local[option as usize] = true;
            self.send_cmd(cmd::WILL, option);
            self.flush_data();
            self.events.push(Event::OptionChanged { option, local: true, enabled: true });
            if option == opt::NAWS {
                self.send_naws();
            }
        } else {
            self.send_cmd(cmd::WONT, option);
        }
    }

    fn on_dont(&mut self, option: u8) {
        if !self.local[option as usize] {
            return;
        }
        self.local[option as usize] = false;
        self.send_cmd(cmd::WONT, option);
        if option == opt::TTYPE {
            self.ttype_index = 0;
        }
        self.flush_data();
        self.events.push(Event::OptionChanged { option, local: true, enabled: false });
    }

    fn remote_enabled(&mut self, option: u8) {
        self.flush_data();
        self.events.push(Event::OptionChanged { option, local: false, enabled: true });
        match option {
            opt::ECHO => self.events.push(Event::ServerEcho(true)),
            opt::GMCP => {
                let hello = serde_json::json!({
                    "client": self.config.client_name,
                    "version": self.config.client_version,
                });
                self.send_gmcp("Core.Hello", Some(&hello.to_string()));
                if !self.config.gmcp_supports.is_empty() {
                    let supports = serde_json::to_string(&self.config.gmcp_supports).unwrap();
                    self.send_gmcp("Core.Supports.Set", Some(&supports));
                }
            }
            _ => {}
        }
    }

    /// Handles a finished subnegotiation. Returns true if compression begins now, which means
    /// every byte after this one is compressed.
    fn on_subnegotiation(&mut self) -> bool {
        let data = std::mem::take(&mut self.sb_data);
        match self.sb_option {
            opt::MCCP2 if self.remote[opt::MCCP2 as usize] => {
                self.flush_data();
                self.inflate = Some(Decompress::new(true));
                self.events.push(Event::CompressionStarted);
                return true;
            }
            opt::GMCP => {
                self.flush_data();
                let text = String::from_utf8_lossy(&data);
                let (package, payload) = match text.split_once(|c: char| c.is_ascii_whitespace()) {
                    Some((p, d)) => (p.to_string(), d.trim_start().to_string()),
                    None => (text.to_string(), String::new()),
                };
                self.events.push(Event::Gmcp { package, data: payload });
            }
            opt::TTYPE if data.first() == Some(&TTYPE_SEND) => self.send_ttype(),
            opt::CHARSET if data.first() == Some(&CHARSET_REQUEST) => self.on_charset_request(&data[1..]),
            opt::MSSP => {
                self.flush_data();
                self.events.push(Event::Mssp(parse_mssp(&data)));
            }
            option => {
                self.flush_data();
                self.events.push(Event::Subnegotiation { option, data });
            }
        }
        false
    }

    fn send_ttype(&mut self) {
        let names = [
            self.config.client_name.clone(),
            self.config.terminal_type.clone(),
            format!("MTTS {}", self.config.mtts),
        ];
        // Per MTTS, the last value repeats so the server can spot the end of the cycle.
        let name = &names[self.ttype_index.min(names.len() - 1)];
        self.ttype_index += 1;
        let mut payload = vec![TTYPE_IS];
        payload.extend_from_slice(name.as_bytes());
        self.send_sb(opt::TTYPE, &payload);
    }

    fn on_charset_request(&mut self, data: &[u8]) {
        // An optional "[TTABLE]" prefix plus version byte, then a separator byte, then
        // separator-delimited names.
        let mut data = data;
        if let Some(rest) = data.strip_prefix(b"[TTABLE]") {
            data = rest.get(1..).unwrap_or(&[]);
        }
        let Some((&sep, rest)) = data.split_first() else {
            self.send_sb(opt::CHARSET, &[CHARSET_REJECTED]);
            return;
        };
        let offered: Vec<String> = rest
            .split(|&b| b == sep)
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect();
        let choice = self
            .config
            .charsets
            .iter()
            .find_map(|want| offered.iter().find(|o| o.eq_ignore_ascii_case(want)))
            .cloned();
        match choice {
            Some(name) => {
                let mut payload = vec![CHARSET_ACCEPTED];
                payload.extend_from_slice(name.as_bytes());
                self.send_sb(opt::CHARSET, &payload);
                self.flush_data();
                self.events.push(Event::CharsetChanged(name));
            }
            None => self.send_sb(opt::CHARSET, &[CHARSET_REJECTED]),
        }
    }

    fn send_sb(&mut self, option: u8, payload: &[u8]) {
        self.out.extend_from_slice(&[cmd::IAC, cmd::SB, option]);
        escape_iac_into(payload, &mut self.out);
        self.out.extend_from_slice(&[cmd::IAC, cmd::SE]);
    }

    /// Records the window size and sends NAWS if the server asked for it.
    pub fn set_window_size(&mut self, width: u16, height: u16) {
        if self.window_size == Some((width, height)) {
            return;
        }
        self.window_size = Some((width, height));
        if self.local[opt::NAWS as usize] {
            self.send_naws();
        }
    }

    fn send_naws(&mut self) {
        if let Some((w, h)) = self.window_size {
            let [w1, w2] = w.to_be_bytes();
            let [h1, h2] = h.to_be_bytes();
            self.send_sb(opt::NAWS, &[w1, w2, h1, h2]);
        }
    }

    /// Queues a GMCP message. Returns false if GMCP isn't enabled.
    pub fn send_gmcp(&mut self, package: &str, data: Option<&str>) -> bool {
        if !self.remote[opt::GMCP as usize] {
            return false;
        }
        let mut payload = package.as_bytes().to_vec();
        if let Some(data) = data {
            payload.push(b' ');
            payload.extend_from_slice(data.as_bytes());
        }
        self.send_sb(opt::GMCP, &payload);
        true
    }

    /// Queues a line of already-encoded text. Escapes IAC and appends CRLF.
    pub fn send_line(&mut self, text: &[u8]) {
        escape_iac_into(text, &mut self.out);
        self.out.extend_from_slice(b"\r\n");
    }
}

pub fn escape_iac_into(data: &[u8], out: &mut Vec<u8>) {
    for &b in data {
        out.push(b);
        if b == cmd::IAC {
            out.push(cmd::IAC);
        }
    }
}

fn parse_mssp(data: &[u8]) -> Vec<(String, Vec<String>)> {
    let mut vars: Vec<(String, Vec<String>)> = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let marker = data[i];
        let start = i + 1;
        let mut end = start;
        while end < data.len() && data[end] != MSSP_VAR && data[end] != MSSP_VAL {
            end += 1;
        }
        let text = String::from_utf8_lossy(&data[start..end]).into_owned();
        match marker {
            MSSP_VAR => vars.push((text, Vec::new())),
            MSSP_VAL => {
                if let Some((_, values)) = vars.last_mut() {
                    values.push(text);
                }
            }
            _ => {}
        }
        i = end;
    }
    vars
}

#[cfg(test)]
mod tests {
    use super::cmd::*;
    use super::*;
    use flate2::{Compress, Compression, FlushCompress};

    fn telnet() -> Telnet {
        Telnet::new(TelnetConfig::default())
    }

    fn data_of(events: &[Event]) -> Vec<u8> {
        events
            .iter()
            .filter_map(|e| match e {
                Event::Data(d) => Some(d.clone()),
                _ => None,
            })
            .flatten()
            .collect()
    }

    #[test]
    fn plain_data_and_escaped_iac() {
        let mut t = telnet();
        let ev = t.feed(b"hello \xFF\xFF world");
        assert_eq!(data_of(&ev), b"hello \xFF world");
    }

    #[test]
    fn commands_split_across_reads() {
        let mut t = telnet();
        let mut ev = t.feed(&[b'a', IAC]);
        ev.extend(t.feed(&[WILL]));
        ev.extend(t.feed(&[opt::GMCP, b'b']));
        assert_eq!(data_of(&ev), b"ab");
        assert!(t.is_remote_enabled(opt::GMCP));
    }

    #[test]
    fn refuses_unknown_options_and_does_not_loop() {
        let mut t = telnet();
        t.feed(&[IAC, WILL, 99, IAC, DO, 99]);
        assert_eq!(t.take_output(), vec![IAC, DONT, 99, IAC, WONT, 99]);
        // A repeated request for an already-enabled option gets no reply.
        t.feed(&[IAC, WILL, opt::EOR]);
        assert_eq!(t.take_output(), vec![IAC, DO, opt::EOR]);
        t.feed(&[IAC, WILL, opt::EOR]);
        assert!(t.take_output().is_empty());
    }

    #[test]
    fn ga_and_eor_mark_prompts() {
        let mut t = telnet();
        let ev = t.feed(&[b'>', b' ', IAC, GA, b'x', IAC, EOR]);
        assert_eq!(
            ev,
            vec![Event::Data(b"> ".to_vec()), Event::Prompt, Event::Data(b"x".to_vec()), Event::Prompt]
        );
    }

    #[test]
    fn gmcp_enable_sends_hello_and_parses_messages() {
        let mut t = Telnet::new(TelnetConfig {
            client_name: "Test".into(),
            client_version: "1".into(),
            gmcp_supports: vec!["Char 1".into()],
            ..Default::default()
        });
        t.feed(&[IAC, WILL, opt::GMCP]);
        let out = t.take_output();
        let text = String::from_utf8_lossy(&out);
        assert!(out.starts_with(&[IAC, DO, opt::GMCP]));
        assert!(text.contains(r#"Core.Hello {"client":"Test","version":"1"}"#));
        assert!(text.contains(r#"Core.Supports.Set ["Char 1"]"#));

        // An escaped IAC inside the payload must survive (BeipMU drops such messages).
        let mut msg = vec![IAC, SB, opt::GMCP];
        msg.extend_from_slice(b"Char.Vitals {\"hp\":10,\"raw\":\"");
        msg.extend_from_slice(&[IAC, IAC]);
        msg.extend_from_slice(b"\"}");
        msg.extend_from_slice(&[IAC, SE]);
        let ev = t.feed(&msg);
        assert_eq!(
            ev,
            vec![Event::Gmcp { package: "Char.Vitals".into(), data: "{\"hp\":10,\"raw\":\"\u{FFFD}\"}".into() }]
        );
    }

    #[test]
    fn gmcp_without_payload() {
        let mut t = telnet();
        t.feed(&[IAC, WILL, opt::GMCP]);
        let mut msg = vec![IAC, SB, opt::GMCP];
        msg.extend_from_slice(b"Core.Ping");
        msg.extend_from_slice(&[IAC, SE]);
        assert_eq!(t.feed(&msg), vec![Event::Gmcp { package: "Core.Ping".into(), data: String::new() }]);
    }

    #[test]
    fn ttype_cycles_then_repeats_last() {
        let mut t = telnet();
        t.feed(&[IAC, DO, opt::TTYPE]);
        t.take_output();
        let send = [IAC, SB, opt::TTYPE, TTYPE_SEND, IAC, SE];
        let mut replies = Vec::new();
        for _ in 0..4 {
            t.feed(&send);
            let out = t.take_output();
            replies.push(String::from_utf8_lossy(&out[4..out.len() - 2]).into_owned());
        }
        let mtts = format!("MTTS {}", TelnetConfig::default().mtts);
        assert_eq!(replies, vec!["MU".to_string(), "XTERM-256COLOR".into(), mtts.clone(), mtts]);
    }

    #[test]
    fn naws_sent_on_enable_and_resize() {
        let mut t = telnet();
        t.set_window_size(80, 24);
        assert!(t.take_output().is_empty());
        t.feed(&[IAC, DO, opt::NAWS]);
        assert_eq!(t.take_output(), vec![IAC, WILL, opt::NAWS, IAC, SB, opt::NAWS, 0, 80, 0, 24, IAC, SE]);
        // A width of 255 has to be escaped.
        t.set_window_size(255, 24);
        assert_eq!(t.take_output(), vec![IAC, SB, opt::NAWS, 0, 255, 255, 0, 24, IAC, SE]);
    }

    #[test]
    fn charset_request_accepts_utf8() {
        let mut t = telnet();
        t.feed(&[IAC, WILL, opt::CHARSET]);
        t.take_output();
        let mut req = vec![IAC, SB, opt::CHARSET, CHARSET_REQUEST];
        req.extend_from_slice(b" ISO-8859-1 utf-8");
        req.extend_from_slice(&[IAC, SE]);
        let ev = t.feed(&req);
        assert_eq!(ev, vec![Event::CharsetChanged("utf-8".into())]);
        let mut expected = vec![IAC, SB, opt::CHARSET, CHARSET_ACCEPTED];
        expected.extend_from_slice(b"utf-8");
        expected.extend_from_slice(&[IAC, SE]);
        assert_eq!(t.take_output(), expected);
    }

    #[test]
    fn charset_request_rejects_unknown() {
        let mut t = telnet();
        let mut req = vec![IAC, SB, opt::CHARSET, CHARSET_REQUEST];
        req.extend_from_slice(b";KOI8-R");
        req.extend_from_slice(&[IAC, SE]);
        t.feed(&req);
        assert_eq!(t.take_output(), vec![IAC, SB, opt::CHARSET, CHARSET_REJECTED, IAC, SE]);
    }

    #[test]
    fn server_echo_events() {
        let mut t = telnet();
        let ev = t.feed(&[IAC, WILL, opt::ECHO]);
        assert!(ev.contains(&Event::ServerEcho(true)));
        let ev = t.feed(&[IAC, WONT, opt::ECHO]);
        assert!(ev.contains(&Event::ServerEcho(false)));
    }

    #[test]
    fn mssp_parsing() {
        let mut t = telnet();
        let mut msg = vec![IAC, SB, opt::MSSP];
        msg.extend_from_slice(b"\x01NAME\x02Test MUD\x01PORT\x024000\x024001");
        msg.extend_from_slice(&[IAC, SE]);
        let ev = t.feed(&msg);
        assert_eq!(
            ev,
            vec![Event::Mssp(vec![
                ("NAME".into(), vec!["Test MUD".into()]),
                ("PORT".into(), vec!["4000".into(), "4001".into()]),
            ])]
        );
    }

    fn compress(data: &[u8], finish: bool) -> Vec<u8> {
        let mut c = Compress::new(Compression::default(), true);
        let mut out = Vec::with_capacity(data.len() + 64);
        let flush = if finish { FlushCompress::Finish } else { FlushCompress::Sync };
        c.compress_vec(data, &mut out, flush).unwrap();
        out
    }

    #[test]
    fn mccp2_starts_mid_buffer() {
        let mut t = telnet();
        t.feed(&[IAC, WILL, opt::MCCP2]);
        assert_eq!(t.take_output(), vec![IAC, DO, opt::MCCP2]);

        let mut stream = b"before ".to_vec();
        stream.extend_from_slice(&[IAC, SB, opt::MCCP2, IAC, SE]);
        let mut inner = b"compressed ".to_vec();
        inner.extend_from_slice(&[IAC, GA]);
        stream.extend_from_slice(&compress(&inner, true));
        stream.extend_from_slice(b"after");

        let ev = t.feed(&stream);
        assert_eq!(
            ev,
            vec![
                Event::Data(b"before ".to_vec()),
                Event::CompressionStarted,
                Event::Data(b"compressed ".to_vec()),
                Event::Prompt,
                Event::CompressionEnded,
                Event::Data(b"after".to_vec()),
            ]
        );
        assert!(!t.is_compressed());
    }

    #[test]
    fn mccp2_byte_at_a_time() {
        let mut t = telnet();
        t.feed(&[IAC, WILL, opt::MCCP2]);
        let mut stream = vec![IAC, SB, opt::MCCP2, IAC, SE];
        let text: Vec<u8> = (0..2000).map(|i| b'a' + (i % 26) as u8).collect();
        stream.extend_from_slice(&compress(&text, false));
        let mut got = Vec::new();
        for b in stream {
            got.extend(data_of(&t.feed(&[b])));
        }
        assert_eq!(got, text);
        assert!(t.is_compressed());
    }

    #[test]
    fn mccp2_corrupt_stream() {
        let mut t = telnet();
        t.feed(&[IAC, WILL, opt::MCCP2]);
        let mut stream = vec![IAC, SB, opt::MCCP2, IAC, SE];
        stream.extend_from_slice(b"definitely not zlib");
        let ev = t.feed(&stream);
        assert!(matches!(ev.last(), Some(Event::CompressionError(_))));
        assert!(!t.is_compressed());
    }

    #[test]
    fn send_line_escapes_iac() {
        let mut t = telnet();
        t.send_line(b"a\xFFb");
        assert_eq!(t.take_output(), b"a\xFF\xFFb\r\n");
    }
}
