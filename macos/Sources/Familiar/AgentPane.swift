import AppKit
import SwiftTerm
import SwiftUI

/// An LLM agent (`/agent`: Claude Code, Codex, ...) running in a terminal in the side dock. The
/// engine prepares it and serves it the world's tools; this runs its program and says when it
/// ends.
final class AgentPane: NSObject, LocalProcessTerminalViewDelegate {
    let name: String
    let token: String
    let view: LocalProcessTerminalView
    /// Called once, when the program ends or can't start (with why).
    private var onEnd: ((String?) -> Void)?

    /// `start` is the engine's agent_start: program, args, dir, env, name and token.
    init(start: JSON, font: NSFont, onEnd: @escaping (String?) -> Void) {
        name = start["name"].string
        token = start["token"].string
        self.onEnd = onEnd
        view = LocalProcessTerminalView(frame: NSRect(x: 0, y: 0, width: 360, height: 400))
        super.init()
        view.font = font.isFixedPitch ? font : NSFont.monospacedSystemFont(ofSize: font.pointSize, weight: .regular)
        view.processDelegate = self
        view.startProcess(
            executable: start["program"].string,
            args: start["args"].array.map(\.string),
            environment: Self.environment(adding: start["env"]),
            currentDirectory: start["dir"].string
        )
    }

    /// The app's environment, with the agent's variables and what a terminal program expects.
    /// (SwiftTerm passes only what it's given.)
    private static func environment(adding extra: JSON) -> [String] {
        var env = ProcessInfo.processInfo.environment
        env["TERM"] = "xterm-256color"
        env["COLORTERM"] = "truecolor"
        // Apps opened from the Finder have no locale variables, and agents print UTF-8.
        if env["LANG"] == nil && env["LC_ALL"] == nil && env["LC_CTYPE"] == nil { env["LANG"] = "en_US.UTF-8" }
        for (key, value) in extra.object { env[key] = value.string }
        return env.map { "\($0.key)=\($0.value)" }
    }

    func focus() {
        // Once it's in a window: a new pane isn't yet.
        DispatchQueue.main.async { [weak self] in
            guard let view = self?.view else { return }
            view.window?.makeFirstResponder(view)
        }
    }

    /// Stops the program, without calling `onEnd`.
    func stop() {
        onEnd = nil
        view.terminate()
    }

    private func ended(_ error: String?) {
        let onEnd = self.onEnd
        self.onEnd = nil
        onEnd?(error)
    }

    // MARK: LocalProcessTerminalViewDelegate (on the main queue)

    func processTerminated(source: TerminalView, exitCode: Int32?) { ended(nil) }
    func processFailedToStart(source: TerminalView, error: LocalProcessError) { ended("\(error)") }
    func sizeChanged(source: LocalProcessTerminalView, newCols: Int, newRows: Int) {}
    func setTerminalTitle(source: LocalProcessTerminalView, title: String) {}
    func hostCurrentDirectoryUpdate(source: TerminalView, directory: String?) {}
}

/// The agent's terminal in the side dock.
struct AgentPaneView: NSViewRepresentable {
    let pane: AgentPane

    func makeNSView(context: Context) -> LocalProcessTerminalView { pane.view }
    func updateNSView(_ view: LocalProcessTerminalView, context: Context) {}
}
