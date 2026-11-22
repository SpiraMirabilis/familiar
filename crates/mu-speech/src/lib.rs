//! Text to speech through speech-dispatcher (SSIP). Speech runs on a background thread, so a
//! slow or missing speech daemon never blocks the UI. Shared by the GTK and terminal clients.

use std::collections::VecDeque;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use ssip_client::{ClientName, MessageId, MessageScope, Priority, fifo};

enum Command {
    Say(String),
    Stop,
}

/// Messages we've spoken recently, so "stop" can cancel just ours (cancelling everything would
/// also silence a screen reader).
const REMEMBERED: usize = 64;

pub struct Speaker {
    commands: mpsc::Sender<Command>,
    errors: mpsc::Receiver<String>,
}

type Client = ssip_client::Client<fifo::UnixStream>;

fn connect(app: &str) -> Result<Client, String> {
    let open = || fifo::Builder::new().timeout(Duration::from_secs(2)).build();
    let mut client = match open() {
        Ok(client) => client,
        Err(_) => {
            // Nothing listening: start the daemon, as libspeechd does (systems with the
            // systemd socket unit never get here).
            let _ = std::process::Command::new("speech-dispatcher").arg("--spawn").status();
            thread::sleep(Duration::from_millis(500));
            open().map_err(|e| e.to_string())?
        }
    };
    let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
    client
        .set_client_name(ClientName::new(&ssip_token(&user), &ssip_token(app)))
        .and_then(|c| c.check_client_name_set())
        .map_err(|e| e.to_string())?;
    // Text priority: messages queue up in order rather than cutting each other off.
    client.set_priority(Priority::Text).and_then(|c| c.check_status(ssip_client::OK_PRIORITY_SET)).map_err(|e| e.to_string())?;
    Ok(client)
}

/// SSIP client name parts can't contain spaces (or colons, which separate them).
fn ssip_token(text: &str) -> String {
    let token: String = text.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
    if token.is_empty() { "mu".into() } else { token }
}

fn run(app: String, commands: mpsc::Receiver<Command>, errors: mpsc::Sender<String>) {
    let mut client: Option<Client> = None;
    let mut spoken: VecDeque<MessageId> = VecDeque::new();
    // Report a failure once, not for every line.
    let mut reported = false;
    for command in commands {
        if client.is_none() {
            match connect(&app) {
                Ok(c) => {
                    client = Some(c);
                    reported = false;
                }
                Err(e) => {
                    if !reported {
                        let _ = errors.send(format!("Speech isn't available ({e}). Is speech-dispatcher installed?"));
                        reported = true;
                    }
                    continue;
                }
            }
        }
        let c = client.as_mut().unwrap();
        let result = match command {
            Command::Say(text) => c
                .speak()
                .and_then(|c| c.check_receiving_data())
                .and_then(|c| c.send_line(&text))
                .and_then(|c| c.receive_message_id())
                .map(|id| {
                    spoken.push_back(id);
                    if spoken.len() > REMEMBERED {
                        spoken.pop_front();
                    }
                }),
            Command::Stop => {
                let mut result = Ok(());
                for id in spoken.drain(..) {
                    // Already-finished messages just fail to cancel; that's fine.
                    if let Err(e) = c.cancel(MessageScope::Message(id)).and_then(|c| c.receive()) {
                        result = Err(e);
                        break;
                    }
                }
                result.map(|_| ())
            }
        };
        if let Err(e) = result {
            // Reconnect next time; the daemon may have restarted.
            client = None;
            if !reported {
                let _ = errors.send(format!("Speech failed: {e}"));
                reported = true;
            }
        }
    }
}

impl Speaker {
    /// Starts the speech thread. Nothing connects until the first [`Speaker::say`].
    pub fn new(app: &str) -> Self {
        let (commands, commands_rx) = mpsc::channel();
        let (errors_tx, errors) = mpsc::channel();
        let app = app.to_string();
        thread::Builder::new()
            .name("mu-speech".into())
            .spawn(move || run(app, commands_rx, errors_tx))
            .expect("failed to start the speech thread");
        Self { commands, errors }
    }

    pub fn say(&self, text: &str) {
        let text = text.trim();
        if !text.is_empty() {
            let _ = self.commands.send(Command::Say(text.to_string()));
        }
    }

    /// Stops speaking and drops anything queued.
    pub fn stop(&self) {
        let _ = self.commands.send(Command::Stop);
    }

    /// A problem to show the user, if one happened since the last call.
    pub fn take_error(&self) -> Option<String> {
        self.errors.try_recv().ok()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn client_names_have_no_spaces() {
        assert_eq!(super::ssip_token("MU Client"), "MU-Client");
        assert_eq!(super::ssip_token("a:b"), "a-b");
        assert_eq!(super::ssip_token(""), "mu");
    }

    /// Needs a speech-dispatcher; run with `cargo test -p mu-speech -- --ignored`. It connects
    /// and sets up a client without saying anything.
    #[test]
    #[ignore]
    fn connects_to_speech_dispatcher() {
        // A name with a space, like the app's, must still be accepted.
        let mut client = super::connect("MU Client test").expect("connects");
        client.quit().expect("quits");
    }
}
