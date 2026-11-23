import Foundation
import SwiftUI

/// A JSON value. The config and everything the engine says travel as these rather than as
/// typed structs, so the fields (and their defaults) are defined once, in Rust, and anything
/// this app doesn't know about passes through untouched.
enum JSON: Equatable {
    case null
    case bool(Bool)
    case int(Int)
    case double(Double)
    case string(String)
    case array([JSON])
    case object([String: JSON])

    // MARK: Reading. A missing or mistyped value reads as empty, false or zero, which is
    // what Rust leaves out when it writes one.

    subscript(key: String) -> JSON {
        get {
            if case .object(let o) = self { return o[key] ?? .null }
            return .null
        }
        set {
            var o: [String: JSON] = [:]
            if case .object(let existing) = self { o = existing }
            o[key] = newValue == .null ? nil : newValue
            self = .object(o)
        }
    }

    subscript(index: Int) -> JSON {
        get {
            if case .array(let a) = self, a.indices.contains(index) { return a[index] }
            return .null
        }
        set {
            guard case .array(var a) = self, a.indices.contains(index) else { return }
            a[index] = newValue
            self = .array(a)
        }
    }

    var string: String {
        get {
            if case .string(let s) = self { return s }
            return ""
        }
        set { self = .string(newValue) }
    }

    var optionalString: String? {
        if case .string(let s) = self { return s }
        return nil
    }

    var bool: Bool {
        get {
            if case .bool(let b) = self { return b }
            return false
        }
        set { self = .bool(newValue) }
    }

    var int: Int {
        get {
            switch self {
            case .int(let i): return i
            case .double(let d): return Int(d)
            default: return 0
            }
        }
        set { self = .int(newValue) }
    }

    var optionalInt: Int? {
        switch self {
        case .int(let i): return i
        case .double(let d): return Int(d)
        default: return nil
        }
    }

    var double: Double {
        switch self {
        case .int(let i): return Double(i)
        case .double(let d): return d
        default: return 0
        }
    }

    var optionalDouble: Double? {
        switch self {
        case .int(let i): return Double(i)
        case .double(let d): return d
        default: return nil
        }
    }

    var array: [JSON] {
        get {
            if case .array(let a) = self { return a }
            return []
        }
        set { self = .array(newValue) }
    }

    var object: [String: JSON] {
        if case .object(let o) = self { return o }
        return [:]
    }

    var isNull: Bool { self == .null }

    // MARK: Text

    init(text: String) throws {
        let any = try JSONSerialization.jsonObject(with: Data(text.utf8), options: [.fragmentsAllowed])
        self = JSON(any: any)
    }

    private init(any: Any) {
        switch any {
        case let n as NSNumber:
            if CFGetTypeID(n) == CFBooleanGetTypeID() {
                self = .bool(n.boolValue)
            } else if CFNumberIsFloatType(n) {
                self = .double(n.doubleValue)
            } else {
                self = .int(n.intValue)
            }
        case let s as String: self = .string(s)
        case let a as [Any]: self = .array(a.map(JSON.init(any:)))
        case let o as [String: Any]: self = .object(o.mapValues(JSON.init(any:)))
        default: self = .null
        }
    }

    private var any: Any {
        switch self {
        case .null: return NSNull()
        case .bool(let b): return b
        case .int(let i): return i
        case .double(let d): return d
        case .string(let s): return s
        case .array(let a): return a.map(\.any)
        case .object(let o): return o.mapValues(\.any)
        }
    }

    var text: String {
        guard let data = try? JSONSerialization.data(withJSONObject: any, options: [.fragmentsAllowed]) else { return "null" }
        return String(decoding: data, as: UTF8.self)
    }
}

extension JSON: ExpressibleByStringLiteral, ExpressibleByBooleanLiteral, ExpressibleByIntegerLiteral, ExpressibleByFloatLiteral,
    ExpressibleByNilLiteral, ExpressibleByArrayLiteral, ExpressibleByDictionaryLiteral
{
    init(stringLiteral value: String) { self = .string(value) }
    init(booleanLiteral value: Bool) { self = .bool(value) }
    init(integerLiteral value: Int) { self = .int(value) }
    init(floatLiteral value: Double) { self = .double(value) }
    init(nilLiteral: ()) { self = .null }
    init(arrayLiteral elements: JSON...) { self = .array(elements) }
    init(dictionaryLiteral elements: (String, JSON)...) { self = .object(Dictionary(elements, uniquingKeysWith: { _, b in b })) }
}

extension JSON {
    /// For building arguments: `["text": .of(text), "secret": .of(secret)]`.
    static func of(_ s: String) -> JSON { .string(s) }
    static func of(_ s: String?) -> JSON { s.map(JSON.string) ?? .null }
    static func of(_ b: Bool) -> JSON { .bool(b) }
    static func of(_ i: Int) -> JSON { .int(i) }
    static func of(_ d: Double) -> JSON { .double(d) }
}

/// Bindings into a JSON object's fields, for forms: `TextField("Name", text: $world.str("name"))`.
extension Binding where Value == JSON {
    func str(_ key: String) -> Binding<String> {
        Binding<String>(get: { wrappedValue[key].string }, set: { wrappedValue[key] = .string($0) })
    }

    func flag(_ key: String) -> Binding<Bool> {
        Binding<Bool>(get: { wrappedValue[key].bool }, set: { wrappedValue[key] = .bool($0) })
    }

    func num(_ key: String) -> Binding<Int> {
        Binding<Int>(get: { wrappedValue[key].int }, set: { wrappedValue[key] = .int($0) })
    }

    /// A string that's stored only when it isn't empty (Rust's `Option<String>`).
    func optionalStr(_ key: String) -> Binding<String> {
        Binding<String>(get: { wrappedValue[key].string }, set: { wrappedValue[key] = $0.isEmpty ? .null : .string($0) })
    }

    func child(_ key: String) -> Binding<JSON> {
        Binding<JSON>(get: { wrappedValue[key] }, set: { wrappedValue[key] = $0 })
    }

    func item(_ index: Int) -> Binding<JSON> {
        Binding<JSON>(get: { wrappedValue[index] }, set: { wrappedValue[index] = $0 })
    }
}
