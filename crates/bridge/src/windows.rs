//! What the Windows app leaves to Rust code the other clients already use there: playing
//! sounds, speaking, and showing notifications. All on the main thread.

use std::path::Path;
use std::sync::OnceLock;

use familiar_config::APP_NAME;
use serde_json::{Value, json};

use crate::{Reply, number, text};

fn player() -> &'static familiar_platform::SoundPlayer {
    static PLAYER: OnceLock<familiar_platform::SoundPlayer> = OnceLock::new();
    PLAYER.get_or_init(familiar_platform::SoundPlayer::new)
}

thread_local! {
    /// One speaker for the whole app, so sessions queue their speech rather than talk over
    /// each other.
    static SPEAKER: familiar_speech::Speaker = familiar_speech::Speaker::new(APP_NAME);
}

pub fn call(method: &str, args: &Value) -> Option<Reply> {
    Some(match method {
        // Plays a file under a name of the app's choosing. "loops" below zero repeats until
        // stopped.
        "sound.play" => (|| {
            let loops = args.get("loops").and_then(Value::as_i64).unwrap_or(1) as i32;
            let volume = number(args, "volume").unwrap_or(1.0);
            player().play_loops(text(args, "name")?, Path::new(text(args, "path")?), volume, loops)?;
            Ok(Value::Null)
        })(),
        // Stops the sound with this name, or every sound without one.
        "sound.stop" => {
            player().stop(args.get("name").and_then(Value::as_str));
            Ok(Value::Null)
        }
        "sound.playing" => text(args, "name").map(|name| player().is_playing(name).into()),
        // "problem" is what went wrong with speech since the last call, if anything did.
        "speech.say" => text(args, "text").map(|t| {
            SPEAKER.with(|speaker| {
                speaker.say(t);
                json!({ "problem": speaker.take_error() })
            })
        }),
        "speech.stop" => {
            SPEAKER.with(|speaker| speaker.stop());
            Ok(Value::Null)
        }
        "notify" => (|| {
            familiar_platform::notify(APP_NAME, text(args, "title")?, text(args, "body")?);
            Ok(Value::Null)
        })(),
        _ => return None,
    })
}
