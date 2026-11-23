import AppKit
import SwiftUI
import UniformTypeIdentifiers

@main
final class AppDelegate: NSObject, NSApplicationDelegate, NSMenuItemValidation {
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
        NSApp.mainMenu = buildMenu()
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
            let window = window("Settings", size: NSSize(width: 480, height: 520), autosave: "FamiliarSettings", PreferencesView())
            window.styleMask.remove(.resizable)
            preferencesWindow = window
        }
        preferencesWindow?.makeKeyAndOrderFront(nil)
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

    @objc func editWorld(_ sender: Any?) {
        guard let session = current, let world = AppState.shared.world(named: session.worldName), let parent = session.window else { return }
        let sheet = NSWindow(contentRect: .zero, styleMask: [.titled], backing: .buffered, defer: false)
        sheet.contentView = NSHostingView(rootView: WorldEditor(draft: WorldDraft(original: session.worldName, world: world)).environment(\.dismiss_, { [weak parent, weak sheet] in
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
        if let url = URL(string: "https://github.com/SpiraMirabilis/familiar#using-familiar") { NSWorkspace.shared.open(url) }
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

    func validateMenuItem(_ item: NSMenuItem) -> Bool {
        let session = current
        switch item.action {
        case #selector(reconnect): return session.map { !$0.isConnected } ?? false
        case #selector(disconnect): return session?.isConnected ?? false
        case #selector(toggleLog):
            item.state = session?.logging == true ? .on : .off
            return session != nil
        case #selector(editWorld): return session.map { AppState.shared.world(named: $0.worldName) != nil } ?? false
        case #selector(clearOutput), #selector(stopSounds), #selector(restartScripts), #selector(showMap), #selector(importMap): return session != nil
        default: return true
        }
    }

    // MARK: The menu bar

    private func item(_ title: String, _ action: Selector?, _ key: String = "", _ modifiers: NSEvent.ModifierFlags = .command, target: Bool = true) -> NSMenuItem {
        let item = NSMenuItem(title: title, action: action, keyEquivalent: key)
        item.keyEquivalentModifierMask = modifiers
        // Edit and Window commands go to whatever has the focus; ours come here.
        if target { item.target = self }
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
        let main = NSMenu()
        let services = NSMenu(title: "Services")
        NSApp.servicesMenu = services
        let servicesItem = NSMenuItem(title: "Services", action: nil, keyEquivalent: "")
        servicesItem.submenu = services
        main.addItem(menu("Familiar", [
            item("About Familiar", #selector(showAbout)),
            .separator(),
            item("Settings…", #selector(showPreferences), ","),
            item("Open Settings Folder", #selector(openSettingsFolder)),
            .separator(),
            servicesItem,
            .separator(),
            item("Hide Familiar", #selector(NSApplication.hide(_:)), "h", target: false),
            item("Hide Others", #selector(NSApplication.hideOtherApplications(_:)), "h", [.command, .option], target: false),
            item("Show All", #selector(NSApplication.unhideAllApplications(_:)), target: false),
            .separator(),
            item("Quit Familiar", #selector(NSApplication.terminate(_:)), "q", target: false),
        ]))
        main.addItem(menu("File", [
            item("Connect…", #selector(showConnect), "n"),
            .separator(),
            item("Import BeipMU Settings…", #selector(importBeipMU)),
            item("Import BeipMU Map…", #selector(importMap)),
            .separator(),
            item("Log to a File", #selector(toggleLog), "l", [.command, .shift]),
            item("Show Logs in Finder", #selector(openLogs)),
            .separator(),
            item("Close Tab", #selector(NSWindow.performClose(_:)), "w", target: false),
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
            {
                let find = item("Find…", #selector(NSTextView.performFindPanelAction(_:)), "f", target: false)
                find.tag = Int(NSFindPanelAction.showFindPanel.rawValue)
                return find
            }(),
            {
                let next = item("Find Next", #selector(NSTextView.performFindPanelAction(_:)), "g", target: false)
                next.tag = Int(NSFindPanelAction.next.rawValue)
                return next
            }(),
            {
                let previous = item("Find Previous", #selector(NSTextView.performFindPanelAction(_:)), "g", [.command, .shift], target: false)
                previous.tag = Int(NSFindPanelAction.previous.rawValue)
                return previous
            }(),
        ]))
        main.addItem(menu("View", [
            item("Clear Output", #selector(clearOutput), "k"),
            item("Show Map", #selector(showMap), "m", [.command, .shift]),
            .separator(),
            item("Enter Full Screen", #selector(NSWindow.toggleFullScreen(_:)), "f", [.command, .control], target: false),
        ]))
        main.addItem(menu("World", [
            item("Reconnect", #selector(reconnect), "r"),
            item("Disconnect", #selector(disconnect), "d", [.command, .shift]),
            .separator(),
            item("Edit World…", #selector(editWorld), "e", [.command, .shift]),
            item("Triggers, Aliases and Macros…", #selector(showRules), "t", [.command, .shift]),
            .separator(),
            item("Restart Scripts", #selector(restartScripts)),
            item("Stop Sounds and Speech", #selector(stopSounds), "."),
        ]))
        let windows = NSMenu(title: "Window")
        windows.addItem(item("Minimize", #selector(NSWindow.performMiniaturize(_:)), "m", target: false))
        windows.addItem(item("Zoom", #selector(NSWindow.performZoom(_:)), target: false))
        windows.addItem(.separator())
        windows.addItem(item("Bring All to Front", #selector(NSApplication.arrangeInFront(_:)), target: false))
        let windowsItem = NSMenuItem(title: "Window", action: nil, keyEquivalent: "")
        windowsItem.submenu = windows
        main.addItem(windowsItem)
        NSApp.windowsMenu = windows
        main.addItem(menu("Help", [item("Familiar Help", #selector(openHelp), "?")]))
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
