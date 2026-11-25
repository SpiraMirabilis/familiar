import AppKit
import Carbon.HIToolbox
import SwiftUI

/// Key presses as the names the engine's macros use ("F5", "KP_8", "Ctrl+K"). They're GDK's
/// names, so macros made in the GTK client work here.
enum Keys {
    private static let named: [Int: String] = [
        kVK_Return: "Return", kVK_ANSI_KeypadEnter: "KP_Enter", kVK_Tab: "Tab", kVK_Escape: "Escape", kVK_Delete: "BackSpace",
        kVK_ForwardDelete: "Delete", kVK_Home: "Home", kVK_End: "End", kVK_PageUp: "Page_Up", kVK_PageDown: "Page_Down",
        kVK_UpArrow: "Up", kVK_DownArrow: "Down", kVK_LeftArrow: "Left", kVK_RightArrow: "Right", kVK_Space: "space",
        kVK_F1: "F1", kVK_F2: "F2", kVK_F3: "F3", kVK_F4: "F4", kVK_F5: "F5", kVK_F6: "F6", kVK_F7: "F7", kVK_F8: "F8",
        kVK_F9: "F9", kVK_F10: "F10", kVK_F11: "F11", kVK_F12: "F12", kVK_F13: "F13", kVK_F14: "F14", kVK_F15: "F15",
        kVK_F16: "F16", kVK_F17: "F17", kVK_F18: "F18", kVK_F19: "F19",
        kVK_ANSI_Keypad0: "KP_0", kVK_ANSI_Keypad1: "KP_1", kVK_ANSI_Keypad2: "KP_2", kVK_ANSI_Keypad3: "KP_3",
        kVK_ANSI_Keypad4: "KP_4", kVK_ANSI_Keypad5: "KP_5", kVK_ANSI_Keypad6: "KP_6", kVK_ANSI_Keypad7: "KP_7",
        kVK_ANSI_Keypad8: "KP_8", kVK_ANSI_Keypad9: "KP_9", kVK_ANSI_KeypadPlus: "KP_Add", kVK_ANSI_KeypadMinus: "KP_Subtract",
        kVK_ANSI_KeypadMultiply: "KP_Multiply", kVK_ANSI_KeypadDivide: "KP_Divide", kVK_ANSI_KeypadDecimal: "KP_Decimal",
        kVK_ANSI_KeypadEquals: "KP_Equal", kVK_ANSI_KeypadClear: "Num_Lock",
    ]

    private static let punctuation: [String: String] = [
        ",": "comma", ".": "period", "/": "slash", ";": "semicolon", "'": "apostrophe", "[": "bracketleft", "]": "bracketright",
        "\\": "backslash", "-": "minus", "=": "equal", "`": "grave",
    ]

    /// The key's name, or nil for something that isn't a key of its own (a modifier, a dead key).
    static func name(_ event: NSEvent) -> String? {
        if let name = named[Int(event.keyCode)] { return name }
        guard let character = event.characters(byApplyingModifiers: [])?.lowercased(), character.count == 1,
              let scalar = character.unicodeScalars.first, scalar.value >= 0x20, !(0xF700...0xF8FF).contains(scalar.value)
        else { return nil }
        return punctuation[character] ?? character
    }

    /// The chord as arguments for the engine's `session.macro` and `keys.match`.
    static func chord(_ event: NSEvent) -> JSON? {
        guard let key = name(event) else { return nil }
        let flags = event.modifierFlags
        return [
            "key": .of(key), "cmd": .of(flags.contains(.command)), "ctrl": .of(flags.contains(.control)), "alt": .of(flags.contains(.option)),
            "shift": .of(flags.contains(.shift)),
        ]
    }

    /// The chord as text for the config ("Ctrl+Shift+F5"), for the macro and shortcut editors.
    static func text(_ event: NSEvent) -> String? {
        guard let key = name(event) else { return nil }
        let flags = event.modifierFlags
        var parts: [String] = []
        if flags.contains(.command) { parts.append("Cmd") }
        if flags.contains(.control) { parts.append("Ctrl") }
        if flags.contains(.option) { parts.append("Alt") }
        if flags.contains(.shift) { parts.append("Shift") }
        parts.append(key.count == 1 ? key.uppercased() : key)
        return parts.joined(separator: "+")
    }
}

/// What was typed before, for Up and Down in the input box.
struct History {
    private var entries: [String] = []
    private var position: Int?
    /// What was being typed when the user started going back.
    private var draft = ""
    var limit: Int

    init(limit: Int) { self.limit = max(limit, 1) }

    mutating func push(_ line: String) {
        position = nil
        guard !line.isEmpty, entries.last != line else { return }
        entries.append(line)
        if entries.count > limit { entries.removeFirst(entries.count - limit) }
    }

    mutating func older(current: String) -> String? {
        guard !entries.isEmpty else { return nil }
        switch position {
        case nil:
            draft = current
            position = entries.count - 1
        case .some(let p) where p > 0:
            position = p - 1
        default:
            return nil
        }
        return entries[position!]
    }

    mutating func newer() -> String? {
        guard let p = position else { return nil }
        if p + 1 < entries.count {
            position = p + 1
            return entries[p + 1]
        }
        position = nil
        return draft
    }

    mutating func reset() { position = nil }

    /// Everything remembered, oldest first.
    var all: [String] { entries }
}

final class InputTextView: NSTextView {
    weak var owner: InputBox?

    override func keyDown(with event: NSEvent) {
        if owner?.handleKey(event) == true { return }
        super.keyDown(with: event)
    }

    // Plain text only, whatever is pasted.
    override func paste(_ sender: Any?) { pasteAsPlainText(sender) }
}

/// The input box under an output area: several lines, history, spell checking, and a password
/// field that takes over when the server turns echo off.
final class InputBox: NSView, NSTextFieldDelegate, NSTextViewDelegate {
    let textView: InputTextView
    private let scroll = NSScrollView()
    private let password = NSSecureTextField()
    private var history: History
    private var height: NSLayoutConstraint!

    /// (text, secret). Secret text is a password: never echoed, logged or kept in history.
    var onSubmit: ((String, Bool) -> Void)?
    /// Sees every key first, for keyboard macros. True means it was used.
    var onKey: ((NSEvent) -> Bool)?
    /// Page Up (true) or Page Down, for scrolling the output.
    var onPage: ((Bool) -> Void)?

    var font: NSFont {
        didSet {
            textView.font = font
            password.font = font
            resize()
        }
    }

    init(font: NSFont, historySize: Int, spellcheck: Bool) {
        self.font = font
        history = History(limit: historySize)
        textView = InputTextView(frame: .zero)
        super.init(frame: .zero)

        textView.owner = self
        textView.delegate = self
        textView.font = font
        textView.isRichText = false
        textView.allowsUndo = true
        textView.isAutomaticQuoteSubstitutionEnabled = false
        textView.isAutomaticDashSubstitutionEnabled = false
        textView.isAutomaticTextReplacementEnabled = false
        textView.isAutomaticSpellingCorrectionEnabled = false
        textView.isAutomaticTextCompletionEnabled = false
        textView.isContinuousSpellCheckingEnabled = spellcheck
        textView.isGrammarCheckingEnabled = false
        textView.textContainerInset = NSSize(width: 4, height: 6)
        textView.drawsBackground = false
        textView.isVerticallyResizable = true
        textView.autoresizingMask = [.width]
        textView.textContainer?.widthTracksTextView = true
        textView.setAccessibilityLabel("Input")

        scroll.documentView = textView
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        scroll.drawsBackground = false
        scroll.borderType = .noBorder

        password.font = font
        password.placeholderString = "Password"
        password.isBordered = false
        password.drawsBackground = false
        password.focusRingType = .none
        password.delegate = self
        password.target = self
        password.action = #selector(submitPassword)
        password.isHidden = true

        for view in [scroll, password] {
            view.translatesAutoresizingMaskIntoConstraints = false
            addSubview(view)
        }
        height = heightAnchor.constraint(equalToConstant: 30)
        NSLayoutConstraint.activate([
            scroll.leadingAnchor.constraint(equalTo: leadingAnchor), scroll.trailingAnchor.constraint(equalTo: trailingAnchor),
            scroll.topAnchor.constraint(equalTo: topAnchor), scroll.bottomAnchor.constraint(equalTo: bottomAnchor),
            password.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 9),
            password.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -9),
            password.centerYAnchor.constraint(equalTo: centerYAnchor),
            height,
        ])
        resize()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    var isPassword: Bool { !password.isHidden }

    var spellcheck: Bool {
        get { textView.isContinuousSpellCheckingEnabled }
        set { textView.isContinuousSpellCheckingEnabled = newValue }
    }

    var historySize: Int {
        get { history.limit }
        set { history.limit = max(newValue, 1) }
    }

    /// What was typed before, oldest first.
    var historyEntries: [String] { history.all }

    func setPasswordMode(_ on: Bool) {
        guard on != isPassword else { return }
        let focused = hasFocus
        password.stringValue = ""
        password.isHidden = !on
        scroll.isHidden = on
        if focused { focus() }
    }

    var hasFocus: Bool {
        guard let responder = window?.firstResponder else { return false }
        return responder === textView || (responder as? NSView)?.isDescendant(of: self) == true
    }

    func focus() {
        window?.makeFirstResponder(isPassword ? password : textView)
    }

    var text: String {
        get { textView.string }
        set {
            textView.string = newValue
            textView.setSelectedRange(NSRange(location: (newValue as NSString).length, length: 0))
            resize()
        }
    }

    /// Types text at the cursor (the user started typing with the output focused).
    func insert(_ text: String) {
        focus()
        if isPassword {
            password.stringValue += text
            password.currentEditor()?.selectedRange = NSRange(location: (password.stringValue as NSString).length, length: 0)
        } else {
            textView.insertText(text, replacementRange: textView.selectedRange())
        }
    }

    /// As tall as its text, from one line up to eight.
    private func resize() {
        guard let layout = textView.layoutManager, let container = textView.textContainer else { return }
        layout.ensureLayout(for: container)
        let line = layout.defaultLineHeight(for: font)
        let used = max(layout.usedRect(for: container).height, line)
        height.constant = min(used, line * 8) + textView.textContainerInset.height * 2
    }

    func textDidChange(_ notification: Notification) { resize() }

    override func layout() {
        super.layout()
        resize()
    }

    @objc private func submitPassword() {
        let secret = password.stringValue
        password.stringValue = ""
        onSubmit?(secret, true)
    }

    private func submit() {
        let entered = textView.string
        text = ""
        history.push(entered)
        onSubmit?(entered, false)
    }

    /// Which line the cursor is on, and how many there are.
    private var cursorLine: (Int, Int) {
        let all = textView.string as NSString
        let before = all.substring(to: min(textView.selectedRange().location, all.length))
        let count = { (s: String) in s.reduce(0) { $1 == "\n" ? $0 + 1 : $0 } }
        return (count(before), count(all as String) + 1)
    }

    fileprivate func handleKey(_ event: NSEvent) -> Bool {
        let flags = event.modifierFlags
        // Composing an accented or CJK character: the keys belong to the input method.
        if textView.hasMarkedText() { return false }
        // Shortcuts have had their turn (ShortcutMonitor); macros come next.
        if onKey?(event) == true { return true }
        if flags.contains(.command) { return false }
        let shift = flags.contains(.shift), control = flags.contains(.control)
        switch Int(event.keyCode) {
        case kVK_Return, kVK_ANSI_KeypadEnter:
            // Shift+Return and Option+Return start a new line.
            if shift || flags.contains(.option) { return false }
            submit()
            return true
        case kVK_UpArrow where control || cursorLine.0 == 0:
            if let older = history.older(current: textView.string) { text = older }
            return true
        case kVK_DownArrow where control || cursorLine.0 == cursorLine.1 - 1:
            if let newer = history.newer() { text = newer }
            return true
        case kVK_PageUp, kVK_PageDown:
            onPage?(Int(event.keyCode) == kVK_PageUp)
            return true
        case kVK_Escape:
            history.reset()
            text = ""
            return true
        default:
            return false
        }
    }
}

struct InputArea: NSViewRepresentable {
    let make: () -> InputBox

    func makeNSView(context: Context) -> InputBox { make() }
    func updateNSView(_ view: InputBox, context: Context) {}
}
