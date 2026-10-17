// Rasterises an SVG with AppKit (macOS 14+). Used by scripts/icons.sh.
// usage: render-svg <in.svg> <out.png> <width> [height]
import AppKit

let a = CommandLine.arguments
guard a.count >= 4, let img = NSImage(contentsOf: URL(fileURLWithPath: a[1])) else {
    fatalError("usage: render-svg <in.svg> <out.png> <width> [height]")
}
let w = Int(a[3])!
let h = a.count > 4 ? Int(a[4])! : w
let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: w, pixelsHigh: h, bitsPerSample: 8,
    samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
img.draw(in: NSRect(x: 0, y: 0, width: w, height: h))
NSGraphicsContext.restoreGraphicsState()
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: a[2]))
