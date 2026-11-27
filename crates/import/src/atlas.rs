//! Reads BeipMU `.atlas` maps into our [`Atlas`]. An atlas is XML (sometimes zipped, holding an
//! `Atlas.xml`): `<map name>` elements contain `<room name rect color>` and `<exit>` elements.
//! An exit's `from` and `to` are indexes of rooms in its map (or in `map_from`/`map_to`, for
//! exits between maps); `name_from` is the command from `from` and `name_to` the way back.
//! Background rectangles, labels, images and exit line shapes have nowhere to go and are
//! counted as skipped.

use std::io::Read;

use familiar_map::{Atlas, RoomId};
use quick_xml::events::{BytesStart, Event};

#[derive(Debug, Default, Clone, PartialEq)]
pub struct AtlasImport {
    /// The area each BeipMU map became (renamed if the atlas already had that area).
    pub areas: Vec<String>,
    pub rooms: usize,
    pub exits: usize,
    /// Drawings and unnamed exits that couldn't be imported, by kind.
    pub skipped: Vec<(String, usize)>,
}

impl AtlasImport {
    pub fn describe(&self) -> String {
        let mut text = format!(
            "Imported {} rooms and {} exits{}",
            self.rooms,
            self.exits,
            match self.areas.as_slice() {
                [] => String::new(),
                [one] => format!(" into area \"{one}\""),
                many => format!(" into areas {}", many.iter().map(|a| format!("\"{a}\"")).collect::<Vec<_>>().join(", ")),
            }
        );
        if !self.skipped.is_empty() {
            let skipped: Vec<String> = self.skipped.iter().map(|(what, n)| format!("{n} {what}")).collect();
            text.push_str(&format!(" (skipped {})", skipped.join(", ")));
        }
        text
    }

    fn skip(&mut self, what: &str) {
        match self.skipped.iter_mut().find(|(w, _)| w == what) {
            Some((_, n)) => *n += 1,
            None => self.skipped.push((what.to_string(), 1)),
        }
    }
}

struct RawExit {
    map: usize,
    map_from: Option<usize>,
    map_to: Option<usize>,
    from: Option<usize>,
    to: Option<usize>,
    name_from: String,
    name_to: String,
}

const MAX_ATLAS_XML_BYTES: usize = 32 * 1024 * 1024;

/// The XML inside a `.atlas` file, unzipping it if needed.
fn xml_text(bytes: &[u8]) -> Result<String, String> {
    if bytes.starts_with(b"PK") {
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| format!("not a readable zip: {e}"))?;
        let name = (0..zip.len())
            .filter_map(|i| zip.name_for_index(i).map(str::to_string))
            .find(|n| n.to_ascii_lowercase().ends_with(".xml"))
            .ok_or("the zip has no XML file in it")?;
        let entry = zip.by_name(&name).map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        entry.take((MAX_ATLAS_XML_BYTES as u64) + 1).read_to_end(&mut out).map_err(|e| e.to_string())?;
        if out.len() > MAX_ATLAS_XML_BYTES {
            return Err("the atlas XML exceeds the 32 MiB size limit".into());
        }
        return String::from_utf8(out).map_err(|_| "the XML inside the zip isn't valid UTF-8".to_string());
    }
    if bytes.len() > MAX_ATLAS_XML_BYTES {
        return Err("the atlas XML exceeds the 32 MiB size limit".into());
    }
    std::str::from_utf8(bytes).map(str::to_string).map_err(|_| "the file isn't UTF-8 XML".to_string())
}

fn attr(e: &BytesStart, name: &str) -> Option<String> {
    e.attributes().flatten().find(|a| a.key.as_ref() == name.as_bytes()).and_then(|a| a.unescape_value().ok()).map(|v| v.into_owned())
}

fn index(e: &BytesStart, name: &str) -> Option<usize> {
    attr(e, name)?.trim().parse().ok()
}

/// `left,top,right,bottom` as x, y, width, height.
fn rect(text: &str) -> Option<(f64, f64, f64, f64)> {
    let v: Vec<f64> = text
        .split(',')
        .map(|p| p.trim().parse::<f64>().ok().filter(|n| n.is_finite()))
        .collect::<Option<_>>()?;
    match v.as_slice() {
        [l, t, r, b] => Some((l.min(*r), t.min(*b), (r - l).abs(), (b - t).abs())),
        _ => None,
    }
}

/// Adds the maps in a `.atlas` file's bytes to `atlas`.
pub fn import_atlas(bytes: &[u8], atlas: &mut Atlas) -> Result<AtlasImport, String> {
    let text = xml_text(bytes)?;
    let mut reader = quick_xml::Reader::from_str(&text);
    let mut report = AtlasImport::default();
    // Room ids for each map, by index within the map.
    let mut maps: Vec<Vec<RoomId>> = Vec::new();
    let mut exits: Vec<RawExit> = Vec::new();
    let mut saw_atlas = false;
    let mut in_map = false;
    let taken: Vec<String> = atlas.areas();
    loop {
        let event = reader.read_event().map_err(|e| format!("bad XML at byte {}: {e}", reader.buffer_position()))?;
        let (e, is_start) = match &event {
            Event::Start(e) => (e, true),
            Event::Empty(e) => (e, false),
            Event::End(e) => {
                if e.name().as_ref() == b"map" {
                    in_map = false;
                }
                continue;
            }
            Event::Eof => break,
            _ => continue,
        };
        match e.name().as_ref() {
            b"atlas" => saw_atlas = true,
            b"map" => {
                let base = attr(e, "name").filter(|n| !n.trim().is_empty()).unwrap_or_else(|| format!("Map {}", maps.len() + 1));
                let mut area = base.clone();
                let mut n = 2;
                while taken.contains(&area) || report.areas.contains(&area) {
                    area = format!("{base} ({n})");
                    n += 1;
                }
                report.areas.push(area);
                maps.push(Vec::new());
                in_map = is_start;
            }
            b"room" => {
                if !in_map {
                    continue;
                }
                let (Some(area), Some(rooms)) = (report.areas.last(), maps.last_mut()) else { continue };
                let name = attr(e, "name").unwrap_or_default();
                let (x, y, w, h) = attr(e, "rect").and_then(|r| rect(&r)).unwrap_or((0.0, 0.0, familiar_map::ROOM_W, familiar_map::ROOM_H));
                let color = attr(e, "color")
                    .filter(|c| c.starts_with('#') && c.len() == 7 && c[1..].bytes().all(|b| b.is_ascii_hexdigit()))
                    .map(|c| c.to_ascii_lowercase());
                rooms.push(atlas.insert_room(&name, area, (x, y, w, h), color));
                report.rooms += 1;
            }
            b"exit" => {
                if !in_map || maps.is_empty() {
                    continue;
                }
                exits.push(RawExit {
                    map: maps.len() - 1,
                    map_from: index(e, "map_from"),
                    map_to: index(e, "map_to"),
                    from: index(e, "from"),
                    to: index(e, "to"),
                    name_from: attr(e, "name_from").unwrap_or_default(),
                    name_to: attr(e, "name_to").unwrap_or_default(),
                });
            }
            b"rectangle" => report.skip("background rectangles"),
            b"label" => report.skip("labels"),
            b"image" => report.skip("images"),
            _ => {}
        }
    }
    if !saw_atlas {
        return Err("this isn't a BeipMU atlas (no <atlas> element)".into());
    }
    // Exits can refer to rooms in later maps, so they're added once every room exists.
    let room = |map: Option<usize>, i: Option<usize>| maps.get(map?)?.get(i?).copied();
    for x in exits {
        let from = room(x.map_from.or(Some(x.map)), x.from);
        let to = room(x.map_to.or(Some(x.map)), x.to);
        let mut added = false;
        if let Some(from) = from
            && !x.name_from.trim().is_empty()
        {
            atlas.set_exit(from, x.name_from.trim(), to, None);
            report.exits += 1;
            added = true;
        }
        if let Some(to) = to
            && !x.name_to.trim().is_empty()
        {
            atlas.set_exit(to, x.name_to.trim(), from, None);
            report.exits += 1;
            added = true;
        }
        if !added {
            report.skip("exits without a command");
        }
    }
    report.areas.retain(|a| atlas.rooms().any(|r| &r.area == a));
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_atlas_with_maps_and_cross_map_exits() {
        let mut atlas = Atlas::default();
        let report = import_atlas(include_bytes!("../tests/data/Fluff.atlas"), &mut atlas).unwrap();
        assert_eq!(report.areas, ["Main", "The Wiztower", "Space Station Celeste"]);
        assert_eq!(report.rooms, 96);
        assert_eq!(atlas.len(), 96);
        let park = atlas.rooms().find(|r| r.name == "Fluff Park").unwrap();
        assert_eq!((park.x, park.y, park.w, park.h, park.color.as_deref()), (70.0, 200.0, 120.0, 80.0, Some("#810d84")));
        assert!(report.exits > 150, "{}", report.describe());
        // Every exit leads somewhere or is waiting to be explored; none point at missing rooms.
        for r in atlas.rooms() {
            for e in &r.exits {
                assert!(e.to.is_none_or(|to| atlas.room(to).is_some()));
            }
        }
        // Walking works across the imported map.
        let lynx = atlas.rooms().find(|r| r.name == "Lynx Lane").unwrap().id;
        assert!(atlas.path(park.id, lynx).is_some_and(|p| !p.is_empty()));
    }

    #[test]
    fn zipped_atlas_and_area_names_stay_unique() {
        let mut atlas = Atlas::default();
        let bytes = include_bytes!("../tests/data/Arx_Map_by_Precisi.atlas");
        let first = import_atlas(bytes, &mut atlas).unwrap();
        assert_eq!(first.rooms, 442);
        assert!(first.skipped.iter().any(|(what, n)| what == "background rectangles" && *n == 17), "{first:?}");
        let second = import_atlas(bytes, &mut atlas).unwrap();
        assert_ne!(first.areas, second.areas, "a second import goes into a new area");
        assert_eq!(atlas.len(), 884);
    }

    #[test]
    fn rejects_other_files() {
        assert!(import_atlas(b"<html></html>", &mut Atlas::default()).is_err());
        assert!(import_atlas(b"PK\x03\x04junk", &mut Atlas::default()).is_err());
    }

    #[test]
    fn ignores_non_finite_rects_and_bad_colors() {
        let xml = r##"<atlas><map name="M"><rooms><room name="R" rect="0,0,NaN,10" color="#zzzzzz"/><room name="Ok" rect="10,20,30,40" color="#00ff00"/></rooms></map></atlas>"##;
        let mut atlas = Atlas::default();
        let report = import_atlas(xml.as_bytes(), &mut atlas).unwrap();
        assert_eq!(report.rooms, 2);
        let r = atlas.rooms().find(|r| r.name == "R").unwrap();
        assert!(r.color.is_none());
        assert!(r.w.is_finite() && r.h.is_finite());
    }
}
