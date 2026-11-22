//! Compass directions: short and long forms, opposites, and where to place a room.

const DIRECTIONS: [(&str, &str, &str, (f64, f64)); 12] = [
    ("n", "north", "s", (0.0, -1.0)),
    ("s", "south", "n", (0.0, 1.0)),
    ("e", "east", "w", (1.0, 0.0)),
    ("w", "west", "e", (-1.0, 0.0)),
    ("ne", "northeast", "sw", (1.0, -1.0)),
    ("nw", "northwest", "se", (-1.0, -1.0)),
    ("se", "southeast", "nw", (1.0, 1.0)),
    ("sw", "southwest", "ne", (-1.0, 1.0)),
    // Up and down can't really be drawn; put them on a diagonal so they don't overlap n/s.
    ("u", "up", "d", (0.5, -1.0)),
    ("d", "down", "u", (-0.5, 1.0)),
    ("in", "in", "out", (1.0, 0.5)),
    ("out", "out", "in", (-1.0, -0.5)),
];

fn find(command: &str) -> Option<&'static (&'static str, &'static str, &'static str, (f64, f64))> {
    let c = command.trim().to_lowercase();
    DIRECTIONS.iter().find(|(short, long, _, _)| c == *short || c == *long)
}

/// A comparable form of a command: directions become their short form, anything else is
/// lowercased and trimmed. "North", "n" and "N" are the same exit.
pub fn canonical(command: &str) -> String {
    find(command).map_or_else(|| command.trim().to_lowercase(), |(short, ..)| short.to_string())
}

/// The opposite direction, in short form.
pub fn reverse(command: &str) -> Option<&'static str> {
    find(command).map(|(_, _, back, _)| *back)
}

/// The grid offset for a direction.
pub fn offset(command: &str) -> Option<(f64, f64)> {
    find(command).map(|(.., offset)| *offset)
}
