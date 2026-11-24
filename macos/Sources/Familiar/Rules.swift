import AppKit
import Combine
import SwiftUI

/// The editor for triggers, aliases, keyboard macros and variables, at any scope.
enum RulesWindow {
    private static var window: NSWindow?
    private static var model: RulesModel?

    /// Shows the rules window (one shared window; bring it to front if already open).
    /// `scopes` are the scopes of the session in front, most specific first; each is a JSON object
    /// {"kind":"global"|"world"|"character", "world":..., "character":..., "label":...}. Preselect the first.
    static func show(scopes: [JSON]) {
        let config = AppState.shared.config
        let preferred = scopes.compactMap(RuleScope.init(json:)).first { $0.rules(in: config) != nil } ?? .global
        if let window, let model {
            // Only when reopening: a window that's merely behind another keeps what it was showing.
            if !window.isVisible { model.scope = preferred }
            window.makeKeyAndOrderFront(nil)
            return
        }
        let model = RulesModel(scope: preferred)
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 880, height: 620), styleMask: [.titled, .closable, .resizable, .miniaturizable], backing: .buffered, defer: false)
        window.title = "Triggers, Aliases & Macros"
        window.isReleasedWhenClosed = false
        window.tabbingMode = .disallowed
        window.contentView = NSHostingView(rootView: RulesView(model: model))
        window.center()
        window.setFrameAutosaveName("FamiliarRules")
        window.delegate = model
        model.window = window
        self.window = window
        self.model = model
        window.makeKeyAndOrderFront(nil)
    }

    #if DEBUG
    /// For the debug script (see App.swift): presses "Add Examples" in the open window.
    static func addExamplesForTest() { model?.addExamples() }
    #endif

    /// For quitting: offers to save unsaved rules. False means the user cancelled.
    static func canQuit() -> Bool {
        guard let model, model.isDirty else { return true }
        window?.makeKeyAndOrderFront(nil)
        return model.resolveUnsaved()
    }
}

// MARK: Model

private enum RuleKind: String, CaseIterable, Identifiable {
    case trigger, alias, macro, variable

    var id: String { rawValue }
    /// The key in a rules object.
    var key: String { self == .variable ? "variables" : rawValue }
    var hasEnabled: Bool { self == .trigger || self == .alias }
    /// Variables are a sorted map in the config, so they can't be reordered.
    var isOrdered: Bool { self != .variable }

    var title: String {
        switch self {
        case .trigger: return "Triggers"
        case .alias: return "Aliases"
        case .macro: return "Macros"
        case .variable: return "Variables"
        }
    }

    var about: String {
        switch self {
        case .trigger: return "Run on each line from the server, in this order. Character triggers run first, then the world’s, then the ones for all worlds."
        case .alias: return "Replace what you type before it is sent. The first matching alias wins."
        case .macro: return "Send a command when you press a key. Aliases apply to it."
        case .variable: return "Use as %name% in trigger sends and alias replacements. You can also type /set name value."
        }
    }

    var blank: JSON {
        switch self {
        case .trigger:
            let fresh = AppState.shared.defaults["trigger"]
            return fresh.isNull ? ["enabled": true, "pattern": ""] : fresh
        case .alias:
            let fresh = AppState.shared.defaults["alias"]
            return fresh.isNull ? ["enabled": true, "pattern": "", "replacement": ""] : fresh
        case .macro: return ["key": "", "command": ""]
        case .variable: return ["name": "", "value": ""]
        }
    }

    /// A rules object's value for this kind as rows. A variable becomes {"name", "value"} so
    /// it can be renamed like any other field.
    func items(from value: JSON) -> [RuleItem] {
        guard self == .variable else { return value.array.map { RuleItem(value: $0) } }
        return value.object.sorted { $0.key < $1.key }.map { RuleItem(value: ["name": .string($0.key), "value": .string($0.value.string)]) }
    }

    func title(of rule: JSON) -> String {
        switch self {
        case .trigger:
            let name = rule["name"].string.isEmpty ? rule["pattern"].string : rule["name"].string
            return name.isEmpty ? "New trigger" : name
        case .alias: return rule["pattern"].string.isEmpty ? "New alias" : rule["pattern"].string
        case .macro: return rule["key"].string.isEmpty ? "No key" : rule["key"].string
        case .variable: return rule["name"].string.isEmpty ? "New variable" : "%\(rule["name"].string)%"
        }
    }

    func summary(of rule: JSON) -> String {
        switch self {
        case .trigger:
            var parts: [String] = []
            if rule["regex"].bool { parts.append("regex") }
            if rule["gag"].bool { parts.append("hide") }
            if !rule["highlight"].isNull { parts.append("highlight") }
            if !rule["replace"].isNull { parts.append("replace") }
            if let first = rule["send"].string.split(separator: "\n").first { parts.append("send “\(first)”") }
            if !rule["spawn"].string.isEmpty { parts.append("→ \(rule["spawn"].string)") }
            if !rule["sound"].string.isEmpty { parts.append("sound") }
            if rule["notify"].bool { parts.append("notify") }
            if !rule["script"].string.isEmpty { parts.append("\(rule["script"].string)()") }
            if !rule["speak"].string.isEmpty { parts.append("speak") }
            if rule["stop"].bool { parts.append("stop") }
            return parts.joined(separator: " · ")
        case .alias: return rule["replacement"].string.replacingOccurrences(of: "\n", with: " ⏎ ")
        case .macro: return rule["command"].string
        case .variable: return rule["value"].string
        }
    }

    /// The rule as it should be saved: stray spaces gone, the key in its standard spelling.
    func cleaned(_ rule: JSON) -> JSON {
        var rule = rule
        func trim(_ key: String, keepEmpty: Bool = false, endOnly: Bool = false) {
            guard case .string(let old) = rule[key] else { return }
            var new = old.trimmingCharacters(in: .whitespacesAndNewlines)
            if endOnly, let last = old.lastIndex(where: { !$0.isWhitespace }) { new = String(old[...last]) }
            rule[key] = new.isEmpty && !keepEmpty ? .null : .string(new)
        }
        switch self {
        case .trigger:
            ["name", "spawn", "sound", "script", "speak"].forEach { trim($0) }
            trim("send", endOnly: true)
        case .alias:
            trim("pattern", keepEmpty: true)
            trim("replacement", keepEmpty: true, endOnly: true)
        case .macro:
            trim("key", keepEmpty: true)
            if let chord = chord(rule["key"].string) { rule["key"] = .string(chord) }
        case .variable:
            rule["name"] = .string(rule["name"].string.trimmingCharacters(in: CharacterSet.whitespaces.union(CharacterSet(charactersIn: "%"))))
        }
        return rule
    }
}

/// The chord the way the engine writes it ("control+k" is "Ctrl+K"), or nil if it isn't one.
private func chord(_ text: String) -> String? {
    (try? Core.callAnywhere("keys.parse", ["text": .of(text)]))?.optionalString
}

/// Where a set of rules lives: everywhere, in a world, or with one of its characters.
private struct RuleScope: Hashable {
    var world: String?
    var character: String?

    static let global = RuleScope()

    init(world: String? = nil, character: String? = nil) {
        self.world = world
        self.character = character
    }

    init?(json: JSON) {
        switch json["kind"].string {
        case "global": self.init()
        case "world": self.init(world: json["world"].string)
        case "character": self.init(world: json["world"].string, character: json["character"].string)
        default: return nil
        }
    }

    /// Every scope in the config: all worlds, then each world followed by its characters.
    static func all(in config: JSON) -> [RuleScope] {
        var scopes = [RuleScope.global]
        for world in config["world"].array {
            let name = world["name"].string
            scopes.append(RuleScope(world: name))
            scopes += world["character"].array.map { RuleScope(world: name, character: $0["name"].string) }
        }
        return scopes
    }

    var label: String {
        guard let world else { return "All worlds" }
        return character.map { "\($0) @ \(world)" } ?? world
    }

    /// In the scope menu a character sits indented under its world.
    var menuTitle: String { character.map { "    \($0)" } ?? label }

    private func worldIndex(in config: JSON) -> Int? {
        config["world"].array.firstIndex { $0["name"].string == world }
    }

    private func characterIndex(in world: JSON) -> Int? {
        world["character"].array.firstIndex { $0["name"].string == character }
    }

    /// The rules stored here, or nil if the world or character has gone.
    func rules(in config: JSON) -> JSON? {
        guard world != nil else { return config["rules"] }
        guard let w = worldIndex(in: config) else { return nil }
        guard character != nil else { return config["world"][w]["rules"] }
        guard let c = characterIndex(in: config["world"][w]) else { return nil }
        return config["world"][w]["character"][c]["rules"]
    }

    func setRules(_ rules: JSON, in config: inout JSON) -> Bool {
        guard world != nil else {
            config["rules"] = rules
            return true
        }
        guard let w = worldIndex(in: config) else { return false }
        guard character != nil else {
            config["world"][w]["rules"] = rules
            return true
        }
        guard let c = characterIndex(in: config["world"][w]) else { return false }
        config["world"][w]["character"][c]["rules"] = rules
        return true
    }
}

/// A rule in the working copy. The id is what the list selects, so a row stays selected when
/// it moves and never points at its neighbour after a removal.
private struct RuleItem: Identifiable, Equatable {
    let id = UUID()
    var value: JSON
}

/// The working copy of one scope's rules.
private struct RulesDraft: Equatable {
    /// What the config had for each kind when its rows were loaded.
    private var base: [RuleKind: JSON] = [:]
    private var lists: [RuleKind: [RuleItem]] = [:]

    init(rules: JSON) {
        adopt(rules, force: true)
    }

    subscript(kind: RuleKind) -> [RuleItem] {
        get { lists[kind] ?? [] }
        set { lists[kind] = newValue }
    }

    /// The rows as the config stores them. Nothing at all is null, which removes the key.
    func json(_ kind: RuleKind) -> JSON {
        let values = self[kind].map(\.value)
        if values.isEmpty { return .null }
        guard kind == .variable else { return .array(values) }
        return .object(Dictionary(values.map { ($0["name"].string, JSON.string($0["value"].string)) }, uniquingKeysWith: { _, last in last }))
    }

    func isDirty(_ kind: RuleKind) -> Bool { json(kind) != (base[kind] ?? .null) }
    var isDirty: Bool { RuleKind.allCases.contains(where: isDirty) }

    /// Whether this row was added or edited here, rather than being as the config has it.
    func isEdited(_ rule: JSON, _ kind: RuleKind) -> Bool {
        let old = base[kind] ?? .null
        if kind == .variable { return old.object[rule["name"].string] != .string(rule["value"].string) }
        return !old.array.contains(rule)
    }

    /// Takes the config's rules for every kind that has no edits here (or, with `force`, for
    /// all of them). Rows that already say the same keep their ids.
    mutating func adopt(_ rules: JSON, force: Bool) {
        for kind in RuleKind.allCases {
            var fresh = rules[kind.key]
            if fresh.array.isEmpty && fresh.object.isEmpty { fresh = .null }
            if json(kind) != fresh {
                guard force || !isDirty(kind) else { continue }
                lists[kind] = kind.items(from: fresh)
            }
            base[kind] = fresh
        }
    }
}

private final class RulesModel: NSObject, ObservableObject, NSWindowDelegate {
    @Published var scope: RuleScope {
        didSet {
            load(scope)
            selectFirst()
        }
    }

    @Published var kind = RuleKind.trigger {
        didSet { selectFirst() }
    }

    @Published var selection: UUID?
    @Published var problem: String?
    /// What the last "Add Examples" did, shown where the note about unsaved changes goes.
    @Published var notice: String?
    /// Working copies of every scope looked at since the last save, so switching scope doesn't
    /// lose edits.
    @Published private(set) var drafts: [RuleScope: RulesDraft] = [:] {
        didSet { window?.isDocumentEdited = isDirty }
    }

    weak var window: NSWindow?
    private var watcher: AnyCancellable?

    init(scope: RuleScope) {
        self.scope = scope
        super.init()
        load(scope)
        selectFirst()
        // The config changes behind us when a world is edited or a script sets a variable.
        // Published values arrive before the property changes, hence the hop.
        watcher = AppState.shared.$config.dropFirst().receive(on: DispatchQueue.main).sink { [weak self] _ in self?.configChanged() }
    }

    var isDirty: Bool { drafts.values.contains(where: \.isDirty) }
    func isDirty(_ scope: RuleScope) -> Bool { drafts[scope]?.isDirty ?? false }

    /// Scopes with a working copy, for the menu to keep listing one whose world has gone.
    var draftedScopes: [RuleScope] { drafts.keys.sorted { $0.label < $1.label } }

    var items: [RuleItem] {
        get { drafts[scope]?[kind] ?? [] }
        set {
            problem = nil
            notice = nil
            drafts[scope]?[kind] = newValue
        }
    }

    private var selectedIndex: Int? { items.firstIndex { $0.id == selection } }

    private func load(_ scope: RuleScope) {
        if drafts[scope] == nil, let rules = scope.rules(in: AppState.shared.config) {
            drafts[scope] = RulesDraft(rules: rules)
        }
    }

    private func selectFirst() {
        selection = items.first?.id
    }

    /// A rule by id rather than by position, so a form that outlives its row (or a row that
    /// has moved) can't write to the wrong one.
    func binding(_ id: UUID) -> Binding<JSON> {
        Binding(
            get: { [weak self] in self?.items.first { $0.id == id }?.value ?? .null },
            set: { [weak self] new in
                guard let self, let i = self.items.firstIndex(where: { $0.id == id }), self.items[i].value != new else { return }
                self.items[i].value = new
            }
        )
    }

    // MARK: Editing the list

    /// Adds the engine's example triggers, aliases, macros and variable to the scope showing,
    /// to learn from and change. They go straight into the settings (the engine skips any
    /// that are there already), so changes that aren't saved yet are saved first.
    func addExamples() {
        notice = nil
        guard !isDirty || save() else { return }
        let target: JSON
        if let world = scope.world {
            if let character = scope.character {
                target = ["kind": "character", "world": .of(world), "character": .of(character)]
            } else {
                target = ["kind": "world", "world": .of(world)]
            }
        } else {
            target = ["kind": "global"]
        }
        do {
            let reply = try Core.call("rules.add_examples", ["scope": target])
            // The rows are reloaded when the config's change reaches us (see configChanged).
            AppState.shared.reloadFromEngine()
            notice = reply["message"].string
        } catch {
            problem = error.localizedDescription
        }
    }

    func add() {
        guard drafts[scope] != nil else { return }
        let item = RuleItem(value: kind.blank)
        items.append(item)
        selection = item.id
    }

    func remove() {
        guard let i = selectedIndex else { return }
        items.remove(at: i)
        selection = items.isEmpty ? nil : items[min(i, items.count - 1)].id
    }

    func canMove(by offset: Int) -> Bool {
        guard kind.isOrdered, let i = selectedIndex else { return false }
        return items.indices.contains(i + offset)
    }

    func move(by offset: Int) {
        guard canMove(by: offset), let i = selectedIndex else { return }
        items.swapAt(i, i + offset)
    }

    func move(from source: IndexSet, to destination: Int) {
        items.move(fromOffsets: source, toOffset: destination)
    }

    // MARK: Saving

    private func configChanged() {
        let config = AppState.shared.config
        let index = selectedIndex
        var updated = drafts
        for (scope, draft) in drafts {
            if let rules = scope.rules(in: config) {
                updated[scope]?.adopt(rules, force: false)
            } else if !draft.isDirty {
                updated[scope] = nil
            }
        }
        guard updated != drafts else { return }
        drafts = updated
        if drafts[scope] == nil {
            scope = .global
        } else if selectedIndex == nil, let index, !items.isEmpty {
            // The rows were reloaded with new ids: stay where we were.
            selection = items[min(index, items.count - 1)].id
        }
    }

    private struct Problem {
        let scope: RuleScope
        let kind: RuleKind
        let id: UUID
        let message: String
    }

    /// What's wrong with the rules edited here, if anything. Rules that were already in the
    /// config aren't checked, so an old one can't stop a save of something else.
    private func firstProblem() -> Problem? {
        let order = [scope] + draftedScopes.filter { $0 != scope }
        for scope in order {
            guard let draft = drafts[scope] else { continue }
            for kind in RuleKind.allCases where draft.isDirty(kind) {
                let all = draft[kind]
                for item in all where draft.isEdited(item.value, kind) {
                    if let message = problem(with: item, among: all, kind) {
                        return Problem(scope: scope, kind: kind, id: item.id, message: message)
                    }
                }
            }
        }
        return nil
    }

    private func problem(with item: RuleItem, among all: [RuleItem], _ kind: RuleKind) -> String? {
        let rule = item.value
        switch kind {
        case .trigger:
            return rule["pattern"].string.isEmpty ? "Enter a pattern for the trigger." : patternProblem(rule)
        case .alias:
            if rule["pattern"].string.isEmpty { return "Enter a pattern for the alias." }
            // A plain alias compares the first word, so only a regular expression can be wrong.
            return rule["regex"].bool ? patternProblem(rule) : nil
        case .macro:
            guard let key = chord(rule["key"].string) else {
                return rule["key"].string.isEmpty ? "Record a key for the macro." : "“\(rule["key"].string)” isn’t a key Familiar knows."
            }
            let same = all.filter { chord($0.value["key"].string)?.lowercased() == key.lowercased() }
            return same.count > 1 ? "Two macros use \(key). A key can have only one." : nil
        case .variable:
            let name = rule["name"].string
            if name.isEmpty || !name.allSatisfy({ $0.isLetter || $0.isNumber || $0 == "_" }) {
                return "Variable names can only contain letters, digits and _."
            }
            return all.filter { $0.value["name"].string == name }.count > 1 ? "There are two variables called \(name)." : nil
        }
    }

    @discardableResult
    func save() -> Bool {
        for (scope, draft) in drafts {
            for kind in RuleKind.allCases where draft.isDirty(kind) {
                drafts[scope]?[kind] = draft[kind].map { item in
                    var item = item
                    if draft.isEdited(item.value, kind) { item.value = kind.cleaned(item.value) }
                    return item
                }
            }
        }
        if let found = firstProblem() {
            if scope != found.scope { scope = found.scope }
            if kind != found.kind { kind = found.kind }
            selection = found.id
            problem = found.message
            return false
        }
        var config = AppState.shared.config
        for (scope, draft) in drafts where draft.isDirty {
            guard var rules = scope.rules(in: config) else {
                self.scope = scope
                problem = "\(scope.label) is no longer in your settings, so its rules can’t be saved. Revert to carry on."
                return false
            }
            // Only the kinds edited here, so a variable a script set meanwhile isn't undone.
            for kind in RuleKind.allCases where draft.isDirty(kind) {
                rules[kind.key] = draft.json(kind)
            }
            _ = scope.setRules(rules.object.isEmpty ? .null : rules, in: &config)
        }
        problem = nil
        guard AppState.shared.save(config) else { return false }
        for scope in drafts.keys {
            if let rules = scope.rules(in: config) { drafts[scope]?.adopt(rules, force: true) }
        }
        return true
    }

    private func discard() {
        drafts = [:]
        problem = nil
        if scope.rules(in: AppState.shared.config) == nil {
            scope = .global
        } else {
            load(scope)
            selectFirst()
        }
    }

    func revert() {
        guard isDirty else { return }
        let alert = NSAlert()
        alert.messageText = "Discard the changes you haven’t saved?"
        alert.informativeText = "Your triggers, aliases, macros and variables go back to how they were last saved."
        alert.addButton(withTitle: "Discard")
        alert.addButton(withTitle: "Cancel")
        alert.buttons.first?.hasDestructiveAction = true
        if alert.runModal() == .alertFirstButtonReturn { discard() }
    }

    /// Asks what to do with unsaved changes. False means carry on editing.
    func resolveUnsaved() -> Bool {
        guard isDirty else { return true }
        let alert = NSAlert()
        alert.messageText = "Save the changes to your rules?"
        alert.informativeText = "If you don’t, the changes you made in this window are lost."
        alert.addButton(withTitle: "Save")
        alert.addButton(withTitle: "Cancel")
        alert.addButton(withTitle: "Don’t Save")
        switch alert.runModal() {
        case .alertFirstButtonReturn:
            return save()
        case .alertThirdButtonReturn:
            discard()
            return true
        default:
            return false
        }
    }

    func windowShouldClose(_ sender: NSWindow) -> Bool {
        resolveUnsaved()
    }
}

// MARK: Window

private struct RulesView: View {
    @ObservedObject var model: RulesModel
    @ObservedObject private var app = AppState.shared

    private var scopes: [RuleScope] {
        let saved = RuleScope.all(in: app.config)
        return saved + model.draftedScopes.filter { !saved.contains($0) }
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            HStack(spacing: 0) {
                sidebar.frame(width: 260)
                Divider()
                detail.frame(maxWidth: .infinity, maxHeight: .infinity)
            }
            Divider()
            footer
        }
        .frame(minWidth: 740, minHeight: 460)
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 12) {
                Picker("Applies to", selection: $model.scope) {
                    ForEach(scopes, id: \.self) { scope in
                        Text(scope.menuTitle + (model.isDirty(scope) ? " (edited)" : "")).tag(scope)
                    }
                }
                .frame(maxWidth: 320)
                .help("Rules can be for every world, for one world, or for one character")
                Spacer()
                Picker("Kind of rule", selection: $model.kind) {
                    ForEach(RuleKind.allCases) { Text($0.title).tag($0) }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .fixedSize()
            }
            Text(model.kind.about).font(.callout).foregroundStyle(.secondary)
        }
        .padding(12)
    }

    private var sidebar: some View {
        let kind = model.kind
        let noun = kind.rawValue
        let move: ((IndexSet, Int) -> Void)? = kind.isOrdered ? { model.move(from: $0, to: $1) } : nil
        return VStack(spacing: 0) {
            List(selection: $model.selection) {
                ForEach(model.items) { item in
                    row(item, kind).tag(item.id)
                }
                .onMove(perform: move)
            }
            .onDeleteCommand { model.remove() }
            Divider()
            HStack(spacing: 2) {
                Button { model.add() } label: { Image(systemName: "plus") }
                    .help("Add a \(noun)")
                    .accessibilityLabel("Add a \(noun)")
                Button { model.remove() } label: { Image(systemName: "minus") }
                    .disabled(model.selection == nil)
                    .help("Remove the \(noun)")
                    .accessibilityLabel("Remove the \(noun)")
                Spacer()
                if kind.isOrdered {
                    Button { model.move(by: -1) } label: { Image(systemName: "chevron.up") }
                        .disabled(!model.canMove(by: -1))
                        .help("Move the \(noun) up")
                        .accessibilityLabel("Move the \(noun) up")
                    Button { model.move(by: 1) } label: { Image(systemName: "chevron.down") }
                        .disabled(!model.canMove(by: 1))
                        .help("Move the \(noun) down")
                        .accessibilityLabel("Move the \(noun) down")
                }
            }
            .buttonStyle(.borderless)
            .padding(6)
        }
    }

    private func row(_ item: RuleItem, _ kind: RuleKind) -> some View {
        let rule = item.value
        let summary = kind.summary(of: rule)
        return HStack(spacing: 6) {
            if kind.hasEnabled {
                Toggle("Enabled", isOn: model.binding(item.id).flag("enabled"))
                    .toggleStyle(.checkbox)
                    .labelsHidden()
                    .help("Enabled")
            }
            VStack(alignment: .leading, spacing: 1) {
                Text(kind.title(of: rule)).lineLimit(1)
                if !summary.isEmpty {
                    Text(summary).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                }
            }
            .opacity(kind.hasEnabled && !rule["enabled"].bool ? 0.5 : 1)
        }
    }

    @ViewBuilder private var detail: some View {
        if let id = model.selection, model.items.contains(where: { $0.id == id }) {
            let rule = model.binding(id)
            Group {
                switch model.kind {
                case .trigger: TriggerForm(rule: rule)
                case .alias: AliasForm(rule: rule)
                case .macro: MacroForm(rule: rule)
                case .variable: VariableForm(rule: rule)
                }
            }
            .formStyle(.grouped)
            // A new form for each rule, so nothing typed into one shows up in the next.
            .id(id)
        } else {
            Text(model.items.isEmpty ? "No \(model.kind.title.lowercased()) here yet. Add one with the + button." : "Select one to edit it.")
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .padding()
        }
    }

    private var footer: some View {
        HStack {
            if let problem = model.problem {
                Label(problem, systemImage: "exclamationmark.triangle.fill").foregroundStyle(.red).font(.callout)
            } else if model.isDirty {
                Text("You have changes that aren’t saved yet.").foregroundStyle(.secondary).font(.callout)
            } else if let notice = model.notice {
                Text(notice).foregroundStyle(.secondary).font(.callout)
            }
            Spacer()
            Button("Add Examples") { model.addExamples() }
                .help("Adds a few triggers, aliases and macros to learn from and change. Triggers and aliases start switched off.")
            Button("Revert") { model.revert() }
                .disabled(!model.isDirty)
                .help("Discard the changes you haven’t saved")
            Button("Save") { model.save() }
                .keyboardShortcut("s", modifiers: .command)
                .disabled(!model.isDirty)
                .help("Save the changes; open worlds pick them up straight away")
        }
        .padding(12)
    }
}

// MARK: Forms

private extension Binding where Value == JSON {
    /// A flag that's stored only when it's on, which is how Rust writes these.
    func sparse(_ key: String) -> Binding<Bool> {
        Binding<Bool>(get: { wrappedValue[key].bool }, set: { wrappedValue[key] = $0 ? .bool(true) : .null })
    }
}

/// What the engine says is wrong with a trigger's or alias's pattern, or nil if it compiles.
private func patternProblem(_ rule: JSON) -> String? {
    do {
        return try Core.callAnywhere("rules.check_pattern", ["rule": rule]).optionalString.map { "Invalid pattern: \($0)" }
    } catch {
        return "Invalid pattern: \(error.localizedDescription)"
    }
}

/// A caution under a pattern that won't compile. A pattern that's still empty isn't nagged
/// about here; saving says so.
private struct PatternCaution: View {
    let rule: JSON

    var body: some View {
        if !rule["pattern"].string.isEmpty, let problem = patternProblem(rule) {
            Label(problem, systemImage: "exclamationmark.triangle")
                .font(.callout)
                .foregroundStyle(.orange)
        }
    }
}

/// A line to try a rule on, and what the rule would do with it.
private struct TestSection: View {
    let prompt: String
    let resultTitle: String
    let idle: String
    @Binding var text: String
    let result: (String) -> String

    var body: some View {
        Section("Test") {
            TextField(prompt, text: $text)
                .autocorrectionDisabled()
            LabeledContent(resultTitle) {
                Text(text.isEmpty ? idle : result(text))
                    .textSelection(.enabled)
                    .multilineTextAlignment(.trailing)
            }
        }
    }
}

private struct TriggerForm: View {
    @Binding var rule: JSON

    private var highlight: Binding<JSON> { $rule.child("highlight") }
    private var highlighting: Bool { !rule["highlight"].isNull }
    private var replacing: Bool { !rule["replace"].isNull }
    @State private var testLine = ""

    private func test(_ line: String) -> String {
        do {
            let reply = try Core.callAnywhere("rules.test_trigger", ["rule": rule, "text": .of(line)])
            return reply["matched"].bool ? reply["result"].string : "No match"
        } catch {
            return "Invalid pattern: \(error.localizedDescription)"
        }
    }

    var body: some View {
        Form {
            Section("Match") {
                TextField("Name", text: $rule.optionalStr("name"), prompt: Text("Optional"))
                    .help("A label to show in the list instead of the pattern")
                TextField("Pattern", text: $rule.str("pattern"))
                    .font(.body.monospaced())
                    .autocorrectionDisabled()
                PatternCaution(rule: rule)
                Toggle("Regular expression", isOn: $rule.sparse("regex"))
                    .help("Treat the pattern as a regular expression (PCRE-like syntax, with lookaround)")
                Toggle("Match case", isOn: $rule.sparse("case_sensitive"))
                Toggle("Whole words only", isOn: $rule.sparse("whole_word"))
                    .disabled(rule["regex"].bool)
                    .help("For plain-text patterns: only match whole words")
                Toggle("Enabled", isOn: $rule.flag("enabled"))
                Toggle(isOn: $rule.sparse("stop")) {
                    Text("Stop here")
                    Text("Don’t run later triggers on lines this one matches")
                }
            }
            TestSection(prompt: "Test with a line from the game", resultTitle: "Result", idle: "Type a line above", text: $testLine, result: test)
            Section("Display") {
                Toggle("Hide the line", isOn: $rule.sparse("gag"))
                    .help("Hide the line from the output window")
                Toggle("Leave it out of the log", isOn: $rule.sparse("gag_log"))
                // An empty highlight is still a highlight, so this is on whenever the key is there.
                Toggle("Highlight", isOn: Binding(get: { highlighting }, set: { rule["highlight"] = $0 ? ["fg": "#ffd700"] : .null }))
                if highlighting {
                    ColorRow(title: "Text colour", fallback: "#ffd700", hex: highlight.child("fg"))
                    ColorRow(title: "Background colour", fallback: "#3a3a7a", hex: highlight.child("bg"))
                    Toggle("Bold", isOn: highlight.sparse("bold"))
                    Toggle("Italic", isOn: highlight.sparse("italic"))
                    Toggle("Underline", isOn: highlight.sparse("underline"))
                    Toggle(isOn: highlight.sparse("whole_line")) {
                        Text("Whole line")
                        Text("Otherwise only the matching text")
                    }
                    Toggle(isOn: highlight.sparse("groups")) {
                        Text("Only the groups")
                        Text("With a regular expression that has groups, style just those")
                    }
                }
                // Replacing with nothing deletes the match, so off has to be a missing key
                // rather than an empty string.
                Toggle(isOn: Binding(get: { replacing }, set: { rule["replace"] = $0 ? .string("") : .null })) {
                    Text("Replace the matching text")
                    Text("For example to censor words; $1 and %name% work")
                }
                if replacing {
                    TextField("With", text: Binding(get: { rule["replace"].string }, set: { rule["replace"] = .string($0) }), prompt: Text("Nothing, which removes it"))
                        .autocorrectionDisabled()
                }
            }
            Section {
                CommandEditor(text: $rule.optionalStr("send"), label: "Commands to send")
                    .frame(height: 84)
            } header: {
                Text("Send")
            } footer: {
                Text("Commands to send, one per line. $1…$9 and ${name} are regex groups, %name% is a variable.")
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            Section("Also") {
                TextField("Copy to spawn window", text: $rule.optionalStr("spawn"), prompt: Text("Window name"))
                    .help("Copy the line to the spawn window with this name")
                HStack {
                    TextField("Play sound file", text: $rule.optionalStr("sound"), prompt: Text("None"))
                    Button("Choose…") {
                        let panel = NSOpenPanel()
                        panel.title = "Choose a Sound"
                        panel.allowedContentTypes = [.audio]
                        panel.allowsOtherFileTypes = true
                        if panel.runModal() == .OK, let url = panel.url { rule["sound"] = .string(url.path) }
                    }
                }
                TextField("Speak aloud", text: $rule.optionalStr("speak"), prompt: Text("$0 is the whole line"))
                    .help("Text to speak, with $1 and %name% filled in; $0 speaks the whole line")
                TextField("Call script function", text: $rule.optionalStr("script"), prompt: Text("Function name"))
                    .font(.body.monospaced())
                    .autocorrectionDisabled()
                    .help("A function in your script, called with the line and the regex groups. Start with beipmu: to call it the way BeipMU did: (ranges, line, window)")
                Toggle("Show a notification", isOn: $rule.sparse("notify"))
                    .help("Show a desktop notification with the line")
                Toggle(isOn: $rule.sparse("important")) {
                    Text("Mark the tab as important")
                    Text("Shows a star on the tab until you look at it")
                }
            }
        }
    }
}

/// A colour that can be left unset: a well and a switch at the end of one row.
private struct ColorRow: View {
    let title: String
    let fallback: String
    @Binding var hex: JSON

    private var color: Binding<Color> {
        Binding(
            get: { Color(nsColor: NSColor(hex: hex.string) ?? NSColor(hex: fallback) ?? .textColor) },
            set: { new in
                guard let c = NSColor(new).usingColorSpace(.sRGB) else { return }
                func byte(_ v: CGFloat) -> Int { Int((min(max(v, 0), 1) * 255).rounded()) }
                hex = .string(String(format: "#%02x%02x%02x", byte(c.redComponent), byte(c.greenComponent), byte(c.blueComponent)))
            }
        )
    }

    var body: some View {
        LabeledContent(title) {
            HStack {
                ColorPicker(title, selection: color, supportsOpacity: false)
                    .labelsHidden()
                    .disabled(hex.isNull)
                Toggle("Set the \(title.lowercased())", isOn: Binding(get: { !hex.isNull }, set: { hex = $0 ? .string(fallback) : .null }))
                    .labelsHidden()
            }
        }
    }
}

private struct AliasForm: View {
    @Binding var rule: JSON
    @State private var testInput = ""

    private func test(_ input: String) -> String {
        do {
            let reply = try Core.callAnywhere("rules.test_alias", ["rule": rule, "text": .of(input)])
            guard reply["matched"].bool else { return "No match, so it’s sent as typed" }
            return reply["lines"].array.map(\.string).joined(separator: " ⏎ ")
        } catch {
            return "Invalid pattern: \(error.localizedDescription)"
        }
    }

    var body: some View {
        Form {
            Section {
                TextField("Pattern", text: $rule.str("pattern"))
                    .font(.body.monospaced())
                    .autocorrectionDisabled()
                PatternCaution(rule: rule)
                Toggle("Regular expression", isOn: $rule.sparse("regex"))
                Toggle("Match case", isOn: $rule.sparse("case_sensitive"))
                Toggle(isOn: $rule.sparse("substitute")) {
                    Text("Replace just the matching text")
                    Text("Everywhere it appears, then carry on with later aliases (as BeipMU does)")
                }
                .help("Plain patterns then match anywhere in what you type, not just the first word")
                Toggle(isOn: $rule.sparse("stop")) {
                    Text("Then stop")
                    Text("Skip later aliases when this one matches")
                }
                .disabled(!rule["substitute"].bool)
                Toggle("Enabled", isOn: $rule.flag("enabled"))
            } header: {
                Text("Match")
            } footer: {
                Text("Plain text matches the first word you type: $1…$9 are the words after it and $* is the rest. A regular expression can match anywhere (anchor with ^ and $); $1… are its groups.")
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            Section {
                CommandEditor(text: $rule.str("replacement"), label: "Commands to send instead")
                    .frame(height: 100)
            } header: {
                Text("Send Instead")
            } footer: {
                Text("One command per line. %name% is a variable.")
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            TestSection(prompt: "Test with something you’d type", resultTitle: "Sends", idle: "Type above to try it", text: $testInput, result: test)
        }
    }
}

private struct MacroForm: View {
    @Binding var rule: JSON

    var body: some View {
        let key = rule["key"].string
        Form {
            Section {
                LabeledContent("Key") {
                    HStack {
                        TextField("Key", text: $rule.str("key"), prompt: Text("Not set"))
                            .labelsHidden()
                            .autocorrectionDisabled()
                            .frame(minWidth: 140)
                        KeyRecorder { rule["key"] = .string($0) }
                            .fixedSize()
                            .help("Click, then press the key or combination")
                    }
                }
                if !key.isEmpty, chord(key) == nil {
                    Label("That isn’t a key Familiar knows.", systemImage: "exclamationmark.triangle")
                        .font(.callout)
                        .foregroundStyle(.orange)
                }
                TextField("Command", text: $rule.str("command"))
                    .autocorrectionDisabled()
                    .help("Sent as if you had typed it, so aliases apply")
                Toggle(isOn: $rule.sparse("insert")) {
                    Text("Put it in the input box")
                    Text("Instead of sending it")
                }
            } header: {
                Text("Macro")
            } footer: {
                Text("Click Record, then press the key or combination you want, for example F5, Ctrl+K or a number-pad key. You can also type it: Ctrl, Alt and Shift joined to the key with +.")
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
    }
}

private struct VariableForm: View {
    @Binding var rule: JSON

    var body: some View {
        Form {
            Section {
                TextField("Name", text: $rule.str("name"))
                    .autocorrectionDisabled()
                    .help("Letters, digits and _ only")
                TextField("Value", text: $rule.str("value"))
                    .autocorrectionDisabled()
            } header: {
                Text("Variable")
            } footer: {
                Text("Write %\(rule["name"].string.isEmpty ? "name" : rule["name"].string)% in a trigger’s commands or an alias’s replacement to use it.")
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
    }
}

// MARK: Controls

/// A box for commands, one per line. A plain text view rather than SwiftUI's editor, which
/// would turn the quotes and dashes in a command into typographic ones.
private struct CommandEditor: NSViewRepresentable {
    @Binding var text: String
    let label: String

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    func makeNSView(context: Context) -> NSScrollView {
        let scroll = NSTextView.scrollableTextView()
        scroll.borderType = .bezelBorder
        guard let view = scroll.documentView as? NSTextView else { return scroll }
        view.delegate = context.coordinator
        view.font = .monospacedSystemFont(ofSize: NSFont.systemFontSize, weight: .regular)
        view.isRichText = false
        view.allowsUndo = true
        view.isAutomaticQuoteSubstitutionEnabled = false
        view.isAutomaticDashSubstitutionEnabled = false
        view.isAutomaticTextReplacementEnabled = false
        view.isAutomaticSpellingCorrectionEnabled = false
        view.isContinuousSpellCheckingEnabled = false
        view.textContainerInset = NSSize(width: 2, height: 5)
        view.setAccessibilityLabel(label)
        view.string = text
        return scroll
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        context.coordinator.parent = self
        if let view = scroll.documentView as? NSTextView, view.string != text {
            view.string = text
        }
    }

    final class Coordinator: NSObject, NSTextViewDelegate {
        var parent: CommandEditor

        init(_ parent: CommandEditor) { self.parent = parent }

        func textDidChange(_ notification: Notification) {
            if let view = notification.object as? NSTextView { parent.text = view.string }
        }
    }
}

/// A button that records the next key pressed as a chord for the config.
private struct KeyRecorder: NSViewRepresentable {
    let onCapture: (String) -> Void

    func makeNSView(context: Context) -> KeyRecorderButton {
        let button = KeyRecorderButton(frame: .zero)
        button.onCapture = onCapture
        return button
    }

    func updateNSView(_ button: KeyRecorderButton, context: Context) {
        button.onCapture = onCapture
    }
}

private final class KeyRecorderButton: NSButton {
    var onCapture: ((String) -> Void)?

    private var recording = false {
        didSet {
            title = recording ? "Press a Key…" : "Record"
            invalidateIntrinsicContentSize()
        }
    }

    override init(frame: NSRect) {
        super.init(frame: frame)
        bezelStyle = .rounded
        title = "Record"
        target = self
        action = #selector(toggle)
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) is not used")
    }

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
        // A modifier or dead key on its own isn't a chord yet: keep listening.
        guard let text = Keys.text(event) else { return }
        recording = false
        onCapture?(text)
    }

    override func keyDown(with event: NSEvent) {
        if recording { capture(event) } else { super.keyDown(with: event) }
    }

    // Function keys and Ctrl chords are offered as key equivalents before keyDown, where a
    // menu or button could take them. Command chords are left for the menus: macros don't use Command.
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        guard recording, event.type == .keyDown, window?.firstResponder === self, !event.modifierFlags.contains(.command) else {
            return super.performKeyEquivalent(with: event)
        }
        capture(event)
        return true
    }
}
