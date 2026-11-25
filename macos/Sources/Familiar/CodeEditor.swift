import AppKit

/// What one level of indentation is, as in the GTK client.
private let indentUnit = "  "
private let indentWidth = 2

// MARK: Languages

private enum EditorLanguage: String, CaseIterable {
    case plain = ""
    case moo, muf, lua

    init(id: String?) {
        self = id.flatMap(EditorLanguage.init(rawValue:)) ?? .plain
    }

    var title: String {
        switch self {
        case .plain: return "Plain Text"
        case .moo: return "MOO (Stunt)"
        case .muf: return "MUF"
        case .lua: return "Lua"
        }
    }

    /// Whether the engine has indentation rules for it (`familiar_code::Lang::from_id`).
    var hasIndentRules: Bool { self != .plain }

    func tokens(_ text: [unichar]) -> [Token] {
        var lexer = Lexer(s: text)
        switch self {
        case .plain: break
        case .moo: lexer.moo()
        case .muf: lexer.muf()
        case .lua: lexer.lua()
        }
        return lexer.out
    }
}

// MARK: Highlighting

private enum TokenKind: UInt8 {
    case comment, string, escape, keyword, number, constant, special, type, builtin, function, directive

    /// System colours, so the theme follows light and dark mode by itself.
    var color: NSColor {
        switch self {
        case .comment: return .secondaryLabelColor
        case .string: return .systemRed
        case .escape: return .systemOrange
        case .keyword: return .systemPurple
        case .number: return .systemBlue
        case .constant: return .systemOrange
        case .special: return .systemBrown
        case .type: return .systemTeal
        case .builtin: return .systemIndigo
        case .function: return .systemPink
        case .directive: return .systemBrown
        }
    }

    var isCode: Bool { self != .comment && self != .string && self != .escape }
}

/// A coloured stretch of text, in UTF-16 offsets. A lexer emits them in order, without overlaps.
private struct Token: Equatable {
    var location: Int
    var length: Int
    var kind: TokenKind
    var end: Int { location + length }
}

private extension unichar {
    static let newline: unichar = 0x0A
    static let quote: unichar = 0x22
    static let backslash: unichar = 0x5C
    static let dot: unichar = 0x2E
    static let minus: unichar = 0x2D

    var isDigit: Bool { self >= 0x30 && self <= 0x39 }
    var isAlpha: Bool { (self >= 0x41 && self <= 0x5A) || (self >= 0x61 && self <= 0x7A) }
    var isHex: Bool { isDigit || (self >= 0x41 && self <= 0x46) || (self >= 0x61 && self <= 0x66) }
    var isWordStart: Bool { isAlpha || self == 0x5F }
    var isWord: Bool { isWordStart || isDigit }
    var isBlank: Bool { self == 0x20 || self == 0x09 }
    var isSpace: Bool { isBlank || (self >= 0x0A && self <= 0x0D) }
    /// ASCII letters in lower case; everything else as it is.
    var lowered: unichar { self >= 0x41 && self <= 0x5A ? self + 0x20 : self }
}

/// An ASCII character as a UTF-16 unit. (`unichar("x")` would be the integer parser.)
private func ch(_ scalar: Unicode.Scalar) -> unichar { unichar(scalar.value) }

private func words(_ list: String) -> Set<String> {
    Set(list.split(whereSeparator: { $0 == " " || $0 == "\n" }).map(String.init))
}

/// The word lists are those of the GTK client's moo.lang and muf.lang.
private enum Words {
    static let mooKeywords = words("if else elseif endif for in endfor fork endfork return while endwhile try except finally endtry break continue any")
    static let mooTypes = words("num obj str list err int float map anon bytes color")
    static let mooVariables = words("player this caller verb args argstr dobj dobjstr prepstr iobj iobjstr")
    static let mooErrors = words("""
        e_none e_type e_div e_perm e_propnf e_verbnf e_varnf e_invind e_recmove e_maxrec e_range e_args e_nacc e_invarg e_quota
        e_float e_file e_exec e_intrpt
        """)
    static let mooBuiltins = words("""
        abs acos add_property add_verb ancestors asin atan binary_hash binary_hmac binary_to_bytes boot_player
        buffered_output_length bytes_to_binary caller_perms callers ceil children chparent chparents chr clear_property color
        connected_players connected_seconds connection_name connection_option connection_options cos cosh crypt ctime
        db_disk_size decode decode_base64 decode_binary decompiled_verb_code delete_property delete_verb descendants
        disassemble dump_database encode encode_base64 encode_binary equal eval exec exp file_chmod file_close file_eof
        file_flush file_last_access file_last_change file_last_modify file_list file_mkdir file_mode file_name file_open
        file_openmode file_read file_readline file_readlines file_remove file_rename file_rmdir file_seek file_size file_stat
        file_tell file_type file_version file_write file_writeline floatstr floor flush_input force_input function_info
        gc_stats generate_json getenv idle_seconds index isa is_clear_property is_member is_player kill_task length listappend
        listdelete listen listeners listinsert listset load_server_options log log10 log_cache_stats mapdelete mapkeys
        mapvalues match max max_object memory_usage min mysql_close mysql_connect mysql_connections mysql_escape_string
        mysql_field_names mysql_next_row mysql_ping mysql_query mysql_set_options mysql_status notify object_bytes
        open_network_connection ord output_delimiters parent parents parse_json pass players process_id properties
        property_info queued_tasks queue_info raise random random_bytes read read_http read_stdin renumber reset_max_object
        respond_to resume rindex rmatch run_gc salt seconds_left server_log server_version setadd set_connection_option
        set_player_flag set_property_info setremove set_task_local set_task_perms set_verb_args set_verb_code set_verb_info
        shutdown sin sinh sqlite_close sqlite_column_names sqlite_delete sqlite_finalize sqlite_handles sqlite_next_row
        sqlite_open sqlite_prepare sqlite_query sqlite_status sqrt strcmp string_hash string_hmac strsub strtr substitute
        suspend switch_player tan tanh task_id task_local task_perms task_stack ticks_left time tofloat toint toliteral tonum
        toobj tostr trunc typeof unlisten valid value_bytes value_hash value_hmac verb_args verb_cache_stats verb_code
        verb_info verbs
        """)

    static let mufDirectives = words("""
        $include $define $enddef $def $undef $ifdef $ifndef $else $endif $iflib $ifnlib $ifver $ifnver $iflibver $ifnlibver
        $author $version $lib-version $note $echo $abort $pubdef $libdef $cleardefs $language $warn
        """)
    static let mufKeywords = words("""
        if else then begin while until repeat continue break for foreach exit try catch catch_detailed endcatch abort public
        wizcall lvar var var! jmp call execute
        """)
    static let mufVariables = words("me loc trigger command")
    static let mufPrimitives = words("""
        ?dup -rot number? string? int? float? dbref? array? dictionary? lock? address? thing? room? player? exit? program? ok?
        flag? propdir? ispid? awake?
        pop dup dupn ldup popn over pick put rot rotate swap nip tuck depth fulldepth reverse lreverse abs sign int float round
        floor ceil sqrt pow exp log log10 sin cos tan asin acos atan atan2 random srand frand gaussian bitand bitor bitxor
        bitshift and or not xor min max strcat strcut strlen strcmp stringcmp strncmp stringpfx instr instring rinstr rinstring
        subst split rsplit explode explode_array toupper tolower striplead striptail strip midstr intostr atoi strtof ftostr
        ftostrc fmtstring smatch notify notify_except notify_exclude notify_descriptor ansi_strlen ansi_strcut ansi_strip
        ansi_midstr textattr name setname location contents next exits owner pennies addpennies moveto dbtop dbcmp set mlevel
        truename getlink setlink newobject newroom newexit copyobj recycle getpropstr getpropval getpropfval getprop setprop
        addprop remove_prop envprop envpropstr nextprop parseprop array_make array_make_dict array_count array_first array_last
        array_next array_prev array_getitem array_setitem array_appenditem array_insertitem array_delitem array_getrange
        array_setrange array_delrange array_keys array_vals array_explode array_join array_sort array_reverse array_union
        array_intersect array_diff array_findval array_excludeval array_matchkey array_matchval array_extract array_cut
        array_nunion array_compare array_notify array_get_propvals array_put_propvals time systime date timesplit timefmt
        sleep pid kill background foreground preempt instances fork queue event_wait event_send event_count timer_start
        timer_stop read tread read_wants_blanks descr descrcon descriptors descr_array conhost condbref concount online
        online_array interp
        """)

    static let luaKeywords = words("and break do else elseif end for function goto if in local not or repeat return then until while")
    static let luaConstants = words("true false nil")
    static let luaBuiltins = words("""
        assert collectgarbage dofile error getmetatable ipairs load loadstring next pairs pcall print rawequal rawget rawlen
        rawset require select setmetatable tonumber tostring type unpack xpcall string table math os io coroutine utf8 debug
        self _G
        """)
}

private struct Lexer {
    let s: [unichar]
    var out: [Token] = []

    private var n: Int { s.count }

    /// The character at `i`, or 0 outside the text.
    private func at(_ i: Int) -> unichar { i >= 0 && i < s.count ? s[i] : 0 }

    private mutating func emit(_ kind: TokenKind, _ from: Int, _ to: Int) {
        if to > from { out.append(Token(location: from, length: to - from, kind: kind)) }
    }

    /// The text of `from..<to`, with ASCII letters in lower case: all three languages' word
    /// lists are matched that way (MOO and MUF ignore case).
    private func word(_ from: Int, _ to: Int, lowercase: Bool = true) -> String {
        String(decoding: s[from..<to].map { lowercase ? $0.lowered : $0 }, as: UTF16.self)
    }

    private func skipBlanks(_ i: Int) -> Int {
        var j = i
        while at(j).isBlank { j += 1 }
        return j
    }

    private func skipDigits(_ i: Int) -> Int {
        var j = i
        while at(j).isDigit { j += 1 }
        return j
    }

    /// The end of an exponent (`e`, an optional sign, digits) starting at `i`, or `i` itself.
    private func exponent(_ i: Int, letter: unichar = ch("e")) -> Int {
        guard at(i).lowered == letter else { return i }
        var j = i + 1
        if at(j) == ch("+") || at(j) == .minus { j += 1 }
        return at(j).isDigit ? skipDigits(j) : i
    }

    private enum Escapes { case moo, any }

    /// How long the escape sequence starting at the backslash at `i` is; 0 if it isn't one.
    private func escapeLength(_ i: Int, _ escapes: Escapes) -> Int {
        let next = at(i + 1)
        switch escapes {
        case .any:
            return next == 0 || next == .newline ? 0 : 2
        case .moo:
            switch next.lowered {
            case ch("x"): return at(i + 2).isHex && at(i + 3).isHex ? 4 : 0
            case ch("u"): return (2...5).allSatisfy { at(i + $0).isHex } ? 6 : 0
            case ch("n"), ch("r"), ch("t"), ch("e"), ch("0"), .quote, .backslash: return 2
            default: return 0
            }
        }
    }

    /// A string that ends at its closing quote or at the end of the line. `start` is where its
    /// colour begins (a MOO bytes literal has a prefix) and `open` is its opening quote.
    /// Returns where it ends.
    private mutating func string(from start: Int, open: Int, escapes: Escapes) -> Int {
        let quote = s[open]
        var segment = start
        var i = open + 1
        while i < n {
            let c = s[i]
            if c == .newline { break }
            if c == quote {
                i += 1
                break
            }
            if c == .backslash {
                let length = escapeLength(i, escapes)
                if length > 0 {
                    emit(.string, segment, i)
                    emit(.escape, i, i + length)
                    i += length
                    segment = i
                } else {
                    // Not a known escape, but it still can't end the string.
                    i += at(i + 1) == 0 || at(i + 1) == .newline ? 1 : 2
                }
                continue
            }
            i += 1
        }
        emit(.string, segment, i)
        return i
    }

    // MARK: MOO

    /// MOO has no line comments; by convention a statement that is only a string is one.
    /// Returns the string's start and the statement's end if the line starting at `i` is one.
    private func mooStringStatement(_ i: Int) -> (start: Int, end: Int)? {
        let start = skipBlanks(i)
        guard at(start) == .quote else { return nil }
        var j = start + 1
        while true {
            let c = at(j)
            if c == 0 || c == .newline { return nil }
            if c == .quote { break }
            if c == .backslash {
                guard at(j + 1) != 0, at(j + 1) != .newline else { return nil }
                j += 1
            }
            j += 1
        }
        j = skipBlanks(j + 1)
        guard at(j) == ch(";") else { return nil }
        let end = j + 1
        j = skipBlanks(end)
        return j >= n || at(j) == .newline ? (start, end) : nil
    }

    mutating func moo() {
        var i = 0
        var lineStart = true
        while i < n {
            let c = s[i]
            if lineStart {
                lineStart = false
                if let statement = mooStringStatement(i) {
                    emit(.comment, statement.start, statement.end)
                    i = statement.end
                    continue
                }
            }
            let previous = at(i - 1)
            if c == .newline {
                lineStart = true
                i += 1
            } else if c == ch("/"), at(i + 1) == ch("*") {
                var j = i + 2
                while j < n, !(s[j] == ch("*") && at(j + 1) == ch("/")) { j += 1 }
                j = min(j + 2, n)
                emit(.comment, i, j)
                i = j
            } else if c == .quote {
                i = string(from: i, open: i, escapes: .moo)
            } else if c == ch("b"), at(i + 1) == .quote, !previous.isWord {
                // Only a lowercase b starts a bytes literal.
                i = string(from: i, open: i + 1, escapes: .moo)
            } else if c == ch("$") {
                var j = i + 1
                if at(j) == ch("#"), at(j + 1).isWord {
                    // A colour literal, $#name.
                    j += 1
                    while at(j).isWord { j += 1 }
                    emit(.special, i, j)
                } else if at(j).isWordStart {
                    while at(j).isWord { j += 1 }
                    emit(.type, i, j)
                }
                i = j
            } else if c == ch("#") {
                var j = i + 1
                if at(j) == .minus { j += 1 }
                if at(j).isDigit {
                    j = skipDigits(j)
                    emit(.special, i, j)
                    i = j
                } else {
                    i += 1
                }
            } else if c.isWordStart {
                var j = i + 1
                while at(j).isWord { j += 1 }
                let name = word(i, j)
                // Built-in variables and types aren't highlighted as property or verb names (x.player).
                let free = previous != .dot && previous != ch(":") && previous != ch("$")
                if Words.mooKeywords.contains(name) {
                    emit(.keyword, i, j)
                } else if Words.mooErrors.contains(name) {
                    emit(.constant, i, j)
                } else if free, Words.mooTypes.contains(name) {
                    emit(.type, i, j)
                } else if free, Words.mooVariables.contains(name) {
                    emit(.special, i, j)
                } else if at(skipBlanks(j)) == ch("(") {
                    // Functions are only highlighted where they're called.
                    if previous == ch(":") {
                        emit(.function, i, j)
                    } else if Words.mooBuiltins.contains(name) {
                        emit(.builtin, i, j)
                    }
                }
                i = j
            } else if c.isDigit {
                var j = skipDigits(i)
                // 1.5, 1. and 1e3 are numbers; the 1 in the range 1..5 is one by itself.
                if at(j) == .dot, at(j + 1) != .dot {
                    j = exponent(skipDigits(j + 1))
                } else {
                    j = exponent(j)
                }
                // Not x.5 (after a single dot), but the 5 in 1..5.
                let afterDot = previous == .dot && at(i - 2) != .dot
                if at(j).isWord {
                    while at(j).isWord { j += 1 }
                } else if !afterDot {
                    emit(.number, i, j)
                }
                i = j
            } else if c == .dot, at(i + 1).isDigit, !previous.isWord, previous != .dot, previous != ch("$"), previous != ch("#") {
                let j = exponent(skipDigits(i + 1))
                if !at(j).isWord { emit(.number, i, j) }
                i = j
            } else {
                i += 1
            }
        }
    }

    // MARK: MUF

    /// -12, 1.5, .5 or 1.5e3 as a whole word.
    private func mufNumber(_ from: Int, _ to: Int) -> Bool {
        var j = from
        if at(j) == .minus { j += 1 }
        let whole = skipDigits(j) - j
        j += whole
        if j == to { return whole > 0 }
        guard at(j) == .dot else { return false }
        let fraction = skipDigits(j + 1) - (j + 1)
        guard whole > 0 || fraction > 0 else { return false }
        j = exponent(j + 1 + fraction)
        return j == to
    }

    /// The end of the whitespace-separated word starting at `i`.
    private func mufWordEnd(_ i: Int) -> Int {
        var j = i
        while j < n, !s[j].isSpace { j += 1 }
        return j
    }

    mutating func muf() {
        var i = 0
        while i < n {
            let c = s[i]
            if c.isSpace {
                i += 1
            } else if c == ch("(") {
                var j = i + 1
                while j < n, s[j] != ch(")") { j += 1 }
                j = min(j + 1, n)
                emit(.comment, i, j)
                i = j
            } else if c == .quote {
                i = string(from: i, open: i, escapes: .any)
            } else {
                // MUF words are anything between whitespace, so "var!" and "?dup" are single words.
                let j = mufWordEnd(i)
                let name = word(i, j)
                i = j
                if name == ":" {
                    // ": name" starts a word definition.
                    emit(.keyword, j - 1, j)
                    var k = j
                    while at(k).isSpace { k += 1 }
                    let end = mufWordEnd(k)
                    emit(.function, k, end)
                    i = end
                } else if name == ";" || Words.mufKeywords.contains(name) {
                    emit(.keyword, j - name.utf16.count, j)
                } else if Words.mufDirectives.contains(name) {
                    emit(.directive, j - name.utf16.count, j)
                } else if Words.mufVariables.contains(name) {
                    emit(.special, j - name.utf16.count, j)
                } else if name.hasPrefix("#"), name.utf16.count > 1, mufNumber(j - name.utf16.count + 1, j), !name.contains(".") {
                    emit(.special, j - name.utf16.count, j)
                } else if mufNumber(j - name.utf16.count, j) {
                    emit(.number, j - name.utf16.count, j)
                } else if Words.mufPrimitives.contains(name) {
                    emit(.builtin, j - name.utf16.count, j)
                }
            }
        }
    }

    // MARK: Lua

    /// The level (number of `=`) of the long bracket `[[`, `[=[`, ... opening at `i`.
    private func luaLongBracket(_ i: Int) -> Int? {
        guard at(i) == ch("[") else { return nil }
        var j = i + 1
        while at(j) == ch("=") { j += 1 }
        return at(j) == ch("[") ? j - i - 1 : nil
    }

    /// Where the long bracket of `level` that's open at `i` ends: after its `]==]`, or at the end.
    private func luaLongEnd(_ i: Int, level: Int) -> Int {
        var j = i
        while j < n {
            if s[j] == ch("]") {
                var k = j + 1
                while k - j - 1 < level, at(k) == ch("=") { k += 1 }
                if k - j - 1 == level, at(k) == ch("]") { return k + 1 }
            }
            j += 1
        }
        return n
    }

    mutating func lua() {
        var i = 0
        while i < n {
            let c = s[i]
            if c == .minus, at(i + 1) == .minus {
                var j = i + 2
                if let level = luaLongBracket(j) {
                    j = luaLongEnd(j + level + 2, level: level)
                } else {
                    while j < n, s[j] != .newline { j += 1 }
                }
                emit(.comment, i, j)
                i = j
            } else if c == .quote || c == ch("'") {
                i = string(from: i, open: i, escapes: .any)
            } else if c == ch("["), let level = luaLongBracket(i) {
                let j = luaLongEnd(i + level + 2, level: level)
                emit(.string, i, j)
                i = j
            } else if c.isWordStart {
                var j = i + 1
                while at(j).isWord { j += 1 }
                // Lua is case-sensitive.
                let name = word(i, j, lowercase: false)
                if Words.luaKeywords.contains(name) {
                    emit(.keyword, i, j)
                } else if Words.luaConstants.contains(name) {
                    emit(.constant, i, j)
                } else if Words.luaBuiltins.contains(name), at(i - 1) != .dot, at(i - 1) != ch(":") {
                    emit(.builtin, i, j)
                }
                i = j
            } else if c.isDigit || (c == .dot && at(i + 1).isDigit && at(i - 1) != .dot) {
                var j = i
                if c == ch("0"), at(i + 1).lowered == ch("x") {
                    j = i + 2
                    while at(j).isHex || at(j) == .dot { j += 1 }
                    j = exponent(j, letter: ch("p"))
                } else {
                    j = skipDigits(j)
                    // Not the dots of 1..2.
                    if at(j) == .dot, at(j + 1) != .dot { j = skipDigits(j + 1) }
                    j = exponent(j)
                }
                emit(.number, i, j)
                i = j
            } else {
                i += 1
            }
        }
    }
}

// MARK: Views

/// A compile error. Its position is a character offset that moves with the text as it's
/// edited (like a GtkSourceView mark), so the marker stays on its line.
private struct ErrorMark {
    var line: Int
    var message: String
    var position: Int
}

private let errorTint = NSColor.systemRed.withAlphaComponent(0.18)

private final class CodeTextView: NSTextView {
    /// Where the lines with compile errors are (any offset in the line).
    var errorPositions: [Int] = [] {
        didSet { needsDisplay = true }
    }

    /// The rectangle, across the whole view, of the line that contains `position`.
    private func lineRect(at position: Int) -> NSRect? {
        guard let layoutManager, let textContainer else { return nil }
        let text = string as NSString
        let position = min(max(position, 0), text.length)
        var rect: NSRect
        if position == text.length, text.length == 0 || text.character(at: text.length - 1) == .newline {
            // The empty last line has no characters to ask about.
            rect = layoutManager.extraLineFragmentRect
        } else {
            let line = text.lineRange(for: NSRange(location: position, length: 0))
            rect = layoutManager.boundingRect(forGlyphRange: layoutManager.glyphRange(forCharacterRange: line, actualCharacterRange: nil), in: textContainer)
        }
        guard rect.height > 0 else { return nil }
        rect.origin.y += textContainerOrigin.y
        rect.origin.x = bounds.minX
        rect.size.width = bounds.width
        return rect
    }

    override func drawBackground(in rect: NSRect) {
        super.drawBackground(in: rect)
        let selection = selectedRange()
        if selection.length == 0, let line = lineRect(at: selection.location), line.intersects(rect) {
            NSColor.labelColor.withAlphaComponent(0.06).setFill()
            line.fill(using: .sourceOver)
        }
        for position in errorPositions {
            guard let line = lineRect(at: position), line.intersects(rect) else { continue }
            errorTint.setFill()
            line.fill(using: .sourceOver)
        }
    }
}

/// Line numbers, with a marker on the lines that have a compile error.
private final class LineNumberRuler: NSRulerView {
    /// The offset each line starts at. Never empty.
    var lineStarts: [Int] = [0] {
        didSet {
            updateThickness()
            needsDisplay = true
        }
    }

    var errors: [ErrorMark] = [] {
        didSet { needsDisplay = true }
    }

    private let font: NSFont
    private let digitWidth: CGFloat
    private let marker = NSImage(systemSymbolName: "exclamationmark.circle.fill", accessibilityDescription: "Error")?
        .withSymbolConfiguration(NSImage.SymbolConfiguration(pointSize: 10, weight: .regular).applying(.init(paletteColors: [.white, .systemRed])))
    /// Tooltips don't retain their owners (here, the message itself).
    private var tips: [NSString] = []

    init(scrollView: NSScrollView, textView: NSTextView, font: NSFont) {
        self.font = NSFont.monospacedDigitSystemFont(ofSize: max(9, (font.pointSize * 0.85).rounded()), weight: .regular)
        digitWidth = ("8" as NSString).size(withAttributes: [.font: self.font]).width
        super.init(scrollView: scrollView, orientation: .verticalRuler)
        clientView = textView
        // Since macOS 14 views can draw outside their bounds, and a ruler's dirty rectangle
        // would paint over the text.
        clipsToBounds = true
        updateThickness()
    }

    @available(*, unavailable)
    required init(coder: NSCoder) { fatalError("not used") }

    private func updateThickness() {
        let digits = max(2, String(lineStarts.count).count)
        let thickness = (digitWidth * CGFloat(digits)).rounded(.up) + 26
        if thickness != ruleThickness { ruleThickness = thickness }
    }

    /// The index of the line that contains the offset `position`.
    func lineIndex(at position: Int) -> Int {
        var low = 0, high = lineStarts.count - 1
        while low < high {
            let middle = (low + high + 1) / 2
            if lineStarts[middle] <= position { low = middle } else { high = middle - 1 }
        }
        return low
    }

    override func draw(_ dirtyRect: NSRect) {
        NSColor.textBackgroundColor.setFill()
        bounds.fill()
        NSColor.separatorColor.setFill()
        NSRect(x: bounds.maxX - 1, y: bounds.minY, width: 1, height: bounds.height).fill()
        drawHashMarksAndLabels(in: dirtyRect)
    }

    override func drawHashMarksAndLabels(in rect: NSRect) {
        guard let textView = clientView as? NSTextView, let layout = textView.layoutManager, let container = textView.textContainer else { return }
        if !tips.isEmpty {
            removeAllToolTips()
            tips = []
        }
        var messages: [Int: String] = [:]
        for error in errors {
            let line = lineIndex(at: error.position)
            messages[line] = (messages[line].map { $0 + "\n" } ?? "") + "Line \(error.line): \(error.message)"
        }
        let length = (textView.string as NSString).length
        let origin = textView.textContainerOrigin
        let visible = textView.visibleRect.offsetBy(dx: -origin.x, dy: -origin.y)
        let characters = layout.characterRange(forGlyphRange: layout.glyphRange(forBoundingRect: visible, in: container), actualGlyphRange: nil)
        let current = lineIndex(at: textView.selectedRange().location)
        var line = lineIndex(at: characters.location)
        while line < lineStarts.count, lineStarts[line] <= NSMaxRange(characters) {
            defer { line += 1 }
            let start = lineStarts[line]
            let fragment = start >= length
                ? layout.extraLineFragmentRect
                : layout.lineFragmentRect(forGlyphAt: layout.glyphIndexForCharacter(at: start), effectiveRange: nil)
            guard fragment.height > 0 else { continue }
            // Converting both edges works whether or not the ruler is flipped.
            let top = convert(NSPoint(x: 0, y: fragment.minY + origin.y), from: textView).y
            let bottom = convert(NSPoint(x: 0, y: fragment.maxY + origin.y), from: textView).y
            let row = NSRect(x: 0, y: min(top, bottom), width: bounds.width - 1, height: abs(bottom - top))
            guard row.intersects(bounds) else { continue }
            if let message = messages[line] {
                errorTint.setFill()
                row.fill(using: .sourceOver)
                if let marker {
                    let size = min(marker.size.height, row.height)
                    marker.draw(in: NSRect(x: 4, y: row.midY - size / 2, width: size, height: size), from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: true, hints: nil)
                }
                let tip = message as NSString
                tips.append(tip)
                addToolTip(row, owner: tip, userData: nil)
            }
            let label = "\(line + 1)" as NSString
            let attributes: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: line == current ? NSColor.labelColor : NSColor.tertiaryLabelColor]
            let size = label.size(withAttributes: attributes)
            label.draw(at: NSPoint(x: bounds.width - 7 - size.width, y: row.midY - size.height / 2), withAttributes: attributes)
        }
    }
}

/// Flipped, so the list of errors starts at the top of its scroll view.
private final class ErrorList: NSStackView {
    override var isFlipped: Bool { true }
}

// MARK: The editor

/// A small code editor for MU* programs: highlighting, line numbers, language-aware
/// auto-indent, search, and compile errors marked on their lines.
final class CodeEditorController: NSWindowController, NSWindowDelegate, NSTextViewDelegate, NSTextStorageDelegate {
    /// Called once when the editor's window has closed.
    var onClose: (() -> Void)?

    private weak var session: Session?
    private let request: JSON
    private var language: EditorLanguage

    private let storage = NSTextStorage()
    private let textView: CodeTextView
    private let scrollView = NSScrollView()
    private let ruler: LineNumberRuler
    private let status = NSTextField(labelWithString: "")
    private let languagePicker = NSPopUpButton(frame: .zero, pullsDown: false)
    private let reindentItem = NSMenuItem(title: "Re-indent", action: #selector(reindent), keyEquivalent: "")
    private let findItem = NSMenuItem(title: "Find…", action: #selector(showFind), keyEquivalent: "")
    private let findNextItem = NSMenuItem(title: "Find Next", action: #selector(findNext), keyEquivalent: "")
    private let findPreviousItem = NSMenuItem(title: "Find Previous", action: #selector(findPrevious), keyEquivalent: "")
    private let sendButton = NSButton(title: "Send", target: nil, action: nil)
    private let errorList = ErrorList()
    private let errorScroll = NSScrollView()
    private var errorHeight: NSLayoutConstraint!

    /// The text as it was last loaded or sent; anything else is an unsent change.
    private var cleanText = ""
    private var modified = false
    private var errors: [ErrorMark] = []
    private var lineStarts: [Int] = [0]

    /// What's coloured now, and the length of the text it was worked out for.
    private var tokens: [Token] = []
    private var highlightedLength = 0
    /// The characters that changed since then.
    private var pendingEdit: NSRange?
    private var bracketsMarked = false

    private static var cascade = NSPoint.zero

    /// `request`: {"reference", "name", "kind": "string"|"string-list"|"moo-code"|"local",
    /// "content", "language": "moo"|"muf"|"lua"|null}
    init(session: Session, request: JSON) {
        self.session = session
        self.request = request
        language = EditorLanguage(id: request["language"].optionalString)
        let font = session.font

        // TextKit 1, built by hand: the ruler and the line tints ask the layout manager.
        let layout = NSLayoutManager()
        storage.addLayoutManager(layout)
        let container = NSTextContainer(size: NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude))
        container.widthTracksTextView = false
        layout.addTextContainer(container)
        textView = CodeTextView(frame: NSRect(x: 0, y: 0, width: 800, height: 560), textContainer: container)
        ruler = LineNumberRuler(scrollView: scrollView, textView: textView, font: font)

        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 820, height: 640), styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.tabbingMode = .disallowed
        window.minSize = NSSize(width: 420, height: 260)
        super.init(window: window)
        window.delegate = self

        let name = request["name"].string
        window.title = name.isEmpty ? "Editor" : name
        window.subtitle = request["kind"].string == "local" ? "Sent line by line" : "\(request["reference"].string) · \(request["kind"].string)"

        setUpTextView(font: font)
        window.contentView = buildContent()
        window.initialFirstResponder = textView
        window.center()
        CodeEditorController.cascade = window.cascadeTopLeft(from: CodeEditorController.cascade)

        NotificationCenter.default.addObserver(self, selector: #selector(scrolled), name: NSView.boundsDidChangeNotification, object: scrollView.contentView)
        NotificationCenter.default.addObserver(self, selector: #selector(showKeys), name: Shortcuts.changed, object: nil)
        setLanguage(language)
        load(request["content"].string)
        window.makeFirstResponder(textView)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    // MARK: Building the window

    private func setUpTextView(font: NSFont) {
        storage.delegate = self
        textView.delegate = self
        textView.isRichText = false
        textView.allowsUndo = true
        textView.font = font
        textView.textColor = .textColor
        textView.backgroundColor = .textBackgroundColor
        textView.insertionPointColor = .textColor
        textView.textContainerInset = NSSize(width: 2, height: 6)
        // Tabs already in a program are four columns wide; the Tab key inserts spaces.
        let paragraph = NSMutableParagraphStyle()
        paragraph.tabStops = []
        paragraph.defaultTabInterval = 4 * (" " as NSString).size(withAttributes: [.font: font]).width
        textView.defaultParagraphStyle = paragraph
        textView.typingAttributes[.paragraphStyle] = paragraph

        // Code isn't prose.
        textView.isAutomaticQuoteSubstitutionEnabled = false
        textView.isAutomaticDashSubstitutionEnabled = false
        textView.isAutomaticTextReplacementEnabled = false
        textView.isAutomaticSpellingCorrectionEnabled = false
        textView.isAutomaticTextCompletionEnabled = false
        textView.isAutomaticLinkDetectionEnabled = false
        textView.isAutomaticDataDetectionEnabled = false
        textView.isContinuousSpellCheckingEnabled = false
        textView.isGrammarCheckingEnabled = false
        textView.smartInsertDeleteEnabled = false
        textView.usesFindBar = true
        textView.isIncrementalSearchingEnabled = true
        textView.setAccessibilityLabel("Program text")

        // Long lines scroll sideways rather than wrap.
        textView.minSize = .zero
        textView.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        textView.isVerticallyResizable = true
        textView.isHorizontallyResizable = true
        textView.autoresizingMask = [.width, .height]

        scrollView.documentView = textView
        scrollView.hasVerticalScroller = true
        scrollView.hasHorizontalScroller = true
        scrollView.autohidesScrollers = true
        scrollView.borderType = .noBorder
        scrollView.verticalRulerView = ruler
        scrollView.hasVerticalRuler = true
        scrollView.rulersVisible = true
        scrollView.contentView.postsBoundsChangedNotifications = true
    }

    private func buildContent() -> NSView {
        errorList.orientation = .vertical
        errorList.alignment = .leading
        errorList.spacing = 2
        errorList.edgeInsets = NSEdgeInsets(top: 6, left: 12, bottom: 6, right: 12)
        errorList.translatesAutoresizingMaskIntoConstraints = false
        errorScroll.documentView = errorList
        errorScroll.hasVerticalScroller = true
        errorScroll.autohidesScrollers = true
        errorScroll.drawsBackground = false
        errorScroll.borderType = .noBorder
        errorScroll.isHidden = true
        errorScroll.setAccessibilityLabel("Compile errors")

        languagePicker.addItems(withTitles: EditorLanguage.allCases.map(\.title))
        languagePicker.target = self
        languagePicker.action = #selector(languagePicked)
        languagePicker.toolTip = "Language"
        languagePicker.setAccessibilityLabel("Language")

        let more = NSPopUpButton(frame: .zero, pullsDown: true)
        more.addItem(withTitle: "")
        more.item(at: 0)?.image = NSImage(systemSymbolName: "ellipsis.circle", accessibilityDescription: "More")
        more.toolTip = "More"
        more.setAccessibilityLabel("More")
        let clear = NSMenuItem(title: "Clear Error Marks", action: #selector(clearErrors), keyEquivalent: "")
        for item in [reindentItem, findItem, findNextItem, findPreviousItem, clear] {
            item.target = self
            more.menu?.addItem(item)
        }
        // Re-indent is enabled by hand, for the languages that have rules.
        more.menu?.autoenablesItems = false

        status.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        status.textColor = .secondaryLabelColor
        status.lineBreakMode = .byTruncatingTail
        status.setContentHuggingPriority(.init(1), for: .horizontal)
        status.setContentCompressionResistancePriority(.init(1), for: .horizontal)

        sendButton.target = self
        sendButton.action = #selector(send)
        sendButton.bezelColor = .controlAccentColor
        showKeys()

        let bar = NSStackView(views: [languagePicker, more, status, sendButton])
        bar.orientation = .horizontal
        // The status line takes the room between the pickers and Send.
        bar.distribution = .fill
        bar.spacing = 8
        bar.edgeInsets = NSEdgeInsets(top: 8, left: 12, bottom: 8, right: 12)

        let separator = NSBox()
        separator.boxType = .separator
        let errorSeparator = NSBox()
        errorSeparator.boxType = .separator

        let root = NSView()
        for view in [scrollView, errorSeparator, errorScroll, separator, bar] as [NSView] {
            view.translatesAutoresizingMaskIntoConstraints = false
            root.addSubview(view)
            NSLayoutConstraint.activate([
                view.leadingAnchor.constraint(equalTo: root.leadingAnchor),
                view.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            ])
        }
        errorHeight = errorScroll.heightAnchor.constraint(equalToConstant: 0)
        errorSeparator.isHidden = true
        self.errorSeparator = errorSeparator
        NSLayoutConstraint.activate([
            scrollView.topAnchor.constraint(equalTo: root.topAnchor),
            errorSeparator.topAnchor.constraint(equalTo: scrollView.bottomAnchor),
            errorScroll.topAnchor.constraint(equalTo: errorSeparator.bottomAnchor),
            errorHeight,
            separator.topAnchor.constraint(equalTo: errorScroll.bottomAnchor),
            bar.topAnchor.constraint(equalTo: separator.bottomAnchor),
            bar.bottomAnchor.constraint(equalTo: root.bottomAnchor),
            errorList.topAnchor.constraint(equalTo: errorScroll.contentView.topAnchor),
            errorList.leadingAnchor.constraint(equalTo: errorScroll.contentView.leadingAnchor),
            errorList.trailingAnchor.constraint(equalTo: errorScroll.contentView.trailingAnchor),
        ])
        return root
    }

    private var errorSeparator: NSBox?

    /// The keymap's keys on the menu and the Send button. The keys work through
    /// `perform`, called by the app's shortcut monitor; the menu only shows them.
    @objc private func showKeys() {
        let shortcuts = Shortcuts.shared
        shortcuts.apply("editor-reindent", to: reindentItem)
        shortcuts.apply("editor-find", to: findItem)
        shortcuts.apply("editor-find-next", to: findNextItem)
        shortcuts.apply("editor-find-previous", to: findPreviousItem)
        let send = shortcuts.display("editor-send")
        sendButton.toolTip = send.isEmpty ? "Send to the server" : "Send to the server (\(send))"
    }

    // MARK: Text

    /// Replaces the text without marking it as changed.
    private func load(_ content: String) {
        let text = content.replacingOccurrences(of: "\r\n", with: "\n").replacingOccurrences(of: "\r", with: "\n")
        textView.string = text
        textView.undoManager?.removeAllActions()
        cleanText = text
        refresh(full: true)
        // Errors reported for the text the server has stay on the lines they were reported for.
        for index in errors.indices {
            errors[index].position = lineStarts[min(max(errors[index].line - 1, 0), lineStarts.count - 1)]
        }
        showErrors()
        textView.setSelectedRange(NSRange(location: 0, length: 0))
        textView.scrollRangeToVisible(NSRange(location: 0, length: 0))
        selectionChanged()
        updateModified()
    }

    /// The server sent the same program again: replace the text unless the user has unsent
    /// changes; bring the window to the front.
    func reload(_ request: JSON) {
        if !modified { load(request["content"].string) }
        if window?.isMiniaturized == true { window?.deminiaturize(nil) }
        window?.makeKeyAndOrderFront(nil)
    }

    private func updateModified() {
        modified = textView.string != cleanText
        window?.isDocumentEdited = modified
    }

    @objc private func send() {
        guard let session else {
            NSSound.beep()
            return
        }
        clearErrors()
        let text = textView.string
        session.saveEdit(request, content: text)
        cleanText = text
        updateModified()
    }

    private func setLanguage(_ language: EditorLanguage) {
        self.language = language
        languagePicker.selectItem(at: EditorLanguage.allCases.firstIndex(of: language) ?? 0)
        reindentItem.isEnabled = language.hasIndentRules
        refresh(full: true)
    }

    @objc private func languagePicked() {
        let all = EditorLanguage.allCases
        setLanguage(all.indices.contains(languagePicker.indexOfSelectedItem) ? all[languagePicker.indexOfSelectedItem] : .plain)
    }

    /// Brings the line table, the ruler and the colours up to date with the text.
    private func refresh(full: Bool) {
        let text = textView.string as NSString
        var characters = [unichar](repeating: 0, count: text.length)
        text.getCharacters(&characters, range: NSRange(location: 0, length: text.length))
        var starts = [0]
        for (index, character) in characters.enumerated() where character == .newline {
            starts.append(index + 1)
        }
        lineStarts = starts
        ruler.lineStarts = starts
        // Colouring text that an input method is still composing would disturb it.
        if !textView.hasMarkedText() { highlight(characters, full: full) }
        showErrorPositions()
    }

    /// Colours what changed. The whole text is tokenized (it's quick, and a block comment can
    /// change everything after it), but only the stretch whose tokens differ from last time
    /// is touched, so typing doesn't lay the whole program out again.
    private func highlight(_ characters: [unichar], full: Bool) {
        let length = characters.count
        let new = language.tokens(characters)
        let old = tokens
        let delta = length - highlightedLength
        let edit = pendingEdit
        tokens = new
        highlightedLength = length
        pendingEdit = nil

        var start = 0
        var end = length
        var changed = new[...]
        if !full {
            guard let edit else { return }
            // Tokens before the edit that are the same as before keep their colour, and so do
            // those after it that only moved.
            var prefix = 0
            while prefix < old.count, prefix < new.count, old[prefix] == new[prefix], new[prefix].end <= edit.location { prefix += 1 }
            var suffix = 0
            while suffix < old.count - prefix, suffix < new.count - prefix {
                let before = old[old.count - 1 - suffix], after = new[new.count - 1 - suffix]
                guard after.location >= NSMaxRange(edit), before.location + delta == after.location, before.length == after.length, before.kind == after.kind else { break }
                suffix += 1
            }
            changed = new[prefix..<(new.count - suffix)]
            let gone = old[prefix..<(old.count - suffix)]
            start = edit.location
            end = NSMaxRange(edit)
            if let first = changed.first, let last = changed.last {
                start = min(start, first.location)
                end = max(end, last.end)
            }
            if let first = gone.first, let last = gone.last {
                start = min(start, first.location)
                end = max(end, last.end + delta)
            }
            start = min(max(start, 0), length)
            end = min(max(end, start), length)
        }
        guard end > start else { return }
        storage.beginEditing()
        storage.addAttribute(.foregroundColor, value: NSColor.textColor, range: NSRange(location: start, length: end - start))
        for token in changed where token.location >= start && token.end <= end {
            storage.addAttribute(.foregroundColor, value: token.kind.color, range: NSRange(location: token.location, length: token.length))
        }
        storage.endEditing()
    }

    func textStorage(_ textStorage: NSTextStorage, didProcessEditing editedMask: NSTextStorageEditActions, range editedRange: NSRange, changeInLength delta: Int) {
        guard editedMask.contains(.editedCharacters) else { return }
        // Where the replaced text ended, in the offsets of before this edit.
        let oldEnd = NSMaxRange(editedRange) - delta
        for index in errors.indices {
            let position = errors[index].position
            if position >= oldEnd {
                errors[index].position = position + delta
            } else if position > editedRange.location {
                errors[index].position = editedRange.location
            }
        }
        if let pending = pendingEdit {
            var pendingEnd = NSMaxRange(pending)
            if oldEnd <= pendingEnd { pendingEnd += delta }
            let start = min(pending.location, editedRange.location)
            pendingEdit = NSRange(location: start, length: max(pendingEnd, NSMaxRange(editedRange), start) - start)
        } else {
            pendingEdit = editedRange
        }
    }

    func textDidChange(_ notification: Notification) {
        refresh(full: false)
        updateModified()
        selectionChanged()
    }

    func textViewDidChangeSelection(_ notification: Notification) {
        selectionChanged()
    }

    @objc private func scrolled() {
        ruler.needsDisplay = true
    }

    /// The current line's highlight, the ruler, the status line and the matching bracket all
    /// follow the cursor.
    private func selectionChanged() {
        textView.needsDisplay = true
        ruler.needsDisplay = true
        markMatchingBracket()
        let cursor = textView.selectedRange().location
        let line = ruler.lineIndex(at: cursor)
        let here = errors.filter { ruler.lineIndex(at: $0.position) == line }
        if here.isEmpty {
            status.stringValue = "Line \(line + 1), Column \(max(cursor - lineStarts[line], 0) + 1)"
            status.textColor = .secondaryLabelColor
            status.toolTip = nil
        } else {
            let message = here.map(\.message).joined(separator: "; ")
            status.stringValue = "Line \(line + 1): \(message)"
            status.textColor = .systemRed
            status.toolTip = message
        }
    }

    // MARK: Brackets

    private func isCode(at position: Int) -> Bool {
        var low = 0, high = tokens.count
        while low < high {
            let middle = (low + high) / 2
            if tokens[middle].end <= position { low = middle + 1 } else { high = middle }
        }
        guard low < tokens.count, tokens[low].location <= position else { return true }
        return tokens[low].kind.isCode
    }

    /// The offset of the bracket that pairs with the one at `position`, skipping strings and comments.
    private func matchingBracket(at position: Int, in text: NSString) -> Int? {
        let pairs: [(open: unichar, close: unichar)] = [(ch("("), ch(")")), (ch("["), ch("]")), (ch("{"), ch("}"))]
        let character = text.character(at: position)
        guard let pair = pairs.first(where: { $0.open == character || $0.close == character }), isCode(at: position) else { return nil }
        let step = character == pair.open ? 1 : -1
        var depth = 0
        var index = position
        // Far enough for any real program without stalling on a huge paste.
        var budget = 100_000
        while index >= 0, index < text.length, budget > 0 {
            let c = text.character(at: index)
            if (c == pair.open || c == pair.close), isCode(at: index) {
                depth += c == character ? 1 : -1
                if depth == 0 { return index }
            }
            index += step
            budget -= 1
        }
        return nil
    }

    private func markMatchingBracket() {
        guard let layout = textView.layoutManager else { return }
        let text = textView.string as NSString
        if bracketsMarked {
            layout.removeTemporaryAttribute(.backgroundColor, forCharacterRange: NSRange(location: 0, length: text.length))
            bracketsMarked = false
        }
        let selection = textView.selectedRange()
        guard selection.length == 0, !textView.hasMarkedText() else { return }
        for position in [selection.location, selection.location - 1] where position >= 0 && position < text.length {
            guard let match = matchingBracket(at: position, in: text) else { continue }
            for bracket in [position, match] {
                layout.addTemporaryAttribute(.backgroundColor, value: NSColor.systemGray.withAlphaComponent(0.35), forCharacterRange: NSRange(location: bracket, length: 1))
            }
            bracketsMarked = true
            return
        }
    }

    // MARK: Errors

    /// The server reported a compile error in what was just sent. `line` counts from 1.
    func addError(line: Int, message: String) {
        let index = min(max(line - 1, 0), lineStarts.count - 1)
        errors.append(ErrorMark(line: line, message: message, position: lineStarts[index]))
        showErrors()
        if errors.count == 1 { goToError(0) }
        selectionChanged()
    }

    @objc private func clearErrors() {
        guard !errors.isEmpty else { return }
        errors = []
        showErrors()
        selectionChanged()
    }

    private func showErrorPositions() {
        ruler.errors = errors
        textView.errorPositions = errors.map(\.position)
    }

    /// Rebuilds the list under the editor.
    private func showErrors() {
        showErrorPositions()
        errorList.arrangedSubviews.forEach { $0.removeFromSuperview() }
        for (index, error) in errors.enumerated() {
            let row = NSButton(title: "", target: self, action: #selector(errorClicked(_:)))
            row.tag = index
            row.isBordered = false
            row.image = NSImage(systemSymbolName: "exclamationmark.circle.fill", accessibilityDescription: "Error")
            row.contentTintColor = .systemRed
            row.imagePosition = .imageLeading
            row.alignment = .left
            let title = NSMutableAttributedString(string: "Line \(error.line)", attributes: [.font: NSFont.boldSystemFont(ofSize: NSFont.systemFontSize), .foregroundColor: NSColor.labelColor])
            title.append(NSAttributedString(string: "  \(error.message)", attributes: [.font: NSFont.systemFont(ofSize: NSFont.systemFontSize), .foregroundColor: NSColor.labelColor]))
            row.attributedTitle = title
            row.cell?.lineBreakMode = .byTruncatingTail
            row.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
            row.toolTip = error.message
            row.setAccessibilityLabel("Line \(error.line): \(error.message)")
            errorList.addArrangedSubview(row)
        }
        errorScroll.isHidden = errors.isEmpty
        errorSeparator?.isHidden = errors.isEmpty
        errorHeight.constant = errors.isEmpty ? 0 : min(errorList.fittingSize.height, 140)
    }

    @objc private func errorClicked(_ sender: NSButton) {
        goToError(sender.tag)
    }

    private func goToError(_ index: Int) {
        guard errors.indices.contains(index) else { return }
        let length = (textView.string as NSString).length
        let range = NSRange(location: min(lineStarts[ruler.lineIndex(at: errors[index].position)], length), length: 0)
        textView.setSelectedRange(range)
        textView.scrollRangeToVisible(range)
        window?.makeFirstResponder(textView)
    }

    // MARK: Editing

    private func lineStart(of position: Int, in text: NSString) -> Int {
        var start = min(position, text.length)
        while start > 0, text.character(at: start - 1) != .newline { start -= 1 }
        return start
    }

    /// Several replacements as one undo step. `edits` are in ascending order and don't overlap.
    private func apply(_ edits: [(range: NSRange, text: String)], actionName: String) -> Bool {
        guard !edits.isEmpty, textView.shouldChangeText(inRanges: edits.map { NSValue(range: $0.range) }, replacementStrings: edits.map(\.text)) else { return false }
        // One at a time and from the end, so each edit's offsets still hold and the error
        // marks move with their own lines.
        for edit in edits.reversed() {
            storage.replaceCharacters(in: edit.range, with: NSAttributedString(string: edit.text, attributes: textView.typingAttributes))
        }
        textView.didChangeText()
        textView.undoManager?.setActionName(actionName)
        return true
    }

    /// Where an offset from before `edits` is after them.
    private func moved(_ position: Int, by edits: [(range: NSRange, text: String)]) -> Int {
        var shift = 0
        for edit in edits {
            let length = (edit.text as NSString).length
            if edit.range.location >= position { break }
            if NSMaxRange(edit.range) <= position {
                shift += length - edit.range.length
            } else {
                shift += edit.range.location + length - position
            }
        }
        return position + shift
    }

    /// Enter with indentation rules: re-indent the finished line, then indent the new one.
    private func smartEnter() -> Bool {
        let text = textView.string as NSString
        let selection = textView.selectedRange()
        let cursor = selection.location
        guard let reply = try? Core.callAnywhere("code.on_enter", ["before": .of(text.substring(to: cursor)), "language": .of(language.rawValue)]), !reply.isNull else { return false }
        let indent = String(repeating: indentUnit, count: reply["current"].int)
        let newline = "\n" + String(repeating: indentUnit, count: reply["next"].int)

        // The finished line's leading whitespace (but not past the cursor).
        let start = lineStart(of: cursor, in: text)
        var body = start
        while body < cursor, text.character(at: body).isBlank { body += 1 }
        // Text after the cursor moves to the new line, without its leading spaces.
        var after = NSMaxRange(selection)
        while after < text.length, text.character(at: after).isBlank { after += 1 }

        // Typed like any other text, so it's one step to undo and the cursor ends after it.
        if text.substring(with: NSRange(location: start, length: body - start)) == indent {
            textView.insertText(newline, replacementRange: NSRange(location: cursor, length: after - cursor))
        } else {
            let kept = text.substring(with: NSRange(location: body, length: cursor - body))
            textView.insertText(indent + kept + newline, replacementRange: NSRange(location: start, length: after - start))
        }
        textView.scrollRangeToVisible(textView.selectedRange())
        return true
    }

    /// Enter without rules keeps the indentation of the line being left.
    private func plainEnter() -> Bool {
        let text = textView.string as NSString
        let selection = textView.selectedRange()
        let start = lineStart(of: selection.location, in: text)
        var body = start
        while body < selection.location, text.character(at: body).isBlank { body += 1 }
        textView.insertText("\n" + text.substring(with: NSRange(location: start, length: body - start)), replacementRange: selection)
        textView.scrollRangeToVisible(textView.selectedRange())
        return true
    }

    /// Tab inserts spaces up to the next indent stop, or indents every selected line.
    private func tab() -> Bool {
        let text = textView.string as NSString
        let selection = textView.selectedRange()
        if text.substring(with: selection).contains("\n") { return shiftLines(outdent: false) }
        let column = selection.location - lineStart(of: selection.location, in: text)
        textView.insertText(String(repeating: " ", count: indentWidth - column % indentWidth), replacementRange: selection)
        return true
    }

    private func shiftLines(outdent: Bool) -> Bool {
        let text = textView.string as NSString
        let selection = textView.selectedRange()
        // A selection that ends at the start of a line doesn't include that line.
        let last = selection.length > 0 ? NSMaxRange(selection) - 1 : selection.location
        var edits: [(range: NSRange, text: String)] = []
        var start = lineStart(of: selection.location, in: text)
        while start <= last {
            if outdent {
                var end = start
                if end < text.length, text.character(at: end) == 0x09 {
                    end += 1
                } else {
                    while end < text.length, end - start < indentWidth, text.character(at: end) == 0x20 { end += 1 }
                }
                if end > start { edits.append((NSRange(location: start, length: end - start), "")) }
            } else if start < text.length, text.character(at: start) != .newline {
                edits.append((NSRange(location: start, length: 0), indentUnit))
            }
            let newline = text.range(of: "\n", options: [], range: NSRange(location: start, length: text.length - start))
            if newline.location == NSNotFound { break }
            start = newline.location + 1
        }
        guard apply(edits, actionName: outdent ? "Unindent" : "Indent") else { return true }
        let from = moved(selection.location, by: edits)
        let to = max(moved(NSMaxRange(selection), by: edits), from)
        textView.setSelectedRange(NSRange(location: from, length: to - from))
        return true
    }

    /// Backspace in a line's indentation goes back to the previous indent stop.
    private func smartBackspace() -> Bool {
        let text = textView.string as NSString
        let selection = textView.selectedRange()
        guard selection.length == 0 else { return false }
        let start = lineStart(of: selection.location, in: text)
        let column = selection.location - start
        guard column > 1, (start..<selection.location).allSatisfy({ text.character(at: $0) == 0x20 }) else { return false }
        let count = (column - 1) % indentWidth + 1
        // A single space is an ordinary backspace.
        guard count > 1 else { return false }
        let range = NSRange(location: selection.location - count, length: count)
        guard textView.shouldChangeText(in: range, replacementString: "") else { return true }
        storage.deleteCharacters(in: range)
        textView.didChangeText()
        textView.setSelectedRange(NSRange(location: range.location, length: 0))
        return true
    }

    /// Re-indents the whole program, keeping the cursor on its line.
    @objc private func reindent() {
        guard language.hasIndentRules else { return }
        let text = textView.string
        guard let reply = try? Core.callAnywhere("code.reindent", ["text": .of(text), "language": .of(language.rawValue), "unit": .of(indentUnit)]),
              let new = reply.optionalString, new != text
        else { return }
        let cursor = textView.selectedRange().location
        let line = ruler.lineIndex(at: cursor)

        // Only the lines' indentation changes, so replace just that: the error marks and the
        // scroll position then stay where they are.
        let oldLines = text.components(separatedBy: "\n"), newLines = new.components(separatedBy: "\n")
        var edits: [(range: NSRange, text: String)] = []
        if oldLines.count == newLines.count {
            var offset = 0
            for (old, new) in zip(oldLines, newLines) {
                let before = Array(old.utf16), after = Array(new.utf16)
                if before != after {
                    var common = 0
                    while common < before.count, common < after.count, before[before.count - 1 - common] == after[after.count - 1 - common] { common += 1 }
                    edits.append((NSRange(location: offset, length: before.count - common), String(decoding: after[..<(after.count - common)], as: UTF16.self)))
                }
                offset += before.count + 1
            }
        } else {
            edits = [(NSRange(location: 0, length: (text as NSString).length), new)]
        }
        guard apply(edits, actionName: "Re-indent") else { return }

        let now = textView.string as NSString
        var position = lineStarts[min(line, lineStarts.count - 1)]
        while position < now.length, now.character(at: position).isBlank { position += 1 }
        textView.setSelectedRange(NSRange(location: position, length: 0))
        textView.scrollRangeToVisible(NSRange(location: position, length: 0))
    }

    func textView(_ textView: NSTextView, doCommandBy selector: Selector) -> Bool {
        switch selector {
        case #selector(NSResponder.insertNewline(_:)):
            return (language.hasIndentRules && smartEnter()) || plainEnter()
        case #selector(NSResponder.insertTab(_:)):
            return tab()
        case #selector(NSResponder.insertBacktab(_:)):
            return shiftLines(outdent: true)
        case #selector(NSResponder.deleteBackward(_:)):
            return smartBackspace()
        case #selector(NSResponder.cancelOperation(_:)):
            // Escape closes the find bar rather than offering word completions.
            if scrollView.isFindBarVisible { find(.hideFindInterface) }
            return true
        default:
            return false
        }
    }

    // MARK: Search and keys

    private func find(_ action: NSTextFinder.Action) {
        let item = NSMenuItem()
        item.tag = action.rawValue
        textView.performTextFinderAction(item)
    }

    @objc private func showFind() {
        find(.showFindInterface)
    }

    @objc private func findNext() { find(.nextMatch) }
    @objc private func findPrevious() { find(.previousMatch) }

    /// Runs one of the program editor's shortcuts ("editor-send" and so on).
    func perform(_ id: String) {
        switch id {
        case "editor-send": send()
        case "editor-find": showFind()
        case "editor-find-next": findNext()
        case "editor-find-previous": findPrevious()
        case "editor-reindent": reindent()
        default: break
        }
    }

    /// Brings the window forward with the cursor at the start of `line` (counting from 1).
    func reveal(line: Int?) {
        if window?.isMiniaturized == true { window?.deminiaturize(nil) }
        window?.makeKeyAndOrderFront(nil)
        guard let line else { return }
        let length = (textView.string as NSString).length
        let range = NSRange(location: min(lineStarts[min(max(line - 1, 0), lineStarts.count - 1)], length), length: 0)
        textView.setSelectedRange(range)
        textView.scrollRangeToVisible(range)
        window?.makeFirstResponder(textView)
    }

    // MARK: Closing

    /// Ask before throwing away unsent changes.
    func windowShouldClose(_ sender: NSWindow) -> Bool {
        guard modified else { return true }
        let alert = NSAlert()
        alert.messageText = "Discard Changes?"
        alert.informativeText = "Your changes haven't been sent to the server."
        alert.addButton(withTitle: "Keep Editing")
        alert.addButton(withTitle: "Discard").hasDestructiveAction = true
        alert.beginSheetModal(for: sender) { [weak self] response in
            if response == .alertSecondButtonReturn { self?.close() }
        }
        return false
    }

    /// Closes without asking (the session is closing, or the user chose to discard).
    override func close() {
        if let window, let sheet = window.attachedSheet { window.endSheet(sheet) }
        super.close()
    }

    func windowWillClose(_ notification: Notification) {
        let closed = onClose
        onClose = nil
        closed?()
    }
}
