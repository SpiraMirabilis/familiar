//! Client logic that sits between the network session and the UI: the per-connection [`Engine`]
//! (triggers, aliases, macros, variables, client commands, logging) plus its building blocks.
//! Nothing here depends on a UI toolkit, so the GTK and terminal frontends share it.

mod engine;
pub mod expand;
pub mod fuzzy;
pub mod history;
mod keys;
pub mod keymap;
pub mod log;
pub mod mcp;
pub mod media;
pub mod restore;
pub mod rules;
pub mod script;
pub mod stats;
pub mod tilemap;

pub use engine::{AGENT_COMMAND, Avatar, COMMANDS, EditRequest, Engine, Output, Status, Target, WebViewCommand, WebViewOpen};
pub use history::History;
pub use keys::KeyChord;
pub use keymap::Keymap;
pub use log::Logger;
pub use mcp::{AgentTool, Show as AgentShow};
