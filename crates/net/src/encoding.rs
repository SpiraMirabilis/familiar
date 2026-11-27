//! Server text encodings. MU*s mostly use UTF-8, Windows-1252 (often labelled "latin1") or CP437.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Encoding {
    /// UTF-8, falling back to Windows-1252 for any line that isn't valid UTF-8. Real
    /// Windows-1252 text almost never happens to be valid multi-byte UTF-8, so this shows both
    /// modern and legacy servers correctly. Sends UTF-8.
    #[default]
    Auto,
    Utf8,
    Windows1252,
    Cp437,
}

impl Encoding {
    /// Looks up an encoding by charset name (as used in telnet CHARSET or config files).
    pub fn from_name(name: &str) -> Option<Self> {
        let n = name.trim().to_ascii_lowercase().replace('_', "-");
        match n.as_str() {
            "auto" => Some(Self::Auto),
            "utf-8" | "utf8" => Some(Self::Utf8),
            "windows-1252" | "cp1252" | "iso-8859-1" | "latin1" | "latin-1" | "us-ascii" | "ascii" => {
                Some(Self::Windows1252)
            }
            "cp437" | "ibm437" | "437" => Some(Self::Cp437),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Utf8 => "UTF-8",
            Self::Windows1252 => "Windows-1252",
            Self::Cp437 => "CP437",
        }
    }

    pub fn decode(self, bytes: &[u8]) -> String {
        match self {
            Self::Auto => match std::str::from_utf8(bytes) {
                Ok(text) => text.to_string(),
                Err(_) => Self::Windows1252.decode(bytes),
            },
            Self::Utf8 => String::from_utf8_lossy(bytes).into_owned(),
            Self::Windows1252 => encoding_rs::WINDOWS_1252.decode_without_bom_handling(bytes).0.into_owned(),
            Self::Cp437 => bytes
                .iter()
                .map(|&b| if b < 0x80 { b as char } else { CP437_HIGH[(b - 0x80) as usize] })
                .collect(),
        }
    }

    /// Decodes the start of a line that's still arriving. A UTF-8 character cut off at the end
    /// is left out rather than shown as a replacement character; it'll be there when the rest of
    /// the line arrives. (With Auto, a trailing Windows-1252 letter that looks like the start of
    /// a UTF-8 character waits too.)
    pub fn decode_partial(self, bytes: &[u8]) -> String {
        match self {
            Self::Auto => self.decode(trim_incomplete_utf8(bytes)),
            Self::Utf8 => self.decode(trim_trailing_incomplete_utf8(bytes)),
            _ => self.decode(bytes),
        }
    }

    /// Encodes outgoing text. Characters the encoding can't represent become '?'.
    pub fn encode(self, text: &str) -> Vec<u8> {
        match self {
            Self::Auto | Self::Utf8 => text.as_bytes().to_vec(),
            Self::Windows1252 => {
                let mut encoder = encoding_rs::WINDOWS_1252.new_encoder();
                let mut out = Vec::with_capacity(text.len());
                let mut buf = [0u8; 256];
                let mut src = text;
                while !src.is_empty() {
                    let (result, read, written) = encoder.encode_from_utf8_without_replacement(src, &mut buf, true);
                    out.extend_from_slice(&buf[..written]);
                    src = &src[read..];
                    if matches!(result, encoding_rs::EncoderResult::Unmappable(_)) {
                        out.push(b'?');
                    }
                }
                out
            }
            Self::Cp437 => text
                .chars()
                .map(|c| {
                    if (c as u32) < 0x80 {
                        c as u8
                    } else {
                        CP437_HIGH.iter().position(|&h| h == c).map_or(b'?', |i| 0x80 + i as u8)
                    }
                })
                .collect(),
        }
    }
}

/// Drops an unfinished UTF-8 sequence from the end of `bytes`, if there is one.
fn trim_incomplete_utf8(bytes: &[u8]) -> &[u8] {
    match std::str::from_utf8(bytes) {
        // error_len() is None when the input ends partway through a character.
        Err(e) if e.error_len().is_none() => &bytes[..e.valid_up_to()],
        _ => bytes,
    }
}

/// Drops an unfinished trailing UTF-8 sequence even if earlier bytes were invalid UTF-8.
fn trim_trailing_incomplete_utf8(bytes: &[u8]) -> &[u8] {
    let start = bytes.len().saturating_sub(3);
    for i in (start..bytes.len()).rev() {
        if let Err(e) = std::str::from_utf8(&bytes[i..])
            && e.valid_up_to() == 0
            && e.error_len().is_none()
        {
            return &bytes[..i];
        }
    }
    bytes
}

const CP437_HIGH: [char; 128] = [
    'Ç', 'ü', 'é', 'â', 'ä', 'à', 'å', 'ç', 'ê', 'ë', 'è', 'ï', 'î', 'ì', 'Ä', 'Å', //
    'É', 'æ', 'Æ', 'ô', 'ö', 'ò', 'û', 'ù', 'ÿ', 'Ö', 'Ü', '¢', '£', '¥', '₧', 'ƒ', //
    'á', 'í', 'ó', 'ú', 'ñ', 'Ñ', 'ª', 'º', '¿', '⌐', '¬', '½', '¼', '¡', '«', '»', //
    '░', '▒', '▓', '│', '┤', '╡', '╢', '╖', '╕', '╣', '║', '╗', '╝', '╜', '╛', '┐', //
    '└', '┴', '┬', '├', '─', '┼', '╞', '╟', '╚', '╔', '╩', '╦', '╠', '═', '╬', '╧', //
    '╨', '╤', '╥', '╙', '╘', '╒', '╓', '╫', '╪', '┘', '┌', '█', '▄', '▌', '▐', '▀', //
    'α', 'ß', 'Γ', 'π', 'Σ', 'σ', 'µ', 'τ', 'Φ', 'Θ', 'Ω', 'δ', '∞', 'φ', 'ε', '∩', //
    '≡', '±', '≥', '≤', '⌠', '⌡', '÷', '≈', '°', '∙', '·', '√', 'ⁿ', '²', '■', '\u{a0}',
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(Encoding::from_name("UTF-8"), Some(Encoding::Utf8));
        assert_eq!(Encoding::from_name("ISO-8859-1"), Some(Encoding::Windows1252));
        assert_eq!(Encoding::from_name("ibm437"), Some(Encoding::Cp437));
        assert_eq!(Encoding::from_name("koi8-r"), None);
    }

    #[test]
    fn auto_reads_utf8_and_falls_back_to_cp1252() {
        let auto = Encoding::Auto;
        assert_eq!(auto.decode("naïve ☕ 日本語 👋🏽 שלום e\u{301}".as_bytes()), "naïve ☕ 日本語 👋🏽 שלום e\u{301}");
        assert_eq!(auto.decode(b"caf\xe9 \x93quoted\x94"), "café \u{201c}quoted\u{201d}");
        assert_eq!(auto.encode("☕"), "☕".as_bytes());
        assert_eq!(Encoding::from_name("AUTO"), Some(Encoding::Auto));
    }

    #[test]
    fn partial_lines_hold_back_split_characters() {
        let bytes = "HP ❤ 10 ☕".as_bytes();
        let cut = &bytes[..bytes.len() - 1];
        assert_eq!(Encoding::Utf8.decode_partial(cut), "HP ❤ 10 ");
        assert_eq!(Encoding::Auto.decode_partial(cut), "HP ❤ 10 ");
        assert_eq!(Encoding::Utf8.decode_partial(bytes), "HP ❤ 10 ☕");
        // Bytes that can't be UTF-8 are decoded as Windows-1252, not dropped.
        assert_eq!(Encoding::Auto.decode_partial(b"caf\xe9 au lait"), "café au lait");
        // A trailing byte that could start a UTF-8 character is ambiguous, so it waits for the
        // rest of the line.
        assert_eq!(Encoding::Auto.decode_partial(b"caf\xe9"), "caf");
        // Strict UTF-8 holds back a split trailing char even after an earlier invalid byte.
        assert_eq!(Encoding::Utf8.decode_partial(b"\xff hi \xe2\x98"), "\u{FFFD} hi ");
    }

    #[test]
    fn round_trips() {
        for enc in [Encoding::Auto, Encoding::Utf8, Encoding::Windows1252, Encoding::Cp437] {
            let text = "café ½";
            assert_eq!(enc.decode(&enc.encode(text)), text, "{enc:?}");
        }
        assert_eq!(Encoding::Cp437.decode(b"\xC9\xCD\xBB"), "╔═╗");
        assert_eq!(Encoding::Windows1252.decode(b"\x93hi\x94"), "\u{201c}hi\u{201d}");
        assert_eq!(Encoding::Windows1252.encode("→"), b"?");
    }
}
