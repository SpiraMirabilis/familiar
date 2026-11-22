//! Server text encodings. MU*s mostly use UTF-8, Windows-1252 (often labelled "latin1") or CP437.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Encoding {
    #[default]
    Utf8,
    Windows1252,
    Cp437,
}

impl Encoding {
    /// Looks up an encoding by charset name (as used in telnet CHARSET or config files).
    pub fn from_name(name: &str) -> Option<Self> {
        let n = name.trim().to_ascii_lowercase().replace('_', "-");
        match n.as_str() {
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
            Self::Utf8 => "UTF-8",
            Self::Windows1252 => "Windows-1252",
            Self::Cp437 => "CP437",
        }
    }

    pub fn decode(self, bytes: &[u8]) -> String {
        match self {
            Self::Utf8 => String::from_utf8_lossy(bytes).into_owned(),
            Self::Windows1252 => encoding_rs::WINDOWS_1252.decode_without_bom_handling(bytes).0.into_owned(),
            Self::Cp437 => bytes
                .iter()
                .map(|&b| if b < 0x80 { b as char } else { CP437_HIGH[(b - 0x80) as usize] })
                .collect(),
        }
    }

    /// Encodes outgoing text. Characters the encoding can't represent become '?'.
    pub fn encode(self, text: &str) -> Vec<u8> {
        match self {
            Self::Utf8 => text.as_bytes().to_vec(),
            Self::Windows1252 => {
                let mut out = Vec::with_capacity(text.len());
                for c in text.chars() {
                    let mut buf = [0u8; 4];
                    let (bytes, _, had_errors) = encoding_rs::WINDOWS_1252.encode(c.encode_utf8(&mut buf));
                    if had_errors { out.push(b'?') } else { out.extend_from_slice(&bytes) }
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
    fn round_trips() {
        for enc in [Encoding::Utf8, Encoding::Windows1252, Encoding::Cp437] {
            let text = "café ½";
            assert_eq!(enc.decode(&enc.encode(text)), text, "{enc:?}");
        }
        assert_eq!(Encoding::Cp437.decode(b"\xC9\xCD\xBB"), "╔═╗");
        assert_eq!(Encoding::Windows1252.decode(b"\x93hi\x94"), "\u{201c}hi\u{201d}");
        assert_eq!(Encoding::Windows1252.encode("→"), b"?");
    }
}
