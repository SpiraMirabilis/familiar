import AppKit
import SwiftUI

/// A tab in a session's side dock.
enum SidePanel: Hashable, Identifiable {
    case map
    case stats(String)
    case spawn(String)
    case tilemap(String)
    case web(String)
    /// An LLM agent's terminal, by the agent's name.
    case agent(String)

    var id: String {
        switch self {
        case .map: return "map"
        case .stats(let name): return "stats:\(name)"
        case .spawn(let name): return "spawn:\(name)"
        case .tilemap(let name): return "tilemap:\(name)"
        case .web(let id): return "web:\(id)"
        case .agent: return "agent"
        }
    }

    var title: String {
        switch self {
        case .map: return "Map"
        case .stats(let name), .spawn(let name), .tilemap(let name), .web(let name): return name
        case .agent(let name): return "✦ \(name)"
        }
    }
}

enum ConnectionState: Equatable {
    case connecting
    case connected
    case disconnected(String?)
}

/// One connection: the engine's session, what it has shown, and the windows that belong to it.
final class Session: ObservableObject {
    let id: Int
    @Published private(set) var info: JSON
    @Published private(set) var state = ConnectionState.disconnected(nil)
    /// MCP status text, shown under the output.
    @Published private(set) var statusText = ""
    @Published private(set) var logging = false
    @Published private(set) var panels: [SidePanel] = []
    @Published var selectedPanel: SidePanel?
    /// The user hid the side dock (it shows again when something asks to be seen).
    @Published private(set) var dockHidden = false
    @Published private(set) var stats: [String: JSON] = [:]
    @Published private(set) var tilemaps: [String: JSON] = [:]
    /// Bumped whenever the map or the current room changes, so the map view redraws.
    @Published private(set) var mapRevision = 0
    /// New lines since this session's tab was last looked at.
    @Published private(set) var unread = false
    @Published private(set) var important = false

    let output: OutputBuffer
    private(set) var spawns: [String: OutputBuffer] = [:]
    private(set) var webPanes: [String: WebPane] = [:]
    /// The LLM agent working on this world, if one is.
    private(set) var agent: AgentPane?
    private var puppets: [String: PuppetWindowController] = [:]
    private var editors: [String: CodeEditorController] = [:]
    /// The last program sent back from an editor, and the first compile error the server
    /// reported for it, so it can be reopened there.
    private var lastEdit: (request: JSON, error: (line: Int, message: String)?)?
    private let sounds = SoundPlayer()
    private var mapSaver: Timer?
    private var closed = false

    weak var outputView: OutputScrollView?
    weak var window: NSWindow?
    /// Tells the window to show activity on its tab.
    var onActivity: (() -> Void)?
    /// Opens a window as a tab beside this session's.
    var onOpenTab: ((NSWindow) -> Void)?

    lazy var input: InputBox = {
        let settings = AppState.shared.settings
        let box = InputBox(font: font, historySize: settings["input_history_size"].int, spellcheck: settings["spellcheck"].bool)
        box.onSubmit = { [weak self] text, secret in self?.submit(text, secret: secret) }
        box.onKey = { [weak self] event in self?.runMacro(event) ?? false }
        box.onPage = { [weak self] up in self?.outputView?.scrollPage(up: up) }
        return box
    }()

    var title: String { info["title"].string }
    var worldName: String { info["world"]["name"].string }
    var font: NSFont { AppState.shared.font(forWorld: worldName) }
    var isConnected: Bool { state == .connected || state == .connecting }

    init(world: JSON, character: String?) throws {
        // This app can run agents (/agent) in a terminal.
        let opened = try Core.call("session.open", ["world": world, "character": .of(character), "agents": true])
        id = opened["id"].int
        info = opened
        let app = AppState.shared
        output = OutputBuffer(font: app.font(forWorld: world["name"].string), maxLines: app.settings["scrollback_lines"].int)
        app.register(self)
        apply(opened["outputs"].array)
        if info["has_map"].bool { showPanel(.map, select: false) }
        // The map is also saved when the session closes.
        mapSaver = Timer.scheduledTimer(withTimeInterval: 30, repeats: true) { [weak self] _ in self?.saveMap() }
    }

    deinit { mapSaver?.invalidate() }

    // MARK: Talking to the engine

    private func call(_ method: String, _ args: JSON = [:]) -> JSON {
        guard !closed else { return .null }
        var args = args
        args["id"] = .int(id)
        let reply = Core.send(method, args)
        apply(reply["outputs"].array)
        return reply
    }

    /// Collects what the engine has for us: network traffic and timers.
    func poll() {
        let reply = call("session.poll")
        if reply["more"].bool {
            // Let the window redraw between batches of a flood.
            DispatchQueue.main.async { [weak self] in self?.poll() }
        }
    }

    func connect() { _ = call("session.connect") }
    func disconnect() { _ = call("session.disconnect") }
    func toggleLog() { _ = call("session.toggle_log") }
    func followLink(_ uri: String) { _ = call("session.link", ["uri": .of(uri)]) }
    func restartScripts() { _ = call("session.restart_scripts") }

    func submit(_ text: String, secret: Bool = false) {
        outputView?.scrollToEnd()
        _ = call("session.submit", ["text": .of(text), "secret": .of(secret)])
    }

    func runMacro(_ event: NSEvent) -> Bool {
        guard let chord = Keys.chord(event) else { return false }
        let handled = call("session.macro", chord)["handled"].bool
        if handled { outputView?.scrollToEnd() }
        return handled
    }

    func resize(cols: Int, rows: Int) {
        _ = call("session.resize", ["cols": .of(cols), "rows": .of(rows)])
    }

    func saveEdit(_ request: JSON, content: String) {
        var sent = request
        sent["content"] = .of(content)
        lastEdit = (sent, nil)
        _ = call("session.save_edit", ["request": request, "content": .of(content)])
    }

    func puppetSubmit(_ name: String, _ text: String) {
        _ = call("session.puppet_submit", ["name": .of(name), "text": .of(text)])
    }

    func saveMap() {
        guard !closed else { return }
        do {
            _ = try Core.call("map.save", ["id": .int(id)])
        } catch {
            output.notice("Couldn’t save the map: \(error.localizedDescription)")
        }
    }

    /// The map as the engine has it now: rooms, areas and the current room.
    func map() -> JSON { closed ? .null : Core.send("map.get", ["id": .int(id)]) }

    /// Runs a map method ("map.walk", "map.move", ...) on this session.
    @discardableResult
    func mapCall(_ method: String, _ args: JSON = [:]) -> JSON { call(method, args) }

    func clearOutput() { output.clear() }

    /// The config changed: fonts and settings may have.
    func configChanged() {
        guard !closed else { return }
        info = Core.send("session.info", ["id": .int(id)])
        let settings = AppState.shared.settings
        let font = self.font
        output.maxLines = settings["scrollback_lines"].int
        output.font = font
        spawns.values.forEach { $0.font = font }
        input.font = font
        input.spellcheck = settings["spellcheck"].bool
        input.historySize = settings["input_history_size"].int
        outputView?.fontChanged()
        puppets.values.forEach { $0.setFont(font) }
        onTitleChange?()
    }

    var onTitleChange: (() -> Void)?

    /// Closes the connection and everything that belongs to it.
    func close() {
        guard !closed else { return }
        sounds.stop(nil)
        puppets.values.forEach { $0.close() }
        puppets = [:]
        webPanes.values.forEach { $0.closeWindow() }
        webPanes = [:]
        if let agent { MainActor.assumeIsolated { agent.stop() } }
        agent = nil
        editors.values.forEach { $0.close() }
        editors = [:]
        do {
            _ = try Core.call("session.close", ["id": .int(id)])
        } catch {
            NSLog("Familiar: closing a session: %@", error.localizedDescription)
        }
        closed = true
        AppState.shared.unregister(self)
    }

    /// The user is looking at this session's tab.
    func seen() {
        unread = false
        important = false
    }

    // MARK: The side dock

    func showPanel(_ panel: SidePanel, select: Bool) {
        if !panels.contains(panel) { panels.append(panel) }
        if select || selectedPanel == nil { selectedPanel = panel }
        if select { dockHidden = false }
    }

    func toggleDock() { dockHidden.toggle() }

    /// Selects the dock's next tab (`step` 1) or previous one (-1), going round, and shows the dock.
    func cyclePanel(by step: Int) {
        guard !panels.isEmpty else { return }
        let at = selectedPanel.flatMap { panels.firstIndex(of: $0) } ?? 0
        selectedPanel = panels[((at + step) % panels.count + panels.count) % panels.count]
        dockHidden = false
    }

    func removePanel(_ panel: SidePanel) {
        panels.removeAll { $0 == panel }
        if selectedPanel == panel { selectedPanel = panels.first }
    }

    private func spawn(_ name: String) -> OutputBuffer {
        if let buffer = spawns[name] { return buffer }
        let buffer = OutputBuffer(font: font, maxLines: AppState.shared.settings["scrollback_lines"].int)
        spawns[name] = buffer
        showPanel(.spawn(name), select: false)
        return buffer
    }

    // MARK: Showing what the engine says

    private func apply(_ outputs: [JSON]) {
        guard !outputs.isEmpty else { return }
        output.storage.beginEditing()
        defer { output.storage.endEditing() }
        for o in outputs {
            switch o["t"].string {
            case "line":
                if let target = o["target"].optionalString {
                    spawn(target).appendLine(o["line"])
                } else {
                    output.appendLine(o["line"])
                    if o["bell"].bool { NSSound.beep() }
                    if !webPanes.isEmpty { toPages(["type": "display", "text": o["plain"]]) }
                }
            case "prompt": output.setPrompt(o["line"], terminated: o["terminated"].bool)
            case "notice": output.notice(o["text"].string)
            case "echo": output.echo(o["text"].string)
            case "bell": NSSound.beep()
            case "password": input.setPasswordMode(o["on"].bool)
            case "status": setStatus(o["status"])
            case "activity": markActivity(important: o["important"].bool)
            case "notify": Notifier.shared.show(title: o["title"].string, body: o["body"].string)
            case "sound": playSound(o["path"].string)
            case "open_uri": open(uri: o["uri"].string)
            case "set_input":
                input.text = o["text"].string
                input.focus()
            case "clear": output.clear()
            case "logging": logging = !o["path"].isNull
            case "compile_error":
                editors[o["reference"].string]?.addError(line: o["line"].int, message: o["message"].string)
                if let last = lastEdit, last.error == nil { lastEdit = (last.request, (o["line"].int, o["message"].string)) }
            case "line_avatar": output.setNextAvatar(o["avatar"])
            case "avatar_updated": output.updateAvatar(o["avatar"])
            case "inline_image": output.appendImage(url: o["url"].string)
            case "open_puppet": openPuppet(o["name"].string, focus: o["focus"].bool)
            case "puppet_line": puppets[o["name"].string]?.appendLine(o["line"])
            case "puppet_echo": puppets[o["name"].string]?.echo(o["text"].string)
            case "speak": Speech.shared.say(o["text"].string)
            case "speech_stop": Speech.shared.stop()
            case "tilemap":
                let name = o["name"].string
                let new = tilemaps[name] == nil
                tilemaps[name] = o["map"]
                showPanel(.tilemap(name), select: new)
            case "map_changed":
                mapRevision += 1
                if !panels.contains(.map), map()["rooms"].array.isEmpty == false { showPanel(.map, select: false) }
            case "show_map":
                mapRevision += 1
                showPanel(.map, select: true)
            case "stats":
                let name = o["pane"].string
                if o["data"].isNull {
                    stats[name] = nil
                    removePanel(.stats(name))
                } else {
                    stats[name] = o["data"]
                    showPanel(.stats(name), select: false)
                }
            case "media": sounds.run(o["command"]) { [weak self] problem in self?.output.notice(problem) }
            case "status_text": statusText = o["text"].string
            case "edit": openEditor(o["request"])
            case "webview": webView(o["command"])
            case "gmcp": if !webPanes.isEmpty { toPages(["type": "gmcp", "package": o["package"], "data": o["data"]]) }
            case "received": if !webPanes.isEmpty { toPages(["type": "receive", "text": o["text"]]) }
            case "sent": if !webPanes.isEmpty { toPages(["type": "send", "text": o["text"]]) }
            case "config_changed": AppState.shared.reloadFromEngine()
            case "agent_start": startAgent(o)
            case "agent_focus":
                if let agent {
                    showPanel(.agent(agent.name), select: true)
                    MainActor.assumeIsolated { agent.focus() }
                }
            case "agent_stop": removeAgent(token: o["token"].string)
            default: NSLog("Familiar: an output this app doesn't know: %@", o.text)
            }
        }
    }

    private func setStatus(_ status: JSON) {
        switch status["state"].string {
        case "connecting": state = .connecting
        case "connected": state = .connected
        default:
            state = .disconnected(status["reason"].optionalString)
            saveMap()
        }
        onTitleChange?()
    }

    private func markActivity(important: Bool) {
        // The tab that's showing doesn't need marking.
        if let window, window.isKeyWindow || (window.isVisible && NSApp.isActive && window.isMainWindow) { return }
        unread = true
        if important { self.important = true }
        onActivity?()
    }

    private func open(uri: String) {
        if let url = URL(string: uri) {
            NSWorkspace.shared.open(url)
        }
    }

    private func playSound(_ path: String) {
        guard FileManager.default.fileExists(atPath: path) else {
            return output.notice("Sound file not found: \(path)")
        }
        sounds.play(name: (path as NSString).lastPathComponent, file: URL(fileURLWithPath: path), volume: 1, loops: 1) { [weak self] problem in
            self?.output.notice(problem)
        }
    }

    func stopSounds() {
        sounds.stop(nil)
        Speech.shared.stop()
    }

    // MARK: Puppets

    private func openPuppet(_ name: String, focus: Bool) {
        let controller: PuppetWindowController
        if let existing = puppets[name] {
            controller = existing
        } else {
            controller = PuppetWindowController(session: self, name: name)
            puppets[name] = controller
            if let window = controller.window { onOpenTab?(window) }
        }
        if focus { controller.focus() }
    }

    /// The user closed a puppet's tab.
    func puppetClosed(_ name: String) {
        puppets[name] = nil
        if !closed { Core.send("session.puppet_open", ["id": .int(id), "name": .of(name), "open": false]) }
    }

    // MARK: Editors

    private func openEditor(_ request: JSON) {
        let reference = request["reference"].string
        // The server sending the same program again reuses its editor.
        if request["kind"].string != "local", let existing = editors[reference] {
            existing.reload(request)
            return
        }
        newEditor(request).showWindow(nil)
    }

    private func newEditor(_ request: JSON) -> CodeEditorController {
        let reference = request["reference"].string
        let editor = CodeEditorController(session: self, request: request)
        editors[reference] = editor
        editor.onClose = { [weak self, weak editor] in
            if let self, self.editors[reference] === editor { self.editors[reference] = nil }
        }
        return editor
    }

    /// Opens the last program sent from an editor again, at its first compile error: its
    /// editor if that's still open, otherwise a new one with what was sent.
    func reopenEditor() {
        guard let (request, error) = lastEdit else {
            return output.notice("Nothing to reopen: no program has been edited here yet")
        }
        if request["kind"].string != "local", let existing = editors[request["reference"].string] {
            return existing.reveal(line: error?.line)
        }
        let editor = newEditor(request)
        editor.showWindow(nil)
        if let error { editor.addError(line: error.line, message: error.message) }
    }

    // MARK: LLM agents

    // Sessions live on the main thread, as SwiftTerm's views must.
    private func startAgent(_ start: JSON) {
        let token = start["token"].string
        let font = self.font
        let pane = MainActor.assumeIsolated {
            AgentPane(start: start, font: font) { [weak self] error in
                guard let self, !self.closed else { return }
                // The program ended (or never started): its tab goes, and the engine closes its endpoint.
                self.removeAgent(token: token)
                _ = self.call("agent.ended", ["token": .of(token), "error": .of(error)])
            }
        }
        agent = pane
        showPanel(.agent(pane.name), select: true)
        MainActor.assumeIsolated { pane.focus() }
    }

    private func removeAgent(token: String) {
        guard let agent, agent.token == token else { return }
        MainActor.assumeIsolated { agent.stop() }
        self.agent = nil
        removePanel(.agent(agent.name))
    }

    // MARK: Web views

    private func toPages(_ event: JSON) {
        webPanes.values.forEach { $0.deliver(event) }
    }

    private func webView(_ command: JSON) {
        let id = command["id"].string
        if command["op"].string == "close" { return closeWebView(id) }
        guard command["from_server"].bool else { return openWebView(command) }
        switch AppState.shared.world(named: worldName)?["allow_webviews"] {
        case .some(.bool(true)): openWebView(command)
        case .some(.bool(false)): output.notice("Blocked a web view from the server (web views are off for this world).")
        // Not asked yet, or a quick-connect world that can't remember the answer.
        case .some: askWebView(command, canRemember: true)
        case .none: askWebView(command, canRemember: false)
        }
    }

    private func askWebView(_ command: JSON, canRemember: Bool) {
        let alert = NSAlert()
        let target = command["url"].optionalString ?? "a page it sent"
        alert.messageText = "Open a Web View?"
        alert.informativeText = "\(title) wants to open \(target) inside Familiar. Pages it opens can send commands and read the output, like a script."
        alert.addButton(withTitle: "Allow Once")
        alert.addButton(withTitle: "Block")
        if canRemember { alert.addButton(withTitle: "Always Allow") }
        let answer: (NSApplication.ModalResponse) -> Void = { [weak self] response in
            guard let self, !self.closed else { return }
            switch response {
            case .alertFirstButtonReturn: self.openWebView(command)
            case .alertThirdButtonReturn:
                let world = self.worldName
                AppState.shared.update { config in
                    var worlds = config["world"].array
                    if let i = worlds.firstIndex(where: { $0["name"].string == world }) {
                        worlds[i]["allow_webviews"] = true
                        config["world"] = .array(worlds)
                    }
                }
                self.openWebView(command)
            default: self.output.notice("Blocked a web view from the server.")
            }
        }
        if let window {
            alert.beginSheetModal(for: window, completionHandler: answer)
        } else {
            answer(alert.runModal())
        }
    }

    private func openWebView(_ command: JSON) {
        let id = command["id"].string
        if let pane = webPanes[id] {
            pane.load(command)
            if pane.docked { selectedPanel = .web(id) } else { pane.present() }
            return
        }
        let pane = WebPane(open: command) { [weak self] message in self?.pageMessage(id, message) }
        webPanes[id] = pane
        if pane.docked {
            showPanel(.web(id), select: true)
        } else {
            pane.openWindow(title: id, beside: window) { [weak self] in self?.webPanes[id] = nil }
        }
    }

    func closeWebView(_ id: String) {
        guard let pane = webPanes.removeValue(forKey: id) else { return }
        if pane.docked { removePanel(.web(id)) } else { pane.closeWindow() }
    }

    /// A request from a page's script (see webview_bridge.js).
    private func pageMessage(_ id: String, _ message: JSON) {
        let text = message["text"].string
        switch message["op"].string {
        case "send": _ = call("session.send_text", ["text": .of(text), "aliases": message["aliases"]])
        case "gmcp": _ = call("session.gmcp", ["package": message["package"], "data": message["data"]])
        case "receive": _ = call("session.inject", ["text": .of(text)])
        case "display":
            if let line = try? Core.callAnywhere("ansi.parse", ["text": .of(text)]) { output.appendLine(line) }
        case "close": closeWebView(id)
        default: break
        }
    }
}
