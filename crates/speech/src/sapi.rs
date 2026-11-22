//! Windows backend: the SAPI voice that ships with Windows, so nothing needs installing.

use windows::Win32::Media::Speech::{ISpVoice, SPF_ASYNC, SPF_IS_NOT_XML, SPF_PURGEBEFORESPEAK, SpVoice};
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};
use windows::core::PCWSTR;

pub const HINT: &str = "";

pub struct Backend {
    voice: ISpVoice,
}

impl Backend {
    /// Called on the speech thread, which then owns the voice.
    pub fn connect(_app: &str) -> Result<Self, String> {
        // SAFETY: plain COM calls. The apartment is initialised once per thread (a repeat call
        // just returns S_FALSE), and the voice never leaves this thread.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let voice: ISpVoice = CoCreateInstance(&SpVoice, None, CLSCTX_ALL).map_err(|e| e.message())?;
            Ok(Self { voice })
        }
    }

    fn speak(&self, text: &str, flags: u32) -> Result<(), String> {
        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: `wide` is NUL-terminated and SAPI copies it before an async Speak returns.
        unsafe { self.voice.Speak(PCWSTR(wide.as_ptr()), flags, None) }.map_err(|e| e.message())
    }

    /// Queues behind whatever this voice is already saying.
    pub fn say(&mut self, text: &str) -> Result<(), String> {
        self.speak(text, (SPF_ASYNC.0 | SPF_IS_NOT_XML.0) as u32)
    }

    /// Purges only this voice's queue, so a screen reader keeps talking.
    pub fn stop(&mut self) -> Result<(), String> {
        self.speak("", (SPF_ASYNC.0 | SPF_PURGEBEFORESPEAK.0) as u32)
    }
}
