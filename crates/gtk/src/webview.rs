//! Embedded web views (WebKitGTK), with a JavaScript bridge to the client. See
//! `webview_bridge.js` for the page-side API.

use gtk::gio;
use familiar_engine::WebViewOpen;
use webkit6::prelude::*;

const BRIDGE_JS: &str = include_str!("webview_bridge.js");

/// A request from a page's script.
#[derive(Debug, Clone, PartialEq)]
pub enum BridgeMessage {
    Send { text: String, aliases: bool },
    Gmcp { package: String, data: Option<String> },
    Receive(String),
    Display(String),
    Close,
}

impl BridgeMessage {
    fn parse(json: &str) -> Option<Self> {
        let v: serde_json::Value = serde_json::from_str(json).ok()?;
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
        Some(match v.get("op")?.as_str()? {
            "send" => Self::Send { text: s("text")?, aliases: v.get("aliases").and_then(|a| a.as_bool()).unwrap_or(false) },
            "gmcp" => Self::Gmcp { package: s("package")?, data: s("data") },
            "receive" => Self::Receive(s("text")?),
            "display" => Self::Display(s("text")?),
            "close" => Self::Close,
            _ => return None,
        })
    }
}

/// An event for the page, delivered to its `SetOn...` callbacks.
pub enum PageEvent<'a> {
    Gmcp { package: &'a str, data: &'a str },
    Receive(&'a str),
    Send(&'a str),
    Display(&'a str),
}

pub struct WebPane {
    pub id: String,
    pub view: webkit6::WebView,
    /// Set when the web view has its own window rather than a tab in the side dock.
    pub window: Option<adw::Window>,
}

impl WebPane {
    pub fn new(open: &WebViewOpen, on_message: impl Fn(BridgeMessage) + 'static) -> Self {
        let content = webkit6::UserContentManager::new();
        content.add_script(&webkit6::UserScript::new(
            BRIDGE_JS,
            webkit6::UserContentInjectedFrames::TopFrame,
            webkit6::UserScriptInjectionTime::Start,
            &[],
            &[],
        ));
        content.register_script_message_handler("mu", None);
        content.connect_script_message_received(Some("mu"), move |_, value| {
            if let Some(message) = value.to_json(0).and_then(|json| BridgeMessage::parse(&json)) {
                on_message(message);
            }
        });
        let view = webkit6::WebView::builder().user_content_manager(&content).vexpand(true).hexpand(true).build();
        let pane = Self { id: open.id.clone(), view, window: None };
        pane.load(open);
        pane
    }

    /// Loads the URL (with any extra HTTP headers) or HTML source.
    pub fn load(&self, open: &WebViewOpen) {
        if let Some(url) = &open.url {
            if open.headers.is_empty() {
                self.view.load_uri(url);
            } else {
                let request = webkit6::URIRequest::new(url);
                if let Some(headers) = request.http_headers() {
                    for (name, value) in &open.headers {
                        headers.append(name, value);
                    }
                }
                self.view.load_request(&request);
            }
        } else if let Some(source) = &open.source {
            self.view.load_html(source, None);
        }
    }

    /// Sends an event to the page's callbacks.
    pub fn deliver(&self, event: PageEvent) {
        let json = match event {
            PageEvent::Gmcp { package, data } => serde_json::json!({ "type": "gmcp", "package": package, "data": data }),
            PageEvent::Receive(text) => serde_json::json!({ "type": "receive", "text": text }),
            PageEvent::Send(text) => serde_json::json!({ "type": "send", "text": text }),
            PageEvent::Display(text) => serde_json::json!({ "type": "display", "text": text }),
        };
        let script = format!("window.mu && window.mu._dispatch({json});");
        self.view.evaluate_javascript(&script, None, None, None::<&gio::Cancellable>, |_| {});
    }
}

#[cfg(test)]
mod tests {
    use super::BridgeMessage;

    #[test]
    fn parses_bridge_messages() {
        assert_eq!(
            BridgeMessage::parse(r#"{"op":"send","text":"look","aliases":true}"#),
            Some(BridgeMessage::Send { text: "look".into(), aliases: true })
        );
        assert_eq!(
            BridgeMessage::parse(r#"{"op":"gmcp","package":"Core.Ping","data":null}"#),
            Some(BridgeMessage::Gmcp { package: "Core.Ping".into(), data: None })
        );
        assert_eq!(BridgeMessage::parse(r#"{"op":"close"}"#), Some(BridgeMessage::Close));
        assert_eq!(BridgeMessage::parse(r#"{"op":"rm -rf"}"#), None);
        assert_eq!(BridgeMessage::parse("not json"), None);
    }
}
