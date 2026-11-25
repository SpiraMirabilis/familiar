import AppKit
import Carbon.HIToolbox
import SwiftUI

/// A view that's recording a key press, for a macro or a shortcut. While it is, no shortcut runs.
protocol KeyRecording: AnyObject {
    var isRecordingKey: Bool { get }
}

/// The client's keyboard shortcuts. The engine keeps them (crates/engine/src/keymap.rs: every
/// action, its default keys and the user's changes); this is the app's view of them.
final class Shortcuts: ObservableObject {
    static let shared = Shortcuts()
    /// Posted when an action's keys change, for whatever shows them.
    static let changed = Notification.Name("FamiliarShortcutsChanged")

    /// Every action this app has, in order, as `keys.actions` describes them: "id", "label",
    /// "group", "scope", "changed", "defaults", and "keys" with what else is on each.
    @Published private(set) var actions: [JSON] = []

    func load() {
        do {
            actions = try Core.call("keys.actions").array
        } catch {
            NSLog("Familiar: reading the shortcuts: %@", error.localizedDescription)
        }
    }

    func action(_ id: String) -> JSON? { actions.first { $0["id"].string == id } }

    /// An action's keys, in the config's spelling ("Cmd+Shift+P"). The first is the one menus show.
    func keys(_ id: String) -> [String] { action(id)?["keys"].array.map { $0["key"].string } ?? [] }

    /// The first key as a menu shows it ("⇧⌘P"), or "" for an action without keys.
    func display(_ id: String) -> String { keys(id).first.map(KeyNames.display) ?? "" }

    /// Sets an action's keys, or with nil puts back its defaults. The engine saves it.
    func set(_ id: String, keys: [String]?) {
        do {
            actions = try Core.call("keys.set", ["id": .of(id), "keys": keys.map { .array($0.map(JSON.string)) } ?? .null]).array
        } catch {
            AppState.shared.alert("The shortcut wasn’t saved", error.localizedDescription)
        }
        // The engine saved the config: the app's copy has to follow, or the next change to a
        // setting would save the old keys back.
        AppState.shared.reloadFromEngine()
        NotificationCenter.default.post(name: Shortcuts.changed, object: nil)
    }

    /// Puts every action back to its defaults.
    func resetAll() {
        for action in actions where action["changed"].bool {
            set(action["id"].string, keys: nil)
        }
    }

    /// The action a key press runs in the main window ("window") or a program editor
    /// ("editor"), if any.
    func match(_ event: NSEvent, scope: String) -> String? {
        guard var chord = Keys.chord(event) else { return nil }
        chord["scope"] = .of(scope)
        return (try? Core.call("keys.match", chord))?.optionalString
    }

    /// Shows an action's first key on a menu item, or no key.
    func apply(_ id: String, to item: NSMenuItem) {
        if let first = keys(id).first, let equivalent = KeyNames.menuEquivalent(first) {
            item.keyEquivalent = equivalent.key
            item.keyEquivalentModifierMask = equivalent.modifiers
        } else {
            item.keyEquivalent = ""
            item.keyEquivalentModifierMask = []
        }
    }
}

// MARK: The Keyboard settings

/// Settings > Keyboard: every action with its keys, which can be added to, removed and reset.
struct KeyboardSettings: View {
    @ObservedObject private var shortcuts = Shortcuts.shared
    @ObservedObject private var app = AppState.shared

    private var groups: [(String, [JSON])] {
        var out: [(String, [JSON])] = []
        for action in shortcuts.actions {
            let group = action["group"].string
            if out.last?.0 == group { out[out.count - 1].1.append(action) } else { out.append((group, [action])) }
        }
        return out
    }

    var body: some View {
        VStack(spacing: 0) {
            Form {
                ForEach(groups, id: \.0) { group, actions in
                    Section(group) {
                        ForEach(actions, id: \.["id"].string) { ShortcutRow(action: $0) }
                    }
                }
            }
            .formStyle(.grouped)
            Divider()
            HStack {
                Text("Shortcuts come before keyboard macros.").font(.callout).foregroundStyle(.secondary)
                Spacer()
                Button("Reset All…") { resetAll() }
                    .disabled(!shortcuts.actions.contains { $0["changed"].bool })
            }
            .padding(12)
        }
        .onAppear { shortcuts.load() }
        // Macros changed elsewhere change what clashes.
        .onChange(of: app.config) { _, _ in shortcuts.load() }
    }

    private func resetAll() {
        let alert = NSAlert()
        alert.messageText = "Reset All Shortcuts?"
        alert.informativeText = "Every action gets its usual keys back."
        alert.addButton(withTitle: "Reset All")
        alert.addButton(withTitle: "Cancel")
        alert.buttons.first?.hasDestructiveAction = true
        if alert.runModal() == .alertFirstButtonReturn { shortcuts.resetAll() }
    }
}

/// One action: its label, its keys (each with a button to remove it, and what else is on it),
/// a button to add a key and one to reset it.
private struct ShortcutRow: View {
    let action: JSON

    private var id: String { action["id"].string }
    private var keys: [String] { action["keys"].array.map { $0["key"].string } }

    private func note(_ key: JSON) -> String? {
        let clashes = key["clashes"].array.map { "Also: \($0.string)" }
        let macros = key["macros"].array.map { "Macro: \($0.string)" }
        guard !clashes.isEmpty || !macros.isEmpty else { return nil }
        var text = (clashes + macros).joined(separator: " · ")
        if !macros.isEmpty { text += macros.count == 1 ? " (the macro won’t run)" : " (the macros won’t run)" }
        return text
    }

    var body: some View {
        LabeledContent {
            VStack(alignment: .trailing, spacing: 4) {
                ForEach(Array(action["keys"].array.enumerated()), id: \.offset) { index, key in
                    HStack(spacing: 4) {
                        Text(KeyNames.display(key["key"].string))
                            .monospacedDigit()
                            .padding(.horizontal, 6)
                            .padding(.vertical, 1)
                            .background(Color.primary.opacity(0.08), in: RoundedRectangle(cornerRadius: 4))
                        Button {
                            var all = keys
                            all.remove(at: index)
                            Shortcuts.shared.set(id, keys: all)
                        } label: { Image(systemName: "xmark.circle.fill") }
                            .buttonStyle(.borderless)
                            .foregroundStyle(.secondary)
                            .help("Remove this key")
                            .accessibilityLabel("Remove \(KeyNames.display(key["key"].string))")
                    }
                    if let note = note(key) {
                        Text(note).font(.caption).foregroundStyle(.orange).multilineTextAlignment(.trailing)
                    }
                }
                HStack(spacing: 6) {
                    ShortcutRecorder { new in Shortcuts.shared.set(id, keys: keys + [new]) }
                        .fixedSize()
                    if action["changed"].bool {
                        Button {
                            Shortcuts.shared.set(id, keys: nil)
                        } label: { Image(systemName: "arrow.counterclockwise") }
                            .buttonStyle(.borderless)
                            .help(defaultsHelp)
                            .accessibilityLabel("Reset \(action["label"].string)")
                    }
                }
            }
        } label: {
            Text(action["label"].string)
        }
    }

    private var defaultsHelp: String {
        let defaults = action["defaults"].array.map { KeyNames.display($0.string) }
        return defaults.isEmpty ? "Back to no key" : "Back to \(defaults.joined(separator: ", "))"
    }
}

/// A button that records the next key pressed, Command chords too, as a key for a shortcut.
/// Escape gives up.
private struct ShortcutRecorder: NSViewRepresentable {
    let onCapture: (String) -> Void

    func makeNSView(context: Context) -> ShortcutRecorderButton {
        let button = ShortcutRecorderButton(frame: .zero)
        button.onCapture = onCapture
        return button
    }

    func updateNSView(_ button: ShortcutRecorderButton, context: Context) {
        button.onCapture = onCapture
    }
}

private final class ShortcutRecorderButton: NSButton, KeyRecording {
    var onCapture: ((String) -> Void)?
    var isRecordingKey: Bool { recording }

    private var recording = false {
        didSet {
            title = recording ? "Press a Key…" : "Add Key"
            invalidateIntrinsicContentSize()
        }
    }

    override init(frame: NSRect) {
        super.init(frame: frame)
        bezelStyle = .rounded
        controlSize = .small
        font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        title = "Add Key"
        target = self
        action = #selector(toggle)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    override var acceptsFirstResponder: Bool { true }

    @objc private func toggle() {
        recording.toggle()
        if recording { window?.makeFirstResponder(self) }
    }

    // Clicking anywhere else gives up on recording.
    override func resignFirstResponder() -> Bool {
        recording = false
        return super.resignFirstResponder()
    }

    private func capture(_ event: NSEvent) {
        if Int(event.keyCode) == kVK_Escape, event.modifierFlags.intersection([.command, .control, .option, .shift]).isEmpty {
            recording = false
            return
        }
        // A modifier or dead key on its own isn't a key yet: keep listening.
        guard let text = Keys.text(event) else { return }
        recording = false
        onCapture?(text)
    }

    override func keyDown(with event: NSEvent) {
        if recording { capture(event) } else { super.keyDown(with: event) }
    }

    // Command chords and function keys are offered here before the menus see them, so while
    // recording they're taken as the key rather than run.
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        guard recording, event.type == .keyDown, window?.firstResponder === self else {
            return super.performKeyEquivalent(with: event)
        }
        capture(event)
        return true
    }
}

// MARK: Pickers

/// A list to choose from under a search box, as a sheet over a window: the command palette and
/// the input history. Up and Down move, Return chooses, Escape closes.
final class ListPicker: NSObject, NSSearchFieldDelegate, NSTableViewDataSource, NSTableViewDelegate {
    struct Item {
        var group = ""
        let label: String
        var detail = ""
        let run: () -> Void
    }

    /// The one showing, which also keeps it alive.
    private static var showing: ListPicker?

    private let items: [Item]
    private var shown: [Item] = []
    private let sheet: NSWindow
    private let search = NSSearchField()
    private let table = NSTableView()
    private weak var parent: NSWindow?

    static func show(_ placeholder: String, items: [Item], over parent: NSWindow) {
        guard showing == nil, parent.attachedSheet == nil else { return }
        let picker = ListPicker(placeholder, items: items, parent: parent)
        showing = picker
        parent.beginSheet(picker.sheet)
        picker.sheet.makeFirstResponder(picker.search)
    }

    private init(_ placeholder: String, items: [Item], parent: NSWindow) {
        self.items = items
        self.parent = parent
        sheet = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 520, height: 380), styleMask: [.titled], backing: .buffered, defer: false)
        super.init()

        search.placeholderString = placeholder
        search.delegate = self

        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("item"))
        column.resizingMask = .autoresizingMask
        column.width = 500
        table.addTableColumn(column)
        table.columnAutoresizingStyle = .lastColumnOnlyAutoresizingStyle
        table.headerView = nil
        table.rowHeight = 22
        table.style = .plain
        table.dataSource = self
        table.delegate = self
        table.target = self
        table.doubleAction = #selector(choose)
        table.refusesFirstResponder = true
        table.setAccessibilityLabel(placeholder)
        let scroll = NSScrollView()
        scroll.documentView = table
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true

        let root = NSView()
        for view in [search, scroll] as [NSView] {
            view.translatesAutoresizingMaskIntoConstraints = false
            root.addSubview(view)
        }
        NSLayoutConstraint.activate([
            search.topAnchor.constraint(equalTo: root.topAnchor, constant: 12),
            search.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 12),
            search.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -12),
            scroll.topAnchor.constraint(equalTo: search.bottomAnchor, constant: 8),
            scroll.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            scroll.bottomAnchor.constraint(equalTo: root.bottomAnchor),
        ])
        sheet.contentView = root
        refilter()
    }

    /// The items matching what's typed, best first (in their own order while nothing is).
    private func refilter() {
        let query = search.stringValue.trimmingCharacters(in: .whitespaces)
        if query.isEmpty {
            shown = items
        } else {
            shown = items.enumerated()
                .compactMap { index, item in Fuzzy.score(query, item.label).map { (score: $0, index: index, item: item) } }
                .sorted { $0.score != $1.score ? $0.score > $1.score : $0.index < $1.index }
                .map(\.item)
        }
        table.reloadData()
        if !shown.isEmpty { select(0) }
    }

    private func select(_ row: Int) {
        guard !shown.isEmpty else { return }
        let row = min(max(row, 0), shown.count - 1)
        table.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
        table.scrollRowToVisible(row)
    }

    func controlTextDidChange(_ notification: Notification) { refilter() }

    func control(_ control: NSControl, textView: NSTextView, doCommandBy selector: Selector) -> Bool {
        switch selector {
        case #selector(NSResponder.moveUp(_:)): select(table.selectedRow - 1)
        case #selector(NSResponder.moveDown(_:)): select(table.selectedRow + 1)
        case #selector(NSResponder.scrollPageUp(_:)), #selector(NSResponder.pageUp(_:)): select(table.selectedRow - 10)
        case #selector(NSResponder.scrollPageDown(_:)), #selector(NSResponder.pageDown(_:)): select(table.selectedRow + 10)
        case #selector(NSResponder.insertNewline(_:)): choose()
        case #selector(NSResponder.cancelOperation(_:)): close(running: nil)
        default: return false
        }
        return true
    }

    @objc private func choose() {
        let row = table.clickedRow >= 0 ? table.clickedRow : table.selectedRow
        guard shown.indices.contains(row) else { return NSSound.beep() }
        close(running: shown[row])
    }

    /// Closes the sheet, then runs the item chosen (with the window it was over in front again).
    private func close(running item: Item?) {
        parent?.endSheet(sheet)
        sheet.orderOut(nil)
        ListPicker.showing = nil
        if let item { DispatchQueue.main.async { item.run() } }
    }

    func numberOfRows(in tableView: NSTableView) -> Int { shown.count }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let item = shown[row]
        let label = NSTextField(labelWithString: item.label)
        label.lineBreakMode = .byTruncatingTail
        label.setContentHuggingPriority(.defaultHigh, for: .horizontal)
        // A command's description gives way before its name does.
        let detail = NSTextField(labelWithString: [item.detail, item.group].filter { !$0.isEmpty }.joined(separator: "  ·  "))
        detail.textColor = .secondaryLabelColor
        detail.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        detail.lineBreakMode = .byTruncatingTail
        detail.alignment = .right
        detail.setContentHuggingPriority(.defaultLow, for: .horizontal)
        detail.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        let row = NSStackView(views: [label, detail])
        row.orientation = .horizontal
        row.distribution = .fill
        row.edgeInsets = NSEdgeInsets(top: 0, left: 12, bottom: 0, right: 12)
        row.setAccessibilityElement(true)
        row.setAccessibilityLabel([item.label, item.detail].filter { !$0.isEmpty }.joined(separator: ", "))
        return row
    }
}
