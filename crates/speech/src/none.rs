//! Platforms with no speech backend.

pub const HINT: &str = "";

pub struct Backend;

impl Backend {
    pub fn connect(_app: &str) -> Result<Self, String> {
        Err("not supported on this system".into())
    }

    pub fn say(&mut self, _text: &str) -> Result<(), String> {
        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), String> {
        Ok(())
    }
}
