//! A Model Context Protocol server for LLM agents (Claude Code, Codex, ...) run beside a world.
//!
//! It serves the Streamable HTTP transport on 127.0.0.1, one endpoint per agent pane at
//! `/<token>/mcp`, where the random token says which world the agent works on. The tools are
//! the ones the world offers through the dns-com-familiar-agent MCP (MUD Client Protocol)
//! package, plus two of the client's own: `world_output`, which reads the world's window, and
//! `send_command`, which types commands into the world and returns what it says back. Those two
//! work on any world, so an agent can install the package on a MOO that doesn't have it yet. Requests go to the main loop
//! as [`AgentRequest`]s, since the sessions live there; [`crate::Agents`] answers them.

use std::collections::HashMap;
use std::convert::Infallible;
use std::path::{Path as FilePath, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use familiar_engine::{AgentHeard, AgentTool};
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc, oneshot};

/// The tool the client adds to the world's: the last lines of the world's window.
pub const WORLD_OUTPUT: &str = "world_output";

/// The tool the client adds to the world's: sends commands and returns what the world says.
pub const SEND_COMMAND: &str = "send_command";

/// How often `send_command` looks for what the world said.
const POLL: Duration = Duration::from_millis(100);
/// How long `send_command` waits for the world to say anything at all, unless told to wait for
/// some text.
const FIRST_WAIT: Duration = Duration::from_secs(5);
const MAX_TIMEOUT: f64 = 300.0;

/// Protocol versions this server speaks, newest first.
const VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// What an agent asks of the world it works on.
pub enum Ask {
    Tools(oneshot::Sender<Vec<AgentTool>>),
    /// Calls a tool. The answer is whether it succeeded and the text it returned.
    Call { tool: String, args: Value, reply: oneshot::Sender<(bool, String)> },
    /// Sends lines to the world. The answer is a mark for [`Ask::Heard`].
    Send { lines: Vec<String>, reply: oneshot::Sender<Result<u64, String>> },
    /// What the world said since a mark from [`Ask::Send`]; `None` if the world is gone.
    Heard { mark: u64, reply: oneshot::Sender<Option<AgentHeard>> },
}

pub struct AgentRequest {
    /// The session the agent works on.
    pub session: u64,
    pub ask: Ask,
}

#[derive(Clone)]
enum Note {
    /// The tools a session offers changed.
    ToolsChanged(u64),
    /// An agent's endpoint was closed.
    Closed(String),
}

struct Shared {
    /// Endpoint tokens, and the sessions and folders of their agents.
    tokens: Mutex<HashMap<String, Endpoint>>,
    requests: mpsc::UnboundedSender<AgentRequest>,
    notes: broadcast::Sender<Note>,
}

#[derive(Clone)]
struct Endpoint {
    session: u64,
    /// The folder the agent runs in, which `send_command` sends files from.
    dir: PathBuf,
    /// The mark after the last line of the world's the agent was given (`u64::MAX` before
    /// any), so that it's given each line once.
    seen: Arc<AtomicU64>,
}

impl Endpoint {
    fn seen(&self) -> Option<u64> {
        Some(self.seen.load(Ordering::Relaxed)).filter(|&n| n != u64::MAX)
    }
}

impl Shared {
    fn endpoint(&self, token: &str) -> Option<Endpoint> {
        self.tokens.lock().ok()?.get(token).cloned()
    }
}

pub struct AgentServer {
    port: u16,
    shared: Arc<Shared>,
}

impl AgentServer {
    /// Starts serving on a free port, on the tokio runtime `runtime`.
    pub fn start(requests: mpsc::UnboundedSender<AgentRequest>, runtime: &tokio::runtime::Handle) -> std::io::Result<Self> {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let listener = {
            let _runtime = runtime.enter();
            tokio::net::TcpListener::from_std(listener)?
        };
        let (notes, _) = broadcast::channel(16);
        let shared = Arc::new(Shared { tokens: Mutex::new(HashMap::new()), requests, notes });
        let app = router(shared.clone());
        runtime.spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(Self { port, shared })
    }

    /// A new endpoint for an agent working on `session` in the folder `dir`: its token and URL.
    pub fn open(&self, session: u64, dir: &FilePath) -> (String, String) {
        let token = random_token();
        if let Ok(mut tokens) = self.shared.tokens.lock() {
            tokens.insert(token.clone(), Endpoint { session, dir: dir.to_path_buf(), seen: Arc::new(AtomicU64::new(u64::MAX)) });
        }
        let url = self.url(&token);
        (token, url)
    }

    pub fn url(&self, token: &str) -> String {
        format!("http://127.0.0.1:{}/{token}/mcp", self.port)
    }

    /// Stops serving an agent's endpoint.
    pub fn close(&self, token: &str) {
        if let Ok(mut tokens) = self.shared.tokens.lock() {
            tokens.remove(token);
        }
        let _ = self.shared.notes.send(Note::Closed(token.to_string()));
    }

    /// Tells agents working on `session` that its tools changed.
    pub fn tools_changed(&self, session: u64) {
        let _ = self.shared.notes.send(Note::ToolsChanged(session));
    }
}

fn random_token() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::fill(&mut bytes).is_err() {
        // Not secret, but still unique; the server only listens on 127.0.0.1.
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        bytes = nanos.to_le_bytes();
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn router(shared: Arc<Shared>) -> Router {
    Router::new()
        .route("/{token}/mcp", post(on_post).get(on_get).delete(|| async { StatusCode::METHOD_NOT_ALLOWED }))
        .with_state(shared)
}

/// Turns away pages in a browser (DNS rebinding): only local origins, or none, may call.
fn foreign_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|o| o.to_str().ok()) else { return false };
    let host = origin.split("://").nth(1).unwrap_or(origin);
    let host = host.rsplit_once(':').map_or(host, |(h, _)| h);
    !matches!(host, "127.0.0.1" | "localhost" | "[::1]")
}

async fn on_post(State(shared): State<Arc<Shared>>, Path(token): Path<String>, headers: HeaderMap, body: Bytes) -> Response {
    if foreign_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(endpoint) = shared.endpoint(&token) else { return StatusCode::NOT_FOUND.into_response() };
    let message: Value = match serde_json::from_slice(&body) {
        Ok(m) => m,
        Err(e) => {
            let error = json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": format!("Parse error: {e}") } });
            return (StatusCode::BAD_REQUEST, axum::Json(error)).into_response();
        }
    };
    let reply = match message {
        Value::Array(batch) => {
            let mut replies = Vec::new();
            for m in batch {
                replies.extend(handle(&shared, &endpoint, m).await);
            }
            (!replies.is_empty()).then(|| Value::Array(replies))
        }
        m => handle(&shared, &endpoint, m).await,
    };
    match reply {
        Some(reply) => axum::Json(reply).into_response(),
        // Only notifications or responses: nothing to answer.
        None => StatusCode::ACCEPTED.into_response(),
    }
}

/// A stream for notifications from the server: that the tools changed.
async fn on_get(State(shared): State<Arc<Shared>>, Path(token): Path<String>, headers: HeaderMap) -> Response {
    if foreign_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(Endpoint { session, .. }) = shared.endpoint(&token) else { return StatusCode::NOT_FOUND.into_response() };
    let notes = shared.notes.subscribe();
    let stream = futures_util::stream::unfold(notes, move |mut notes| {
        let token = token.clone();
        async move {
            loop {
                match notes.recv().await {
                    Ok(Note::ToolsChanged(s)) if s == session => break,
                    Ok(Note::Closed(t)) if t == token => return None,
                    Ok(_) => continue,
                    // Missed some: say the tools changed, in case that was one of them.
                    Err(broadcast::error::RecvError::Lagged(_)) => break,
                    Err(broadcast::error::RecvError::Closed) => return None,
                }
            }
            let note = json!({ "jsonrpc": "2.0", "method": "notifications/tools/list_changed" });
            Some((Ok::<_, Infallible>(Event::default().event("message").data(note.to_string())), notes))
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::default()).into_response()
}

/// Answers one JSON-RPC message. Notifications and responses get no answer.
async fn handle(shared: &Shared, endpoint: &Endpoint, message: Value) -> Option<Value> {
    let session = endpoint.session;
    let id = message.get("id").cloned()?;
    let method = message.get("method").and_then(Value::as_str)?;
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => Ok(initialize(&params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools(shared, session).await })),
        "tools/call" => call(shared, endpoint, &params).await,
        _ => Err((-32601, format!("Method not found: {method}"))),
    };
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
    })
}

fn initialize(params: &Value) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or("");
    let version = VERSIONS.iter().find(|v| **v == asked).unwrap_or(&VERSIONS[1]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": true } },
        "serverInfo": { "name": "familiar", "title": familiar_config::APP_NAME, "version": env!("CARGO_PKG_VERSION") },
        "instructions": "These tools work on the MUD/MOO world open beside you in the Familiar client, \
            as the character logged in there, with that character's permissions. \
            world_output shows what the player sees in the world's window, and send_command types commands into the world \
            and returns what it says back. Use the world's own tools, when it offers any, rather than send_command. \
            If a MOO offers no tools of its own, it hasn't installed the dns-com-familiar-agent package: \
            the familiar-moo folder where you run has the MOO code for it, and how to install it (a wizard must).",
    })
}

fn world_output_tool() -> Value {
    json!({
        "name": WORLD_OUTPUT,
        "description": "The last lines shown in the world's window in the client (what the player sees), oldest first. \
            With `new`, only the lines that came since this tool or send_command last gave you the world's output.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "lines": { "type": "integer", "description": "How many lines, at most 1000 (default 50; with new, 1000)" },
                "new": { "type": "boolean", "description": "Only lines you haven't been given yet" },
            },
        },
        "annotations": { "readOnlyHint": true },
    })
}

fn send_command_tool() -> Value {
    json!({
        "name": SEND_COMMAND,
        "description": "Types commands into the world, as the player would at the input line (no client aliases or /commands), \
            and returns what the world says back. Each line is a command. It returns when the world has been quiet for `quiet` seconds \
            after it starts answering (or says nothing for 5 seconds), or once its output contains `until`, and after `timeout` seconds at most. \
            Other things that happen in the world meanwhile come back too, and so does anything the world said since you were last given \
            its output (a late answer to your last command, say), marked as coming before this command. \
            To send a long text, such as MOO code to paste, write it to a file in your folder and pass `file` rather than `command`.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "The command, or several commands one per line" },
                "file": { "type": "string", "description": "Sends this file's lines instead: a path relative to the folder you run in" },
                "until": { "type": "string", "description": "Wait until the world's output contains this text (e.g. \"Verb programmed\")" },
                "quiet": { "type": "number", "description": "Seconds of quiet that end the answer (default 1)" },
                "timeout": { "type": "number", "description": "The most seconds to wait (default 15, at most 300)" },
            },
        },
        "annotations": { "readOnlyHint": false, "destructiveHint": true, "openWorldHint": true },
    })
}

async fn tools(shared: &Shared, session: u64) -> Vec<Value> {
    let (reply, answer) = oneshot::channel();
    let _ = shared.requests.send(AgentRequest { session, ask: Ask::Tools(reply) });
    let mut tools: Vec<Value> = answer
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|t| t.name != WORLD_OUTPUT && t.name != SEND_COMMAND)
        .map(|t| json!({ "name": t.name, "description": t.description, "inputSchema": t.input_schema }))
        .collect();
    tools.push(world_output_tool());
    tools.push(send_command_tool());
    tools
}

async fn call(shared: &Shared, endpoint: &Endpoint, params: &Value) -> Result<Value, (i64, String)> {
    let session = endpoint.session;
    let Some(tool) = params.get("name").and_then(Value::as_str) else { return Err((-32602, "No tool name".into())) };
    let args = match params.get("arguments") {
        None | Some(Value::Null) => json!({}),
        Some(args) => args.clone(),
    };
    let (ok, text) = if tool == SEND_COMMAND {
        match send_command(shared, endpoint, &args).await {
            Ok(text) => (true, text),
            Err(e) => (false, e),
        }
    } else if tool == WORLD_OUTPUT && args.get("new").and_then(Value::as_bool) == Some(true) && endpoint.seen().is_some() {
        match new_output(shared, endpoint, &args).await {
            Ok(text) => (true, text),
            Err(e) => (false, e),
        }
    } else {
        if tool == WORLD_OUTPUT {
            // The agent has seen the window as it is now.
            if let Ok(heard) = heard(shared, endpoint.session, u64::MAX).await {
                endpoint.seen.store(heard.next, Ordering::Relaxed);
            }
        }
        let (reply, answer) = oneshot::channel();
        let _ = shared.requests.send(AgentRequest { session, ask: Ask::Call { tool: tool.to_string(), args, reply } });
        answer.await.unwrap_or_else(|_| (false, "The world's tab was closed".into()))
    };
    Ok(json!({ "content": [{ "type": "text", "text": text }], "isError": !ok }))
}

/// A number of seconds from `args`, within limits.
fn seconds(args: &Value, key: &str, default: f64) -> Duration {
    let n = args.get(key).and_then(Value::as_f64).filter(|n| n.is_finite()).unwrap_or(default);
    Duration::from_secs_f64(n.clamp(0.1, MAX_TIMEOUT))
}

/// The lines `send_command` sends: its command, or the lines of a file in the agent's folder.
fn command_lines(dir: &FilePath, args: &Value) -> Result<Vec<String>, String> {
    let text = match (args.get("command").and_then(Value::as_str), args.get("file").and_then(Value::as_str)) {
        (Some(_), Some(_)) => return Err("Pass command or file, not both".into()),
        (None, None) => return Err("Pass the command to send, or a file of commands".into()),
        // An empty command is a blank line, as when the player presses Enter.
        (Some(""), None) => return Ok(vec![String::new()]),
        (Some(command), None) => command.to_string(),
        (None, Some(file)) => {
            use std::path::Component;
            let relative = FilePath::new(file);
            if !relative.components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir)) {
                return Err(format!("{file} isn't a path inside the folder you run in"));
            }
            std::fs::read_to_string(dir.join(relative)).map_err(|e| format!("Couldn't read {file}: {e}"))?
        }
    };
    Ok(text.lines().map(str::to_string).collect())
}

const CLOSED: &str = "The world's tab was closed";

/// What the world said since `mark`.
async fn heard(shared: &Shared, session: u64, mark: u64) -> Result<AgentHeard, String> {
    let (reply, answer) = oneshot::channel();
    shared.requests.send(AgentRequest { session, ask: Ask::Heard { mark, reply } }).map_err(|_| CLOSED.to_string())?;
    answer.await.ok().flatten().ok_or_else(|| CLOSED.to_string())
}

/// `world_output` with `new`: the lines since the agent was last given the world's output.
async fn new_output(shared: &Shared, endpoint: &Endpoint, args: &Value) -> Result<String, String> {
    let max = args.get("lines").and_then(Value::as_u64).map_or(crate::MAX_OUTPUT_LINES, |n| n as usize).clamp(1, crate::MAX_OUTPUT_LINES);
    let seen = endpoint.seen().unwrap_or(u64::MAX);
    let heard = heard(shared, endpoint.session, seen).await?;
    endpoint.seen.store(heard.next, Ordering::Relaxed);
    let lines: Vec<&String> = heard.since(seen).collect();
    let cut = lines.len().saturating_sub(max);
    let mut text = Vec::new();
    if heard.dropped + cut as u64 > 0 {
        text.push(format!("({} earlier lines left out)", heard.dropped + cut as u64));
    }
    text.extend(lines[cut..].iter().map(|l| l.to_string()));
    if lines.is_empty() {
        text.push("(Nothing new)".into());
    }
    Ok(text.join("\n"))
}

/// Sends lines to the world and returns what it says back, after what it said since the agent
/// was last given its output.
async fn send_command(shared: &Shared, endpoint: &Endpoint, args: &Value) -> Result<String, String> {
    let lines = command_lines(&endpoint.dir, args)?;
    let until = args.get("until").and_then(Value::as_str).filter(|u| !u.is_empty());
    let quiet = seconds(args, "quiet", 1.0);
    let timeout = seconds(args, "timeout", 15.0);

    let (reply, answer) = oneshot::channel();
    shared.requests.send(AgentRequest { session: endpoint.session, ask: Ask::Send { lines, reply } }).map_err(|_| CLOSED.to_string())?;
    let mark = answer.await.map_err(|_| CLOSED.to_string())??;
    let from = endpoint.seen().map_or(mark, |seen| seen.min(mark));

    let start = tokio::time::Instant::now();
    let mut heard = AgentHeard::default();
    let mut changed = start;
    let found = loop {
        tokio::time::sleep(POLL).await;
        let now_heard = self::heard(shared, endpoint.session, from).await?;
        let now = tokio::time::Instant::now();
        if now_heard != heard {
            heard = now_heard;
            changed = now;
        }
        let mut answer = heard.since(mark).peekable();
        if let Some(until) = until {
            if answer.any(|l| l.contains(until)) {
                break true;
            }
        } else if answer.peek().is_none() {
            if now - start >= FIRST_WAIT.min(timeout) {
                break false;
            }
        } else if now - changed >= quiet {
            break false;
        }
        if now - start >= timeout {
            break false;
        }
    };
    endpoint.seen.store(heard.next, Ordering::Relaxed);
    Ok(describe(&heard, from, mark, until.filter(|_| !found), start.elapsed()))
}

/// What `send_command` returns: what the world said from mark `from` on, with the lines before
/// the command's `mark` set apart, and notes on what's missing.
fn describe(heard: &AgentHeard, from: u64, mark: u64, missing: Option<&str>, waited: Duration) -> String {
    let all: Vec<&String> = heard.since(from).collect();
    let answer = heard.since(mark).count();
    let before = all.len() - answer;
    let cut = all.len().saturating_sub(crate::MAX_OUTPUT_LINES);
    let mut text = Vec::new();
    let earlier = heard.dropped + cut as u64;
    if earlier > 0 {
        text.push(format!("({earlier} earlier lines left out)"));
    }
    if before > cut {
        text.push("(Before your command:)".into());
        text.extend(all[cut..before].iter().map(|l| l.to_string()));
        text.push("(After your command:)".into());
    }
    text.extend(all[before.max(cut)..].iter().map(|l| l.to_string()));
    if answer == 0 {
        text.push(format!("(The world said nothing in {:.1} seconds)", waited.as_secs_f64()));
    }
    if let Some(until) = missing {
        text.push(format!("(Stopped after {:.1} seconds without seeing \"{until}\")", waited.as_secs_f64()));
    }
    text.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn shared() -> (Arc<Shared>, mpsc::UnboundedReceiver<AgentRequest>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let (notes, _) = broadcast::channel(16);
        let mut tokens = HashMap::new();
        tokens.insert("secret".to_string(), Endpoint { session: 7, dir: std::env::temp_dir(), seen: Arc::new(AtomicU64::new(u64::MAX)) });
        (Arc::new(Shared { tokens: Mutex::new(tokens), requests: tx, notes }), rx)
    }

    async fn post_json(shared: &Arc<Shared>, path: &str, body: Value) -> (StatusCode, Option<Value>) {
        let request = Request::post(path)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ACCEPT, "application/json, text/event-stream")
            .body(Body::from(body.to_string()))
            .unwrap();
        let response = router(shared.clone()).oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).ok())
    }

    /// Answers requests the way the app would, with one world tool. The world answers each
    /// command `c` with "You c", except "later", whose answer "Psst" comes just before the
    /// next command's.
    fn fake_world(mut rx: mpsc::UnboundedReceiver<AgentRequest>) {
        tokio::spawn(async move {
            let mut said: Vec<String> = vec!["Welcome".into()];
            let mut late = false;
            while let Some(req) = rx.recv().await {
                assert_eq!(req.session, 7);
                match req.ask {
                    Ask::Tools(reply) => {
                        let tool = AgentTool::parse(r#"{"name":"list_verb","description":"Shows a verb's code","input_schema":{"properties":{"obj":{"type":"string"}}}}"#).unwrap();
                        let _ = reply.send(vec![tool]);
                    }
                    Ask::Call { tool, args, reply } => {
                        let ok = tool == "list_verb";
                        let _ = reply.send((ok, format!("{tool} {args}")));
                    }
                    Ask::Send { lines, reply } => {
                        if std::mem::take(&mut late) {
                            said.push("Psst".into());
                        }
                        let mark = said.len() as u64;
                        for line in lines {
                            if line == "later" {
                                late = true;
                            } else {
                                said.push(format!("You {line}"));
                            }
                        }
                        let _ = reply.send(Ok(mark));
                    }
                    Ask::Heard { mark, reply } => {
                        let next = said.len() as u64;
                        let lines = said.iter().skip(mark.min(next) as usize).cloned().collect();
                        let _ = reply.send(Some(AgentHeard { lines, partial: None, dropped: 0, next }));
                    }
                }
            }
        });
    }

    #[tokio::test]
    async fn initialize_list_and_call() {
        let (shared, rx) = shared();
        fake_world(rx);
        let (status, reply) = post_json(&shared, "/secret/mcp", json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"t","version":"1"}}})).await;
        assert_eq!(status, StatusCode::OK);
        let reply = reply.unwrap();
        assert_eq!(reply["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(reply["result"]["capabilities"]["tools"]["listChanged"], true);

        let (status, reply) = post_json(&shared, "/secret/mcp", json!({"jsonrpc":"2.0","method":"notifications/initialized"})).await;
        assert_eq!((status, reply), (StatusCode::ACCEPTED, None));

        let (_, reply) = post_json(&shared, "/secret/mcp", json!({"jsonrpc":"2.0","id":"a","method":"tools/list"})).await;
        let tools = reply.unwrap()["result"]["tools"].clone();
        assert_eq!(tools[0]["name"], "list_verb");
        assert_eq!(tools[0]["inputSchema"]["type"], "object");
        assert_eq!(tools[1]["name"], WORLD_OUTPUT);
        assert_eq!(tools[2]["name"], SEND_COMMAND);

        let (_, reply) = post_json(&shared, "/secret/mcp", json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_verb","arguments":{"obj":"#1"}}})).await;
        let result = reply.unwrap()["result"].clone();
        assert_eq!(result["isError"], false);
        assert_eq!(result["content"][0]["text"], r##"list_verb {"obj":"#1"}"##);

        let (_, reply) = post_json(&shared, "/secret/mcp", json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"nope"}})).await;
        assert_eq!(reply.unwrap()["result"]["isError"], true);

        let (_, reply) = post_json(&shared, "/secret/mcp", json!({"jsonrpc":"2.0","id":4,"method":"resources/list"})).await;
        assert_eq!(reply.unwrap()["error"]["code"], -32601);
    }

    async fn send(shared: &Arc<Shared>, args: Value) -> (bool, String) {
        let (_, reply) = post_json(shared, "/secret/mcp", json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":SEND_COMMAND,"arguments":args}})).await;
        let result = reply.unwrap()["result"].clone();
        (result["isError"] == false, result["content"][0]["text"].as_str().unwrap().to_string())
    }

    #[tokio::test]
    async fn send_command_returns_each_line_once() {
        let (shared, rx) = shared();
        fake_world(rx);
        let quick = |command: &str| json!({"command": command, "quiet": 0.2});
        // The first time, only the answer: not what came before.
        assert_eq!(send(&shared, quick("look\nsmile")).await, (true, "You look\nYou smile".into()));
        assert_eq!(send(&shared, json!({"command": "wave", "until": "wave"})).await, (true, "You wave".into()));
        // "Psst" came after the command it answered was done with, so it comes with the next one.
        let said = send(&shared, json!({"command": "later", "timeout": 0.3})).await;
        assert!(said.1.starts_with("(The world said nothing in 0."), "{said:?}");
        let said = send(&shared, json!({"command": "say hi", "until": "nobody says this", "timeout": 0.3})).await;
        assert!(said.1.starts_with("(Before your command:)\nPsst\n(After your command:)\nYou say hi\n(Stopped after 0."), "{said:?}");

        // world_output with new, likewise.
        let (_, reply) = post_json(&shared, "/secret/mcp", json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":WORLD_OUTPUT,"arguments":{"new":true}}})).await;
        assert_eq!(reply.unwrap()["result"]["content"][0]["text"], "(Nothing new)");

        assert!(!send(&shared, json!({})).await.0);
        assert!(!send(&shared, json!({"command": "x", "file": "y"})).await.0);
        let (ok, text) = send(&shared, json!({"file": "../../etc/passwd"})).await;
        assert!(!ok && text.contains("isn't a path inside"), "{text}");
    }

    #[test]
    fn earlier_lines_are_set_apart() {
        let heard = AgentHeard { lines: vec!["late".into(), "You look".into()], partial: Some(">".into()), dropped: 0, next: 12 };
        let text = describe(&heard, 10, 11, None, Duration::from_secs(1));
        assert_eq!(text, "(Before your command:)\nlate\n(After your command:)\nYou look\n>");
        assert_eq!(describe(&heard, 11, 11, None, Duration::from_secs(1)), "You look\n>");
        let silent = AgentHeard { next: 12, ..AgentHeard::default() };
        assert_eq!(describe(&silent, 12, 12, Some("done"), Duration::from_secs(2)), "(The world said nothing in 2.0 seconds)\n(Stopped after 2.0 seconds without seeing \"done\")");
    }

    #[tokio::test]
    async fn wrong_token_and_foreign_origin() {
        let (shared, rx) = shared();
        fake_world(rx);
        let (status, _) = post_json(&shared, "/guess/mcp", json!({"jsonrpc":"2.0","id":1,"method":"ping"})).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let request = Request::post("/secret/mcp")
            .header(header::ORIGIN, "https://evil.example")
            .body(Body::from(json!({"jsonrpc":"2.0","id":1,"method":"ping"}).to_string()))
            .unwrap();
        assert_eq!(router(shared.clone()).oneshot(request).await.unwrap().status(), StatusCode::FORBIDDEN);
        let mut local = HeaderMap::new();
        local.insert(header::ORIGIN, "http://localhost:3000".parse().unwrap());
        assert!(!foreign_origin(&local));
    }

    #[tokio::test]
    async fn closed_world_fails_the_call() {
        let (shared, rx) = shared();
        drop(rx);
        let (_, reply) = post_json(&shared, "/secret/mcp", json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"x"}})).await;
        let result = reply.unwrap()["result"].clone();
        assert_eq!(result["isError"], true);
        assert_eq!(result["content"][0]["text"], "The world's tab was closed");
    }

    /// The real server, over a socket: an endpoint works until it's closed, and the stream of
    /// notifications says when the tools change.
    #[tokio::test]
    async fn serves_on_a_local_port() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (tx, rx) = mpsc::unbounded_channel();
        fake_world(rx);
        let server = AgentServer::start(tx, &tokio::runtime::Handle::current()).unwrap();
        let (token, url) = server.open(7, &std::env::temp_dir());
        let address = url.trim_start_matches("http://").split('/').next().unwrap().to_string();
        let path = format!("/{token}/mcp");

        let body = json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}).to_string();
        let mut stream = tokio::net::TcpStream::connect(&address).await.unwrap();
        let request = format!(
            "POST {path} HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(response.contains("list_verb"));

        let mut events = tokio::net::TcpStream::connect(&address).await.unwrap();
        events.write_all(format!("GET {path} HTTP/1.1\r\nHost: {address}\r\nAccept: text/event-stream\r\n\r\n").as_bytes()).await.unwrap();
        let mut seen = String::new();
        let mut buf = [0u8; 4096];
        // Once the stream is open, a change is sent down it.
        let n = events.read(&mut buf).await.unwrap();
        seen.push_str(&String::from_utf8_lossy(&buf[..n]));
        server.tools_changed(7);
        while !seen.contains("list_changed") {
            let n = tokio::time::timeout(std::time::Duration::from_secs(5), events.read(&mut buf)).await.unwrap().unwrap();
            assert!(n > 0, "{seen}");
            seen.push_str(&String::from_utf8_lossy(&buf[..n]));
        }
        assert!(seen.contains("text/event-stream"), "{seen}");

        server.close(&token);
        let mut stream = tokio::net::TcpStream::connect(&address).await.unwrap();
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        assert!(response.starts_with("HTTP/1.1 404"), "{response}");
    }
}
