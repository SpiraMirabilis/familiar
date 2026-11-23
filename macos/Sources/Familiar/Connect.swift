import AppKit
import SwiftUI

/// The Connect window: saved worlds, a quick connect for anything typed as host:port, and the
/// built-in directory of public worlds.
struct ConnectView: View {
    @ObservedObject private var app = AppState.shared
    @State private var query = ""
    @State private var selection: String?
    @State private var editing: WorldDraft?
    @State private var directory: [JSON] = []
    @FocusState private var searching: Bool
    let connect: (JSON, String?) -> Void

    struct Row: Identifiable {
        enum Kind { case quick, saved, directory }
        let id: String
        let kind: Kind
        let world: JSON
        var character: String?
        var detail = ""
    }

    private var quick: Row? {
        guard let world = try? Core.callAnywhere("address.parse", ["text": .of(query)]), !world.isNull, world["host"].string.contains(".") || world["host"].string == "localhost" || query.contains(":") || query.contains(" ") else { return nil }
        return Row(id: "quick", kind: .quick, world: world)
    }

    private func matches(_ fields: [String]) -> Bool {
        let words = query.lowercased().split(separator: " ")
        return words.allSatisfy { word in fields.contains { $0.lowercased().contains(word) } }
    }

    private var saved: [Row] {
        app.worlds.filter { matches([$0["name"].string, $0["host"].string]) }.flatMap { world -> [Row] in
            let name = world["name"].string
            let characters = world["character"].array.map { $0["name"].string }.filter { !$0.isEmpty }
            // A world with characters is opened as one of them.
            if characters.isEmpty { return [Row(id: "saved:\(name)", kind: .saved, world: world)] }
            return characters.map { Row(id: "saved:\(name)/\($0)", kind: .saved, world: world, character: $0) }
        }
    }

    private var listed: [Row] {
        directory.filter { matches([$0["name"].string, $0["host"].string, $0["description"].string] + $0["keywords"].array.map(\.string)) }
            .map { entry in
                // A world with a guest account is opened as the guest.
                Row(id: "dir:\(entry["name"].string)", kind: .directory, world: entry["world"], character: entry["world"]["character"][0]["name"].optionalString, detail: entry["description"].string)
            }
    }

    private var rows: [Row] { (quick.map { [$0] } ?? []) + saved + listed }
    private var selected: Row? { rows.first { $0.id == selection } }

    private func address(_ world: JSON) -> String {
        "\(world["host"].string):\(world["port"].int)" + (world["tls"].bool ? " · TLS" : "")
    }

    private func label(_ row: Row) -> some View {
        HStack {
            VStack(alignment: .leading, spacing: 1) {
                switch row.kind {
                case .quick: Text("Connect to \(row.world["name"].string)")
                case .saved: Text(row.character.map { "\($0) @ \(row.world["name"].string)" } ?? row.world["name"].string)
                case .directory: Text(row.world["name"].string)
                }
                if !row.detail.isEmpty {
                    Text(row.detail).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                }
            }
            Spacer()
            Text(address(row.world)).font(.caption).foregroundStyle(.secondary).monospacedDigit()
        }
        .contentShape(Rectangle())
        .onTapGesture(count: 2) { open(row) }
        .onTapGesture { selection = row.id }
    }

    private func open(_ row: Row) {
        connect(row.world, row.character)
    }

    private func delete(_ row: Row) {
        let name = row.world["name"].string
        let alert = NSAlert()
        alert.messageText = "Delete \(name)?"
        alert.informativeText = "Its characters, triggers, aliases and other settings are deleted with it. Its map and logs are kept."
        alert.addButton(withTitle: "Delete")
        alert.addButton(withTitle: "Cancel")
        alert.buttons.first?.hasDestructiveAction = true
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        app.update { $0["world"] = .array($0["world"].array.filter { $0["name"].string != name }) }
    }

    var body: some View {
        VStack(spacing: 0) {
            TextField("Search worlds, or type host:port", text: $query)
                .textFieldStyle(.roundedBorder)
                .focused($searching)
                .padding(12)
                .onSubmit {
                    if let row = selected ?? rows.first { open(row) }
                }
            List(selection: $selection) {
                if let quick {
                    Section("Quick Connect") { label(quick).tag(quick.id) }
                }
                if !saved.isEmpty {
                    Section("My Worlds") {
                        ForEach(saved) { label($0).tag($0.id) }
                    }
                }
                if !listed.isEmpty {
                    Section("Directory") {
                        ForEach(listed) { label($0).tag($0.id) }
                    }
                }
            }
            .listStyle(.inset)
            if let row = selected, !row.detail.isEmpty {
                Divider()
                ScrollView {
                    Text(row.detail).font(.callout).frame(maxWidth: .infinity, alignment: .leading).padding(10).textSelection(.enabled)
                }
                .frame(maxHeight: 84)
            }
            Divider()
            HStack {
                Button("New World…") { editing = WorldDraft(original: nil, world: app.defaults["world"]) }
                if let row = selected {
                    switch row.kind {
                    case .saved:
                        Button("Edit…") { editing = WorldDraft(original: row.world["name"].string, world: row.world) }
                        Button("Delete…", role: .destructive) { delete(row) }
                    case .directory, .quick:
                        Button("Save to My Worlds…") { editing = WorldDraft(original: nil, world: row.world) }
                    }
                }
                Spacer()
                Button("Connect") {
                    if let row = selected ?? rows.first { open(row) }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(rows.isEmpty)
            }
            .padding(12)
        }
        .frame(minWidth: 520, minHeight: 440)
        .sheet(item: $editing) { draft in
            WorldEditor(draft: draft)
        }
        .onAppear {
            searching = true
            if directory.isEmpty {
                directory = (try? Core.callAnywhere("directory"))?.array ?? []
            }
        }
        .onChange(of: query) { _, _ in
            // Keep something selected so Return connects to the best match.
            if selected == nil { selection = rows.first?.id }
        }
    }
}

/// A world being added or edited. `original` is its name in the config, or nil for a new one.
struct WorldDraft: Identifiable {
    let id = UUID()
    let original: String?
    var world: JSON
}

/// Picks a font the way the config stores one ("Menlo 12"). With `optional`, the font can be
/// switched off, which stores an empty string (meaning "use the default").
struct FontPicker: View {
    let title: String
    @Binding var description: String
    var optional = false
    var fallback = ""

    private var shown: (family: String, size: CGFloat) { Fonts.parse(description.isEmpty ? fallback : description) }

    var body: some View {
        if optional {
            Toggle("Use a different font for this world", isOn: Binding(get: { !description.isEmpty }, set: { description = $0 ? fallback : "" }))
        }
        if !optional || !description.isEmpty {
            Picker(title, selection: Binding(get: { shown.family }, set: { description = Fonts.describe(family: $0, size: shown.size) })) {
                // Whatever is set stays in the list even if this Mac doesn't have it.
                if !Fonts.monospacedFamilies.contains(shown.family) {
                    Text(shown.family.isEmpty || shown.family.lowercased() == "monospace" ? "System Monospaced" : shown.family).tag(shown.family)
                }
                ForEach(Fonts.monospacedFamilies, id: \.self) { Text($0).tag($0) }
            }
            Stepper(value: Binding(get: { Double(shown.size) }, set: { description = Fonts.describe(family: shown.family, size: CGFloat($0)) }), in: 6...48, step: 1) {
                Text("Size: \(Int(shown.size)) pt")
            }
        }
    }
}

struct WorldEditor: View {
    @State var draft: WorldDraft
    @State private var character: Int?
    @State private var problem: String?
    @Environment(\.dismiss) private var dismissSheet
    /// Set when the editor is shown as an AppKit sheet (World > Edit World).
    @Environment(\.dismiss_) private var dismissWindow
    @ObservedObject private var app = AppState.shared

    private func dismiss() {
        if let dismissWindow { dismissWindow() } else { dismissSheet() }
    }

    private var world: Binding<JSON> { $draft.world }
    private var characters: [JSON] { draft.world["character"].array }

    private func save() {
        var world = draft.world
        let name = world["name"].string.trimmingCharacters(in: .whitespaces)
        world["name"] = .string(name)
        world["host"] = .string(world["host"].string.trimmingCharacters(in: .whitespaces))
        if name.isEmpty { return problem = "Give the world a name." }
        if world["host"].string.isEmpty { return problem = "Enter the server’s host name." }
        if !(1...65535).contains(world["port"].int) { return problem = "The port must be between 1 and 65535." }
        if name != draft.original, app.world(named: name) != nil { return problem = "You already have a world called \(name)." }
        if world["character"].array.contains(where: { $0["name"].string.trimmingCharacters(in: .whitespaces).isEmpty }) {
            return problem = "Give every character a name."
        }
        var worlds = app.worlds
        if let i = worlds.firstIndex(where: { $0["name"].string == draft.original }) {
            worlds[i] = world
        } else {
            worlds.append(world)
        }
        var config = app.config
        config["world"] = .array(worlds)
        if app.save(config) { dismiss() }
    }

    var body: some View {
        VStack(spacing: 0) {
            TabView {
                general.tabItem { Text("World") }
                charactersTab.tabItem { Text("Characters") }
            }
            .padding(12)
            Divider()
            HStack {
                if let problem { Text(problem).foregroundStyle(.red).font(.callout) }
                Spacer()
                Button("Cancel", role: .cancel) { dismiss() }.keyboardShortcut(.cancelAction)
                Button("Save") { save() }.keyboardShortcut(.defaultAction)
            }
            .padding(12)
        }
        .frame(width: 560, height: 560)
    }

    private var general: some View {
        Form {
            Section("Server") {
                TextField("Name", text: world.str("name"))
                TextField("Host", text: world.str("host"))
                TextField("Port", value: world.num("port"), format: .number.grouping(.never))
                Toggle("Use TLS", isOn: world.flag("tls"))
                Toggle("Verify the server’s certificate", isOn: world.flag("verify_certificate"))
                    .disabled(!draft.world["tls"].bool)
                    .help("Turn off for servers with self-signed certificates")
                Toggle("Connect through the bouncer", isOn: world.flag("bouncer"))
                    .help("The bouncer set up in config.toml keeps the character connected while you're away")
                Picker("Encoding", selection: world.str("encoding")) {
                    ForEach(["Auto", "UTF-8", "Windows-1252", "CP437"], id: \.self) { Text($0).tag($0) }
                }
                .help("Auto reads UTF-8 and falls back to Windows-1252")
                Toggle("Use MCP if the server offers it", isOn: world.flag("mcp"))
            }
            Section("Programs and Scripts") {
                Picker("Program language", selection: world.str("program_language")) {
                    Text("MOO").tag("")
                    Text("MUF").tag("muf")
                    Text("Lua").tag("lua")
                }
                .help("For the code editor (MCP simpleedit and /edit)")
                HStack {
                    TextField("Script file", text: world.str("script"), prompt: Text("Optional"))
                    Button("Choose…") {
                        let panel = NSOpenPanel()
                        panel.allowedContentTypes = [.javaScript]
                        panel.allowsOtherFileTypes = true
                        if panel.runModal() == .OK, let url = panel.url { draft.world["script"] = .string(url.path) }
                    }
                }
                Picker("Web views from the server", selection: Binding<String>(
                    get: { draft.world["allow_webviews"].isNull ? "ask" : draft.world["allow_webviews"].bool ? "allow" : "block" },
                    set: { draft.world["allow_webviews"] = $0 == "ask" ? .null : .bool($0 == "allow") }
                )) {
                    Text("Ask").tag("ask")
                    Text("Allow").tag("allow")
                    Text("Block").tag("block")
                }
            }
            Section("Appearance") {
                FontPicker(title: "Font", description: world.str("font"), optional: true, fallback: app.settings["font"].string)
            }
        }
        .formStyle(.grouped)
    }

    private var charactersTab: some View {
        HStack(spacing: 0) {
            VStack(spacing: 0) {
                List(selection: $character) {
                    ForEach(Array(characters.enumerated()), id: \.offset) { index, c in
                        Text(c["name"].string.isEmpty ? "New character" : c["name"].string).tag(index)
                    }
                }
                Divider()
                HStack(spacing: 2) {
                    Button {
                        draft.world["character"] = .array(characters + [["name": ""]])
                        character = characters.count - 1
                    } label: { Image(systemName: "plus") }
                        .accessibilityLabel("Add a character")
                    Button {
                        if let i = character, characters.indices.contains(i) {
                            var all = characters
                            all.remove(at: i)
                            draft.world["character"] = .array(all)
                            character = all.isEmpty ? nil : min(i, all.count - 1)
                        }
                    } label: { Image(systemName: "minus") }
                        .disabled(character == nil)
                        .accessibilityLabel("Remove the character")
                    Spacer()
                }
                .buttonStyle(.borderless)
                .padding(6)
            }
            .frame(width: 160)
            Divider()
            if let i = character, characters.indices.contains(i) {
                CharacterEditor(character: world.child("character").item(i), newPuppet: app.defaults["puppet"])
            } else {
                Text(characters.isEmpty ? "Add a character to log in automatically and to give it rules and puppets of its own." : "Select a character.")
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                    .padding()
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
    }
}

struct CharacterEditor: View {
    @Binding var character: JSON
    let newPuppet: JSON

    private var puppets: [JSON] { character["puppet"].array }

    var body: some View {
        Form {
            Section {
                TextField("Name", text: $character.str("name"))
                TextField("Login command", text: $character.str("login"), prompt: Text("connect name password"))
                    .help("Sent right after connecting. It’s stored in your settings file as plain text.")
            }
            ForEach(Array(puppets.enumerated()), id: \.offset) { index, puppet in
                let p = $character.child("puppet").item(index)
                Section(puppet["name"].string.isEmpty ? "New Puppet" : "Puppet: \(puppet["name"].string)") {
                    TextField("Name", text: p.str("name"))
                    TextField("Lines start with", text: p.str("receive_prefix"))
                        .help("Lines starting with this (or matching it, as a regular expression) belong to the puppet")
                    Toggle("That’s a regular expression", isOn: p.flag("regex"))
                    Toggle("Hide it in the puppet’s tab", isOn: p.flag("hide_prefix"))
                    TextField("Put before what’s typed", text: p.str("send_prefix"), prompt: Text("@force puppet="))
                    Toggle("Don’t add it twice if it was typed", isOn: p.flag("remove_accidental_prefix"))
                    Toggle("Open the tab when a line arrives", isOn: p.flag("auto_open"))
                    Toggle("Open the tab on connecting", isOn: p.flag("open_on_connect"))
                    Button("Remove Puppet", role: .destructive) {
                        var all = puppets
                        all.remove(at: index)
                        character["puppet"] = all.isEmpty ? .null : .array(all)
                    }
                }
            }
            Section {
                Button("Add Puppet") { character["puppet"] = .array(puppets + [newPuppet]) }
            } footer: {
                Text("A puppet is a second character you control through this one’s connection. It gets a tab of its own.")
            }
        }
        .formStyle(.grouped)
    }
}

struct PreferencesView: View {
    @ObservedObject private var app = AppState.shared

    /// A binding to one setting that saves as it changes.
    private func setting<T>(_ key: String, _ get: @escaping (JSON) -> T, _ set: @escaping (T) -> JSON) -> Binding<T> {
        Binding(get: { get(app.settings[key]) }, set: { value in app.update { $0["settings"][key] = set(value) } })
    }

    private func flag(_ key: String) -> Binding<Bool> { setting(key, { $0.bool }, { .bool($0) }) }
    private func number(_ key: String) -> Binding<Int> { setting(key, { $0.int }, { .int(max($0, 0)) }) }

    var body: some View {
        Form {
            Section("Appearance") {
                FontPicker(title: "Font", description: setting("font", { $0.string }, { .string($0) }))
                TextField("Lines of scrollback", value: number("scrollback_lines"), format: .number.grouping(.never))
            }
            Section("Input") {
                Toggle("Show what I send in the output", isOn: flag("local_echo"))
                Toggle("Check spelling as I type", isOn: flag("spellcheck"))
                TextField("Commands to remember", value: number("input_history_size"), format: .number.grouping(.never))
            }
            Section("Output") {
                Toggle("Show pictures under links to images", isOn: flag("inline_images"))
                    .help("Fetching a picture someone posted tells that site your IP address")
                Toggle("Speak lines as they arrive", isOn: flag("speak_output"))
                TextField("Wait for a prompt (milliseconds)", value: number("prompt_timeout_ms"), format: .number.grouping(.never))
                    .help("How long a line without an ending sits before it’s shown as a prompt")
            }
            Section("Logs") {
                Toggle("Put the time before each line", isOn: flag("log_timestamps"))
                LabeledContent("Logs are kept in") {
                    Button(app.start["log_dir"].string) { NSWorkspace.shared.open(URL(fileURLWithPath: app.start["log_dir"].string)) }
                        .buttonStyle(.link)
                }
            }
        }
        .formStyle(.grouped)
        .frame(width: 480, height: 520)
    }
}
