//! The engine's types as JSON, in the shape the Swift app decodes.

use familiar_ansi::{Palette, StyledLine};
use familiar_config::Scope;
use familiar_engine::media::MediaCommand;
use familiar_engine::stats::{Align, BarColors, StatColor, StatPane, StatValue};
use familiar_engine::tilemap::Tilemap;
use familiar_engine::{Avatar, EditRequest, Status, WebViewCommand};
use serde_json::{Value, json};

/// A line with its colours worked out. Offsets count UTF-16 units, which is what NSString uses.
/// `fg` is always given; `bg` only where it differs from the window's background.
pub fn line(line: &StyledLine, palette: &Palette) -> Value {
    let mut line = line.clone();
    line.linkify();
    let mut at = 0usize;
    let spans: Vec<Value> = line
        .spans
        .iter()
        .map(|span| {
            let len: usize = line.text[span.start..span.end].encode_utf16().count();
            let (fg, bg) = palette.resolve(&span.style);
            let mut v = json!({ "start": at, "len": len, "fg": fg.to_hex() });
            at += len;
            let o = v.as_object_mut().unwrap();
            if bg != palette.background {
                o.insert("bg".into(), bg.to_hex().into());
            }
            for (name, on) in [
                ("bold", span.style.bold),
                ("italic", span.style.italic),
                ("underline", span.style.underline),
                ("strike", span.style.strike),
                ("blink", span.style.blink),
            ] {
                if on {
                    o.insert(name.into(), true.into());
                }
            }
            if let Some(link) = &span.style.link {
                o.insert("link".into(), link.to_string().into());
            }
            v
        })
        .collect();
    json!({ "text": line.text, "spans": spans })
}

pub fn palette(palette: &Palette) -> Value {
    json!({
        "foreground": palette.foreground.to_hex(),
        "background": palette.background.to_hex(),
        "ansi": palette.ansi.iter().map(|c| c.to_hex()).collect::<Vec<_>>(),
    })
}

pub fn scope(scope: &Scope) -> Value {
    let mut v = match scope {
        Scope::Global => json!({ "kind": "global" }),
        Scope::World { world } => json!({ "kind": "world", "world": world }),
        Scope::Character { world, character } => json!({ "kind": "character", "world": world, "character": character }),
    };
    v["label"] = scope.label().into();
    v
}

/// The other way: a scope as [`scope`] writes it (its label is ignored).
pub fn parse_scope(v: &Value) -> Option<Scope> {
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    match v.get("kind")?.as_str()? {
        "global" => Some(Scope::Global),
        "world" => Some(Scope::World { world: s("world")? }),
        "character" => Some(Scope::Character { world: s("world")?, character: s("character")? }),
        _ => None,
    }
}

pub fn status(status: &Status) -> Value {
    match status {
        Status::Connecting => json!({ "state": "connecting" }),
        Status::Connected => json!({ "state": "connected" }),
        Status::Disconnected(reason) => json!({ "state": "disconnected", "reason": reason }),
    }
}

pub fn avatar(avatar: &Avatar) -> Value {
    json!({ "id": avatar.id, "url": avatar.url, "hover": avatar.hover, "click_url": avatar.click_url })
}

pub fn edit_request(request: &EditRequest) -> Value {
    json!({
        "reference": request.reference,
        "name": request.name,
        "kind": request.kind,
        "content": request.content,
        "language": request.language,
    })
}

pub fn parse_edit_request(v: &Value) -> Option<EditRequest> {
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    Some(EditRequest { reference: s("reference")?, name: s("name")?, kind: s("kind")?, content: s("content").unwrap_or_default(), language: s("language") })
}

pub fn media(command: &MediaCommand) -> Value {
    match command {
        MediaCommand::Play { name, url, volume, loops, restart } => {
            json!({ "op": "play", "name": name, "url": url, "volume": volume, "loops": loops, "restart": restart })
        }
        MediaCommand::Preload { url } => json!({ "op": "preload", "url": url }),
        MediaCommand::Stop { name } => json!({ "op": "stop", "name": name }),
    }
}

pub fn webview(command: &WebViewCommand) -> Value {
    match command {
        WebViewCommand::Open(open) => json!({
            "op": "open",
            "id": open.id,
            "url": open.url,
            "source": open.source,
            "dock": open.dock,
            "width": open.width,
            "height": open.height,
            "headers": open.headers.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(),
            "from_server": open.from_server,
        }),
        WebViewCommand::Close { id } => json!({ "op": "close", "id": id }),
    }
}

fn color(c: &Option<StatColor>) -> Value {
    match c {
        Some(StatColor::Rgb(r, g, b)) => format!("#{r:02x}{g:02x}{b:02x}").into(),
        Some(StatColor::Transparent) => "transparent".into(),
        None => Value::Null,
    }
}

fn bar(colors: &BarColors) -> Value {
    json!({ "fill": color(&colors.fill), "empty": color(&colors.empty), "outline": color(&colors.outline) })
}

pub fn stat_pane(pane: &StatPane) -> Value {
    let values: Vec<Value> = pane
        .values
        .iter()
        .map(|(key, stat)| {
            let (kind, extra) = match &stat.value {
                StatValue::None => ("none", json!({})),
                StatValue::Int(_) => ("text", json!({})),
                StatValue::Text(_) => ("text", json!({})),
                StatValue::Range { colors, .. } => ("bar", bar(colors)),
                StatValue::Progress { colors, .. } => ("bar", bar(colors)),
            };
            json!({
                "key": key,
                "name": stat.display_name(key),
                "align": match stat.name_alignment { Align::Left => "left", Align::Center => "center", Align::Right => "right" },
                "name_color": color(&stat.name_color),
                "value_color": color(&stat.value_color),
                "kind": kind,
                "text": stat.value.text(),
                "fraction": stat.value.fraction(),
                "colors": extra,
            })
        })
        .collect();
    json!({ "background": color(&pane.background), "values": values })
}

pub fn tilemap(map: &Tilemap) -> Value {
    json!({
        "tile_url": map.info.tile_url,
        "tile_width": map.info.tile_width,
        "tile_height": map.info.tile_height,
        "width": map.info.width,
        "height": map.info.height,
        "tiles": map.tiles,
    })
}
