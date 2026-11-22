//! The interface between the engine and a scripting language. The engine calls the host's hooks
//! at fixed points (lines arriving, commands being sent, GMCP, timers). Scripts never call back
//! into the engine directly: what they ask for is queued as [`ScriptAction`]s, which the engine
//! applies after the hook returns. That keeps scripts from reentering the engine mid-line.

use std::collections::BTreeMap;
use std::time::Instant;

#[derive(Debug, Clone, PartialEq)]
pub enum ScriptAction {
    /// Send to the server as is.
    Send(String),
    /// Handle as if the user typed it: aliases and `/` commands apply.
    Input(String),
    /// Show a client message.
    Notice(String),
    /// Show a line in the output. ANSI codes are allowed.
    Display(String),
    /// Show a line in a spawn window.
    Spawn { window: String, text: String },
    Gmcp { package: String, data: Option<String> },
    /// Set (Some) or remove (None) a variable.
    SetVariable { name: String, value: Option<String> },
    /// Speak text aloud.
    Speak(String),
    /// A script error, already formatted with its stack.
    Error(String),
}

/// Facts about the session that scripts can read.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScriptContext {
    pub world: String,
    pub host: String,
    pub port: u16,
    pub character: Option<String>,
    pub connected: bool,
}

pub trait ScriptHost {
    /// Runs code typed by the user (`/@`). Returns the result to show, if any.
    fn eval(&mut self, code: &str) -> Result<Option<String>, String>;
    /// Runs a script file.
    fn load_file(&mut self, path: &std::path::Path) -> Result<(), String>;

    fn set_context(&mut self, context: &ScriptContext);
    fn set_variables(&mut self, variables: &BTreeMap<String, String>);

    /// A line arrived, before any processing.
    fn on_receive(&mut self, text: &str);
    /// A line is about to be shown (after triggers). Returns false to hide it.
    fn on_line(&mut self, text: &str) -> bool;
    /// A command is about to be sent. Returns the (possibly changed) command, or None to drop it.
    fn on_send(&mut self, text: &str) -> Option<String>;
    fn on_gmcp(&mut self, package: &str, data: &str);
    /// Named events: "connect", "disconnect".
    fn on_event(&mut self, name: &str);
    /// Calls a global function by name, for the trigger "script" action.
    fn call(&mut self, function: &str, args: &[String]);

    /// Runs timers that are due.
    fn tick(&mut self, now: Instant);
    /// When the next timer is due.
    fn next_deadline(&mut self) -> Option<Instant>;

    /// Everything scripts asked for since the last call.
    fn take_actions(&mut self) -> Vec<ScriptAction>;
}
