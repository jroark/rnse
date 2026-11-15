import AVFoundation
import AppKit

final class Audio {
  let engine = AVAudioEngine(), player = AVAudioPlayerNode()
  let format = AVAudioFormat(standardFormatWithSampleRate: 44100, channels: 2)!
  var pending = 0
  init() {
    engine.attach(player)
    engine.connect(player, to: engine.mainMixerNode, format: format)
  }
  func play(_ data: Data) {
    guard !data.isEmpty, pending < 12 else { return }
    if !engine.isRunning {
      do {
        try engine.start()
        player.play()
      } catch { return }
    }
    let frames = data.count / 4
    guard let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(frames))
    else { return }
    buffer.frameLength = AVAudioFrameCount(frames)
    data.withUnsafeBytes { raw in
      let samples = raw.bindMemory(to: Int16.self)
      for i in 0..<frames {
        for ch in 0..<2 {
          buffer.floatChannelData![ch][i] = Float(Int16(littleEndian: samples[i * 2 + ch])) / 32768
        }
      }
    }
    pending += 1
    player.scheduleBuffer(buffer) {
      DispatchQueue.main.async { self.pending = max(0, self.pending - 1) }
    }
  }
  func flush() {
    player.stop()
    engine.stop()
    pending = 0
  }
}

final class Display: NSView {
  var image: NSImage? { didSet { needsDisplay = true } }
  var input: ((String, String) -> Void)?
  var held = Set<String>()
  override var acceptsFirstResponder: Bool { true }
  let plate = NSImage(contentsOf: Bundle.main.url(forResource: "faceplate", withExtension: "png")!)!
  var targets: [(NSView, NSRect)] = []
  override var isFlipped: Bool { true }
  var plateRect: NSRect {
    let scale = min(bounds.width / 1620, bounds.height / 971)
    return NSRect(
      x: (bounds.width - 1620 * scale) / 2, y: (bounds.height - 971 * scale) / 2,
      width: 1620 * scale, height: 971 * scale)
  }
  func place(_ r: NSRect) -> NSRect {
    let p = plateRect
    let s = p.width / 1620
    return NSRect(
      x: p.minX + r.minX * s, y: p.minY + r.minY * s, width: r.width * s, height: r.height * s)
  }
  func configureFaceplate() {
    for (name, rect) in faceplateKeys {
      let b = FaceplateKey(title: name, target: nil, action: nil)
      b.isBordered = false
      b.target = b
      b.action = #selector(PressButton.activate)
      b.toolTip = name
      b.setAccessibilityLabel(name)
      b.send = { [weak self] down in self?.input?("key", "\(name):\(down ? 1:0)") }
      addSubview(b)
      targets.append((b, rect))
    }
    for (name, cx, cy, r) in [("main", 1437.0, 400.0, 106.0), ("volume", 1472.0, 787.0, 72.0)] {
      let rect = NSRect(x: cx - r, y: cy - r, width: 2 * r, height: 2 * r)
      let cg = plate.cgImage(forProposedRect: nil, context: nil, hints: nil)?.cropping(to: rect)
      let knob = FaceplateKnob(crop: cg.map { NSImage(cgImage: $0, size: rect.size) })
      knob.turn = { [weak self] d in self?.input?("knob", "\(name):\(d)") }
      if name == "main" {
        knob.press = { [weak self] down in self?.input?("key", "SELECT:\(down ? 1:0)") }
      }
      knob.toolTip =
        name == "main"
        ? "Selection: drag or scroll to turn; click center to select"
        : "Volume: drag or scroll to turn"
      knob.setAccessibilityLabel(name == "main" ? "Selection knob" : "Volume knob")
      addSubview(knob)
      targets.append((knob, rect))
    }
  }
  override func layout() {
    super.layout()
    for (view, rect) in targets { view.frame = place(rect) }
  }
  override func draw(_ dirtyRect: NSRect) {
    NSColor.windowBackgroundColor.setFill()
    bounds.fill()
    plate.draw(
      in: plateRect, from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: true,
      hints: nil)
    let screen = place(NSRect(x: 115, y: 105, width: 1125, height: 627))
    NSColor.black.setFill()
    screen.fill()
    if let image {
      image.draw(
        in: screen, from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: true,
        hints: [.interpolation: NSImageInterpolation.none])
    }
  }
  override func mouseDown(with event: NSEvent) { window?.makeFirstResponder(self) }
  override func scrollWheel(with event: NSEvent) {
    if event.scrollingDeltaY != 0 { input?("knob", "main:\(event.scrollingDeltaY < 0 ? 1 : -1)") }
  }
  let keys: [UInt16: String] = [
    36: "SELECT", 53: "RETURN", 123: "PREV", 124: "NEXT", 18: "SOFT_TL", 19: "SOFT_TR",
    20: "SOFT_BL", 21: "SOFT_BR", 15: "RADIO", 46: "MEDIA", 45: "NAV", 17: "TEL", 34: "INFO",
    8: "CAR", 1: "SETUP",
  ]
  override func keyDown(with e: NSEvent) {
    if e.keyCode == 125 || e.keyCode == 126 {
      input?("knob", "main:\(e.keyCode == 125 ? 1 : -1)")
      return
    }
    if let key = keys[e.keyCode], !e.isARepeat {
      held.insert(key)
      input?("key", "\(key):1")
    }
  }
  override func keyUp(with e: NSEvent) {
    if let key = keys[e.keyCode] {
      held.remove(key)
      input?("key", "\(key):0")
    }
  }
  func releaseKeys() {
    for key in held { input?("key", "\(key):0") }
    held.removeAll()
  }
}
class PressButton: NSButton {
  var send: ((Bool) -> Void)?
  var trackingPress = false
  override func mouseDown(with event: NSEvent) {
    trackingPress = true
    send?(true)
    super.mouseDown(with: event)
    send?(false)
    trackingPress = false
  }
  @objc func activate() {
    if !trackingPress {
      send?(true)
      send?(false)
    }
  }
}

final class App: NSObject, NSApplicationDelegate, NSWindowDelegate {
  let queue = DispatchQueue(label: "rnse.emulator", qos: .userInitiated)
  var session: OpaquePointer!
  var timer: DispatchSourceTimer?
  var active = false, paused = false, busy = false, generation = 0
  var base = Date(), emuBase = 0.0
  let audio = Audio(), display = Display(),
    status = NSTextField(labelWithString: "Open an RNS-E firmware update image to begin.")
  let variants = NSPopUpButton(), log = NSTextView(),
    firmware = NSTextField(labelWithString: "No firmware selected")
  let serial = NSTextField(), lat = NSTextField(), lon = NSTextField(), course = NSTextField()
  let gps = NSButton(
    checkboxWithTitle: "GPS fix (supported 193 firmware)", target: nil, action: nil)
  let ignition = NSButton(checkboxWithTitle: "Ignition", target: nil, action: nil)
  var window: NSWindow!, logScroll: NSScrollView!, sd2: NSButton!, pauseButton: NSButton!
  var choices: [String] = [], variantNames: [String] = []
  var media: [String: String] = [:]
  let defaults = UserDefaults.standard
  func applicationDidFinishLaunching(_ n: Notification) {
    session = rnse_create()
    window = NSWindow(
      contentRect: NSRect(x: 0, y: 0, width: 1080, height: 850),
      styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false
    )
    window.title = "RNS-E"
    window.minSize = NSSize(width: 820, height: 680)
    window.delegate = self
    let root = NSStackView()
    root.orientation = .vertical
    root.spacing = 10
    root.edgeInsets = NSEdgeInsets(top: 12, left: 12, bottom: 12, right: 12)
    root.translatesAutoresizingMaskIntoConstraints = false
    window.contentView!.addSubview(root)
    NSLayoutConstraint.activate([
      root.leadingAnchor.constraint(equalTo: window.contentView!.leadingAnchor),
      root.trailingAnchor.constraint(equalTo: window.contentView!.trailingAnchor),
      root.topAnchor.constraint(equalTo: window.contentView!.topAnchor),
      root.bottomAnchor.constraint(equalTo: window.contentView!.bottomAnchor),
    ])
    root.addArrangedSubview(
      row([
        button("Open Firmware…", #selector(openFirmware)), firmware, variants,
        button("Boot / Reset", #selector(boot)),
      ]))
    variants.target = self
    variants.action = #selector(variantChanged)
    pauseButton = button("Pause", #selector(pause))
    ignition.state = .on
    ignition.target = self
    ignition.action = #selector(ignitionChanged)
    root.addArrangedSubview(
      row([
        pauseButton, button("Stop", #selector(stop)), ignition,
        button("Screenshot…", #selector(screenshot)),
        button("Show / Hide Log", #selector(toggleLog)), key("EJECT", title: "Open Display"),
        key("CD_EJECT", title: "CD Eject"),
      ]))
    display.translatesAutoresizingMaskIntoConstraints = false
    display.heightAnchor.constraint(greaterThanOrEqualToConstant: 240).isActive = true
    display.input = { [weak self] c, v in self?.command(c, v) }
    display.configureFaceplate()
    root.addArrangedSubview(display)
    display.widthAnchor.constraint(equalTo: root.widthAnchor, constant: -24).isActive = true
    display.setContentHuggingPriority(.defaultLow, for: .vertical)
    sd2 = button("SD 2…", #selector(mountSD2))
    root.addArrangedSubview(
      row([
        button("Disc…", #selector(mountDisc)), button("Eject Disc", #selector(ejectDisc)),
        button("Audio CD…", #selector(mountAudio)), button("SD 1…", #selector(mountSD1)),
        button("Eject SD 1", #selector(ejectSD1)), sd2, button("Eject SD 2", #selector(ejectSD2)),
      ]))
    serial.placeholderString = "Optional serial number"
    serial.stringValue = defaults.string(forKey: "serial") ?? ""
    serial.widthAnchor.constraint(equalToConstant: 170).isActive = true
    for (field, name, fallback) in [
      (lat, "Latitude", "37.7749"), (lon, "Longitude", "-122.4194"), (course, "Course", "0"),
    ] {
      field.placeholderString = name
      field.stringValue = defaults.string(forKey: name) ?? fallback
      field.widthAnchor.constraint(equalToConstant: 95).isActive = true
    }
    root.addArrangedSubview(row([serial, gps, lat, lon, course]))
    status.lineBreakMode = .byTruncatingMiddle
    root.addArrangedSubview(status)
    log.isEditable = false
    log.font = NSFont.monospacedSystemFont(ofSize: 11, weight: .regular)
    logScroll = NSScrollView()
    logScroll.documentView = log
    logScroll.hasVerticalScroller = true
    logScroll.heightAnchor.constraint(equalToConstant: 150).isActive = true
    logScroll.isHidden = true
    root.addArrangedSubview(logScroll)
    logScroll.widthAnchor.constraint(equalTo: root.widthAnchor, constant: -24).isActive = true
    createMenus()
    window.center()
    window.makeKeyAndOrderFront(nil)
    NSApp.activate(ignoringOtherApps: true)
    NotificationCenter.default.addObserver(
      forName: NSWindow.didResignKeyNotification, object: window, queue: .main
    ) { [weak self] _ in self?.display.releaseKeys() }
    if let path = defaults.string(forKey: "firmware") { loadFirmware(path) }
    if let saved = defaults.dictionary(forKey: "media") as? [String: String] { media = saved }
    display.registerForDraggedTypes([.fileURL])
  }
  func row(_ views: [NSView]) -> NSStackView {
    let s = NSStackView(views: views)
    s.spacing = 8
    s.distribution = .fillProportionally
    return s
  }
  func button(_ title: String, _ action: Selector) -> NSButton {
    NSButton(title: title, target: self, action: action)
  }
  func key(_ name: String, title: String? = nil) -> NSButton {
    let b = PressButton(title: title ?? name, target: nil, action: nil)
    b.target = b
    b.action = #selector(PressButton.activate)
    b.send = { [weak self] down in self?.command("key", "\(name):\(down ? 1:0)") }
    return b
  }
  func createMenus() {
    let main = NSMenu()
    let appItem = NSMenuItem()
    main.addItem(appItem)
    let appMenu = NSMenu()
    appItem.submenu = appMenu
    appMenu.addItem(
      withTitle: "Quit RNS-E", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
    let file = NSMenuItem()
    file.title = "File"
    main.addItem(file)
    file.submenu = NSMenu(title: "File")
    for (title, action, key) in [
      ("Open Firmware…", #selector(openFirmware), "o"), ("Boot / Reset", #selector(boot), "r"),
      ("Pause / Resume", #selector(pause), "p"), ("Save Screenshot…", #selector(screenshot), "s"),
    ] {
      let item = file.submenu!.addItem(withTitle: title, action: action, keyEquivalent: key)
      item.target = self
    }
    let edit = NSMenuItem()
    edit.title = "Edit"
    main.addItem(edit)
    edit.submenu = NSMenu(title: "Edit")
    for (t, a, k) in [
      ("Copy", #selector(NSText.copy(_:)), "c"), ("Paste", #selector(NSText.paste(_:)), "v"),
      ("Select All", #selector(NSText.selectAll(_:)), "a"),
    ] { edit.submenu!.addItem(withTitle: t, action: a, keyEquivalent: k) }
    NSApp.mainMenu = main
  }
  func buffer(_ kind: UInt32) -> Data {
    var size = 0
    guard let p = rnse_buffer(session, kind, &size) else { return Data() }
    return Data(bytes: p, count: size)
  }
  func invoke(_ c: String, _ v: String) -> Bool {
    c.withCString { c in v.withCString { v in rnse_command(session, c, v) == 0 } }
  }
  func errorText() -> String { String(decoding: buffer(0), as: UTF8.self) }
  func report(_ text: String) {
    status.stringValue = text
    let a = NSAlert()
    a.messageText = "RNS-E"
    a.informativeText = text
    a.beginSheetModal(for: window)
  }
  func command(_ c: String, _ v: String = "") {
    guard active else { return }
    queue.async {
      let ok = self.invoke(c, v)
      let error = ok ? "" : self.errorText()
      if !ok { DispatchQueue.main.async { self.report(error) } }
    }
  }
  func choose(multiple: Bool = false, _ done: @escaping ([String]) -> Void) {
    let p = NSOpenPanel()
    p.canChooseDirectories = false
    p.allowsMultipleSelection = multiple
    p.beginSheetModal(for: window) { if $0 == .OK { done(p.urls.map(\.path)) } }
  }
  @objc func openFirmware() { choose { self.loadFirmware($0[0]) } }
  func loadFirmware(_ path: String) {
    stop()
    status.stringValue = "Reading firmware…"
    queue.async {
      let ok = self.invoke("firmware", path)
      let result = ok ? String(decoding: self.buffer(0), as: UTF8.self) : self.errorText()
      DispatchQueue.main.async {
        if !ok {
          self.report(result)
          return
        }
        self.firmware.stringValue = URL(fileURLWithPath: path).lastPathComponent
        self.defaults.set(path, forKey: "firmware")
        self.choices = result.components(separatedBy: "\n")
        self.variantNames = self.choices.map { $0.components(separatedBy: "\t")[0] }
        self.variants.removeAllItems()
        self.variants.addItems(
          withTitles: self.choices.map {
            $0.components(separatedBy: "\t").prefix(3).joined(separator: " · ")
          })
        let preferred = self.defaults.string(forKey: "variant") ?? "V220"
        self.variants.selectItem(at: self.variantNames.firstIndex(of: preferred) ?? 0)
        self.variantChanged()
        self.status.stringValue = "Ready to boot"
      }
    }
  }
  @objc func variantChanged() {
    guard variants.indexOfSelectedItem >= 0 else { return }
    let is193 = choices[variants.indexOfSelectedItem].contains("193")
    sd2.isEnabled = !is193
    serial.isEnabled = is193
    gps.isEnabled = choices[variants.indexOfSelectedItem].contains("GPS")
    gps.state = gps.isEnabled && defaults.object(forKey: "gps") as? Bool != false ? .on : .off
  }
  @objc func boot() {
    guard variants.indexOfSelectedItem >= 0 else { return }
    stop()
    generation += 1
    let gen = generation
    let variant = variantNames[variants.indexOfSelectedItem]
    defaults.set(variant, forKey: "variant")
    defaults.set(serial.stringValue, forKey: "serial")
    defaults.set(gps.state == .on, forKey: "gps")
    for (f, n) in [(lat, "Latitude"), (lon, "Longitude"), (course, "Course")] {
      defaults.set(f.stringValue, forKey: n)
    }
    let config = [
      variant, serial.isEnabled ? serial.stringValue : "", gps.state == .on ? "1" : "0",
      lat.stringValue, lon.stringValue, course.stringValue,
    ].joined(separator: "\n")
    let mounted = media
    status.stringValue = "Booting…"
    log.string = ""
    queue.async {
      let ok = self.invoke("boot", config)
      var error = ok ? "" : self.errorText()
      if ok {
        for (c, v) in mounted.sorted(by: { $0.key < $1.key }) {
          if !self.invoke(c, v) { error += "\n\(self.errorText())" }
        }
      }
      DispatchQueue.main.async {
        guard gen == self.generation else { return }
        if !ok {
          self.report(error)
          return
        }
        self.active = true
        self.paused = false
        self.ignition.state = .on
        self.base = Date()
        self.emuBase = 0
        self.startTimer()
        self.window.makeFirstResponder(self.display)
        if !error.isEmpty { self.report("Some media could not be restored: \(error)") }
      }
    }
  }
  func startTimer() {
    timer?.cancel()
    let t = DispatchSource.makeTimerSource(queue: .main)
    t.schedule(deadline: .now(), repeating: .milliseconds(10))
    t.setEventHandler { [weak self] in self?.tick() }
    timer = t
    t.resume()
  }
  func tick() {
    guard active, !paused, !busy else { return }
    busy = true
    let gen = generation
    let target = Date().timeIntervalSince(base) + emuBase
    queue.async {
      var ok = true
      let deadline = Date().addingTimeInterval(0.025)
      var pcm = Data()
      var logs = Data()
      repeat {
        if rnse_time(self.session) >= target { break }
        ok = rnse_step(self.session) == 0
        pcm.append(self.buffer(2))
        logs.append(self.buffer(3))
      } while ok && Date() < deadline
      let rgb = self.buffer(1)
      let w = Int(rnse_width(self.session))
      let h = Int(rnse_height(self.session))
      let time = rnse_time(self.session)
      let text = String(decoding: self.buffer(4), as: UTF8.self)
      let error = ok ? "" : self.errorText()
      DispatchQueue.main.async {
        self.busy = false
        guard gen == self.generation else { return }
        if w > 0, h > 0, rgb.count == w * h * 3, let provider = CGDataProvider(data: rgb as CFData),
          let image = CGImage(
            width: w, height: h, bitsPerComponent: 8, bitsPerPixel: 24, bytesPerRow: w * 3,
            space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGBitmapInfo(rawValue: 0),
            provider: provider, decode: nil, shouldInterpolate: false, intent: .defaultIntent)
        {
          self.display.image = NSImage(cgImage: image, size: NSSize(width: w, height: h))
        }
        self.audio.play(pcm)
        let encoding = String.Encoding(
          rawValue: CFStringConvertEncodingToNSStringEncoding(
            CFStringEncoding(CFStringEncodings.EUC_JP.rawValue)))
        self.log.string +=
          String(data: logs, encoding: encoding) ?? String(decoding: logs, as: UTF8.self)
        if self.log.string.utf8.count > 262144 {
          self.log.string = String(self.log.string.suffix(131072))
        }
        let lag = target - time
        self.status.stringValue = text + (lag > 0.3 ? " · Running slower than real time" : "")
        if lag > 0.3 {
          self.base = Date()
          self.emuBase = time
        }
        if !ok {
          self.paused = true
          self.audio.flush()
          self.display.releaseKeys()
          self.report(error + " — use Boot / Reset to restart.")
        }
      }
    }
  }
  @objc func pause() {
    guard active else { return }
    display.releaseKeys()
    paused.toggle()
    pauseButton.title = paused ? "Resume" : "Pause"
    audio.flush()
    if !paused {
      queue.async {
        let time = rnse_time(self.session)
        DispatchQueue.main.async {
          self.base = Date()
          self.emuBase = time
        }
      }
    }
  }
  @objc func stop() {
    display.releaseKeys()
    active = false
    paused = false
    generation += 1
    timer?.cancel()
    timer = nil
    audio.flush()
    pauseButton?.title = "Pause"
    queue.async { _ = self.invoke("stop", "") }
    status.stringValue = "Stopped"
  }
  @objc func ignitionChanged() { command("ignition", ignition.state == .on ? "1" : "0") }
  func mount(_ kind: String, multiple: Bool = false) {
    choose(multiple: multiple) { paths in
      let value = paths.joined(separator: "\n")
      if kind == "disc" || kind == "audio" {
        self.media.removeValue(forKey: kind == "disc" ? "audio" : "disc")
      }
      self.media[kind] = value
      self.defaults.set(self.media, forKey: "media")
      self.command(kind, value)
      self.status.stringValue =
        "Selected \(paths.map{URL(fileURLWithPath:$0).lastPathComponent}.joined(separator:", "))"
    }
  }
  @objc func mountDisc() { mount("disc") }
  @objc func mountAudio() { mount("audio", multiple: true) }
  @objc func mountSD1() { mount("sd0") }
  @objc func mountSD2() { mount("sd1") }
  func eject(_ cmd: String, _ kinds: [String]) {
    for k in kinds { media.removeValue(forKey: k) }
    if cmd == "eject" { media["disc"] = "" }
    defaults.set(media, forKey: "media")
    command(cmd)
  }
  @objc func ejectDisc() { eject("eject", ["disc", "audio"]) }
  @objc func ejectSD1() { eject("eject0", ["sd0"]) }
  @objc func ejectSD2() { eject("eject1", ["sd1"]) }
  @objc func toggleLog() { logScroll.isHidden.toggle() }
  @objc func screenshot() {
    guard let image = display.image, let tiff = image.tiffRepresentation,
      let rep = NSBitmapImageRep(data: tiff),
      let png = rep.representation(using: .png, properties: [:])
    else { return }
    let p = NSSavePanel()
    p.nameFieldStringValue = "RNS-E.png"
    p.beginSheetModal(for: window) {
      if $0 == .OK, let url = p.url {
        do { try png.write(to: url) } catch { self.report(error.localizedDescription) }
      }
    }
  }
  func windowWillClose(_ notification: Notification) { NSApp.terminate(nil) }
  func applicationWillTerminate(_ notification: Notification) {
    stop()
    queue.sync {
      rnse_destroy(session)
      session = nil
    }
  }
}
extension Display {
  override func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation { .copy }
  override func performDragOperation(_ sender: NSDraggingInfo) -> Bool {
    guard let urls = sender.draggingPasteboard.readObjects(forClasses: [NSURL.self]) as? [URL],
      let url = urls.first, let app = NSApp.delegate as? App
    else { return false }
    app.loadFirmware(url.path)
    return true
  }
}
let app = NSApplication.shared
let delegate = App()
app.setActivationPolicy(.regular)
app.delegate = delegate
app.run()
