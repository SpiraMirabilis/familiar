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
    targets: [
        .systemLibrary(name: "FamiliarBridge"),
        .executableTarget(
            name: "Familiar",
            dependencies: ["FamiliarBridge"],
            swiftSettings: [.unsafeFlags(["-swift-version", "5"])],
            linkerSettings: [
                .unsafeFlags(["-L\(rustLibs)"]),
                .linkedFramework("Security"),
                .linkedFramework("CoreFoundation"),
                .linkedFramework("SystemConfiguration"),
            ]
        ),
    ]
)
