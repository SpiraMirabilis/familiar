//! Windows backend: the SAPI voice that ships with Windows, so nothing needs installing.

use windows::Win32::Media::Speech::{ISpVoice, SPF_ASYNC, SPF_IS_NOT_XML, SPF_PURGEBEFORESPEAK, SpVoice};
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize};
use windows::core::PCWSTR;

pub const HINT: &str = "";

pub struct Backend {
    voice: ISpVoice,
    com_initialized: bool,
}

impl Backend {
    /// Called on the speech thread, which then owns the voice.
    pub fn connect(_app: &str) -> Result<Self, String> {
        // SAFETY: plain COM calls. The apartment is initialised once per connection and paired
        // with CoUninitialize in Drop when CoInitializeEx succeeds.
        unsafe {
            let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
            let com_initialized = hr.is_ok();
            let voice: ISpVoice = match CoCreateInstance(&SpVoice, None, CLSCTX_ALL) {
                Ok(v) => v,
                Err(e) => {
                    if com_initialized {
                        CoUninitialize();
                    }
                    return Err(e.message());
                }
            };
            Ok(Self { voice, com_initialized })
        }
    }

    fn speak(&self, text: &str, flags: u32) -> Result<(), String> {
        let wide: Vec<u16> = text.chars().filter(|&c| c != '\0').flat_map(|c| {
            let mut buf = [0u16; 2];
            c.encode_utf16(&mut buf).to_vec()
        }).chain(std::iter::once(0)).collect();
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

impl Drop for Backend {
    fn drop(&mut self) {
        if self.com_initialized {
            // SAFETY: paired with successful CoInitializeEx on the same speech thread.
            unsafe { CoUninitialize() };
        }
    }
}
