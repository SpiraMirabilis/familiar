//! Client logic that sits between the network session and the UI. For now this is logging and
//! input history. Triggers, aliases and macros will be added here in M2.

pub mod history;
pub mod log;

pub use history::History;
pub use log::Logger;
