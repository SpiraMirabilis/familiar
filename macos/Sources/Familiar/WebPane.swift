import AppKit
import SwiftUI
import WebKit

/// A web view the server or the user opened (GMCP WebView, `/webview`), docked beside the
/// output or in a window of its own. Pages talk to the client through `window.mu` (see
/// crates/gtk/src/webview_bridge.js, which both clients inject).
final class WebPane: NSObject, WKScriptMessageHandler, WKUIDelegate, NSWindowDelegate {
    let view: WKWebView
    let docked: Bool
    private var window: NSWindow?
    private let onMessage: (JSON) -> Void
    private var onWindowClosed: (() -> Void)?
    private let size: NSSize

    init(open: JSON, onMessage: @escaping (JSON) -> Void) {
        self.onMessage = onMessage
        docked = !open["dock"].isNull
        size = NSSize(width: open["width"].optionalInt ?? 900, height: open["height"].optionalInt ?? 700)
        let configuration = WKWebViewConfiguration()
        let script = AppState.shared.start["webview_bridge_js"].string
        configuration.userContentController.addUserScript(WKUserScript(source: script, injectionTime: .atDocumentStart, forMainFrameOnly: true))
        view = WKWebView(frame: NSRect(origin: .zero, size: size), configuration: configuration)
        super.init()
        // Held weakly, so the pane can go away while the page still exists.
        configuration.userContentController.add(WeakHandler(self), name: "mu")
        view.uiDelegate = self
        load(open)
    }

    /// Loads the URL (with any extra HTTP headers) or the HTML the server sent.
    func load(_ open: JSON) {
        if let address = open["url"].optionalString, let url = URL(string: address) {
            var request = URLRequest(url: url)
            for header in open["headers"].array {
                request.setValue(header[1].string, forHTTPHeaderField: header[0].string)
            }
            view.load(request)
        } else if let source = open["source"].optionalString {
            view.loadHTMLString(source, baseURL: nil)
        }
    }

    /// Sends an event (gmcp, receive, send, display) to the page's callbacks.
    func deliver(_ event: JSON) {
        view.evaluateJavaScript("window.mu && window.mu._dispatch(\(event.text));", completionHandler: nil)
    }

    func userContentController(_ controller: WKUserContentController, didReceive message: WKScriptMessage) {
        guard message.frameInfo.isMainFrame,
              let data = try? JSONSerialization.data(withJSONObject: message.body, options: [.fragmentsAllowed]),
              let json = try? JSON(text: String(decoding: data, as: UTF8.self))
        else { return }
        onMessage(json)
    }

    // Links that ask for a new window open in the browser.
    func webView(_ webView: WKWebView, createWebViewWith configuration: WKWebViewConfiguration, for action: WKNavigationAction, windowFeatures: WKWindowFeatures) -> WKWebView? {
        if let url = action.request.url { NSWorkspace.shared.open(url) }
        return nil
    }

    func openWindow(title: String, beside parent: NSWindow?, onClose: @escaping () -> Void) {
        let window = NSWindow(contentRect: NSRect(origin: .zero, size: size), styleMask: [.titled, .closable, .resizable, .miniaturizable], backing: .buffered, defer: false)
        window.title = title
        window.contentView = view
        window.isReleasedWhenClosed = false
        window.tabbingMode = .disallowed
        window.delegate = self
        window.center()
        onWindowClosed = onClose
        self.window = window
        window.makeKeyAndOrderFront(nil)
    }

    func present() { window?.makeKeyAndOrderFront(nil) }

    func closeWindow() {
        onWindowClosed = nil
        window?.close()
        window = nil
    }

    func windowWillClose(_ notification: Notification) {
        onWindowClosed?()
        onWindowClosed = nil
    }
}

private final class WeakHandler: NSObject, WKScriptMessageHandler {
    weak var target: WKScriptMessageHandler?
    init(_ target: WKScriptMessageHandler) { self.target = target }

    func userContentController(_ controller: WKUserContentController, didReceive message: WKScriptMessage) {
        target?.userContentController(controller, didReceive: message)
    }
}

/// A docked web view in the side dock.
struct WebPaneView: NSViewRepresentable {
    let pane: WebPane

    func makeNSView(context: Context) -> WKWebView { pane.view }
    func updateNSView(_ view: WKWebView, context: Context) {}
}
