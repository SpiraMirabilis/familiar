//! GMCP `Room.Info` in its common shapes:
//!
//! - IRE (Achaea etc.): `{"num": 1234, "name": "...", "area": "...", "coords": "45,5,4,0",
//!   "exits": {"n": 1235}}`
//! - Aardwolf: `{"num": 1234, "name": "...", "zone": "...", "coord": {"x": 3, "y": 7},
//!   "exits": {"n": 1235}}`
//! - BeipMU: `{"area": "...", "name": "...", "coords": {"x": 100, "y": 40},
//!   "size": {"x": 96, "y": 32}}`, with rooms identified by name and placed in map units.

use serde_json::Value;

use crate::{GRID_X, GRID_Y};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RoomInfo {
    /// The server's room id.
    pub id: Option<String>,
    pub name: Option<String>,
    pub area: Option<String>,
    /// Exit commands, with the server's id for where they lead when it says.
    pub exits: Vec<(String, Option<String>)>,
    /// Position in map units.
    pub position: Option<(f64, f64)>,
    pub size: Option<(f64, f64)>,
}

fn id_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn pair(v: &Value) -> Option<(f64, f64)> {
    let num = |k: &str, alt: &str| v.get(k).or_else(|| v.get(alt)).and_then(Value::as_f64);
    Some((num("x", "w")?, num("y", "h")?))
}

impl RoomInfo {
    pub fn parse(json: &str) -> Option<Self> {
        let v: Value = serde_json::from_str(json).ok()?;
        let obj = v.as_object()?;
        let s = |k: &str| obj.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
        let id = ["num", "id", "vnum"].iter().find_map(|k| obj.get(*k).and_then(id_string));
        let size = obj.get("size").and_then(pair);
        let position = match (obj.get("coords"), obj.get("coord")) {
            // BeipMU: map units already.
            (Some(c @ Value::Object(_)), _) => pair(c),
            // IRE: "area,x,y,z" in grid cells.
            (Some(Value::String(c)), _) => {
                let parts: Vec<f64> = c.split(',').filter_map(|p| p.trim().parse().ok()).collect();
                (parts.len() >= 3).then(|| (parts[1] * GRID_X, parts[2] * GRID_Y))
            }
            // Aardwolf: grid cells.
            (_, Some(c)) => pair(c).map(|(x, y)| (x * GRID_X, y * GRID_Y)),
            _ => None,
        };
        let exits = match obj.get("exits") {
            Some(Value::Object(exits)) => exits.iter().map(|(dir, to)| (dir.clone(), id_string(to))).collect(),
            Some(Value::Array(exits)) => exits.iter().filter_map(Value::as_str).map(|d| (d.to_string(), None)).collect(),
            _ => Vec::new(),
        };
        Some(Self { id, name: s("name"), area: s("area").or_else(|| s("zone")), exits, position, size })
    }
}
