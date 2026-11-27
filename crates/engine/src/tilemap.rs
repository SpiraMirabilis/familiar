//! The `Beip.Tilemap` GMCP package: a grid of tiles cut from one image, sent compactly.
//! See BeipMU's TileMap.md.

use std::collections::HashMap;
use std::io::Read;

use base64::Engine as _;
use serde_json::Value;

/// The largest map and tile size BeipMU allows, in each dimension.
const MAX_SIZE: u32 = 256;

#[derive(Debug, Clone, PartialEq)]
pub struct TileInfo {
    pub tile_url: String,
    pub tile_width: u32,
    pub tile_height: u32,
    pub width: u32,
    pub height: u32,
    pub encoding: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tilemap {
    pub info: TileInfo,
    /// Tile numbers, left to right then top to bottom: width * height of them.
    pub tiles: Vec<u8>,
}

#[derive(Debug, Default)]
pub struct Tilemaps {
    maps: HashMap<String, Tilemap>,
}

fn size(v: &Value, key: &str) -> Result<(u32, u32), String> {
    let text = v.get(key).and_then(Value::as_str).ok_or_else(|| format!("missing {key}"))?;
    let (w, h) = text.split_once(',').ok_or_else(|| format!("{key} should be \"width,height\""))?;
    let parse = |s: &str| s.trim().parse::<u32>().ok().filter(|n| (1..=MAX_SIZE).contains(n));
    match (parse(w), parse(h)) {
        (Some(w), Some(h)) => Ok((w, h)),
        _ => Err(format!("{key} must be 1 to {MAX_SIZE} in each dimension")),
    }
}

/// Decodes map data in one of BeipMU's encodings.
pub fn decode(encoding: &str, data: &str) -> Result<Vec<u8>, String> {
    // BeipMU's own examples leave off the base64 padding, so it's optional.
    let engine = base64::engine::GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        base64::engine::GeneralPurposeConfig::new().with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent),
    );
    let base64 = |s: &str| engine.decode(s.trim()).map_err(|e| format!("bad base64: {e}"));
    match encoding.to_ascii_lowercase().as_str() {
        "hex_4" => data
            .chars()
            .map(|c| c.to_digit(16).map(|d| d as u8).ok_or_else(|| format!("bad hex digit {c:?}")))
            .collect(),
        "hex_8" => {
            let digits: Vec<char> = data.chars().collect();
            if !digits.len().is_multiple_of(2) {
                return Err("hex_8 data needs an even number of digits".into());
            }
            digits
                .chunks(2)
                .map(|pair| u8::from_str_radix(&pair.iter().collect::<String>(), 16).map_err(|_| "bad hex digit".to_string()))
                .collect()
        }
        "base64_8" => base64(data),
        "zbase64_8" => {
            let compressed = base64(data)?;
            let max_bytes = (MAX_SIZE * MAX_SIZE) as usize;
            enum InflateErr {
                TooLarge,
                Io(std::io::Error),
            }
            let read_capped = |mut reader: Box<dyn Read>| -> Result<Vec<u8>, InflateErr> {
                let mut out = Vec::new();
                reader.by_ref().take((max_bytes as u64) + 1).read_to_end(&mut out).map_err(InflateErr::Io)?;
                if out.len() > max_bytes {
                    return Err(InflateErr::TooLarge);
                }
                Ok(out)
            };
            // RFC 1950 zlib header check; BeipMU examples also use raw deflate, so fall back cleanly.
            let looks_zlib = compressed.len() >= 2
                && (compressed[0] & 0x0f) == 8
                && ((compressed[0] as u16) * 256 + compressed[1] as u16).is_multiple_of(31);
            let out = if looks_zlib {
                match read_capped(Box::new(flate2::read::ZlibDecoder::new(compressed.as_slice()))) {
                    Ok(v) => Ok(v),
                    Err(InflateErr::TooLarge) => Err(InflateErr::TooLarge),
                    Err(InflateErr::Io(_)) => read_capped(Box::new(flate2::read::DeflateDecoder::new(compressed.as_slice()))),
                }
            } else {
                read_capped(Box::new(flate2::read::DeflateDecoder::new(compressed.as_slice())))
            };
            out.map_err(|e| match e {
                InflateErr::TooLarge => "tilemap data exceeds size limit".to_string(),
                InflateErr::Io(e) => format!("couldn't decompress: {e}"),
            })
        }
        other => Err(format!("unknown encoding {other}")),
    }
}

impl Tilemaps {
    pub fn get(&self, name: &str) -> Option<&Tilemap> {
        self.maps.get(name)
    }

    /// Handles `beip.tilemap.info`. Returns the names of maps that changed.
    pub fn info(&mut self, json: &str) -> Result<Vec<String>, String> {
        let root: Value = serde_json::from_str(json).map_err(|e| format!("beip.tilemap.info: {e}"))?;
        let root = root.as_object().ok_or("beip.tilemap.info: expected an object")?;
        let mut changed = Vec::new();
        for (name, v) in root {
            if !self.maps.contains_key(name) && self.maps.len() >= 64 {
                return Err("beip.tilemap.info: too many tilemaps".into());
            }
            let fail = |e: String| format!("beip.tilemap.info \"{name}\": {e}");
            let (tile_width, tile_height) = size(v, "tile-size").map_err(fail)?;
            let (width, height) = size(v, "map-size").map_err(fail)?;
            let tile_url = v.get("tile-url").and_then(Value::as_str).ok_or_else(|| fail("missing tile-url".into()))?;
            if !crate::media::is_http_url(tile_url) {
                return Err(fail("tile-url must be an http:// or https:// URL".into()));
            }
            let info = TileInfo {
                tile_url: tile_url.to_string(),
                tile_width,
                tile_height,
                width,
                height,
                encoding: v.get("encoding").and_then(Value::as_str).unwrap_or("hex_4").to_string(),
            };
            let count = (width * height) as usize;
            let map = self.maps.entry(name.clone()).or_insert_with(|| Tilemap { info: info.clone(), tiles: vec![0; count] });
            // A new size starts over with tile 0, as in BeipMU.
            if map.tiles.len() != count {
                map.tiles = vec![0; count];
            }
            map.info = info;
            changed.push(name.clone());
        }
        Ok(changed)
    }

    /// Handles `beip.tilemap.data`. Returns the names of maps that changed.
    pub fn data(&mut self, json: &str) -> Result<Vec<String>, String> {
        let root: Value = serde_json::from_str(json).map_err(|e| format!("beip.tilemap.data: {e}"))?;
        let root = root.as_object().ok_or("beip.tilemap.data: expected an object")?;
        let mut changed = Vec::new();
        for (name, v) in root {
            let map = self.maps.get_mut(name).ok_or_else(|| format!("beip.tilemap.data: \"{name}\" needs a beip.tilemap.info first"))?;
            let tiles = decode(&map.info.encoding, v.as_str().unwrap_or("")).map_err(|e| format!("beip.tilemap.data \"{name}\": {e}"))?;
            if tiles.len() != map.tiles.len() {
                return Err(format!(
                    "beip.tilemap.data \"{name}\": {} tiles, but the map is {}x{} ({})",
                    tiles.len(),
                    map.info.width,
                    map.info.height,
                    map.tiles.len()
                ));
            }
            map.tiles = tiles;
            changed.push(name.clone());
        }
        Ok(changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documented_hex_example() {
        let mut maps = Tilemaps::default();
        maps.info(r#"{"Map1":{"tile-url":"https://x/U5.png","tile-size":"16,16","map-size":"10,4","encoding":"Hex_4"}}"#).unwrap();
        assert_eq!(maps.get("Map1").unwrap().tiles, vec![0; 40]);
        maps.data(r#"{"Map1":"0123456701234567012345670123456701234567"}"#).unwrap();
        let map = maps.get("Map1").unwrap();
        assert_eq!(&map.tiles[..9], &[0, 1, 2, 3, 4, 5, 6, 7, 0]);
        assert_eq!((map.info.tile_width, map.info.width, map.info.height), (16, 10, 4));
    }

    #[test]
    fn documented_compressed_examples() {
        // Both examples from BeipMU's TileMap.md.
        let lighthouse = "tZBBCsIwEEVXuc5g9Qqz9h9AvVNRcS0URGh13YXo1mM5bRI6xsmiWj80/eHNHz5xbjqRZWnq3fSfxqPYSMErtxdgBk5q5gPzs5+5QStywdwPLblabzrzwIXPmodEKbblcCQ8IjFbRt0YXL7rDry/H5DkvcSV4aibFXR/3xGt0T8OAIVcFvb7dUGX4xH8xGcixUn/i2HUzs+9cvy9Rk5Hk1Oap2/3W/wF";
        assert_eq!(decode("zbase64_8", lighthouse).unwrap().len(), 32 * 32);
        let lab = "k+ZFAXzaLKxIgE1/5ixHhtsMYBBzLFlMf+WqQIbXrxg4OLkYcq4Vi+lrg2R+MUhISjHUMDSA+FqLQULGIALCnw7RvnsnlL+cQRfEP30Syg9lsGW4fJmB4SZcngHIvwzXv50BLMBwB8yPZUgHG3eZ4Q2YX8vQzhDNcIQBZt9XBgQA8VVUkYCavo8vCvADAA";
        assert_eq!(decode("zbase64_8", lab).unwrap().len(), 16 * 16);
    }

    #[test]
    fn other_encodings_and_errors() {
        assert_eq!(decode("hex_8", "00ff10").unwrap(), vec![0, 255, 16]);
        assert_eq!(decode("base64_8", "AAEC").unwrap(), vec![0, 1, 2]);
        assert!(decode("hex_4", "0g").is_err());
        assert!(decode("hex_8", "abc").is_err());
        assert!(decode("rle", "").is_err());

        let mut maps = Tilemaps::default();
        assert!(maps.data(r#"{"Nope":"0"}"#).unwrap_err().contains("info first"));
        assert!(maps.info(r#"{"Big":{"tile-url":"https://x/u.png","tile-size":"16,16","map-size":"999,2"}}"#).is_err());
        assert!(maps.info(r#"{"BadUrl":{"tile-url":"file:///etc/passwd","tile-size":"8,8","map-size":"2,2"}}"#).is_err());
        assert!(maps.info(r#"{"RelUrl":{"tile-url":"u","tile-size":"8,8","map-size":"2,2"}}"#).is_err());
        maps.info(r#"{"M":{"tile-url":"https://x/u.png","tile-size":"8,8","map-size":"2,2","encoding":"hex_4"}}"#).unwrap();
        assert!(maps.data(r#"{"M":"012"}"#).unwrap_err().contains("3 tiles"));
        // A size change resets the tiles.
        maps.data(r#"{"M":"1111"}"#).unwrap();
        maps.info(r#"{"M":{"tile-url":"https://x/u.png","tile-size":"8,8","map-size":"3,1","encoding":"hex_4"}}"#).unwrap();
        assert_eq!(maps.get("M").unwrap().tiles, vec![0, 0, 0]);
    }

    #[test]
    fn zbase64_rejects_decompression_bombs() {
        use flate2::Compression;
        use flate2::write::ZlibEncoder;
        use std::io::Write;
        let mut enc = ZlibEncoder::new(Vec::new(), Compression::best());
        enc.write_all(&vec![0u8; (MAX_SIZE * MAX_SIZE) as usize + 1024]).unwrap();
        let compressed = enc.finish().unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(compressed);
        assert!(decode("zbase64_8", &b64).unwrap_err().contains("exceeds size limit"));
    }
}
