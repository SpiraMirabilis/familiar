import AppKit
import SwiftUI

extension NSColor {
    /// "#rrggbb". Anything else is nil.
    convenience init?(hex: String) {
        guard hex.hasPrefix("#"), hex.count == 7, let v = UInt32(hex.dropFirst(), radix: 16) else { return nil }
        self.init(srgbRed: CGFloat((v >> 16) & 0xff) / 255, green: CGFloat((v >> 8) & 0xff) / 255, blue: CGFloat(v & 0xff) / 255, alpha: 1)
    }
}

extension NSAttributedString.Key {
    /// Marks an avatar picture with its beip.line.id, so a later update can find it.
    static let avatarID = NSAttributedString.Key("FamiliarAvatarID")
}

/// The text of one output area (a world's main output, a spawn window, a puppet's tab): the
/// lines themselves, the prompt at the bottom, and the scrollback limit. Views show `storage`.
final class OutputBuffer {
    let storage = NSTextStorage()
    var maxLines: Int
    var font: NSFont {
        didSet { if font != oldValue { refont() } }
    }

    private enum Prompt { case none, partial, terminated }
    private var prompt = Prompt.none
    /// Where the prompt's text starts (before the newline that puts it on its own row).
    private var promptStart = 0
    private var lines = 0
    private var nextPrefix: NSAttributedString?
    private var boldFont: NSFont
    private var italicFont: NSFont
    private var boldItalicFont: NSFont

    private let foreground: NSColor
    private let noticeColor: NSColor
    private let echoColor: NSColor

    init(font: NSFont, maxLines: Int) {
        self.font = font
        self.maxLines = max(maxLines, 100)
        let palette = AppState.shared.palette
        foreground = palette.foreground
        noticeColor = palette.foreground.blended(withFraction: 0.45, of: palette.background) ?? .gray
        echoColor = NSColor(hex: "#e5c07b") ?? .systemYellow
        (boldFont, italicFont, boldItalicFont) = OutputBuffer.variants(of: font)
    }

    private static func variants(of font: NSFont) -> (NSFont, NSFont, NSFont) {
        let manager = NSFontManager.shared
        let bold = manager.convert(font, toHaveTrait: .boldFontMask)
        let italic = manager.convert(font, toHaveTrait: .italicFontMask)
        return (bold, italic, manager.convert(bold, toHaveTrait: .italicFontMask))
    }

    /// Applies a new font to what's already shown, keeping bold and italic.
    private func refont() {
        let old = (boldFont, italicFont, boldItalicFont)
        (boldFont, italicFont, boldItalicFont) = OutputBuffer.variants(of: font)
        storage.beginEditing()
        storage.enumerateAttribute(.font, in: NSRange(location: 0, length: storage.length)) { value, range, _ in
            guard let current = value as? NSFont else { return }
            let traits = current.fontDescriptor.symbolicTraits
            let new: NSFont
            if current == old.2 || (traits.contains(.bold) && traits.contains(.italic)) {
                new = boldItalicFont
            } else if current == old.0 || traits.contains(.bold) {
                new = boldFont
            } else if current == old.1 || traits.contains(.italic) {
                new = italicFont
            } else {
                new = font
            }
            storage.addAttribute(.font, value: new, range: range)
        }
        storage.endEditing()
    }

    private var base: [NSAttributedString.Key: Any] { [.font: font, .foregroundColor: foreground] }

    /// A line from the engine (`json::line` in the bridge) as attributed text.
    func styled(_ line: JSON) -> NSAttributedString {
        let text = line["text"].string as NSString
        let out = NSMutableAttributedString(string: text as String, attributes: base)
        for span in line["spans"].array {
            let range = NSRange(location: span["start"].int, length: span["len"].int)
            guard range.length > 0, NSMaxRange(range) <= text.length else { continue }
            var attributes: [NSAttributedString.Key: Any] = [:]
            if let fg = NSColor(hex: span["fg"].string) { attributes[.foregroundColor] = fg }
            if let bg = NSColor(hex: span["bg"].string) { attributes[.backgroundColor] = bg }
            switch (span["bold"].bool, span["italic"].bool) {
            case (true, true): attributes[.font] = boldItalicFont
            case (true, false): attributes[.font] = boldFont
            case (false, true): attributes[.font] = italicFont
            default: break
            }
            if span["underline"].bool { attributes[.underlineStyle] = NSUnderlineStyle.single.rawValue }
            if span["strike"].bool { attributes[.strikethroughStyle] = NSUnderlineStyle.single.rawValue }
            if let link = span["link"].optionalString { attributes[.link] = link }
            out.addAttributes(attributes, range: range)
        }
        return out
    }

    private func clearPartial() {
        if prompt == .partial, promptStart <= storage.length {
            storage.deleteCharacters(in: NSRange(location: promptStart, length: storage.length - promptStart))
            lines = max(0, lines - 1)
        }
        prompt = .none
    }

    /// Starts a new row, without leaving an empty one at the bottom.
    private func startLine() {
        if storage.length > 0 {
            storage.append(NSAttributedString(string: "\n", attributes: base))
        }
        lines += 1
    }

    private func append(_ text: NSAttributedString) {
        clearPartial()
        startLine()
        if let prefix = nextPrefix {
            storage.append(prefix)
            storage.append(NSAttributedString(string: " ", attributes: base))
            nextPrefix = nil
        }
        storage.append(text)
        trim()
    }

    func appendLine(_ line: JSON) {
        append(styled(line))
    }

    func appendAttributed(_ text: NSAttributedString) {
        append(text)
    }

    /// A client message, shown dimmed.
    func notice(_ text: String) {
        append(NSAttributedString(string: text, attributes: [.font: font, .foregroundColor: noticeColor]))
    }

    /// A command the user sent. It goes on the prompt's line, if there is a prompt.
    func echo(_ text: String) {
        let echoed = NSAttributedString(string: text, attributes: [.font: font, .foregroundColor: echoColor])
        if prompt == .none {
            append(echoed)
        } else {
            prompt = .none
            storage.append(echoed)
        }
    }

    func setPrompt(_ line: JSON, terminated: Bool) {
        clearPartial()
        promptStart = storage.length
        startLine()
        storage.append(styled(line))
        prompt = terminated ? .terminated : .partial
    }

    func clear() {
        storage.setAttributedString(NSAttributedString())
        prompt = .none
        lines = 0
        nextPrefix = nil
    }

    /// Drops the oldest lines once there are too many, a few hundred at a time.
    private func trim() {
        guard lines > maxLines + 200 else { return }
        let text = storage.string as NSString
        var drop = lines - maxLines
        var dropped = 0
        var end = 0
        while drop > 0, end < text.length {
            end = NSMaxRange(text.lineRange(for: NSRange(location: end, length: 0)))
            drop -= 1
            dropped += 1
        }
        storage.deleteCharacters(in: NSRange(location: 0, length: end))
        promptStart = max(0, promptStart - end)
        lines = max(0, lines - dropped)
    }

    // MARK: Pictures

    private func attachment(size: NSSize) -> NSTextAttachment {
        let attachment = NSTextAttachment()
        attachment.image = NSImage(size: NSSize(width: 1, height: 1))
        attachment.bounds = NSRect(origin: .zero, size: size)
        return attachment
    }

    private func redraw(_ attachment: NSTextAttachment) {
        storage.enumerateAttribute(.attachment, in: NSRange(location: 0, length: storage.length)) { value, range, stop in
            if value as? NSTextAttachment === attachment {
                // Touching the attribute makes the layout notice the picture's new size.
                storage.edited(.editedAttributes, range: range, changeInLength: 0)
                stop.pointee = true
            }
        }
    }

    /// Puts an avatar at the start of the next line.
    func setNextAvatar(_ avatar: JSON) {
        let side = max(16, ceil(font.ascender - font.descender))
        let picture = attachment(size: NSSize(width: side, height: side))
        picture.bounds.origin.y = font.descender
        let text = NSMutableAttributedString(attachment: picture)
        var attributes: [NSAttributedString.Key: Any] = [.font: font]
        if let id = avatar["id"].optionalString { attributes[.avatarID] = id }
        if let link = avatar["click_url"].optionalString ?? avatar["url"].optionalString { attributes[.link] = link }
        if let hover = avatar["hover"].optionalString { attributes[.toolTip] = hover }
        text.addAttributes(attributes, range: NSRange(location: 0, length: text.length))
        nextPrefix = text
        load(avatar["url"].optionalString, into: picture)
    }

    private func load(_ url: String?, into picture: NSTextAttachment) {
        guard let url else { return }
        Images.shared.get(url) { [weak self] image in
            picture.image = image
            self?.redraw(picture)
        }
    }

    /// The server changed an avatar (beip.ids): update the ones already shown.
    func updateAvatar(_ avatar: JSON) {
        guard let id = avatar["id"].optionalString else { return }
        var found: [(NSTextAttachment, NSRange)] = []
        storage.enumerateAttribute(.avatarID, in: NSRange(location: 0, length: storage.length)) { value, range, _ in
            if value as? String == id, let picture = storage.attribute(.attachment, at: range.location, effectiveRange: nil) as? NSTextAttachment {
                found.append((picture, range))
            }
        }
        for (picture, range) in found {
            if let link = avatar["click_url"].optionalString ?? avatar["url"].optionalString {
                storage.addAttribute(.link, value: link, range: range)
            }
            if let hover = avatar["hover"].optionalString {
                storage.addAttribute(.toolTip, value: hover, range: range)
            }
            load(avatar["url"].optionalString, into: picture)
        }
    }

    /// A thumbnail on a line of its own, at most 320 by 200, that opens the image when clicked.
    func appendImage(url: String) {
        let picture = attachment(size: .zero)
        let text = NSMutableAttributedString(attachment: picture)
        text.addAttributes([.font: font, .link: url, .toolTip: url], range: NSRange(location: 0, length: text.length))
        append(text)
        Images.shared.get(url) { [weak self] image in
            let size = image.size
            guard size.width > 0, size.height > 0 else { return }
            let scale = min(320 / size.width, 200 / size.height, 1)
            picture.image = image
            picture.bounds = NSRect(x: 0, y: 0, width: size.width * scale, height: size.height * scale)
            self?.redraw(picture)
        }
    }
}

/// Downloads and remembers pictures (avatars, inline images, map tiles).
final class Images {
    static let shared = Images()
    private static let maxCached = 256
    private static let maxBytes = 20 * 1024 * 1024
    private var cache: [String: NSImage] = [:]
    private var order: [String] = []
    private var waiting: [String: [(NSImage?) -> Void]] = [:]

    /// Calls `done` on the main thread once the picture is here, or `failed` if it can't be had.
    func get(_ url: String, failed: (() -> Void)? = nil, _ done: @escaping (NSImage) -> Void) {
        if let image = cache[url] { return done(image) }
        let finish: (NSImage?) -> Void = { image in
            if let image { done(image) } else { failed?() }
        }
        if waiting[url] != nil {
            waiting[url]?.append(finish)
            return
        }
        guard let address = URL(string: url), ["http", "https"].contains(address.scheme?.lowercased() ?? "") else {
            failed?()
            return
        }
        waiting[url] = [finish]
        var request = URLRequest(url: address, timeoutInterval: 30)
        request.setValue("Familiar (MU* client; +https://github.com/SpiraMirabilis/familiar)", forHTTPHeaderField: "User-Agent")
        URLSession.shared.dataTask(with: request) { data, response, _ in
            let expected = response?.expectedContentLength ?? -1
            let capped: Data? = if expected > Int64(Self.maxBytes) {
                nil
            } else if let data, data.count <= Self.maxBytes {
                data
            } else {
                nil
            }
            let image = capped.flatMap(NSImage.init(data:))
            DispatchQueue.main.async {
                let callbacks = self.waiting.removeValue(forKey: url) ?? []
                if let image {
                    if self.cache[url] == nil {
                        self.order.append(url)
                        while self.cache.count >= Self.maxCached, !self.order.isEmpty {
                            let oldest = self.order.removeFirst()
                            self.cache.removeValue(forKey: oldest)
                        }
                    }
                    self.cache[url] = image
                }
                callbacks.forEach { $0(image) }
            }
        }.resume()
    }
}

/// The text view that shows an [`OutputBuffer`]: read-only, selectable, with clickable links.
final class OutputTextView: NSTextView {
    /// Typing here goes to the input box instead.
    var onType: ((NSEvent) -> Bool)?

    override func keyDown(with event: NSEvent) {
        if onType?(event) == true { return }
        super.keyDown(with: event)
    }
}

final class OutputScrollView: NSScrollView, NSTextViewDelegate {
    let textView: OutputTextView
    let buffer: OutputBuffer
    /// Stay at the bottom as lines arrive. Off while the user has scrolled up.
    private var follow = true
    private var scrolling = false
    var onLink: ((String) -> Void)?
    /// The visible size in character cells changed (for telnet NAWS).
    var onGridSize: ((Int, Int) -> Void)?
    private var grid = (0, 0)

    init(buffer: OutputBuffer) {
        self.buffer = buffer
        let layout = NSLayoutManager()
        buffer.storage.addLayoutManager(layout)
        let container = NSTextContainer(size: NSSize(width: 0, height: CGFloat.greatestFiniteMagnitude))
        container.widthTracksTextView = true
        layout.addTextContainer(container)
        // Laying out off-screen scrollback lazily keeps a 10,000 line buffer quick.
        layout.allowsNonContiguousLayout = true
        textView = OutputTextView(frame: .zero, textContainer: container)
        super.init(frame: .zero)

        let palette = AppState.shared.palette
        textView.isEditable = false
        textView.isSelectable = true
        textView.isRichText = true
        textView.drawsBackground = true
        textView.backgroundColor = palette.background
        textView.textContainerInset = NSSize(width: 6, height: 6)
        textView.isVerticallyResizable = true
        textView.isHorizontallyResizable = false
        textView.autoresizingMask = [.width]
        textView.minSize = .zero
        textView.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        textView.usesFindBar = true
        textView.isIncrementalSearchingEnabled = true
        textView.linkTextAttributes = [.underlineStyle: NSUnderlineStyle.single.rawValue, .cursor: NSCursor.pointingHand]
        textView.displaysLinkToolTips = false
        textView.delegate = self
        textView.setAccessibilityLabel("Output")

        documentView = textView
        hasVerticalScroller = true
        drawsBackground = true
        backgroundColor = palette.background
        scrollerKnobStyle = .light
        contentView.postsBoundsChangedNotifications = true
        postsFrameChangedNotifications = true
        let center = NotificationCenter.default
        center.addObserver(self, selector: #selector(scrolled), name: NSView.boundsDidChangeNotification, object: contentView)
        center.addObserver(self, selector: #selector(resized), name: NSView.frameDidChangeNotification, object: self)
        center.addObserver(self, selector: #selector(textChanged), name: NSTextStorage.didProcessEditingNotification, object: buffer.storage)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    deinit {
        NotificationCenter.default.removeObserver(self)
        if let layout = textView.layoutManager {
            buffer.storage.removeLayoutManager(layout)
        }
    }

    private var bottom: CGFloat {
        max(0, (documentView?.frame.height ?? 0) - contentView.bounds.height)
    }

    @objc private func scrolled() {
        // Our own scrolling (and the layout settling) mustn't switch following off.
        if !scrolling {
            follow = contentView.bounds.origin.y >= bottom - 4
        }
    }

    @objc private func resized() {
        if follow { scrollToEnd() }
        reportGrid()
    }

    @objc private func textChanged() {
        guard follow else { return }
        // After the edit has been laid out.
        DispatchQueue.main.async { [weak self] in
            guard let self, self.follow else { return }
            self.scrollToEnd()
        }
    }

    func scrollToEnd() {
        follow = true
        scrolling = true
        textView.layoutManager?.ensureLayout(for: textView.textContainer!)
        contentView.scroll(to: NSPoint(x: 0, y: bottom))
        reflectScrolledClipView(contentView)
        scrolling = false
    }

    func scrollPage(up: Bool) {
        let step = contentView.bounds.height * 0.9
        let y = min(max(contentView.bounds.origin.y + (up ? -step : step), 0), bottom)
        follow = y >= bottom - 4
        scrolling = true
        contentView.scroll(to: NSPoint(x: 0, y: y))
        reflectScrolledClipView(contentView)
        scrolling = false
    }

    private func reportGrid() {
        let font = buffer.font
        let cell = ("M" as NSString).size(withAttributes: [.font: font])
        let lineHeight = textView.layoutManager?.defaultLineHeight(for: font) ?? cell.height
        let inset = textView.textContainerInset
        let width = contentView.bounds.width - inset.width * 2 - (textView.textContainer?.lineFragmentPadding ?? 0) * 2
        let height = contentView.bounds.height - inset.height * 2
        guard cell.width > 0, lineHeight > 0, width > 0, height > 0 else { return }
        let new = (max(1, Int(width / cell.width)), max(1, Int(height / lineHeight)))
        if new != grid {
            grid = new
            onGridSize?(new.0, new.1)
        }
    }

    func fontChanged() {
        grid = (0, 0)
        reportGrid()
        if follow { scrollToEnd() }
    }

    func textView(_ textView: NSTextView, clickedOnLink link: Any, at charIndex: Int) -> Bool {
        let uri = (link as? String) ?? (link as? URL)?.absoluteString
        if let uri { onLink?(uri) }
        return true
    }
}

/// An output area in a SwiftUI layout.
struct OutputArea: NSViewRepresentable {
    let buffer: OutputBuffer
    var onLink: (String) -> Void = { _ in }
    var onGridSize: ((Int, Int) -> Void)?
    var onType: ((NSEvent) -> Bool)?
    /// Hands the view to whoever needs to scroll it.
    var onCreate: ((OutputScrollView) -> Void)?

    func makeNSView(context: Context) -> OutputScrollView {
        let view = OutputScrollView(buffer: buffer)
        view.onLink = onLink
        view.onGridSize = onGridSize
        view.textView.onType = onType
        onCreate?(view)
        DispatchQueue.main.async { view.scrollToEnd() }
        return view
    }

    func updateNSView(_ view: OutputScrollView, context: Context) {
        view.onLink = onLink
        view.onGridSize = onGridSize
        view.textView.onType = onType
    }
}
