//! JavaScript scripting with QuickJS. [`JsHost`] implements the engine's
//! [`ScriptHost`](familiar_engine::script::ScriptHost); the API scripts see is in `prelude.js`.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

use familiar_engine::script::{ScriptAction, ScriptContext, ScriptHost};
use rquickjs::context::EvalOptions;
use rquickjs::{Context, Ctx, Function, Object, Runtime, Value};

const PRELUDE: &str = include_str!("prelude.js");
/// BeipMU's `app` and `window` objects, so its scripts run here.
const BEIPMU: &str = include_str!("beipmu.js");

/// A single hook or eval may run this long before it's stopped.
const TIME_LIMIT: Duration = Duration::from_secs(2);
const MEMORY_LIMIT: usize = 256 * 1024 * 1024;

pub struct JsHost {
    runtime: Runtime,
    context: Context,
    actions: Rc<RefCell<Vec<ScriptAction>>>,
    start: Instant,
    /// While JavaScript runs, when to interrupt it.
    deadline: Rc<Cell<Option<Instant>>>,
}

fn options(filename: &str) -> EvalOptions {
    let mut opts = EvalOptions::default();
    opts.global = true;
    opts.strict = false;
    // No top-level await, so eval returns the value itself rather than a promise.
    opts.promise = false;
    opts.filename = Some(filename.to_string());
    opts
}

fn parse_action(kind: &str, args_json: &str) -> ScriptAction {
    let args: Vec<serde_json::Value> = serde_json::from_str(args_json).unwrap_or_default();
    let s = |i: usize| args.get(i).and_then(|v| v.as_str()).map(str::to_string);
    let text = || s(0).unwrap_or_default();
    match kind {
        "send" => ScriptAction::Send(text()),
        "input" => ScriptAction::Input(text()),
        "notice" => ScriptAction::Notice(text()),
        "display" => ScriptAction::Display(text()),
        "spawn" => ScriptAction::Spawn { window: text(), text: s(1).unwrap_or_default() },
        "gmcp" => ScriptAction::Gmcp { package: text(), data: s(1) },
        "setvar" => ScriptAction::SetVariable { name: text(), value: s(1) },
        "speak" => ScriptAction::Speak(text()),
        "receive" => ScriptAction::Receive(text()),
        "setinput" => ScriptAction::SetInput(text()),
        "sound" => ScriptAction::PlaySound(text()),
        "error" => ScriptAction::Error(text()),
        other => ScriptAction::Error(format!("unknown action {other}")),
    }
}

/// Turns a QuickJS error into a message, with the stack for exceptions.
fn describe(ctx: &Ctx, error: rquickjs::Error) -> String {
    if !matches!(error, rquickjs::Error::Exception) {
        return error.to_string();
    }
    let value = ctx.catch();
    if let Some(ex) = value.as_exception() {
        let mut message = ex.message().unwrap_or_default();
        if message == "interrupted" {
            return format!("the script ran for more than {} seconds and was stopped", TIME_LIMIT.as_secs());
        }
        // QuickJS leaves the error type out of the message; "SyntaxError: ..." is more useful.
        if let Ok(name) = ex.as_object().get::<_, String>("name")
            && !name.is_empty()
        {
            message = format!("{name}: {message}");
        }
        return match ex.stack().filter(|s| !s.trim().is_empty()) {
            Some(stack) => format!("{message}\n{}", stack.trim_end()),
            None => message,
        };
    }
    let show: Option<Function> = ctx.globals().get::<_, Object>("__mu").and_then(|m| m.get("show")).ok();
    show.and_then(|f| f.call::<_, String>((value.clone(),)).ok())
        .unwrap_or_else(|| format!("{value:?}"))
}

impl JsHost {
    pub fn new() -> Result<Self, String> {
        let runtime = Runtime::new().map_err(|e| e.to_string())?;
        runtime.set_memory_limit(MEMORY_LIMIT);
        let deadline: Rc<Cell<Option<Instant>>> = Rc::default();
        {
            let deadline = deadline.clone();
            runtime.set_interrupt_handler(Some(Box::new(move || deadline.get().is_some_and(|d| Instant::now() > d))));
        }
        let context = Context::full(&runtime).map_err(|e| e.to_string())?;
        let actions: Rc<RefCell<Vec<ScriptAction>>> = Rc::default();
        let start = Instant::now();
        let host = Self { runtime, context, actions, start, deadline };
        host.run(|ctx| {
            let globals = ctx.globals();
            let actions = host.actions.clone();
            globals.set(
                "__mu_post",
                Function::new(ctx.clone(), move |kind: String, args: String| {
                    actions.borrow_mut().push(parse_action(&kind, &args));
                })?,
            )?;
            globals.set("__mu_now", Function::new(ctx.clone(), move || start.elapsed().as_secs_f64() * 1000.0)?)?;
            ctx.eval_with_options::<(), _>(PRELUDE, options("prelude.js"))?;
            ctx.eval_with_options::<(), _>(BEIPMU, options("beipmu.js"))
        })?;
        Ok(host)
    }

    /// Runs `f` under the time limit, then any promise jobs it queued.
    fn run<T>(&self, f: impl FnOnce(&Ctx) -> rquickjs::Result<T>) -> Result<T, String> {
        self.deadline.set(Some(Instant::now() + TIME_LIMIT));
        let result = self.context.with(|ctx| f(&ctx).map_err(|e| describe(&ctx, e)));
        while self.runtime.is_job_pending() {
            if let Err(e) = self.runtime.execute_pending_job() {
                let message = e.0.with(|ctx| describe(&ctx, rquickjs::Error::Exception));
                self.actions.borrow_mut().push(ScriptAction::Error(format!("in a promise: {message}")));
            }
        }
        self.deadline.set(None);
        result
    }

    /// Calls `__mu.<name>(args)` and reports errors as actions.
    fn call_hook<A, R>(&self, name: &str, args: A) -> Option<R>
    where
        A: for<'js> rquickjs::function::IntoArgs<'js>,
        R: for<'js> rquickjs::FromJs<'js>,
    {
        let result = self.run(|ctx| {
            let hooks: Object = ctx.globals().get("__mu")?;
            let f: Function = hooks.get(name)?;
            f.call::<A, R>(args)
        });
        match result {
            Ok(r) => Some(r),
            Err(e) => {
                self.actions.borrow_mut().push(ScriptAction::Error(e));
                None
            }
        }
    }

    fn ms_to_instant(&self, ms: f64) -> Instant {
        self.start + Duration::from_secs_f64(ms.max(0.0) / 1000.0)
    }
}

impl ScriptHost for JsHost {
    fn eval(&mut self, code: &str) -> Result<Option<String>, String> {
        self.run(|ctx| {
            let value: Value = ctx.eval_with_options(code, options("input"))?;
            if value.is_undefined() {
                return Ok(None);
            }
            let hooks: Object = ctx.globals().get("__mu")?;
            let show: Function = hooks.get("show")?;
            show.call::<_, String>((value,)).map(Some)
        })
    }

    fn load_file(&mut self, path: &Path) -> Result<(), String> {
        let code = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let name = path.display().to_string();
        self.run(|ctx| ctx.eval_with_options::<(), _>(code, options(&name)))
    }

    fn set_context(&mut self, context: &ScriptContext) {
        let json = serde_json::json!({
            "world": context.world,
            "host": context.host,
            "port": context.port,
            "character": context.character,
            "connected": context.connected,
        });
        self.call_hook::<_, ()>("setContext", (json.to_string(),));
    }

    fn set_variables(&mut self, variables: &BTreeMap<String, String>) {
        let json = serde_json::to_string(variables).unwrap_or_else(|_| "{}".into());
        self.call_hook::<_, ()>("setVars", (json,));
    }

    fn on_receive(&mut self, text: &str) {
        self.call_hook::<_, ()>("receive", (text,));
    }

    fn on_line(&mut self, text: &str) -> bool {
        self.call_hook::<_, bool>("line", (text,)).unwrap_or(true)
    }

    fn on_send(&mut self, text: &str) -> Option<String> {
        // A broken hook mustn't eat the user's commands, so errors send the original.
        match self.call_hook::<_, Option<String>>("send", (text,)) {
            Some(result) => result,
            None => Some(text.to_string()),
        }
    }

    fn on_gmcp(&mut self, package: &str, data: &str) {
        self.call_hook::<_, ()>("gmcp", (package, data));
    }

    fn on_event(&mut self, name: &str) {
        self.call_hook::<_, ()>("event", (name,));
    }

    fn call(&mut self, function: &str, args: &[String]) {
        let json = serde_json::to_string(args).unwrap_or_else(|_| "[]".into());
        self.call_hook::<_, ()>("call", (function, json));
    }

    fn tick(&mut self, now: Instant) {
        let ms = now.saturating_duration_since(self.start).as_secs_f64() * 1000.0;
        self.call_hook::<_, ()>("tick", (ms,));
    }

    fn next_deadline(&mut self) -> Option<Instant> {
        let ms = self.call_hook::<_, f64>("nextDue", ())?;
        (ms >= 0.0).then(|| self.ms_to_instant(ms))
    }

    fn take_actions(&mut self) -> Vec<ScriptAction> {
        std::mem::take(&mut self.actions.borrow_mut())
    }
}

#[cfg(test)]
mod tests;
