import AppKit
import Carbon.HIToolbox
import SwiftUI

/// A world's tab: the output, the input under it, and the side dock when there's something in it.
struct SessionView: View {
    @ObservedObject var session: Session

    var body: some View {
        HSplitView {
            VStack(spacing: 0) {
                OutputArea(
                    buffer: session.output,
                    onLink: { session.followLink($0) },
                    onGridSize: { session.resize(cols: $0, rows: $1) },
                    onType: { typed(session.input, $0) { session.runMacro($0) } },
                    onCreate: { session.outputView = $0 }
                )
                if !session.statusText.isEmpty {
                    Text(session.statusText)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 10)
                        .padding(.vertical, 3)
                }
                Divider()
                InputArea { session.input }
                    .background(Color(nsColor: .textBackgroundColor))
            }
            .frame(minWidth: 320, maxWidth: .infinity, maxHeight: .infinity)
            .layoutPriority(1)
            if !session.panels.isEmpty && !session.dockHidden {
                SideDock(session: session)
                    .frame(minWidth: 330, idealWidth: 360, maxWidth: .infinity, maxHeight: .infinity)
            }
        }
    }
}

/// A key pressed while an output area has the focus: macros run, and anything typed goes to
/// the input box.
func typed(_ input: InputBox, _ event: NSEvent, macro: (NSEvent) -> Bool) -> Bool {
    let flags = event.modifierFlags
    if macro(event) { return true }
    if flags.contains(.command) { return false }
    if flags.contains(.control) || flags.contains(.option) { return false }
    if [kVK_Return, kVK_ANSI_KeypadEnter].contains(Int(event.keyCode)) {
        input.focus()
        return true
    }
    guard let characters = event.characters, let first = characters.unicodeScalars.first, first.value >= 0x20, first.value != 0x7f,
          !(0xF700...0xF8FF).contains(first.value)
    else { return false }
    input.insert(characters)
    return true
}

/// Spawn windows, stat panes, the map, tilemaps and docked web views, as tabs beside the output.
struct SideDock: View {
    @ObservedObject var session: Session

    var body: some View {
        let selected = session.selectedPanel ?? session.panels.first
        VStack(spacing: 0) {
            if session.panels.count > 1 {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 2) {
                        ForEach(session.panels) { panel in
                            Button(panel.title) { session.selectedPanel = panel }
                                .buttonStyle(.plain)
                                .padding(.horizontal, 9)
                                .padding(.vertical, 4)
                                .background(panel == selected ? Color.accentColor.opacity(0.22) : Color.clear, in: RoundedRectangle(cornerRadius: 5))
                                .accessibilityAddTraits(panel == selected ? .isSelected : [])
                        }
                    }
                    .padding(.horizontal, 6)
                    .padding(.vertical, 4)
                }
                Divider()
            }
            if let selected {
                content(selected).id(selected.id)
            }
        }
    }

    @ViewBuilder
    private func content(_ panel: SidePanel) -> some View {
        switch panel {
        case .map: MapPanel(session: session)
        case .stats(let name): StatsPanel(pane: session.stats[name] ?? .null)
        case .spawn(let name):
            if let buffer = session.spawns[name] {
                OutputArea(buffer: buffer, onLink: { session.followLink($0) }, onType: { typed(session.input, $0) { session.runMacro($0) } })
            }
        case .tilemap(let name): TilemapPanel(map: session.tilemaps[name] ?? .null) { session.output.notice($0) }
        case .web(let id):
            if let pane = session.webPanes[id] { WebPaneView(pane: pane) }
        case .agent:
            if let pane = session.agent { AgentPaneView(pane: pane) }
        }
    }
}

/// Stat bars and values from the server (GMCP Beip.Stats).
struct StatsPanel: View {
    let pane: JSON

    private func color(_ value: JSON, _ fallback: Color) -> Color {
        if value.string == "transparent" { return .clear }
        return NSColor(hex: value.string).map(Color.init(nsColor:)) ?? fallback
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 7) {
                ForEach(Array(pane["values"].array.enumerated()), id: \.offset) { _, stat in
                    row(stat)
                }
            }
            .padding(10)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .background(color(pane["background"], Color(nsColor: .windowBackgroundColor)))
    }

    @ViewBuilder
    private func row(_ stat: JSON) -> some View {
        let name = Text(stat["name"].string)
            .foregroundStyle(color(stat["name_color"], .primary))
            .frame(maxWidth: .infinity, alignment: stat["align"].string == "right" ? .trailing : stat["align"].string == "center" ? .center : .leading)
        switch stat["kind"].string {
        case "bar":
            VStack(alignment: .leading, spacing: 2) {
                HStack {
                    name
                    Text(stat["text"].string).foregroundStyle(color(stat["value_color"], .secondary)).monospacedDigit()
                }
                GeometryReader { box in
                    let colors = stat["colors"]
                    ZStack(alignment: .leading) {
                        RoundedRectangle(cornerRadius: 3).fill(color(colors["empty"], Color.primary.opacity(0.12)))
                        RoundedRectangle(cornerRadius: 3)
                            .fill(color(colors["fill"], .accentColor))
                            .frame(width: box.size.width * min(max(stat["fraction"].double, 0), 1))
                        if !colors["outline"].isNull {
                            RoundedRectangle(cornerRadius: 3).stroke(color(colors["outline"], .clear))
                        }
                    }
                }
                .frame(height: 8)
            }
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(stat["name"].string)
            .accessibilityValue(stat["text"].string)
        case "text":
            HStack {
                name
                Text(stat["text"].string).foregroundStyle(color(stat["value_color"], .secondary)).textSelection(.enabled)
            }
        default: name
        }
    }
}

/// The window (a tab, usually) for one session.
final class SessionWindowController: NSWindowController, NSWindowDelegate {
    let session: Session
    /// Open session windows. Also what keeps them alive.
    private(set) static var all: [SessionWindowController] = []
    static let tabbingID = "FamiliarSession"

    init(session: Session) {
        self.session = session
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 960, height: 660), styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.tabbingMode = .preferred
        window.tabbingIdentifier = SessionWindowController.tabbingID
        window.minSize = NSSize(width: 420, height: 260)
        window.contentView = NSHostingView(rootView: SessionView(session: session))
        window.center()
        window.setFrameAutosaveName("FamiliarSession")
        super.init(window: window)
        window.delegate = self
        session.window = window
        session.onTitleChange = { [weak self] in self?.updateTitle() }
        session.onActivity = { [weak self] in self?.updateTitle() }
        session.onOpenTab = { [weak self] tab in self?.window?.addTabbedWindow(tab, ordered: .above) }
        updateTitle()
        SessionWindowController.all.append(self)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    /// The session whose tab is in front, if a session's tab is.
    static var current: SessionWindowController? {
        let front = NSApp.keyWindow ?? NSApp.mainWindow
        return all.first { $0.window === front } ?? all.first { $0.window?.isMainWindow == true }
    }

    /// Shows the window, as a tab of the session window that's in front if there is one.
    func present() {
        guard let window else { return }
        let others = SessionWindowController.all.filter { $0 !== self }
        if let host = (others.first { $0.window?.isMainWindow == true } ?? others.last)?.window {
            host.addTabbedWindow(window, ordered: .above)
        }
        window.makeKeyAndOrderFront(nil)
        session.input.focus()
    }

    private func updateTitle() {
        guard let window else { return }
        window.title = session.title
        let note: String
        switch session.state {
        case .connecting: note = "Connecting…"
        case .connected: note = session.logging ? "Logging" : ""
        case .disconnected(let reason): note = reason.map { "Disconnected: \($0)" } ?? "Disconnected"
        }
        window.subtitle = note
        // The tab shows what the title bar can't when it isn't in front.
        let text = NSMutableAttributedString()
        if session.unread {
            let dot = session.important ? NSColor.systemOrange : NSColor.controlAccentColor
            text.append(NSAttributedString(string: "● ", attributes: [.foregroundColor: dot]))
        }
        let connected = session.isConnected
        text.append(NSAttributedString(string: session.title, attributes: connected ? [:] : [.foregroundColor: NSColor.secondaryLabelColor]))
        window.tab.attributedTitle = text
        window.tab.toolTip = [session.info["world"]["host"].string + ":" + String(session.info["world"]["port"].int), note].filter { !$0.isEmpty }.joined(separator: " — ")
    }

    func windowDidBecomeKey(_ notification: Notification) {
        session.seen()
        updateTitle()
        if !(window?.firstResponder is NSTextView) { session.input.focus() }
    }

    func windowShouldClose(_ sender: NSWindow) -> Bool {
        guard session.state == .connected else { return true }
        let alert = NSAlert()
        alert.messageText = "Disconnect from \(session.title)?"
        alert.informativeText = "Closing this tab closes the connection."
        alert.addButton(withTitle: "Disconnect")
        alert.addButton(withTitle: "Cancel")
        alert.beginSheetModal(for: sender) { response in
            if response == .alertFirstButtonReturn { sender.close() }
        }
        return false
    }

    func windowWillClose(_ notification: Notification) {
        session.close()
        SessionWindowController.all.removeAll { $0 === self }
    }
}

/// A puppet's tab: a second character driven through the session's connection.
final class PuppetWindowController: NSWindowController, NSWindowDelegate {
    private weak var session: Session?
    private let name: String
    private let buffer: OutputBuffer
    private let input: InputBox
    private var closing = false

    init(session: Session, name: String) {
        self.session = session
        self.name = name
        let settings = AppState.shared.settings
        buffer = OutputBuffer(font: session.font, maxLines: settings["scrollback_lines"].int)
        input = InputBox(font: session.font, historySize: 200, spellcheck: settings["spellcheck"].bool)
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 960, height: 660), styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.tabbingMode = .preferred
        window.tabbingIdentifier = SessionWindowController.tabbingID
        window.title = "\(name) (\(session.title))"
        super.init(window: window)
        let input = self.input
        input.onSubmit = { [weak session] text, _ in session?.puppetSubmit(name, text) }
        input.onKey = { [weak session] event in session?.runMacro(event) ?? false }
        weak var scroller: OutputScrollView?
        input.onPage = { up in scroller?.scrollPage(up: up) }
        let view = VStack(spacing: 0) {
            OutputArea(
                buffer: buffer,
                onLink: { [weak session] in session?.followLink($0) },
                onType: { [weak session, weak input] in
                    guard let input else { return false }
                    return typed(input, $0) { session?.runMacro($0) ?? false }
                },
                onCreate: { scroller = $0 }
            )
            Divider()
            InputArea { input }.background(Color(nsColor: .textBackgroundColor))
        }
        window.contentView = NSHostingView(rootView: view)
        window.delegate = self
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    func appendLine(_ line: JSON) {
        buffer.appendLine(line)
        if window?.isKeyWindow != true, let tab = window?.tab {
            let text = NSMutableAttributedString(string: "● ", attributes: [.foregroundColor: NSColor.controlAccentColor])
            text.append(NSAttributedString(string: window?.title ?? name))
            tab.attributedTitle = text
        }
    }

    func echo(_ text: String) { buffer.echo(text) }

    func setFont(_ font: NSFont) {
        buffer.font = font
        input.font = font
    }

    func focus() {
        window?.makeKeyAndOrderFront(nil)
        input.focus()
    }

    func windowDidBecomeKey(_ notification: Notification) {
        window?.tab.attributedTitle = nil
        if !(window?.firstResponder is NSTextView) { input.focus() }
    }

    /// Closes the tab because its session is closing.
    override func close() {
        closing = true
        super.close()
    }

    func windowWillClose(_ notification: Notification) {
        if !closing { session?.puppetClosed(name) }
    }
}
