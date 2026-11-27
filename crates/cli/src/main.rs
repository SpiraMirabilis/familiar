//! Headless terminal client, used to check the core crates against real servers.

use std::io::Write;

use anyhow::Result;
use clap::Parser;
use familiar_ansi::{AnsiParser, Color, Style, StyledLine};
use familiar_net::{BouncerOptions, BouncerRequest, ConnectOptions, Encoding, SessionEvent, SessionOptions, spawn_session};
use tokio::io::{AsyncBufReadExt, BufReader};

#[derive(Parser)]
#[command(about = "Minimal MU* terminal client")]
struct Args {
    host: String,
    port: u16,
    #[arg(long)]
    tls: bool,
    /// Accept any TLS certificate.
    #[arg(long)]
    insecure: bool,
    #[arg(long)]
    ipv4: bool,
    /// Server encoding: auto (UTF-8 with a Windows-1252 fallback), utf-8, windows-1252 or cp437.
    #[arg(long, default_value = "auto")]
    encoding: String,
    /// Print GMCP and MSSP messages to stderr.
    #[arg(long)]
    gmcp: bool,
    /// Connect through an mbnc bouncer at HOST:PORT (TLS unless --bnc-plaintext). The password
    /// comes from $FAMILIAR_BNC_PASSWORD. Type /bnc list, /bnc reconnect or /bnc close.
    #[arg(long, value_name = "HOST:PORT")]
    bnc: Option<String>,
    /// The character name the bouncer files the session under.
    #[arg(long, default_value = "familiar-cli")]
    bnc_character: String,
    /// A login line for the bouncer to send whenever it connects (repeatable).
    #[arg(long)]
    bnc_login: Vec<String>,
    #[arg(long)]
    bnc_plaintext: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let encoding = Encoding::from_name(&args.encoding)
        .ok_or_else(|| anyhow::anyhow!("unknown encoding {}", args.encoding))?;

    let mut connect = ConnectOptions::new(&args.host, args.port);
    connect.tls = args.tls;
    connect.verify_certificate = !args.insecure;
    connect.force_ipv4 = args.ipv4;
    let mut opts = SessionOptions::new(connect);
    opts.encoding = encoding;
    opts.telnet.client_name = "familiar-cli".into();
    if let Some(bnc) = &args.bnc {
        let parsed = familiar_config::Address::parse(bnc).ok_or_else(|| anyhow::anyhow!("bad bouncer address {bnc}"))?;
        let mut connect = ConnectOptions::new(&parsed.host, parsed.port.unwrap_or(7777));
        connect.tls = !args.bnc_plaintext;
        let (login, _) = familiar_net::login_script(&args.bnc_login.join("\n"), |_| None);
        opts.bouncer = Some(BouncerOptions {
            connect,
            password: std::env::var("FAMILIAR_BNC_PASSWORD").unwrap_or_default(),
            character: args.bnc_character.clone(),
            login: (!login.is_empty()).then_some(login),
        });
    }

    eprintln!("Connecting to {}:{}...", args.host, args.port);
    let (session, mut events) = spawn_session(opts);
    if let Some((terminal_size::Width(w), terminal_size::Height(h))) = terminal_size::terminal_size() {
        session.set_window_size(w, h);
    }

    let mut stdin = BufReader::new(tokio::io::stdin()).lines();
    let mut stdin_open = true;
    let mut ansi = AnsiParser::new();
    let mut out = Output::default();

    loop {
        tokio::select! {
            event = events.recv() => {
                let Some(event) = event else { break };
                match event {
                    SessionEvent::Connected { addr } => eprintln!("Connected to {addr}"),
                    SessionEvent::Line(text) => out.line(&ansi.parse_line(&text)),
                    SessionEvent::Prompt(text) => out.prompt(&ansi.parse_line(&text), true),
                    SessionEvent::PartialLine(text) => {
                        // Parse a copy so the real line, when it arrives, starts with the same state.
                        out.prompt(&ansi.clone().parse_line(&text), false)
                    }
                    SessionEvent::Gmcp { package, data } if args.gmcp => {
                        out.notice(&format!("GMCP {package} {data}"))
                    }
                    SessionEvent::Mssp(vars) if args.gmcp => out.notice(&format!("MSSP {vars:?}")),
                    SessionEvent::Gmcp { .. } | SessionEvent::Mssp(_) => {}
                    SessionEvent::ServerEcho(on) => {
                        out.notice(if on { "Server echo on (input is not hidden here)" } else { "Server echo off" })
                    }
                    SessionEvent::EncodingChanged(e) => out.notice(&format!("Encoding is now {}", e.name())),
                    SessionEvent::Compression(on) => {
                        out.notice(if on { "MCCP compression on" } else { "MCCP compression off" })
                    }
                    SessionEvent::Notice(text) => out.notice(&text),
                    SessionEvent::Disconnected { error } => {
                        match error {
                            Some(e) => out.notice(&format!("Disconnected: {e}")),
                            None => out.notice("Disconnected"),
                        }
                        break;
                    }
                }
            }
            line = stdin.next_line(), if stdin_open => match line? {
                Some(line) if line == "/quit" => { session.disconnect(); }
                Some(line) if line.starts_with("/bnc") => {
                    let request = match line.split_whitespace().nth(1) {
                        Some("reconnect") => BouncerRequest::Reconnect,
                        Some("close") => BouncerRequest::Close,
                        _ => BouncerRequest::List,
                    };
                    session.bouncer(request);
                }
                Some(line) => {
                    out.input_sent();
                    session.send_line(line);
                }
                None => {
                    stdin_open = false;
                    session.disconnect();
                }
            },
        }
    }
    Ok(())
}

/// Tracks the cursor so prompts and partial lines can be overwritten cleanly.
#[derive(Default)]
struct Output {
    /// The cursor is after a GA prompt, so the next output needs a newline first.
    after_prompt: bool,
    /// A partial line is on screen and should be replaced by whatever comes next.
    partial_shown: bool,
}

impl Output {
    fn prepare(&mut self, stdout: &mut impl Write) {
        if self.partial_shown {
            let _ = write!(stdout, "\r\x1b[K");
        } else if self.after_prompt {
            let _ = writeln!(stdout);
        }
        self.partial_shown = false;
        self.after_prompt = false;
    }

    fn line(&mut self, line: &StyledLine) {
        let mut stdout = std::io::stdout().lock();
        self.prepare(&mut stdout);
        let _ = writeln!(stdout, "{}", render(line));
        if line.bell {
            let _ = write!(stdout, "\x07");
        }
    }

    fn prompt(&mut self, line: &StyledLine, terminated: bool) {
        let mut stdout = std::io::stdout().lock();
        self.prepare(&mut stdout);
        let _ = write!(stdout, "{}", render(line));
        let _ = stdout.flush();
        if terminated {
            self.after_prompt = true;
        } else {
            self.partial_shown = true;
        }
    }

    fn notice(&mut self, text: &str) {
        let mut stdout = std::io::stdout().lock();
        self.prepare(&mut stdout);
        let safe: String = text.chars().filter(|c| !c.is_control() || *c == '\t').collect();
        let _ = writeln!(stdout, "\x1b[2;3m% {safe}\x1b[0m");
    }

    /// The user pressed Enter, so the terminal cursor is on a fresh line.
    fn input_sent(&mut self) {
        self.after_prompt = false;
        self.partial_shown = false;
    }
}

/// Turns a styled line back into terminal escape codes.
fn render(line: &StyledLine) -> String {
    let mut s = String::with_capacity(line.text.len() + 16);
    for span in &line.spans {
        let style = &span.style;
        let safe_link = style.link.as_deref().filter(|l| !l.chars().any(char::is_control));
        if let Some(link) = safe_link {
            s.push_str(&format!("\x1b]8;;{link}\x1b\\"));
        }
        let codes = sgr(style);
        s.push_str(&codes);
        s.push_str(line.span_text(span));
        if !codes.is_empty() {
            s.push_str("\x1b[0m");
        }
        if safe_link.is_some() {
            s.push_str("\x1b]8;;\x1b\\");
        }
    }
    s
}

fn sgr(style: &Style) -> String {
    let mut codes: Vec<String> = Vec::new();
    for (on, code) in [
        (style.bold, "1"),
        (style.faint, "2"),
        (style.italic, "3"),
        (style.underline, "4"),
        (style.blink, "5"),
        (style.reverse, "7"),
        (style.conceal, "8"),
        (style.strike, "9"),
    ] {
        if on {
            codes.push(code.into());
        }
    }
    let color = |c: Color, base: u8| -> Option<String> {
        match c {
            Color::Default => None,
            Color::Indexed(n) if n < 8 => Some(format!("{}", base + n)),
            Color::Indexed(n) if n < 16 => Some(format!("{}", base + 60 + n - 8)),
            Color::Indexed(n) => Some(format!("{};5;{n}", base + 8)),
            Color::Rgb(r, g, b) => Some(format!("{};2;{r};{g};{b}", base + 8)),
        }
    };
    codes.extend(color(style.fg, 30));
    codes.extend(color(style.bg, 40));
    if codes.is_empty() { String::new() } else { format!("\x1b[{}m", codes.join(";")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_round_trips() {
        let input = "\x1b[1;31mred\x1b[0m \x1b[38;2;1;2;3;104mrgb";
        let line = AnsiParser::new().parse_line(input);
        let again = AnsiParser::new().parse_line(&render(&line));
        assert_eq!(line, again);
    }
}
