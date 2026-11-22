//! Client logic that sits between the network session and the UI: the per-connection [`Engine`]
//! (triggers, aliases, macros, variables, client commands, logging) plus its building blocks.
//! Nothing here depends on a UI toolkit, so the GTK and terminal frontends share it.

mod engine;
pub mod expand;
pub mod history;
mod keys;
pub mod log;
pub mod rules;

pub use engine::{Engine, Output, Status, Target};
pub use history::History;
pub use keys::KeyChord;
pub use log::Logger;
