//! World maps for MU* clients: rooms and exits, an automapper fed by GMCP `Room.Info`, shortest
//! paths for walking, and live tracking from room names. Nothing here depends on a UI.
//!
//! Positions are in map units, roughly pixels at 100% zoom. Rooms sit on a grid of
//! [`GRID_X`] by [`GRID_Y`] when they're placed automatically.

mod directions;
mod room_info;

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::Path;

use serde::{Deserialize, Serialize};

pub use directions::{canonical, offset, reverse};
pub use room_info::RoomInfo;

pub const ROOM_W: f64 = 96.0;
pub const ROOM_H: f64 = 32.0;
pub const GRID_X: f64 = 144.0;
pub const GRID_Y: f64 = 72.0;

pub type RoomId = u64;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Exit {
    /// What to type to take the exit, e.g. "north" or "enter portal".
    pub command: String,
    /// The room it leads to, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<RoomId>,
    /// The server's id for the destination, waiting for that room to be mapped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_server_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Room {
    pub id: RoomId,
    pub name: String,
    #[serde(default)]
    pub area: String,
    pub x: f64,
    pub y: f64,
    #[serde(default = "default_w")]
    pub w: f64,
    #[serde(default = "default_h")]
    pub h: f64,
    /// The server's id for the room (GMCP Room.Info "num"), when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exits: Vec<Exit>,
    /// "#rrggbb", or None for the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

fn default_w() -> f64 {
    ROOM_W
}

fn default_h() -> f64 {
    ROOM_H
}

impl Room {
    pub fn center(&self) -> (f64, f64) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x <= self.x + self.w && y >= self.y && y <= self.y + self.h
    }

    pub fn exit(&self, command: &str) -> Option<&Exit> {
        let wanted = canonical(command);
        self.exits.iter().find(|e| canonical(&e.command) == wanted)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Atlas {
    #[serde(default)]
    rooms: BTreeMap<RoomId, Room>,
    #[serde(default)]
    next_id: RoomId,
    /// Changed since the last save.
    #[serde(skip)]
    dirty: bool,
}

impl Atlas {
    pub fn load(path: &Path) -> Result<Self, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// Saves if anything changed, writing a temporary file and renaming it into place.
    pub fn save(&mut self, path: &Path) -> Result<(), String> {
        if !self.dirty {
            return Ok(());
        }
        let text = serde_json::to_string(self).map_err(|e| e.to_string())?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))?;
        self.dirty = false;
        Ok(())
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn room(&self, id: RoomId) -> Option<&Room> {
        self.rooms.get(&id)
    }

    pub fn rooms(&self) -> impl Iterator<Item = &Room> {
        self.rooms.values()
    }

    pub fn len(&self) -> usize {
        self.rooms.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rooms.is_empty()
    }

    /// Area names, sorted, with "" for rooms without one.
    pub fn areas(&self) -> Vec<String> {
        let mut areas: Vec<String> = self.rooms.values().map(|r| r.area.clone()).collect();
        areas.sort();
        areas.dedup();
        areas
    }

    pub fn room_at(&self, area: &str, x: f64, y: f64) -> Option<RoomId> {
        // Later rooms are drawn on top, so search from the end.
        self.rooms.values().rev().find(|r| r.area == area && r.contains(x, y)).map(|r| r.id)
    }

    pub fn find_by_server_id(&self, server_id: &str) -> Option<RoomId> {
        self.rooms.values().find(|r| r.server_id.as_deref() == Some(server_id)).map(|r| r.id)
    }

    /// Rooms whose names contain `text`, ignoring case. Exact matches come first.
    pub fn search(&self, text: &str) -> Vec<RoomId> {
        let wanted = text.to_lowercase();
        let mut found: Vec<&Room> = self.rooms.values().filter(|r| r.name.to_lowercase().contains(&wanted)).collect();
        found.sort_by_key(|r| (r.name.to_lowercase() != wanted, r.name.len()));
        found.into_iter().map(|r| r.id).collect()
    }

    fn is_free(&self, area: &str, x: f64, y: f64, w: f64, h: f64, ignore: Option<RoomId>) -> bool {
        !self.rooms.values().any(|r| {
            Some(r.id) != ignore && r.area == area && x < r.x + r.w && x + w > r.x && y < r.y + r.h && y + h > r.y
        })
    }

    /// The nearest free grid position to (x, y), searching outwards.
    fn free_spot(&self, area: &str, x: f64, y: f64) -> (f64, f64) {
        for ring in 0i32..20 {
            for dy in -ring..=ring {
                for dx in -ring..=ring {
                    if dx.abs() != ring && dy.abs() != ring {
                        continue;
                    }
                    let (cx, cy) = (x + dx as f64 * GRID_X, y + dy as f64 * GRID_Y);
                    if self.is_free(area, cx, cy, ROOM_W, ROOM_H, None) {
                        return (cx, cy);
                    }
                }
            }
        }
        (x, y)
    }

    /// Adds a room. Unless `at` is given, it goes in the nearest free spot to the origin.
    pub fn add_room(&mut self, name: &str, area: &str, at: Option<(f64, f64)>) -> RoomId {
        let (x, y) = at.unwrap_or_else(|| self.free_spot(area, 0.0, 0.0));
        self.next_id += 1;
        let id = self.next_id;
        self.rooms.insert(
            id,
            Room { id, name: name.into(), area: area.into(), x, y, w: ROOM_W, h: ROOM_H, server_id: None, exits: Vec::new(), color: None },
        );
        self.dirty = true;
        id
    }

    /// Adds (or retargets) an exit from `from`.
    pub fn set_exit(&mut self, from: RoomId, command: &str, to: Option<RoomId>, to_server_id: Option<String>) {
        let Some(room) = self.rooms.get_mut(&from) else { return };
        let wanted = canonical(command);
        match room.exits.iter_mut().find(|e| canonical(&e.command) == wanted) {
            Some(exit) => {
                if to.is_some() {
                    exit.to = to;
                }
                if to_server_id.is_some() {
                    exit.to_server_id = to_server_id;
                }
            }
            None => room.exits.push(Exit { command: command.to_string(), to, to_server_id }),
        }
        self.dirty = true;
    }

    /// Creates a room reached from `from` by `there`, with `back` leading back. The new room is
    /// placed in that direction when `there` is a compass direction.
    pub fn add_room_from(&mut self, from: RoomId, name: &str, there: &str, back: Option<&str>) -> Option<RoomId> {
        let origin = self.rooms.get(&from)?.clone();
        let (dx, dy) = offset(there).unwrap_or((1.0, 0.0));
        let (x, y) = self.free_spot(&origin.area, origin.x + dx * GRID_X, origin.y + dy * GRID_Y);
        let id = self.add_room(name, &origin.area, Some((x, y)));
        self.set_exit(from, there, Some(id), None);
        if let Some(back) = back.map(str::to_string).or_else(|| reverse(there).map(str::to_string)) {
            self.set_exit(id, &back, Some(from), None);
        }
        Some(id)
    }

    pub fn move_room(&mut self, id: RoomId, x: f64, y: f64) {
        if let Some(room) = self.rooms.get_mut(&id) {
            room.x = x;
            room.y = y;
            self.dirty = true;
        }
    }

    pub fn rename_room(&mut self, id: RoomId, name: &str) {
        if let Some(room) = self.rooms.get_mut(&id) {
            room.name = name.to_string();
            self.dirty = true;
        }
    }

    /// Deletes a room and every exit into it.
    pub fn delete_room(&mut self, id: RoomId) {
        if self.rooms.remove(&id).is_none() {
            return;
        }
        for room in self.rooms.values_mut() {
            room.exits.retain(|e| e.to != Some(id));
        }
        self.dirty = true;
    }

    pub fn clear(&mut self) {
        self.rooms.clear();
        self.dirty = true;
    }

    /// The commands for the shortest walk from `from` to `to` (breadth-first over known exits).
    pub fn path(&self, from: RoomId, to: RoomId) -> Option<Vec<String>> {
        if from == to {
            return Some(Vec::new());
        }
        let mut came_from: HashMap<RoomId, (RoomId, &str)> = HashMap::new();
        let mut queue = VecDeque::from([from]);
        while let Some(current) = queue.pop_front() {
            for exit in self.rooms.get(&current).map(|r| r.exits.as_slice()).unwrap_or_default() {
                let Some(next) = exit.to else { continue };
                if next == from || came_from.contains_key(&next) {
                    continue;
                }
                came_from.insert(next, (current, &exit.command));
                if next == to {
                    let mut steps = Vec::new();
                    let mut at = to;
                    while at != from {
                        let (prev, command) = came_from[&at];
                        steps.push(command.to_string());
                        at = prev;
                    }
                    steps.reverse();
                    return Some(steps);
                }
                queue.push_back(next);
            }
        }
        None
    }

    /// Updates the map from GMCP Room.Info and returns the room the player is in.
    /// `previous` is the room they were in, and `last_move` the command that took them here,
    /// used to place new rooms and add exits the server didn't mention.
    pub fn apply_room_info(&mut self, info: &RoomInfo, previous: Option<RoomId>, last_move: Option<&str>) -> Option<RoomId> {
        let name = info.name.clone().unwrap_or_default();
        let area = info.area.clone().unwrap_or_else(|| previous.and_then(|p| self.rooms.get(&p)).map(|r| r.area.clone()).unwrap_or_default());
        if name.is_empty() && info.id.is_none() {
            return None;
        }
        let existing = match &info.id {
            Some(id) => self.find_by_server_id(id),
            // Without ids (BeipMU style), rooms are identified by name within their area.
            None => self.rooms.values().find(|r| r.area == area && r.name == name).map(|r| r.id),
        };
        let id = match existing {
            Some(id) => id,
            None => {
                let at = match (info.position, previous.and_then(|p| self.rooms.get(&p)), last_move.and_then(offset)) {
                    (Some(pos), _, _) => pos,
                    (None, Some(prev), Some((dx, dy))) if prev.area == area => self.free_spot(&area, prev.x + dx * GRID_X, prev.y + dy * GRID_Y),
                    _ => self.free_spot(&area, 0.0, 0.0),
                };
                let id = self.add_room(&name, &area, Some(at));
                if let Some(server_id) = &info.id {
                    self.rooms.get_mut(&id).unwrap().server_id = Some(server_id.clone());
                    // Exits elsewhere that were waiting for this room.
                    for room in self.rooms.values_mut() {
                        for exit in &mut room.exits {
                            if exit.to.is_none() && exit.to_server_id.as_deref() == Some(server_id) {
                                exit.to = Some(id);
                            }
                        }
                    }
                }
                id
            }
        };
        let room = self.rooms.get_mut(&id).unwrap();
        if !name.is_empty() && room.name != name {
            room.name = name;
        }
        if let Some(area) = &info.area {
            room.area = area.clone();
        }
        if let Some((x, y)) = info.position {
            room.x = x;
            room.y = y;
        }
        if let Some((w, h)) = info.size {
            room.w = w;
            room.h = h;
        }
        for (command, target) in &info.exits {
            let to = target.as_deref().and_then(|t| self.find_by_server_id(t));
            self.set_exit(id, command, to, target.clone());
        }
        if let (Some(prev), Some(command)) = (previous, last_move)
            && prev != id
            && self.rooms.get(&prev).and_then(|r| r.exit(command)).is_none_or(|e| e.to.is_none())
        {
            self.set_exit(prev, command, Some(id), None);
        }
        self.dirty = true;
        Some(id)
    }

    /// For live tracking without GMCP: if `line` is the name of a room next to `current`,
    /// returns it. `command`, the last movement command, breaks ties between neighbours with
    /// the same name.
    pub fn track(&self, current: RoomId, line: &str, command: Option<&str>) -> Option<RoomId> {
        let line = line.trim();
        let room = self.rooms.get(&current)?;
        if let Some(command) = command
            && let Some(to) = room.exit(command).and_then(|e| e.to)
            && self.rooms.get(&to).is_some_and(|r| r.name == line)
        {
            return Some(to);
        }
        room.exits.iter().filter_map(|e| e.to).find(|to| self.rooms.get(to).is_some_and(|r| r.name == line))
    }
}

#[cfg(test)]
mod tests;
