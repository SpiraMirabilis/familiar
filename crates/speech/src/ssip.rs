//! speech-dispatcher (SSIP) backend, for Linux and other Unix systems.

use std::collections::VecDeque;
use std::thread;
use std::time::Duration;

use ssip_client::{ClientName, MessageId, MessageScope, Priority, fifo};

/// Messages we've spoken recently, so "stop" can cancel just ours (cancelling everything would
/// also silence a screen reader).
const REMEMBERED: usize = 64;

pub const HINT: &str = " Is speech-dispatcher installed?";

type Client = ssip_client::Client<fifo::UnixStream>;

pub struct Backend {
    client: Client,
    spoken: VecDeque<MessageId>,
}

pub(crate) fn connect_client(app: &str) -> Result<Client, String> {
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
pub(crate) fn ssip_token(text: &str) -> String {
    let token: String = text.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
    if token.is_empty() { "mu".into() } else { token }
}

impl Backend {
    pub fn connect(app: &str) -> Result<Self, String> {
        Ok(Self { client: connect_client(app)?, spoken: VecDeque::new() })
    }

    pub fn say(&mut self, text: &str) -> Result<(), String> {
        let id = self
            .client
            .speak()
            .and_then(|c| c.check_receiving_data())
            .and_then(|c| c.send_line(text))
            .and_then(|c| c.receive_message_id())
            .map_err(|e| e.to_string())?;
        self.spoken.push_back(id);
        if self.spoken.len() > REMEMBERED {
            self.spoken.pop_front();
        }
        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), String> {
        for id in self.spoken.drain(..) {
            // Already-finished messages just fail to cancel; that's fine.
            self.client.cancel(MessageScope::Message(id)).and_then(|c| c.receive()).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn client_names_have_no_spaces() {
        assert_eq!(super::ssip_token("Familiar Client"), "Familiar-Client");
        assert_eq!(super::ssip_token("a:b"), "a-b");
        assert_eq!(super::ssip_token(""), "mu");
    }

    /// Needs a speech-dispatcher; run with `cargo test -p familiar-speech -- --ignored`. It
    /// connects and sets up a client without saying anything.
    #[test]
    #[ignore]
    fn connects_to_speech_dispatcher() {
        // A name with a space, like the app's, must still be accepted.
        let mut client = super::connect_client("Familiar test").expect("connects");
        client.quit().expect("quits");
    }
}
