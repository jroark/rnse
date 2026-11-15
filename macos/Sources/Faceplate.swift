import AppKit

// Coordinates match the photographed 1620 × 971 faceplate and assets/ui.html.
let faceplateKeys: [(String, NSRect)] = [
  ("PREV", NSRect(x: 1303, y: 88, width: 119, height: 72)),
  ("NEXT", NSRect(x: 1438, y: 88, width: 120, height: 72)),
  ("SOFT_TL", NSRect(x: 1308, y: 205, width: 116, height: 127)),
  ("SOFT_TR", NSRect(x: 1440, y: 205, width: 122, height: 127)),
  ("SOFT_BL", NSRect(x: 1308, y: 468, width: 116, height: 120)),
  ("SOFT_BR", NSRect(x: 1440, y: 468, width: 122, height: 120)),
  ("RETURN", NSRect(x: 1305, y: 605, width: 258, height: 72)),
  ("RADIO", NSRect(x: 70, y: 833, width: 148, height: 67)),
  ("MEDIA", NSRect(x: 236, y: 833, width: 152, height: 67)),
  ("NAME", NSRect(x: 405, y: 833, width: 143, height: 67)),
  ("TEL", NSRect(x: 562, y: 833, width: 143, height: 67)),
  ("NAV", NSRect(x: 722, y: 833, width: 143, height: 67)),
  ("INFO", NSRect(x: 882, y: 833, width: 148, height: 67)),
  ("CAR", NSRect(x: 1050, y: 833, width: 142, height: 67)),
  ("SETUP", NSRect(x: 1210, y: 833, width: 152, height: 67)),
]
final class FaceplateKey: PressButton {
  override func draw(_ dirtyRect: NSRect) {
    if isHighlighted {
      NSColor(calibratedRed: 1, green: 0.3, blue: 0.15, alpha: 0.25).setFill()
      NSBezierPath(roundedRect: bounds, xRadius: 5, yRadius: 5).fill()
    }
    if window?.firstResponder === self {
      NSColor.keyboardFocusIndicatorColor.setStroke()
      let p = NSBezierPath(roundedRect: bounds.insetBy(dx: 2, dy: 2), xRadius: 5, yRadius: 5)
      p.lineWidth = 2
      p.stroke()
    }
  }
}
final class FaceplateKnob: NSView {
  var turn: ((Int) -> Void)?
  var press: ((Bool) -> Void)?
  var angle: CGFloat = 0
  var lastAngle: CGFloat = 0
  var remainder: CGFloat = 0
  var start: NSPoint = .zero
  var dragged = false
  var crop: NSImage?
  override var isFlipped: Bool { true }
  override var acceptsFirstResponder: Bool { true }
  init(crop: NSImage?) {
    self.crop = crop
    super.init(frame: .zero)
    setAccessibilityElement(true)
    setAccessibilityRole(.slider)
  }
  required init?(coder: NSCoder) { fatalError() }
  override func draw(_ dirtyRect: NSRect) {
    NSGraphicsContext.saveGraphicsState()
    NSBezierPath(ovalIn: bounds).addClip()
    let transform = NSAffineTransform()
    transform.translateX(by: bounds.midX, yBy: bounds.midY)
    transform.rotate(byRadians: angle)
    transform.translateX(by: -bounds.midX, yBy: -bounds.midY)
    transform.concat()
    crop?.draw(
      in: bounds, from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: true, hints: nil
    )
    NSGraphicsContext.restoreGraphicsState()
  }
  func detent(_ d: Int) {
    angle += CGFloat(d) * 0.12
    needsDisplay = true
    turn?(d)
  }
  override func mouseDown(with e: NSEvent) {
    window?.makeFirstResponder(superview)
    start = convert(e.locationInWindow, from: nil)
    lastAngle = atan2(start.y - bounds.midY, start.x - bounds.midX)
    remainder = 0
    dragged = false
  }
  override func mouseDragged(with e: NSEvent) {
    let p = convert(e.locationInWindow, from: nil)
    if hypot(p.x - start.x, p.y - start.y) < 3 && !dragged { return }
    dragged = true
    let next = atan2(p.y - bounds.midY, p.x - bounds.midX)
    var delta = next - lastAngle
    if delta > .pi { delta -= 2 * .pi }
    if delta < -.pi { delta += 2 * .pi }
    lastAngle = next
    remainder += delta
    let steps = Int(remainder / 0.12)
    if steps != 0 {
      remainder -= CGFloat(steps) * 0.12
      detent(steps)
    }
  }
  override func mouseUp(with e: NSEvent) {
    if !dragged {
      let p = convert(e.locationInWindow, from: nil)
      if hypot(p.x - bounds.midX, p.y - bounds.midY) < bounds.width * 0.29 {
        press?(true)
        press?(false)
      } else {
        detent(p.x < bounds.midX ? -1 : 1)
      }
    }
  }
  override func scrollWheel(with e: NSEvent) {
    if e.scrollingDeltaY != 0 { detent(e.scrollingDeltaY < 0 ? 1 : -1) }
  }
  override func accessibilityPerformIncrement() -> Bool {
    detent(1)
    return true
  }
  override func accessibilityPerformDecrement() -> Bool {
    detent(-1)
    return true
  }
  override func accessibilityPerformPress() -> Bool {
    press?(true)
    press?(false)
    return press != nil
  }
}
