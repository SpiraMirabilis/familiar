//! LLM agents (`/agent`) beside a world: a terminal (VTE) in the side dock running the agent,
//! and the host that answers its requests of the world through `familiar_agent`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use adw::prelude::*;
use familiar_agent::{AgentRequest, Agents, Launch};
use familiar_engine::{AgentTool, Engine};
use gtk::{gdk, gio, glib};
use vte4::prelude::*;

use super::SessionView;
use crate::output::OutputView;

/// The app's agents: one MCP server for every world, and the worlds it answers for.
pub struct AgentHost {
    agents: RefCell<Agents>,
    worlds: RefCell<HashMap<u64, Weak<SessionView>>>,
    next_id: Cell<u64>,
}

impl AgentHost {
    /// Must be called on the GTK main thread: agents' requests are answered there.
    pub fn new() -> Rc<Self> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AgentRequest>();
        let host = Rc::new(Self {
            agents: RefCell::new(Agents::new(tx, Some(crate::runtime().handle().clone()))),
            worlds: RefCell::default(),
            next_id: Cell::new(1),
        });
        let weak = Rc::downgrade(&host);
        glib::MainContext::default().spawn_local(async move {
            while let Some(request) = rx.recv().await {
                let Some(host) = weak.upgrade() else { break };
                host.answer(request);
            }
        });
        host
    }

    /// A number for a new world, which its agent's requests come with.
    pub fn new_id(&self) -> u64 {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        id
    }

    /// Answers requests for world `id` from `session`.
    pub fn register(&self, id: u64, session: &Rc<SessionView>) {
        self.worlds.borrow_mut().insert(id, Rc::downgrade(session));
    }

    fn answer(&self, request: AgentRequest) {
        let session = self.worlds.borrow().get(&request.session).and_then(Weak::upgrade);
        let mut agents = self.agents.borrow_mut();
        let Some(session) = session else { return agents.answer::<World>(request, None) };
        {
            let mut world = World { engine: session.engine.borrow_mut(), output: &session.output };
            agents.answer(request, Some(&mut world));
        }
        drop(agents);
        // A call has a deadline now.
        session.schedule_timer();
    }
}

/// A world as `familiar_agent` sees it.
struct World<'a> {
    engine: std::cell::RefMut<'a, Engine>,
    output: &'a OutputView,
}

impl familiar_agent::World for World<'_> {
    fn agent_tools(&self) -> Option<&[AgentTool]> {
        self.engine.agent_tools()
    }

    fn agent_call(&mut self, tool: &str, args: &serde_json::Value) -> Result<u64, String> {
        self.engine.agent_call(tool, args)
    }

    fn last_lines(&self, n: usize) -> Vec<String> {
        self.output.last_lines(n)
    }
}

/// A running agent: its terminal, in the side dock.
pub struct AgentPane {
    root: gtk::ScrolledWindow,
    terminal: vte4::Terminal,
    token: String,
    name: String,
}

impl SessionView {
    /// `/agent [name]`, `/agent stop`, `/agent list`.
    pub(super) fn agent_command(self: &Rc<Self>, args: &str) {
        let settings = self.state.config.borrow().settings.clone();
        match args.trim() {
            "list" => {
                let names = familiar_agent::names(&settings);
                self.output.notice(&format!("Agents: {names} ([[settings.agent]] in the config adds more)"));
            }
            "stop" => {
                if !self.stop_agent() {
                    self.output.notice("There's no agent working on this world");
                }
            }
            wanted => {
                if let Some(pane) = self.agent.borrow().as_ref() {
                    self.select_side_tab(&pane.root);
                    pane.terminal.grab_focus();
                    return self.output.notice("This world has an agent already (/agent stop stops it)");
                }
                match familiar_agent::choose(&settings, wanted) {
                    Ok(agent) => self.start_agent(&agent),
                    Err(e) => self.output.notice(&e),
                }
            }
        }
    }

    fn start_agent(self: &Rc<Self>, agent: &familiar_config::Agent) {
        let (world, title) = {
            let engine = self.engine.borrow();
            (engine.world().name.clone(), engine.title())
        };
        let world = if world.is_empty() { title } else { world };
        let launch = match self.state.agents.agents.borrow_mut().launch(agent, self.id, &world) {
            Ok(launch) => launch,
            Err(e) => return self.output.notice(&e),
        };
        let pane = self.agent_pane(&launch);
        self.add_side_tab(&pane.root, &format!("✦ {}", launch.name));
        self.select_side_tab(&pane.root);
        pane.terminal.grab_focus();
        *self.agent.borrow_mut() = Some(pane);
        if self.engine.borrow().agent_tools().is_none() {
            self.output.notice("This world offers no tools (its MOO needs the dns-com-familiar-agent package), so the agent can only read its output");
        }
    }

    fn agent_pane(self: &Rc<Self>, launch: &Launch) -> AgentPane {
        let terminal = vte4::Terminal::builder().hexpand(true).vexpand(true).build();
        let font = self.state.config.borrow().settings.font.clone();
        terminal.set_font(Some(&monospace_font(&terminal, &font)));
        terminal.set_scrollback_lines(10_000);
        copy_and_paste_keys(&terminal);
        let root = gtk::ScrolledWindow::builder().child(&terminal).hscrollbar_policy(gtk::PolicyType::Never).build();

        let weak = Rc::downgrade(self);
        let token = launch.token.clone();
        terminal.connect_child_exited(move |_, _| {
            if let Some(this) = weak.upgrade() {
                this.agent_ended(&token);
            }
        });

        let argv: Vec<&str> = std::iter::once(launch.program.as_str()).chain(launch.args.iter().map(String::as_str)).collect();
        let env: Vec<String> = launch.env.iter().map(|(k, v)| format!("{k}={v}")).collect();
        let env: Vec<&str> = env.iter().map(String::as_str).collect();
        let weak = Rc::downgrade(self);
        let (token, program) = (launch.token.clone(), launch.program.clone());
        terminal.spawn_async(
            vte4::PtyFlags::DEFAULT,
            launch.dir.to_str(),
            &argv,
            &env,
            glib::SpawnFlags::SEARCH_PATH,
            || {},
            -1,
            None::<&gio::Cancellable>,
            move |result| {
                if let (Err(e), Some(this)) = (result, weak.upgrade()) {
                    this.output.notice(&format!("Couldn't run {program}: {e}"));
                    this.agent_ended(&token);
                }
            },
        );
        AgentPane { root, terminal, token: launch.token.clone(), name: launch.name.clone() }
    }

    /// The agent's program ended, or never started: its tab goes.
    fn agent_ended(&self, token: &str) {
        let pane = self.agent.borrow_mut().take_if(|pane| pane.token == token);
        if let Some(pane) = pane {
            let name = pane.name.clone();
            self.close_agent_pane(pane);
            self.output.notice(&format!("{name} stopped"));
        }
    }

    /// Stops the agent, if there is one.
    pub fn stop_agent(&self) -> bool {
        let Some(pane) = self.agent.borrow_mut().take() else { return false };
        let name = pane.name.clone();
        self.close_agent_pane(pane);
        self.output.notice(&format!("Stopped {name}"));
        true
    }

    fn close_agent_pane(&self, pane: AgentPane) {
        self.state.agents.agents.borrow().close(&pane.token);
        self.remove_side_tab(&pane.root);
        // Taking the terminal away hangs up on the program (VTE sends it SIGHUP).
        drop(pane);
    }

    /// The tools the world offers changed: tell its agent.
    pub(super) fn agent_tools_changed(&self) {
        self.state.agents.agents.borrow().tools_changed(self.id);
    }

    /// A tool call finished.
    pub(super) fn agent_result(&self, call: u64, ok: bool, content: String) {
        self.state.agents.agents.borrow_mut().result(self.id, call, ok, content);
    }

    /// The world's tab is closing: its agent stops, and calls still waiting fail.
    pub fn close_agent(&self) {
        if let Some(pane) = self.agent.borrow_mut().take() {
            self.close_agent_pane(pane);
        }
        self.state.agents.agents.borrow_mut().forget(self.id);
        self.state.agents.worlds.borrow_mut().remove(&self.id);
    }
}

/// The output font, for the terminal, if it names a monospace family that is installed; else
/// the system monospace font at its size. VTE sizes every cell from the font, so a proportional
/// one (which is what fontconfig substitutes for a family it doesn't have) spreads the text out.
fn monospace_font(terminal: &vte4::Terminal, font: &str) -> gtk::pango::FontDescription {
    let mut desc = gtk::pango::FontDescription::from_string(font);
    let families = terminal.pango_context().font_map().map(|map| map.list_families()).unwrap_or_default();
    let wanted = desc.family().map(|f| f.to_string()).unwrap_or_default();
    let installed = wanted.split(',').map(str::trim).find(|name| {
        families.iter().any(|f| f.is_monospace() && f.name().eq_ignore_ascii_case(name))
    });
    desc.set_family(installed.unwrap_or("Monospace"));
    desc
}

/// Ctrl+Shift+C and Ctrl+Shift+V copy and paste, as in other terminals. (Ctrl+C goes to the
/// program.)
fn copy_and_paste_keys(terminal: &vte4::Terminal) {
    let keys = gtk::EventControllerKey::new();
    let weak = terminal.downgrade();
    keys.connect_key_pressed(move |_, key, _, state| {
        let Some(terminal) = weak.upgrade() else { return glib::Propagation::Proceed };
        let wanted = gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK;
        if state & (wanted | gdk::ModifierType::ALT_MASK) != wanted {
            return glib::Propagation::Proceed;
        }
        match key.to_lower() {
            gdk::Key::c => terminal.copy_clipboard_format(vte4::Format::Text),
            gdk::Key::v => terminal.paste_clipboard(),
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    });
    // Before the terminal sees the keys.
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    terminal.add_controller(keys);
}
