import AppKit

/// How a key in the config's spelling ("Cmd+Shift+bracketright", GDK's key names) looks on a
/// Mac: as a menu item's key equivalent, and as text with the modifier symbols ("⇧⌘]").
enum KeyNames {
    struct Chord: Equatable {
        var cmd = false, ctrl = false, alt = false, shift = false
        var key: String
    }

    /// Reads "Cmd+Alt+Right" or "Ctrl++" (the plus key), as the engine does. Nil for a key
    /// without a name or a modifier it doesn't know.
    static func parse(_ text: String) -> Chord? {
        var parts = text.split(separator: "+", omittingEmptySubsequences: false).map { $0.trimmingCharacters(in: .whitespaces) }
        let key: String
        if parts.count >= 2, parts[parts.count - 1].isEmpty, parts[parts.count - 2].isEmpty {
            key = "+"
            parts.removeLast(2)
        } else {
            key = parts.removeLast()
        }
        guard !key.isEmpty else { return nil }
        var chord = Chord(key: key)
        for modifier in parts {
            switch modifier.lowercased() {
            case "cmd", "command", "super": chord.cmd = true
            case "ctrl", "control": chord.ctrl = true
            case "alt", "meta": chord.alt = true
            case "shift": chord.shift = true
            default: return nil
            }
        }
        return chord
    }

    /// Punctuation by its GDK name.
    private static let punctuation: [String: String] = [
        "comma": ",", "period": ".", "slash": "/", "semicolon": ";", "apostrophe": "'", "bracketleft": "[", "bracketright": "]",
        "backslash": "\\", "minus": "-", "equal": "=", "grave": "`", "question": "?", "plus": "+",
    ]

    private static func function(_ code: Int) -> String { String(UnicodeScalar(UInt16(code)).map(Character.init) ?? " ") }

    /// Keys a menu item can show, as the character AppKit wants for them.
    private static let menuKeys: [String: String] = [
        "Return": "\r", "KP_Enter": "\u{3}", "Tab": "\t", "Escape": "\u{1b}", "BackSpace": "\u{8}", "space": " ",
        "Delete": function(NSDeleteFunctionKey), "Insert": function(NSInsertFunctionKey), "Home": function(NSHomeFunctionKey),
        "End": function(NSEndFunctionKey), "Page_Up": function(NSPageUpFunctionKey), "Page_Down": function(NSPageDownFunctionKey),
        "Up": function(NSUpArrowFunctionKey), "Down": function(NSDownArrowFunctionKey), "Left": function(NSLeftArrowFunctionKey),
        "Right": function(NSRightArrowFunctionKey),
    ]

    /// Keys shown as a symbol, the way menus show them.
    private static let symbols: [String: String] = [
        "Return": "↩", "KP_Enter": "⌤", "Tab": "⇥", "Escape": "⎋", "BackSpace": "⌫", "Delete": "⌦", "Home": "↖", "End": "↘",
        "Page_Up": "⇞", "Page_Down": "⇟", "Up": "↑", "Down": "↓", "Left": "←", "Right": "→", "space": "Space",
    ]

    /// The F-key's number ("F3" is 3).
    private static func functionNumber(_ key: String) -> Int? {
        guard key.count >= 2, key.first == "F" || key.first == "f" else { return nil }
        return Int(key.dropFirst()).flatMap { (1...35).contains($0) ? $0 : nil }
    }

    /// A menu item's key equivalent and modifiers, or nil for a key a menu can't show (the
    /// keypad's, which look like the main keys').
    static func menuEquivalent(_ text: String) -> (key: String, modifiers: NSEvent.ModifierFlags)? {
        guard let chord = parse(text) else { return nil }
        let key: String
        if let named = menuKeys.first(where: { $0.key.lowercased() == chord.key.lowercased() })?.value {
            key = named
        } else if let n = functionNumber(chord.key) {
            key = function(NSF1FunctionKey + n - 1)
        } else if let character = punctuation[chord.key.lowercased()] {
            key = character
        } else if chord.key.count == 1 {
            key = chord.key.lowercased()
        } else {
            return nil
        }
        var modifiers: NSEvent.ModifierFlags = []
        if chord.cmd { modifiers.insert(.command) }
        if chord.ctrl { modifiers.insert(.control) }
        if chord.alt { modifiers.insert(.option) }
        if chord.shift { modifiers.insert(.shift) }
        return (key, modifiers)
    }

    /// The key as a Mac shows it: "⌃⌥⇧⌘" then the key ("⇧⌘]", "⌥⌘→", "F3", "⌘Keypad 8").
    static func display(_ text: String) -> String {
        guard let chord = parse(text) else { return text }
        var out = ""
        if chord.ctrl { out += "⌃" }
        if chord.alt { out += "⌥" }
        if chord.shift { out += "⇧" }
        if chord.cmd { out += "⌘" }
        let lower = chord.key.lowercased()
        if let symbol = symbols.first(where: { $0.key.lowercased() == lower })?.value {
            out += symbol
        } else if let character = punctuation[lower] {
            out += character
        } else if lower.hasPrefix("kp_") {
            let rest = String(chord.key.dropFirst(3))
            let keypad = ["Add": "+", "Subtract": "-", "Multiply": "*", "Divide": "/", "Decimal": ".", "Equal": "="]
            out += "Keypad " + (keypad[rest] ?? rest.replacingOccurrences(of: "_", with: " "))
        } else {
            out += chord.key.count == 1 ? chord.key.uppercased() : chord.key.replacingOccurrences(of: "_", with: " ")
        }
        return out
    }
}

/// A small fuzzy matcher for the pickers, as the terminal client's (crates/tui/src/fuzzy.rs):
/// the query's characters must appear in order. Matches at word starts and runs of
/// consecutive characters score higher.
enum Fuzzy {
    static func score(_ query: String, _ text: String) -> Int? {
        if query.isEmpty { return 0 }
        let characters = Array(text)
        let lower = characters.map { $0.lowercased().first ?? $0 }
        var score = 0, at = 0
        var previous: Int?
        var first: Int?
        for q in query.lowercased() where q != " " {
            guard let found = (at..<lower.count).first(where: { lower[$0] == q }) else { return nil }
            let wordStart = found == 0 || !(characters[found - 1].isLetter || characters[found - 1].isNumber)
            score += 10
            if wordStart { score += 20 }
            if previous.map({ $0 + 1 == found }) == true { score += 15 }
            score -= found - at
            if first == nil { first = found }
            previous = found
            at = found + 1
        }
        if first == 0 { score += 30 }
        // Shorter texts win ties.
        return score - characters.count / 8
    }
}

/// What choosing a client command in the palette puts in the input box: the usage up to its
/// first argument ("/delay [every] <time> <command>" gives "/delay ").
func insertText(forUsage usage: String) -> String {
    var literal: [String] = []
    var more = false
    for (i, token) in usage.split(separator: " ").enumerated() {
        if i > 0 && (token.contains(where: { "<[|".contains($0) }) || token == "...") {
            more = true
            break
        }
        let cut = token.firstIndex(of: "<").map { String(token[..<$0]) } ?? String(token)
        literal.append(cut)
        if cut.count < token.count { break }
    }
    return literal.joined(separator: " ") + (more ? " " : "")
}
