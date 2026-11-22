//! The map: tracking the current room, walking, and the `/map` command.

use super::*;

impl Engine {
    /// Uses an in-memory map that's never saved (for tests and throwaway sessions).
    pub fn detach_map(&mut self) {
        self.map = Atlas::default();
        self.map_path = None;
    }

    pub fn map(&self) -> &Atlas {
        &self.map
    }

    pub fn current_room(&self) -> Option<RoomId> {
        self.current_room
    }

    /// Saves the map if it changed. The UI calls this now and then, and when the tab closes.
    pub fn save_map(&mut self) -> Result<(), String> {
        match &self.map_path {
            Some(path) => self.map.save(path),
            None => Ok(()),
        }
    }

    pub fn map_set_current(&mut self, room: Option<RoomId>) -> Vec<Output> {
        self.current_room = room;
        vec![Output::MapChanged]
    }

    /// Sends the commands to walk to `room` by the shortest known path.
    pub fn map_walk_to(&mut self, room: RoomId) -> Vec<Output> {
        let mut out = Vec::new();
        let Some(from) = self.current_room else {
            out.push(Output::Notice("Set your current room first: right-click it on the map, or use /map here name".into()));
            return out;
        };
        let Some(path) = self.map.path(from, room) else {
            out.push(Output::Notice("No known way there from here".into()));
            return out;
        };
        for step in &path {
            self.send_command(step, &mut out);
        }
        // Without Room.Info nothing will report each step, so assume the walk worked.
        if !self.room_info_seen {
            self.current_room = Some(room);
        }
        self.last_move = None;
        out.push(Output::MapChanged);
        out
    }

    pub fn map_move_room(&mut self, room: RoomId, x: f64, y: f64) -> Vec<Output> {
        self.map.move_room(room, x, y);
        vec![Output::MapChanged]
    }

    pub fn map_rename_room(&mut self, room: RoomId, name: &str) -> Vec<Output> {
        self.map.rename_room(room, name);
        vec![Output::MapChanged]
    }

    pub fn map_delete_room(&mut self, room: RoomId) -> Vec<Output> {
        self.map.delete_room(room);
        if self.current_room == Some(room) {
            self.current_room = None;
        }
        vec![Output::MapChanged]
    }

    /// Adds an unconnected room at a position, e.g. from the map's right-click menu.
    pub fn map_add_room_at(&mut self, name: &str, area: &str, x: f64, y: f64) -> (RoomId, Vec<Output>) {
        let id = self.map.add_room(name, area, Some((x, y)));
        (id, vec![Output::MapChanged])
    }

    /// Adds a room reached from the current one (or a first room when there's none).
    pub fn map_add_room(&mut self, name: &str, there: Option<&str>, back: Option<&str>) -> Vec<Output> {
        let id = match (self.current_room, there) {
            (Some(from), Some(there)) => self.map.add_room_from(from, name, there, back),
            _ => Some(self.map.add_room(name, "", None)),
        };
        self.current_room = id.or(self.current_room);
        vec![Output::MapChanged]
    }

    /// Adds the rooms in a BeipMU `.atlas` file to this world's map.
    pub fn map_import(&mut self, path: &std::path::Path) -> Vec<Output> {
        let result = std::fs::read(path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| familiar_import::atlas::import_atlas(&bytes, &mut self.map));
        match result {
            Ok(report) => {
                let mut out = vec![Output::Notice(report.describe()), Output::MapChanged, Output::ShowMap];
                if let Err(e) = self.save_map() {
                    out.push(Output::Notice(format!("Couldn't save the map: {e}")));
                }
                out
            }
            Err(e) => vec![Output::Notice(format!("Couldn't import {}: {e}", path.display()))],
        }
    }

    pub(super) fn map_command(&mut self, args: &str, out: &mut Vec<Output>) {
        let (sub, rest) = args.split_once(char::is_whitespace).unwrap_or((args, ""));
        let rest = rest.trim();
        let find = |map: &Atlas, name: &str| map.search(name).first().copied();
        match sub {
            "" | "show" => out.push(Output::ShowMap),
            "go" | "walk" if !rest.is_empty() => match find(&self.map, rest) {
                Some(room) => out.extend(self.map_walk_to(room)),
                None => out.push(Output::Notice(format!("No room on the map matches \"{rest}\""))),
            },
            "here" if !rest.is_empty() => match find(&self.map, rest) {
                Some(room) => out.extend(self.map_set_current(Some(room))),
                None => out.push(Output::Notice(format!("No room on the map matches \"{rest}\""))),
            },
            "addroom" if !rest.is_empty() => {
                // /map addroom "Room name" there [back], as in BeipMU.
                let (name, rest) = match rest.strip_prefix('"').and_then(|r| r.split_once('"')) {
                    Some((name, rest)) => (name.to_string(), rest.trim()),
                    None => {
                        let (name, rest) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
                        (name.to_string(), rest.trim())
                    }
                };
                let mut words = rest.split_whitespace();
                out.extend(self.map_add_room(&name, words.next(), words.next()));
            }
            "rename" if !rest.is_empty() => match self.current_room {
                Some(room) => out.extend(self.map_rename_room(room, rest)),
                None => out.push(Output::Notice("There's no current room to rename".into())),
            },
            "delete" => match self.current_room {
                Some(room) => out.extend(self.map_delete_room(room)),
                None => out.push(Output::Notice("There's no current room to delete".into())),
            },
            "track" => {
                self.map_tracking = rest != "off";
                out.push(Output::Notice(format!("Tracking by room name is {}", if self.map_tracking { "on" } else { "off" })));
            }
            "clear" if rest == "yes" => {
                self.map.clear();
                self.current_room = None;
                out.push(Output::MapChanged);
                out.push(Output::Notice("Map cleared".into()));
            }
            "clear" => out.push(Output::Notice("This deletes every room on this world's map. Type /map clear yes to do it.".into())),
            "import" if !rest.is_empty() => {
                let path = familiar_config::paths::expand(rest.trim_matches('"'));
                out.extend(self.map_import(&path));
            }
            _ => out.push(Output::Notice(
                "Map commands: /map, /map go name, /map here name, /map addroom \"name\" there [back], /map rename name, /map delete, /map track on|off, /map import file.atlas, /map clear".into(),
            )),
        }
    }
}
