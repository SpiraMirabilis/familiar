import FamiliarBridge
import Foundation

struct CoreError: LocalizedError {
    let message: String
    var errorDescription: String? { message }
}

/// The Rust engine. Everything but `callAnywhere` must be used on the main thread.
enum Core {
    /// Called on the main thread when a session has network traffic or a timer waiting.
    static var onWake: ((Int) -> Void)?

    static func start() {
        familiar_init { session in
            // The engine calls this from its network threads.
            DispatchQueue.main.async { Core.onWake?(Int(session)) }
        }
    }

    private static func run(_ method: String, _ args: JSON) throws -> JSON {
        guard let raw = familiar_call(method, args.text) else { throw CoreError(message: "\(method): no reply") }
        defer { familiar_free(raw) }
        let reply = try JSON(text: String(cString: raw))
        if let error = reply["error"].optionalString {
            throw CoreError(message: error)
        }
        return reply["ok"]
    }

    static func call(_ method: String, _ args: JSON = .null) throws -> JSON {
        dispatchPrecondition(condition: .onQueue(.main))
        return try run(method, args)
    }

    /// For calls that can't sensibly fail (a session we know is open). A failure is a bug, so
    /// it's logged rather than shown.
    @discardableResult
    static func send(_ method: String, _ args: JSON = .null) -> JSON {
        do {
            return try call(method, args)
        } catch {
            NSLog("Familiar: %@ failed: %@", method, error.localizedDescription)
            return .null
        }
    }

    /// The few methods the engine allows on any thread (media.fetch, directory, ansi.parse,
    /// address.parse, code.*, keys.parse).
    static func callAnywhere(_ method: String, _ args: JSON = .null) throws -> JSON {
        try run(method, args)
    }
}
