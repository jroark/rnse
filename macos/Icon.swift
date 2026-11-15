import AppKit

let folder = CommandLine.arguments[1]
for size in [16, 32, 64, 128, 256, 512, 1024] {
  let image = NSImage(size: NSSize(width: size, height: size))
  image.lockFocus()
  let scale = CGFloat(size) / 1024
  NSAffineTransform(transform: AffineTransform(scale: scale)).concat()
  let outer = NSBezierPath(
    roundedRect: NSRect(x: 40, y: 40, width: 944, height: 944), xRadius: 190, yRadius: 190)
  NSColor(calibratedWhite: 0.13, alpha: 1).setFill()
  outer.fill()
  let screen = NSBezierPath(
    roundedRect: NSRect(x: 135, y: 270, width: 754, height: 560), xRadius: 45, yRadius: 45)
  NSColor.black.setFill()
  screen.fill()
  NSColor(calibratedRed: 1, green: 0.48, blue: 0.05, alpha: 1).setStroke()
  screen.lineWidth = 12
  screen.stroke()
  let attrs: [NSAttributedString.Key: Any] = [
    .font: NSFont.systemFont(ofSize: 115, weight: .semibold),
    .foregroundColor: NSColor(calibratedRed: 1, green: 0.48, blue: 0.05, alpha: 1),
  ]
  ("RNS-E" as NSString).draw(at: NSPoint(x: 315, y: 355), withAttributes: attrs)
  let arrow = NSBezierPath()
  arrow.move(to: NSPoint(x: 512, y: 750))
  arrow.line(to: NSPoint(x: 410, y: 540))
  arrow.line(to: NSPoint(x: 512, y: 585))
  arrow.line(to: NSPoint(x: 614, y: 540))
  arrow.close()
  NSColor.white.setFill()
  arrow.fill()
  for x in [CGFloat(245), CGFloat(779)] {
    let knob = NSBezierPath(ovalIn: NSRect(x: x - 55, y: 115, width: 110, height: 110))
    NSColor.darkGray.setFill()
    knob.fill()
    NSColor.gray.setStroke()
    knob.lineWidth = 8
    knob.stroke()
  }
  image.unlockFocus()
  let data = NSBitmapImageRep(data: image.tiffRepresentation!)!.representation(
    using: .png, properties: [:])!
  let name = size == 1024 ? "icon_512x512@2x.png" : "icon_\(size)x\(size).png"
  try data.write(to: URL(fileURLWithPath: folder).appendingPathComponent(name))
}
