import AppKit

let args = CommandLine.arguments
if args.count != 5 { fatalError("smoke FIRMWARE VARIANT SECONDS PNG") }
if args[2] == "auto" {
  let probe = rnse_create()!
  let result = "firmware".withCString { c in args[1].withCString { v in rnse_command(probe, c, v) }
  }
  var n = 0
  let ptr = rnse_buffer(probe, 0, &n)!
  let listing = String(decoding: Data(bytes: ptr, count: n), as: UTF8.self)
  rnse_destroy(probe)
  if result != 0 { fatalError(listing) }
  for model in ["192", "193"] {
    guard
      let line = listing.components(separatedBy: "\n").first(where: { $0.contains("Rnse" + model) })
    else { continue }
    let variant = line.components(separatedBy: "\t")[0]
    let child = Process()
    child.executableURL = URL(fileURLWithPath: args[0])
    child.arguments = [args[1], variant, args[3], args[4] + "-" + model + ".png"]
    try child.run()
    child.waitUntilExit()
    if child.terminationStatus != 0 { exit(child.terminationStatus) }
  }
  exit(0)
}
let session = rnse_create()!
defer { rnse_destroy(session) }
func data(_ k: UInt32) -> Data {
  var n = 0
  let p = rnse_buffer(session, k, &n)!
  return Data(bytes: p, count: n)
}
func command(_ c: String, _ v: String) {
  let r = c.withCString { c in v.withCString { v in rnse_command(session, c, v) } }
  if r != 0 { fatalError(String(decoding: data(0), as: UTF8.self)) }
}
command("firmware", args[1])
print(String(decoding: data(0), as: UTF8.self))
command("boot", args[2] + "\n\n0")
let end = Double(args[3])!
var samples = 0
var logs = 0
var frames = 0
var opened = false
while rnse_time(session) < end {
  if rnse_step(session) != 0 { fatalError(String(decoding: data(0), as: UTF8.self)) }
  samples += data(2).count / 4
  logs += data(3).count
  frames += 1
  if rnse_time(session) > end - 2 && !opened {
    command("key", "SETUP:1")
    opened = true
  }
  if rnse_time(session) > end - 1 { command("key", "SETUP:0") }
}
let rgb = data(1)
let w = Int(rnse_width(session))
let h = Int(rnse_height(session))
assert(rgb.count == w * h * 3 && rgb.contains { $0 != 0 })
assert(samples > 0)
let provider = CGDataProvider(data: rgb as CFData)!
let image = CGImage(
  width: w, height: h, bitsPerComponent: 8, bitsPerPixel: 24, bytesPerRow: w * 3,
  space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGBitmapInfo(rawValue: 0), provider: provider,
  decode: nil, shouldInterpolate: false, intent: .defaultIntent)!
try NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:])!.write(
  to: URL(fileURLWithPath: args[4]))
command("key", "SELECT:1")
command("key", "SELECT:0")
command("knob", "main:1")
command("ignition", "0")
command("eject", "")
command("eject0", "")
print("PASS \(args[2]): \(w)x\(h), \(frames) batches, \(samples) audio frames, \(logs) log bytes")
command("stop", "")
command("boot", args[2] + "\n\n0")
assert(rnse_time(session) == 0)
