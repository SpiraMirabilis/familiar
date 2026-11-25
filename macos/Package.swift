// swift-tools-version:5.9
// The macOS app. It links the Rust engine as a static library, so build that first:
//   cargo build --release -p familiar-bridge      (or let packaging/macos/build.sh do both)
// Set FAMILIAR_RUST_PROFILE=debug to link target/debug instead.
import Foundation
import PackageDescription

let environment = ProcessInfo.processInfo.environment
let profile = environment["FAMILIAR_RUST_PROFILE"] ?? "release"
// FAMILIAR_RUST_LIB_DIR points somewhere else entirely (the universal library build.sh makes).
let rustLibs = environment["FAMILIAR_RUST_LIB_DIR"]
    ?? URL(fileURLWithPath: #filePath).deletingLastPathComponent().appendingPathComponent("../target/\(profile)").standardized.path

let package = Package(
    name: "Familiar",
    platforms: [.macOS(.v14)],
    dependencies: [
        // The terminal LLM agents (/agent) run in.
        .package(url: "https://github.com/migueldeicaza/SwiftTerm", from: "1.2.0"),
    ],
    targets: [
        .systemLibrary(name: "FamiliarBridge"),
        .executableTarget(
            name: "Familiar",
            dependencies: ["FamiliarBridge", .product(name: "SwiftTerm", package: "SwiftTerm")],
            swiftSettings: [.unsafeFlags(["-swift-version", "5"])],
            linkerSettings: [
                // The static library by its path: with only -L, the linker would take the
                // .dylib cargo builds beside it, which the signed app then can't load.
                .unsafeFlags(["-Xlinker", "\(rustLibs)/libfamiliar_bridge.a"]),
                .linkedFramework("Security"),
                .linkedFramework("CoreFoundation"),
                .linkedFramework("SystemConfiguration"),
            ]
        ),
    ]
)
