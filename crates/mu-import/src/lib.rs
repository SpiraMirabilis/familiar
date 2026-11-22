//! Importers for BeipMU's files: `Config.txt` (worlds, characters, puppets, triggers, aliases,
//! macros and variables) and `.atlas` maps. What can't be carried over is reported, not dropped
//! silently.

pub mod atlas;
pub mod config;
pub mod tree;
