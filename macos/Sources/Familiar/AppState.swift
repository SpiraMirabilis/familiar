import AppKit
import SwiftUI

struct Palette {
    let foreground: NSColor
    let background: NSColor
}

/// What the whole app shares: the config (owned by the engine; this is a copy), the open
/// sessions, and what the engine told us at startup.
final class AppState: ObservableObject {
    static let shared = AppState()

    /// The config as JSON, in the shape of config.toml. Change it with `save`.
    @Published private(set) var config: JSON = .null
    private(set) var start: JSON = .null
    private(set) var palette = Palette(foreground: .white, background: .black)
    private(set) var sessions: [Int: Session] = [:]

    var settings: JSON { config["settings"] }
    var worlds: [JSON] { config["world"].array }
    var defaults: JSON { start["defaults"] }

    func launch() {
        Core.start()
        Core.onWake = { [weak self] id in self?.sessions[id]?.poll() }
        do {
            start = try Core.call("start")
        } catch {
            alert("Familiar couldn’t start its engine", error.localizedDescription)
        }
        config = start["config"]
        palette = Palette(
            foreground: NSColor(hex: start["palette"]["foreground"].string) ?? .white,
            background: NSColor(hex: start["palette"]["background"].string) ?? .black
        )
        if let problem = start["load_error"].optionalString {
            alert("Your settings couldn’t be read", "\(problem)\n\nFamiliar is using the defaults and won’t save over your file. Fix it and start Familiar again.")
        }
    }

    /// Replaces the config: saves it and has every open session pick it up.
    @discardableResult
    func save(_ new: JSON) -> Bool {
        config = new
        do {
            _ = try Core.call("config.set", ["config": new])
        } catch {
            alert("Your settings weren’t saved", error.localizedDescription)
            return false
        }
        sessions.values.forEach { $0.configChanged() }
        return true
    }

    /// Changes one thing in the config and saves.
    func update(_ change: (inout JSON) -> Void) {
        var new = config
        change(&new)
        if new != config { save(new) }
    }

    /// The engine changed the config itself (a script set a variable, /images on).
    func reloadFromEngine() {
        if let fresh = try? Core.call("config.get") {
            config = fresh
        }
    }

    func world(named name: String) -> JSON? {
        worlds.first { $0["name"].string == name }
    }

    func register(_ session: Session) { sessions[session.id] = session }
    func unregister(_ session: Session) { sessions[session.id] = nil }

    func alert(_ title: String, _ detail: String) {
        let alert = NSAlert()
        alert.messageText = title
        alert.informativeText = detail
        alert.runModal()
    }

    // MARK: Fonts

    /// The font for a world: its own if it has one, otherwise the one in the settings.
    func font(forWorld name: String) -> NSFont {
        let own = world(named: name)?["font"].string ?? ""
        return Fonts.font(own.trimmingCharacters(in: .whitespaces).isEmpty ? settings["font"].string : own)
    }
}

/// Fonts are stored the way the GTK client writes them ("Fira Mono Bold 12"), so both clients
/// can share a config.
enum Fonts {
    static let defaultSize: CGFloat = 12

    /// Splits "Family Name 12" into the family and the size.
    static func parse(_ description: String) -> (family: String, size: CGFloat) {
        var words = description.split(separator: " ").map(String.init)
        var size = defaultSize
        if let last = words.last, let n = Double(last.replacingOccurrences(of: "px", with: "")), n > 0 {
            size = CGFloat(n)
            words.removeLast()
        }
        return (words.joined(separator: " "), size)
    }

    static func describe(family: String, size: CGFloat) -> String {
        let n = size == size.rounded() ? String(Int(size)) : String(format: "%.1f", size)
        return family.isEmpty ? "Monospace \(n)" : "\(family) \(n)"
    }

    static func font(_ description: String) -> NSFont {
        let (name, size) = parse(description)
        let fallback = NSFont.monospacedSystemFont(ofSize: size, weight: .regular)
        // The generic names a config written on Linux uses.
        if name.isEmpty || ["monospace", "mono"].contains(name.lowercased()) {
            return fallback
        }
        if let exact = NSFont(name: name, size: size) {
            return exact
        }
        // "Fira Mono Bold": a family followed by style words.
        var family = name.split(separator: " ").map(String.init)
        var traits: NSFontTraitMask = []
        while family.count > 1 {
            let last = family.removeLast().lowercased()
            if last == "bold" { traits.insert(.boldFontMask) }
            if last == "italic" || last == "oblique" { traits.insert(.italicFontMask) }
            let candidate = family.joined(separator: " ")
            if let font = NSFontManager.shared.font(withFamily: candidate, traits: traits, weight: 5, size: size) {
                return font
            }
        }
        return NSFontManager.shared.font(withFamily: name, traits: [], weight: 5, size: size) ?? fallback
    }

    /// Fixed-width families, for the font pickers.
    static let monospacedFamilies: [String] = {
        let manager = NSFontManager.shared
        return manager.availableFontFamilies.filter { family in
            guard !family.hasPrefix("."), let font = manager.font(withFamily: family, traits: [], weight: 5, size: 12) else { return false }
            return font.isFixedPitch
        }.sorted()
    }()
}
