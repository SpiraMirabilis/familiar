//! A Model Context Protocol server for LLM agents (Claude Code, Codex, ...) run beside a world.
//!
//! It serves the Streamable HTTP transport on 127.0.0.1, one endpoint per agent pane at
//! `/<token>/mcp`, where the random token says which world the agent works on. The tools are
//! the ones the world offers through the dns-com-familiar-agent MCP (MUD Client Protocol)
//! package, plus `world_output`, which reads the world's window. Requests go to the main loop
//! as [`AgentRequest`]s, since the sessions live there.

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use familiar_engine::AgentTool;
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc, oneshot};

/// The tool the client adds to the world's: the last lines of the world's window.
pub const WORLD_OUTPUT: &str = "world_output";

/// Protocol versions this server speaks, newest first.
const VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// What an agent asks of the world it works on.
pub enum Ask {
    Tools(oneshot::Sender<Vec<AgentTool>>),
    /// Calls a tool. The answer is whether it succeeded and the text it returned.
    Call { tool: String, args: Value, reply: oneshot::Sender<(bool, String)> },
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
    /// Endpoint tokens and their sessions.
    tokens: Mutex<HashMap<String, u64>>,
    requests: mpsc::UnboundedSender<AgentRequest>,
    notes: broadcast::Sender<Note>,
}

impl Shared {
    fn session(&self, token: &str) -> Option<u64> {
        self.tokens.lock().ok()?.get(token).copied()
    }
}

pub struct AgentServer {
    port: u16,
    shared: Arc<Shared>,
}

impl AgentServer {
    /// Starts serving on a free port. Must be called inside the tokio runtime.
    pub fn start(requests: mpsc::UnboundedSender<AgentRequest>) -> std::io::Result<Self> {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let listener = tokio::net::TcpListener::from_std(listener)?;
        let (notes, _) = broadcast::channel(16);
        let shared = Arc::new(Shared { tokens: Mutex::new(HashMap::new()), requests, notes });
        let app = router(shared.clone());
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(Self { port, shared })
    }

    /// A new endpoint for an agent working on `session`: its token and URL.
    pub fn open(&self, session: u64) -> (String, String) {
        let token = random_token();
        if let Ok(mut tokens) = self.shared.tokens.lock() {
            tokens.insert(token.clone(), session);
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
    let Some(session) = shared.session(&token) else { return StatusCode::NOT_FOUND.into_response() };
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
                replies.extend(handle(&shared, session, m).await);
            }
            (!replies.is_empty()).then(|| Value::Array(replies))
        }
        m => handle(&shared, session, m).await,
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
    let Some(session) = shared.session(&token) else { return StatusCode::NOT_FOUND.into_response() };
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
async fn handle(shared: &Shared, session: u64, message: Value) -> Option<Value> {
    let id = message.get("id").cloned()?;
    let method = message.get("method").and_then(Value::as_str)?;
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => Ok(initialize(&params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools(shared, session).await })),
        "tools/call" => call(shared, session, &params).await,
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
            world_output shows what the player sees in the world's window.",
    })
}

fn world_output_tool() -> Value {
    json!({
        "name": WORLD_OUTPUT,
        "description": "The last lines shown in the world's window in the client (what the player sees), oldest first.",
        "inputSchema": {
            "type": "object",
            "properties": { "lines": { "type": "integer", "description": "How many lines, at most 1000 (default 50)" } },
        },
        "annotations": { "readOnlyHint": true },
    })
}

async fn tools(shared: &Shared, session: u64) -> Vec<Value> {
    let (reply, answer) = oneshot::channel();
    let _ = shared.requests.send(AgentRequest { session, ask: Ask::Tools(reply) });
    let mut tools: Vec<Value> = answer
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|t| t.name != WORLD_OUTPUT)
        .map(|t| json!({ "name": t.name, "description": t.description, "inputSchema": t.input_schema }))
        .collect();
    tools.push(world_output_tool());
    tools
}

async fn call(shared: &Shared, session: u64, params: &Value) -> Result<Value, (i64, String)> {
    let Some(tool) = params.get("name").and_then(Value::as_str) else { return Err((-32602, "No tool name".into())) };
    let args = match params.get("arguments") {
        None | Some(Value::Null) => json!({}),
        Some(args) => args.clone(),
    };
    let (reply, answer) = oneshot::channel();
    let _ = shared.requests.send(AgentRequest { session, ask: Ask::Call { tool: tool.to_string(), args, reply } });
    let (ok, text) = answer.await.unwrap_or_else(|_| (false, "The world's tab was closed".into()));
    Ok(json!({ "content": [{ "type": "text", "text": text }], "isError": !ok }))
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
        tokens.insert("secret".to_string(), 7);
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

    /// Answers requests the way the app would, with one world tool.
    fn fake_world(mut rx: mpsc::UnboundedReceiver<AgentRequest>) {
        tokio::spawn(async move {
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

        let (_, reply) = post_json(&shared, "/secret/mcp", json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_verb","arguments":{"obj":"#1"}}})).await;
        let result = reply.unwrap()["result"].clone();
        assert_eq!(result["isError"], false);
        assert_eq!(result["content"][0]["text"], r##"list_verb {"obj":"#1"}"##);

        let (_, reply) = post_json(&shared, "/secret/mcp", json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"nope"}})).await;
        assert_eq!(reply.unwrap()["result"]["isError"], true);

        let (_, reply) = post_json(&shared, "/secret/mcp", json!({"jsonrpc":"2.0","id":4,"method":"resources/list"})).await;
        assert_eq!(reply.unwrap()["error"]["code"], -32601);
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
        let server = AgentServer::start(tx).unwrap();
        let (token, url) = server.open(7);
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
