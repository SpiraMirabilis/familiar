import AppKit
import SwiftUI
import UniformTypeIdentifiers

@main
final class AppDelegate: NSObject, NSApplicationDelegate, NSMenuItemValidation, NSMenuDelegate {
    private var connectWindow: NSWindow?
    private var preferencesWindow: NSWindow?

    static func main() {
        let app = NSApplication.shared
        let delegate = AppDelegate()
        app.delegate = delegate
        app.setActivationPolicy(.regular)
        app.run()
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSWindow.allowsAutomaticWindowTabbing = true
        AppState.shared.launch()
        Shortcuts.shared.load()
        NSApp.mainMenu = buildMenu()
        showKeys()
        NotificationCenter.default.addObserver(self, selector: #selector(showKeys), name: Shortcuts.changed, object: nil)
        NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            self?.runShortcut(event) == true ? nil : event
        }
        for message in AppState.shared.start["migrated"].array {
            NSLog("Familiar: %@", message.string)
        }
        NSApp.activate(ignoringOtherApps: true)
        #if DEBUG
        if runDebugScript() { return }
        #endif
        // `Familiar WORLD [CHARACTER]` connects to a saved world, as the GTK client does.
        // (macOS adds arguments of its own, which start with a dash and come in pairs.)
        let arguments = Array(CommandLine.arguments.dropFirst().prefix { !$0.hasPrefix("-") })
        if let name = arguments.first {
            if let world = AppState.shared.world(named: name) {
                return openSession(world: world, character: arguments.dropFirst().first)
            }
            AppState.shared.alert("There’s no saved world called \(name)", "Choose a world to connect to instead.")
        }
        showConnect(nil)
    }

    #if DEBUG
    /// For checking the app without a person at it (debug builds only): connects to
    /// FAMILIAR_TEST_CONNECT ("host:port"), types FAMILIAR_TEST_INPUT (one command per line),
    /// then saves a picture of every window and the output's text into the folder
    /// FAMILIAR_TEST_SNAPSHOT and quits.
    private func runDebugScript() -> Bool {
        let env = ProcessInfo.processInfo.environment
        guard let folder = env["FAMILIAR_TEST_SNAPSHOT"] else { return false }
        if let address = env["FAMILIAR_TEST_CONNECT"], let world = try? Core.callAnywhere("address.parse", ["text": .of(address)]), !world.isNull {
            openSession(world: world, character: nil)
        } else {
            showConnect(nil)
        }
        if env["FAMILIAR_TEST_SETTINGS"] != nil { showPreferences(nil) }
        if env["FAMILIAR_TEST_RULES"] != nil { showRules(nil) }
        // With FAMILIAR_TEST_RULES: adds the example rules, as the button in that window does.
        if env["FAMILIAR_TEST_EXAMPLES"] != nil {
            DispatchQueue.main.asyncAfter(deadline: .now() + 1) { RulesWindow.addExamplesForTest() }
        }
        let commands = (env["FAMILIAR_TEST_INPUT"] ?? "").split(separator: "\n").map(String.init)
        for (i, command) in commands.enumerated() {
            DispatchQueue.main.asyncAfter(deadline: .now() + 1.5 + Double(i) * 0.4) { [weak self] in self?.current?.submit(command) }
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 2.5 + Double(commands.count) * 0.4) {
            let out = URL(fileURLWithPath: folder)
            try? FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
            for (i, window) in NSApp.windows.enumerated() where window.isVisible {
                guard let view = window.contentView?.superview ?? window.contentView, let bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { continue }
                view.cacheDisplay(in: view.bounds, to: bitmap)
                let name = "\(i)-\(window.title.isEmpty ? "window" : window.title).png".replacingOccurrences(of: "/", with: "_").replacingOccurrences(of: ":", with: "_")
                try? bitmap.representation(using: .png, properties: [:])?.write(to: out.appendingPathComponent(name))
            }
            if let session = SessionWindowController.all.first?.session {
                let panels = session.panels.map(\.id).joined(separator: ", ")
                let report = "title: \(session.title)\nstate: \(session.state)\npanels: \(panels)\n---\n\(session.output.storage.string)\n"
                try? report.write(to: out.appendingPathComponent("output.txt"), atomically: true, encoding: .utf8)
            }
            SessionWindowController.all.forEach { $0.session.close() }
            exit(0)
        }
        return true
    }
    #endif

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows visible: Bool) -> Bool {
        if !visible { showConnect(nil) }
        return true
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        // Unsaved triggers and aliases get a chance to be saved.
        if !RulesWindow.canQuit() { return .terminateCancel }
        let connected = SessionWindowController.all.filter { $0.session.state == .connected }
        guard !connected.isEmpty else { return .terminateNow }
        let alert = NSAlert()
        alert.messageText = connected.count == 1 ? "Disconnect from \(connected[0].session.title) and quit?" : "Disconnect from \(connected.count) worlds and quit?"
        alert.addButton(withTitle: "Quit")
        alert.addButton(withTitle: "Cancel")
        return alert.runModal() == .alertFirstButtonReturn ? .terminateNow : .terminateCancel
    }

    func applicationWillTerminate(_ notification: Notification) {
        // Saves each world's map.
        SessionWindowController.all.forEach { $0.session.close() }
    }

    // MARK: Opening things

    private var current: Session? { SessionWindowController.current?.session }

    func openSession(world: JSON, character: String?) {
        do {
            let session = try Session(world: world, character: character)
            SessionWindowController(session: session).present()
            session.connect()
        } catch {
            AppState.shared.alert("Couldn’t open \(world["name"].string)", error.localizedDescription)
        }
    }

    private func window(_ title: String, size: NSSize, autosave: String, _ view: some View) -> NSWindow {
        let window = NSWindow(contentRect: NSRect(origin: .zero, size: size), styleMask: [.titled, .closable, .resizable, .miniaturizable], backing: .buffered, defer: false)
        window.title = title
        window.isReleasedWhenClosed = false
        window.tabbingMode = .disallowed
        window.contentView = NSHostingView(rootView: view)
        window.center()
        window.setFrameAutosaveName(autosave)
        return window
    }

    @objc func showConnect(_ sender: Any?) {
        if connectWindow == nil {
            connectWindow = window("Connect", size: NSSize(width: 580, height: 520), autosave: "FamiliarConnect", ConnectView { [weak self] world, character in
                self?.connectWindow?.close()
                self?.openSession(world: world, character: character)
            })
        }
        connectWindow?.makeKeyAndOrderFront(nil)
    }

    @objc func showPreferences(_ sender: Any?) {
        if preferencesWindow == nil {
            let window = window("Settings", size: NSSize(width: 560, height: 620), autosave: "FamiliarSettings", PreferencesView())
            window.styleMask.remove(.resizable)
            preferencesWindow = window
        }
        preferencesWindow?.makeKeyAndOrderFront(nil)
    }

    /// Settings, on its Keyboard tab.
    @objc func showKeyboard(_ sender: Any?) {
        SettingsPage.shared.tab = "keyboard"
        showPreferences(sender)
    }

    @objc func showRules(_ sender: Any?) {
        RulesWindow.show(scopes: current?.info["scopes"].array ?? [["kind": "global", "label": "All worlds"]])
    }

    @objc func reconnect(_ sender: Any?) { current?.connect() }
    @objc func disconnect(_ sender: Any?) { current?.disconnect() }
    @objc func clearOutput(_ sender: Any?) { current?.clearOutput() }
    @objc func toggleLog(_ sender: Any?) { current?.toggleLog() }
    @objc func stopSounds(_ sender: Any?) { current?.stopSounds() }
    @objc func restartScripts(_ sender: Any?) { current?.restartScripts() }
    @objc func showMap(_ sender: Any?) { current?.showPanel(.map, select: true) }
    @objc func reopenEditor(_ sender: Any?) { current?.reopenEditor() }
    @objc func quit(_ sender: Any?) { NSApp.terminate(sender) }

    @objc func editWorld(_ sender: Any?) {
        guard let session = current, let world = AppState.shared.world(named: session.worldName), let parent = session.window else { return }
        showWorldEditor(WorldDraft(original: session.worldName, world: world), over: parent)
    }

    /// The world editor for a new world, as the Connect window's New World button opens it.
    @objc func newWorld(_ sender: Any?) {
        if current?.window == nil { showConnect(sender) }
        guard let parent = current?.window ?? connectWindow else { return }
        showWorldEditor(WorldDraft(original: nil, world: AppState.shared.defaults["world"]), over: parent)
    }

    private func showWorldEditor(_ draft: WorldDraft, over parent: NSWindow) {
        guard parent.attachedSheet == nil else { return }
        let sheet = NSWindow(contentRect: .zero, styleMask: [.titled], backing: .buffered, defer: false)
        sheet.contentView = NSHostingView(rootView: WorldEditor(draft: draft).environment(\.dismiss_, { [weak parent, weak sheet] in
            if let sheet { parent?.endSheet(sheet) }
        }))
        sheet.setContentSize(NSSize(width: 560, height: 560))
        parent.beginSheet(sheet)
    }

    @objc func importBeipMU(_ sender: Any?) {
        let panel = NSOpenPanel()
        panel.title = "Import BeipMU Configuration"
        panel.message = "Choose BeipMU’s Config.txt"
        panel.allowedContentTypes = [.plainText]
        panel.allowsOtherFileTypes = true
        guard panel.runModal() == .OK, let url = panel.url else { return }
        do {
            let result = try Core.call("import.beipmu", ["path": .of(url.path)])
            AppState.shared.reloadFromEngine()
            SessionWindowController.all.forEach { $0.session.configChanged() }
            let alert = NSAlert()
            alert.messageText = result["heading"].string
            alert.informativeText = result["body"].string
            let warnings = result["warnings"].array.map(\.string)
            if !warnings.isEmpty {
                let scroll = NSTextView.scrollableTextView()
                scroll.frame = NSRect(x: 0, y: 0, width: 420, height: 180)
                if let text = scroll.documentView as? NSTextView {
                    text.isEditable = false
                    text.string = "Not carried over or changed (\(warnings.count)):\n\n" + warnings.map { "• \($0)" }.joined(separator: "\n")
                    text.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
                }
                alert.accessoryView = scroll
            }
            alert.runModal()
        } catch {
            AppState.shared.alert("The import failed", error.localizedDescription)
        }
    }

    @objc func importMap(_ sender: Any?) {
        guard let session = current else { return }
        let panel = NSOpenPanel()
        panel.title = "Import a BeipMU Map"
        panel.allowedContentTypes = [UTType(filenameExtension: "atlas") ?? .data]
        panel.allowsOtherFileTypes = true
        guard panel.runModal() == .OK, let url = panel.url else { return }
        session.mapCall("map.import", ["path": .of(url.path)])
    }

    @objc func openSettingsFolder(_ sender: Any?) {
        let folder = URL(fileURLWithPath: AppState.shared.start["config_dir"].string)
        try? FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        NSWorkspace.shared.open(folder)
    }

    @objc func openLogs(_ sender: Any?) {
        let folder = URL(fileURLWithPath: AppState.shared.start["log_dir"].string)
        try? FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        NSWorkspace.shared.open(folder)
    }

    @objc func openHelp(_ sender: Any?) {
        if let url = URL(string: "https://github.com/SpiraMirabilis/familiar#readme") { NSWorkspace.shared.open(url) }
    }

    @objc func showAbout(_ sender: Any?) {
        NSApp.orderFrontStandardAboutPanel(options: [
            .applicationName: "Familiar",
            .applicationVersion: AppState.shared.start["version"].string,
            .version: "",
            .credits: NSAttributedString(
                string: "A client for MUDs, MUSHes, MUCKs and MOOs.\nInspired by BeipMU.",
                attributes: [.font: NSFont.systemFont(ofSize: NSFont.smallSystemFontSize), .foregroundColor: NSColor.labelColor]
            ),
        ])
    }

    // MARK: Shortcuts

    /// What each of the keymap's actions does here. Tab 1 to 9 are `selectTab`.
    private static let actions: [String: Selector] = [
        "connect": #selector(AppDelegate.showConnect(_:)), "new-world": #selector(AppDelegate.newWorld(_:)),
        "edit-world": #selector(AppDelegate.editWorld(_:)), "reconnect": #selector(AppDelegate.reconnect(_:)),
        "disconnect": #selector(AppDelegate.disconnect(_:)), "close-tab": #selector(AppDelegate.closeTab(_:)),
        "next-tab": #selector(AppDelegate.nextTab(_:)), "prev-tab": #selector(AppDelegate.previousTab(_:)),
        "find": #selector(AppDelegate.find(_:)), "find-next": #selector(AppDelegate.findNext(_:)),
        "find-previous": #selector(AppDelegate.findPrevious(_:)), "clear": #selector(AppDelegate.clearOutput(_:)),
        "toggle-log": #selector(AppDelegate.toggleLog(_:)), "open-logs": #selector(AppDelegate.openLogs(_:)),
        "toggle-sidebar": #selector(AppDelegate.toggleDock(_:)), "next-panel": #selector(AppDelegate.nextPanel(_:)),
        "prev-panel": #selector(AppDelegate.previousPanel(_:)), "show-map": #selector(AppDelegate.showMap(_:)),
        "palette": #selector(AppDelegate.showPalette(_:)), "history-search": #selector(AppDelegate.searchHistory(_:)),
        "rules": #selector(AppDelegate.showRules(_:)), "reopen-editor": #selector(AppDelegate.reopenEditor(_:)),
        "restart-scripts": #selector(AppDelegate.restartScripts(_:)), "stop-speech": #selector(AppDelegate.stopSounds(_:)),
        "import-beipmu": #selector(AppDelegate.importBeipMU(_:)), "import-map": #selector(AppDelegate.importMap(_:)),
        "preferences": #selector(AppDelegate.showPreferences(_:)), "keyboard": #selector(AppDelegate.showKeyboard(_:)),
        "open-settings-folder": #selector(AppDelegate.openSettingsFolder(_:)), "help": #selector(AppDelegate.openHelp(_:)),
        "about": #selector(AppDelegate.showAbout(_:)), "quit": #selector(AppDelegate.quit(_:)),
    ]

    /// Runs an action by its id, if it can run now.
    func perform(_ id: String) {
        if id.hasPrefix("tab-"), let n = Int(id.dropFirst(4)) { return selectTab(n - 1) }
        guard let action = AppDelegate.actions[id], isEnabled(action) else { return }
        NSApp.sendAction(action, to: self, from: nil)
    }

    /// Every key press comes here first, before the menus and the focused view: a shortcut's
    /// key runs the action, and the key goes no further (so a macro on it doesn't run).
    private func runShortcut(_ event: NSEvent) -> Bool {
        // Not while a dialog or sheet has the keys, a key is being recorded, or an input
        // method is composing a character.
        guard NSApp.modalWindow == nil, let window = NSApp.keyWindow, !window.isSheet, window.attachedSheet == nil else { return false }
        let responder = window.firstResponder
        if (responder as? KeyRecording)?.isRecordingKey == true || (responder as? NSTextView)?.hasMarkedText() == true { return false }
        let shortcuts = Shortcuts.shared
        if let editor = window.windowController as? CodeEditorController, let id = shortcuts.match(event, scope: "editor") {
            editor.perform(id)
            return true
        }
        guard let id = shortcuts.match(event, scope: "window") else { return false }
        perform(id)
        return true
    }

    /// Menu items showing a shortcut, by action id.
    private var keyed: [(String, NSMenuItem)] = []

    /// Puts each action's first key on its menu item. (The keys themselves work through
    /// `runShortcut`, which sees them first.)
    @objc private func showKeys() {
        for (id, item) in keyed { Shortcuts.shared.apply(id, to: item) }
        if let windows = NSApp.windowsMenu { menuNeedsUpdate(windows) }
    }

    /// AppKit adds Show Next Tab and Show Previous Tab to the Window menu, on ⌃⇥ and ⌃⇧⇥: they
    /// get the keymap's keys instead, so changing those changes what the keys do.
    func menuNeedsUpdate(_ menu: NSMenu) {
        for item in menu.items {
            switch item.action {
            case #selector(NSWindow.selectNextTab(_:)): Shortcuts.shared.apply("next-tab", to: item)
            case #selector(NSWindow.selectPreviousTab(_:)): Shortcuts.shared.apply("prev-tab", to: item)
            default: break
            }
        }
    }

    // MARK: Tabs and the side dock

    /// The tabs of the session window in front (puppets' tabs among them), and which is showing.
    private var tabs: (windows: [NSWindow], selected: Int)? {
        let front = NSApp.keyWindow.flatMap { $0.tabbedWindows == nil ? nil : $0 } ?? current?.window
        guard let front else { return nil }
        let windows = front.tabbedWindows ?? [front]
        return (windows, windows.firstIndex(of: front) ?? 0)
    }

    private func selectTab(_ index: Int) {
        guard let tabs, tabs.windows.indices.contains(index) else { return }
        tabs.windows[index].makeKeyAndOrderFront(nil)
    }

    @objc func nextTab(_ sender: Any?) {
        guard let tabs else { return }
        selectTab((tabs.selected + 1) % tabs.windows.count)
    }

    @objc func previousTab(_ sender: Any?) {
        guard let tabs else { return }
        selectTab((tabs.selected + tabs.windows.count - 1) % tabs.windows.count)
    }

    @objc func closeTab(_ sender: Any?) { NSApp.keyWindow?.performClose(sender) }
    @objc func toggleDock(_ sender: Any?) { current?.toggleDock() }
    @objc func nextPanel(_ sender: Any?) { current?.cyclePanel(by: 1) }
    @objc func previousPanel(_ sender: Any?) { current?.cyclePanel(by: -1) }

    // MARK: Finding

    /// Searches the output of the tab in front, a program editor's text, or in another window
    /// (the rules', say) the text being edited.
    private func find(_ action: NSTextFinder.Action, editor id: String) {
        if let editor = NSApp.keyWindow?.windowController as? CodeEditorController { return editor.perform(id) }
        guard let view = current?.outputView?.textView ?? NSApp.keyWindow?.firstResponder as? NSTextView else { return }
        let item = NSMenuItem()
        item.tag = action.rawValue
        view.performTextFinderAction(item)
    }

    @objc func find(_ sender: Any?) { find(.showFindInterface, editor: "editor-find") }
    @objc func findNext(_ sender: Any?) { find(.nextMatch, editor: "editor-find-next") }
    @objc func findPrevious(_ sender: Any?) { find(.previousMatch, editor: "editor-find-previous") }

    // MARK: The pickers

    /// The /agent command, which this app has as well as the engine's.
    private static let agentCommand = ["/agent [<name>|stop|list]", "run an LLM agent (claude, codex, agy, grok) beside the world, with the world’s tools"]

    /// The command palette: every action that can run now, the saved worlds, and the client's
    /// commands (choosing one puts it in the input box).
    @objc func showPalette(_ sender: Any?) {
        guard let window = NSApp.keyWindow ?? NSApp.mainWindow else { return NSSound.beep() }
        let shortcuts = Shortcuts.shared
        var items: [ListPicker.Item] = []
        for action in shortcuts.actions where action["scope"].string == "window" {
            let id = action["id"].string
            if id == "palette" { continue }
            if id.hasPrefix("tab-") {
                // Only the tabs there are.
                let count = tabs?.windows.count ?? 0
                guard let n = Int(id.dropFirst(4)), count > 1, n <= count else { continue }
            } else if !isEnabled(AppDelegate.actions[id]) {
                continue
            }
            items.append(ListPicker.Item(group: "Commands", label: action["label"].string, detail: shortcuts.display(id)) { [weak self] in self?.perform(id) })
        }
        for world in AppState.shared.worlds {
            let name = world["name"].string
            let address = "\(world["host"].string):\(world["port"].int)"
            let characters = world["character"].array.map { $0["name"].string }.filter { !$0.isEmpty }
            // A world with characters is opened as one of them, as in the Connect window.
            for character in characters.isEmpty ? [nil] : characters.map(Optional.some) {
                let label = character.map { "\($0) @ \(name)" } ?? name
                items.append(ListPicker.Item(group: "Worlds", label: "Connect to \(label)", detail: address) { [weak self] in self?.openSession(world: world, character: character) })
            }
        }
        if let session = current {
            for command in AppState.shared.start["commands"].array.map({ $0.array.map(\.string) }) + [AppDelegate.agentCommand] where command.count == 2 {
                let text = insertText(forUsage: command[0])
                items.append(ListPicker.Item(group: "Client commands", label: command[0], detail: command[1]) { [weak session] in
                    session?.input.text = text
                    session?.input.focus()
                })
            }
        }
        ListPicker.show("Search commands, worlds and client commands", items: items, over: window)
    }

    /// What was typed in the tab in front, newest first; choosing a line puts it in the input
    /// box to edit or send.
    @objc func searchHistory(_ sender: Any?) {
        guard let session = current, let window = session.window else { return }
        var seen = Set<String>()
        let lines = session.input.historyEntries.reversed().filter { seen.insert($0).inserted }
        let items = lines.map { line in
            ListPicker.Item(label: line) { [weak session] in
                session?.input.text = line
                session?.input.focus()
            }
        }
        ListPicker.show(items.isEmpty ? "Nothing typed yet" : "Search what you’ve typed", items: items, over: window)
    }

    // MARK: Menus

    private func isEnabled(_ action: Selector?) -> Bool {
        let session = current
        switch action {
        case #selector(reconnect): return session.map { !$0.isConnected } ?? false
        case #selector(disconnect): return session?.isConnected ?? false
        case #selector(editWorld): return session.map { AppState.shared.world(named: $0.worldName) != nil } ?? false
        case #selector(clearOutput), #selector(stopSounds), #selector(restartScripts), #selector(showMap), #selector(importMap), #selector(toggleLog),
             #selector(reopenEditor), #selector(searchHistory):
            return session != nil
        case #selector(find(_:)), #selector(findNext), #selector(findPrevious):
            return session != nil || NSApp.keyWindow?.windowController is CodeEditorController || NSApp.keyWindow?.firstResponder is NSTextView
        case #selector(toggleDock), #selector(nextPanel), #selector(previousPanel): return !(session?.panels.isEmpty ?? true)
        case #selector(nextTab), #selector(previousTab): return (tabs?.windows.count ?? 0) > 1
        case #selector(closeTab): return NSApp.keyWindow != nil
        default: return true
        }
    }

    func validateMenuItem(_ item: NSMenuItem) -> Bool {
        switch item.action {
        case #selector(toggleLog): item.state = current?.logging == true ? .on : .off
        case #selector(toggleDock): item.title = current?.dockHidden == true ? "Show Side Dock" : "Hide Side Dock"
        default: break
        }
        return isEnabled(item.action)
    }

    // MARK: The menu bar

    /// A menu item. One of the keymap's actions (`id`) shows its first key; others get
    /// `key` (the Edit and Window menus' standard ones).
    private func item(_ title: String, _ action: Selector?, id: String? = nil, _ key: String = "", _ modifiers: NSEvent.ModifierFlags = .command, target: Bool = true) -> NSMenuItem {
        let item = NSMenuItem(title: title, action: action, keyEquivalent: key)
        item.keyEquivalentModifierMask = modifiers
        // Edit and Window commands go to whatever has the focus; ours come here.
        if target { item.target = self }
        if let id { keyed.append((id, item)) }
        return item
    }

    private func menu(_ title: String, _ items: [NSMenuItem]) -> NSMenuItem {
        let menu = NSMenu(title: title)
        items.forEach(menu.addItem)
        let holder = NSMenuItem(title: title, action: nil, keyEquivalent: "")
        holder.submenu = menu
        return holder
    }

    private func buildMenu() -> NSMenu {
        keyed = []
        let main = NSMenu()
        let services = NSMenu(title: "Services")
        NSApp.servicesMenu = services
        let servicesItem = NSMenuItem(title: "Services", action: nil, keyEquivalent: "")
        servicesItem.submenu = services
        main.addItem(menu("Familiar", [
            item("About Familiar", #selector(showAbout), id: "about"),
            .separator(),
            item("Settings…", #selector(showPreferences), id: "preferences"),
            item("Open Settings Folder", #selector(openSettingsFolder), id: "open-settings-folder"),
            .separator(),
            servicesItem,
            .separator(),
            item("Hide Familiar", #selector(NSApplication.hide(_:)), "h", target: false),
            item("Hide Others", #selector(NSApplication.hideOtherApplications(_:)), "h", [.command, .option], target: false),
            item("Show All", #selector(NSApplication.unhideAllApplications(_:)), target: false),
            .separator(),
            item("Quit Familiar", #selector(quit), id: "quit"),
        ]))
        main.addItem(menu("File", [
            item("Connect…", #selector(showConnect), id: "connect"),
            item("New World…", #selector(newWorld), id: "new-world"),
            .separator(),
            item("Import BeipMU Settings…", #selector(importBeipMU), id: "import-beipmu"),
            item("Import BeipMU Map…", #selector(importMap), id: "import-map"),
            .separator(),
            item("Log to a File", #selector(toggleLog), id: "toggle-log"),
            item("Show Logs in Finder", #selector(openLogs), id: "open-logs"),
            .separator(),
            item("Close Tab", #selector(closeTab), id: "close-tab"),
        ]))
        main.addItem(menu("Edit", [
            item("Undo", Selector(("undo:")), "z", target: false),
            item("Redo", Selector(("redo:")), "z", [.command, .shift], target: false),
            .separator(),
            item("Cut", #selector(NSText.cut(_:)), "x", target: false),
            item("Copy", #selector(NSText.copy(_:)), "c", target: false),
            item("Paste", #selector(NSText.paste(_:)), "v", target: false),
            item("Select All", #selector(NSText.selectAll(_:)), "a", target: false),
            .separator(),
            item("Find…", #selector(find(_:)), id: "find"),
            item("Find Next", #selector(findNext), id: "find-next"),
            item("Find Previous", #selector(findPrevious), id: "find-previous"),
            .separator(),
            item("Search Input History…", #selector(searchHistory), id: "history-search"),
        ]))
        main.addItem(menu("View", [
            item("Command Palette…", #selector(showPalette), id: "palette"),
            .separator(),
            item("Clear Output", #selector(clearOutput), id: "clear"),
            item("Show Map", #selector(showMap), id: "show-map"),
            item("Hide Side Dock", #selector(toggleDock), id: "toggle-sidebar"),
            item("Next Side Dock Tab", #selector(nextPanel), id: "next-panel"),
            item("Previous Side Dock Tab", #selector(previousPanel), id: "prev-panel"),
            .separator(),
            item("Enter Full Screen", #selector(NSWindow.toggleFullScreen(_:)), "f", [.command, .control], target: false),
        ]))
        main.addItem(menu("World", [
            item("Reconnect", #selector(reconnect), id: "reconnect"),
            item("Disconnect", #selector(disconnect), id: "disconnect"),
            .separator(),
            item("Edit World…", #selector(editWorld), id: "edit-world"),
            item("Triggers, Aliases and Macros…", #selector(showRules), id: "rules"),
            .separator(),
            item("Reopen the Last Program", #selector(reopenEditor), id: "reopen-editor"),
            item("Restart Scripts", #selector(restartScripts), id: "restart-scripts"),
            item("Stop Sounds and Speech", #selector(stopSounds), id: "stop-speech"),
        ]))
        let windows = NSMenu(title: "Window")
        windows.delegate = self
        windows.addItem(item("Minimize", #selector(NSWindow.performMiniaturize(_:)), "m", target: false))
        windows.addItem(item("Zoom", #selector(NSWindow.performZoom(_:)), target: false))
        windows.addItem(.separator())
        windows.addItem(item("Bring All to Front", #selector(NSApplication.arrangeInFront(_:)), target: false))
        let windowsItem = NSMenuItem(title: "Window", action: nil, keyEquivalent: "")
        windowsItem.submenu = windows
        main.addItem(windowsItem)
        NSApp.windowsMenu = windows
        main.addItem(menu("Help", [
            item("Familiar Help", #selector(openHelp), id: "help"),
            item("Keyboard Shortcuts…", #selector(showKeyboard), id: "keyboard"),
        ]))
        return main
    }
}

/// Lets a SwiftUI view that was written for a `.sheet` (it calls `dismiss`) be shown as an
/// AppKit sheet too.
private struct DismissKey: EnvironmentKey {
    static let defaultValue: (() -> Void)? = nil
}

extension EnvironmentValues {
    var dismiss_: (() -> Void)? {
        get { self[DismissKey.self] }
        set { self[DismissKey.self] = newValue }
    }
}
