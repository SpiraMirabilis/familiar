// Makes Familiar.icns from the SVG icon: `swift packaging/macos/make-icon.swift` from the
// repository's top folder. The artwork is drawn at 824 of 1024 points, the margin macOS icons have.
import AppKit

let svg = URL(fileURLWithPath: "data/io.github.SpiraMirabilis.Familiar.svg")
guard let image = NSImage(contentsOf: svg) else { fatalError("can't read \(svg.path)") }
let iconset = URL(fileURLWithPath: NSTemporaryDirectory()).appendingPathComponent("Familiar.iconset")
try? FileManager.default.removeItem(at: iconset)
try FileManager.default.createDirectory(at: iconset, withIntermediateDirectories: true)

for (points, scale) in [(16, 1), (16, 2), (32, 1), (32, 2), (128, 1), (128, 2), (256, 1), (256, 2), (512, 1), (512, 2)] {
    let pixels = points * scale
    let bitmap = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: pixels, pixelsHigh: pixels, bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: bitmap)
    NSGraphicsContext.current?.imageInterpolation = .high
    let inset = CGFloat(pixels) * 100 / 1024
    image.draw(in: NSRect(x: inset, y: inset, width: CGFloat(pixels) - inset * 2, height: CGFloat(pixels) - inset * 2))
    NSGraphicsContext.restoreGraphicsState()
    let name = scale == 1 ? "icon_\(points)x\(points).png" : "icon_\(points)x\(points)@2x.png"
    try bitmap.representation(using: .png, properties: [:])!.write(to: iconset.appendingPathComponent(name))
}

let iconutil = Process()
iconutil.executableURL = URL(fileURLWithPath: "/usr/bin/iconutil")
iconutil.arguments = ["-c", "icns", "-o", "packaging/macos/Familiar.icns", iconset.path]
try iconutil.run()
iconutil.waitUntilExit()
exit(iconutil.terminationStatus)
