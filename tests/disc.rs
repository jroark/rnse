//! End-to-end tests that need a real update disc image.
//!
//! The firmware is not redistributable, so these only run when the
//! environment variable `RNSE_DISC` points at an ISO image of an update
//! CD:
//!
//!     RNSE_DISC=/path/to/disc.iso cargo test --release -- --nocapture
//!
//! The navigation test further needs `RNSE_NAV_DISC`, a navigation DVD
//! for the 193 (an ISO image, or the image of an SD card that carries
//! one), and, if that disc is tied to a unit, `RNSE_SERIAL` with the
//! unit's serial number.  `RNSE_NAV_VARIANT` names the firmware variant
//! the disc is for (default V575, North America).

use rnse::fw;
use rnse::machine::Machine;
use rnse::sh4::cpu::Stop;

fn disc() -> Option<Vec<u8>> {
    let path = std::env::var_os("RNSE_DISC")?;
    Some(std::fs::read(path).expect("RNSE_DISC is not readable"))
}

/// Exercise the real SD/MP3 driver, including both downloads, polling,
/// companion-chip receive interrupts and the switch to block DMA.
#[test]
fn media_dsps_boot() {
    use rnse::board::Model;
    let Some(image) = disc() else { return };
    let variants = fw::load_variants(&image).unwrap();
    let v = variants.iter().find(|v| Model::for_variant(&v.name) == Some(Model::Rnse192))
        .expect("RNSE_DISC needs a 192 variant for the media DSP test");
    let index = fw::IndexInfo::from_disc(&image, &v.name).unwrap();
    let mut m = Machine::new();
    m.echo_console = false;
    m.install_boot_stub(&v.name);
    m.install(v, &index);
    m.atapi().load(image);
    m.set_ignition(true);
    let printf = Machine::known_printf(&v.module("OSM").unwrap().version);
    if let Some(address) = printf { m.hook_printf(address); }
    m.start_os(v).unwrap();
    assert_eq!(m.run(30_000 * m.cycles_per_ms()), Stop::Budget, "{:?}", m.cpu.fault);
    let log = String::from_utf8_lossy(&m.console);
    for ch in 0..2 {
        assert!(m.board.media.initialized(ch), "DSP{ch} initialization failed:\n{log}");
        assert_eq!(m.board.media.mode(ch), 2, "DSP{ch} transfer mode was not selected");
        assert!(m.board.media.uploaded_words(ch) > 20_000, "DSP{ch} download was incomplete");
        assert!(m.board.media.command_count(ch) >= 2, "DSP{ch} runtime commands stalled:\n{log}");
        if printf.is_some() {
            assert!(log.contains(&format!("SDC{ch}-DSPC_SYS_INIT Success")), "driver rejected DSP{ch}:\n{log}");
        }
    }
    assert!(m.board.media.command_count(0) >= 5, "DSP0 DMA commands stalled:\n{log}");
    assert!(m.cpu.irq_counts[0x7e0 >> 5] > 0, "DSP0 DMA completion never interrupted");
    assert!(!log.contains("BOOT ERROR"), "DSP boot failure:\n{log}");
    assert!(!log.contains("ERR_cmd_dma"), "media DMA did not complete:\n{log}");
}

/// The boot ROM supplies CAN software state and a running CAN timer.
/// Without these the UI boots, but cyclic status and transport retries
/// remain queued forever. Exercise both implementations of that handover.
#[test]
fn can_timer_drives_status_and_transport() {
    use rnse::board::Model;
    use rnse::dev::hcan::Frame;
    let Some(image) = disc() else { return };
    let variants = fw::load_variants(&image).unwrap();
    for model in [Model::Rnse192, Model::Rnse193] {
        let Some(v) = variants.iter().find(|v| Model::for_variant(&v.name) == Some(model)) else { continue };
        let index = fw::IndexInfo::from_disc(&image, &v.name).unwrap();
        let mut m = Machine::for_model(model);
        m.echo_console = false;
        m.install_boot_stub(&v.name);
        m.install(v, &index);
        m.atapi().load(image.clone());
        m.set_ignition(true);
        // In the 192 this uses the shared 0x600..0x67f receive box.
        m.can_send_cyclic(Frame::new(0x665, &[0; 7]), 100);
        m.start_os(v).unwrap();
        let ms = m.cycles_per_ms();
        assert_eq!(m.run(30_000 * ms), Stop::Budget, "{}: {:?}", v.name, m.cpu.fault);
        let frames = &m.can().tx;
        assert!(frames.iter().filter(|f| f.id == 0x661).count() >= 3,
            "{}: periodic CAN status never started", v.name);
        assert!(frames.iter().any(|f| f.id == 0x6c0 && f.data[0] == 0xa0),
            "{}: transport channel-open timer never ran", v.name);
        if model == Model::Rnse192 {
            assert!(frames.iter().any(|f| f.id == 0x6c4 && f.data[0] == 0xa0),
                "{}: telephone status never reached its handler", v.name);
        }
        let timer_irq = if model == Model::Rnse192 { 0x440 } else { 0x480 };
        assert!(m.cpu.irq_counts[timer_irq >> 5] > 100,
            "{}: CAN timer did not interrupt", v.name);
    }
}

#[test]
fn every_module_on_the_disc_has_a_valid_checksum() {
    let Some(image) = disc() else { return };
    let variants = fw::load_variants(&image).unwrap();
    assert!(!variants.is_empty());
    for v in &variants {
        for m in &v.modules {
            assert!(m.checksum_ok(), "{} {}", v.name, m.name);
        }
    }
}

/// Let the disc's own loader update an empty unit and compare the flash
/// it wrote with what `Machine::install` produces directly.
#[test]
fn loader_installs_the_firmware() {
    let Some(image) = disc() else { return };
    let variants = fw::load_variants(&image).unwrap();
    let v = &variants[0];
    let ldm = v.module("LDM").unwrap();

    let mut m = Machine::new();
    m.echo_console = false;
    m.install_boot_stub(&v.name);
    m.atapi().load(image.clone());
    m.start_loader(ldm, [0; 3]);
    let stop = m.run(120_000 * m.cycles_per_ms());
    assert_eq!(stop, Stop::Fault, "the loader ends by jumping to the reset entry");
    assert!(m.reset_requested);
    let text = String::from_utf8_lossy(&m.console);
    assert!(text.contains("[FINISH]"), "loader log:\n{}", text);

    let index = fw::IndexInfo::from_disc(&image, &v.name).unwrap();
    let mut direct = Machine::new();
    direct.install_boot_stub(&v.name);
    direct.install(v, &index);
    for module in &v.modules {
        if module.kind == "LDR" {
            continue;
        }
        let a = module.phys_addr() as usize;
        let n = module.size as usize;
        assert_eq!(&m.board.flash[a..a + n], &module.data[..n], "module {} as written by the loader", module.name);
    }
    // Module directory below the OS.
    assert_eq!(&m.board.flash[0xc000..0xc100], &direct.board.flash[0xc000..0xc100], "module directory");
}

/// The loaders of the 193-family variants: they finish by letting the
/// watchdog reset the unit, having written every module and their own
/// kind of module directory.
#[test]
fn loader_installs_the_firmware_193() {
    let Some(image) = disc() else { return };
    let variants = fw::load_variants(&image).unwrap();
    for v in variants.iter().filter(|v| rnse::board::Model::for_variant(&v.name) == Some(rnse::board::Model::Rnse193)) {
        let ldm = v.module("LDM").unwrap();
        let mut m = Machine::for_model(rnse::board::Model::Rnse193);
        m.echo_console = false;
        m.install_boot_stub(&v.name);
        m.atapi().load(image.clone());
        m.start_loader(ldm, [0; 3]);
        m.hook_printf(Machine::known_printf(&ldm.version).expect("printf address of this loader"));
        let stop = m.run(120_000 * m.cycles_per_ms());
        assert_eq!(stop, Stop::Fault, "{}: the loader ends with a watchdog reset", v.name);
        assert!(m.reset_requested, "{}: {:?}", v.name, m.cpu.fault);
        let text = String::from_utf8_lossy(&m.console);
        assert!(text.contains("[FINISH]"), "{} loader log:\n{}", v.name, text);
        for module in &v.modules {
            if module.kind == "LDR" {
                continue;
            }
            let a = module.phys_addr() as usize;
            let n = module.size as usize;
            assert_eq!(&m.board.flash[a..a + n], &module.data[..n], "{}: module {} as written by the loader", v.name, module.name);
        }
        // The directory in front of the OS is the one `install` builds.
        let os = v.module("OSM").unwrap();
        let dir = os.phys_addr() as usize - 0x200;
        let index = fw::IndexInfo::from_disc(&image, &v.name).unwrap();
        assert_eq!(&m.board.flash[dir..dir + 0x200], &fw::module_directory_for(v, &index)[..], "{}: module directory", v.name);
    }
}

/// The 193's OS, started the way its boot ROM does it (every module's
/// entry points, stage by stage), must get its tick going and keep
/// running.
#[test]
fn os_runs_193() {
    let Some(image) = disc() else { return };
    let variants = fw::load_variants(&image).unwrap();
    for v in variants.iter().filter(|v| rnse::board::Model::for_variant(&v.name) == Some(rnse::board::Model::Rnse193)) {
        let index = fw::IndexInfo::from_disc(&image, &v.name).unwrap();
        let mut m = Machine::for_model(rnse::board::Model::Rnse193);
        m.echo_console = false;
        m.install_boot_stub(&v.name);
        m.install(v, &index);
        m.atapi().load(image.clone());
        m.start_os(v).unwrap();
        let stop = m.run(10_000 * m.cycles_per_ms());
        assert_eq!(stop, Stop::Budget, "{}: {:?}", v.name, m.cpu.fault);
        assert!(m.cpu.irq_counts[0x400 >> 5] > 1000, "{}: system tick is not running", v.name);
    }
}

/// With the ignition on, the 193's OS draws its screens through the
/// graphics engine, shows them on the display unit's planes and takes a
/// key from the faceplate.
#[test]
fn os_draws_193() {
    let Some(image) = disc() else { return };
    let variants = fw::load_variants(&image).unwrap();
    for v in variants.iter().filter(|v| rnse::board::Model::for_variant(&v.name) == Some(rnse::board::Model::Rnse193)) {
        let index = fw::IndexInfo::from_disc(&image, &v.name).unwrap();
        let mut m = Machine::for_model(rnse::board::Model::Rnse193);
        m.echo_console = false;
        m.install_boot_stub(&v.name);
        m.install(v, &index);
        m.atapi().load(image.clone());
        m.set_ignition(true);
        m.start_os(v).unwrap();
        let ms = m.cycles_per_ms();
        assert_eq!(m.run(40_000 * ms), Stop::Budget, "{}: {:?}", v.name, m.cpu.fault);
        let navi = m.cpu.io.navi.as_ref().unwrap();
        assert!(navi.ge.lists > 20, "{}: the graphics engine ran {} display lists", v.name, navi.ge.lists);
        assert!(navi.ge.unknown.is_empty(), "{}: display list commands not understood: {:x?}", v.name, navi.ge.unknown);
        let (w, h, picture) = m.render();
        assert_eq!((w, h), (800, 480));
        // By now the start-up picture has given way to a screen of text.
        let lit = picture.chunks(3).filter(|p| p.iter().any(|&c| c > 0x40)).count();
        assert!(lit > 3_000, "{}: only {} pixels of the screen are lit", v.name, lit);

        // A function key from the faceplate brings up another screen.
        assert!(m.panel.online, "{}: the OS never acknowledged the faceplate controller", v.name);
        assert_eq!(m.run(25_000 * ms), Stop::Budget, "{}: {:?}", v.name, m.cpu.fault);
        let before = m.render().2;
        assert!(m.panel_key("SETUP", true));
        m.run(200 * ms);
        assert!(m.panel_key("SETUP", false));
        assert_eq!(m.run(4_000 * ms), Stop::Budget, "{}: {:?}", v.name, m.cpu.fault);
        assert_ne!(before, m.render().2, "{}: the screen did not change for SETUP", v.name);
        let navi = m.cpu.io.navi.as_ref().unwrap();
        assert!(navi.ge.unknown.is_empty(), "{}: display list commands not understood: {:x?}", v.name, navi.ge.unknown);
    }
}

/// An audio CD, in the first variant of each unit on the disc: MEDIA
/// makes the OS read the table of contents and start the drive's player,
/// and the position moves in real time.
#[test]
fn cd_audio_plays() {
    use rnse::board::Model;
    use rnse::dev::atapi::{AudioDisc, AudioStatus};
    let Some(image) = disc() else { return };
    let variants = fw::load_variants(&image).unwrap();
    for model in [Model::Rnse192, Model::Rnse193] {
        let Some(v) = variants.iter().find(|v| Model::for_variant(&v.name) == Some(model)) else { continue };
        let index = fw::IndexInfo::from_disc(&image, &v.name).unwrap();
        let mut m = Machine::for_model(model);
        m.echo_console = false;
        m.install_boot_stub(&v.name);
        m.install(v, &index);
        m.atapi().insert_audio(AudioDisc::uniform(5, 180), false);
        m.set_ignition(true);
        m.start_os(v).unwrap();
        let ms = m.cycles_per_ms();
        assert_eq!(m.run(65_000 * ms), Stop::Budget, "{}: {:?}", v.name, m.cpu.fault);
        assert!(m.panel_key("MEDIA", true));
        m.run(200 * ms);
        assert!(m.panel_key("MEDIA", false));
        assert_eq!(m.run(10_000 * ms), Stop::Budget, "{}: {:?}", v.name, m.cpu.fault);
        assert_eq!(m.atapi().audio_status, AudioStatus::Playing, "{}: the OS did not start the player", v.name);
        let before = m.atapi().audio_frame();
        assert_eq!(m.run(10_000 * ms), Stop::Budget, "{}: {:?}", v.name, m.cpu.fault);
        let played = m.atapi().audio_frame() - before;
        assert!((700..=800).contains(&played), "{}: ten seconds should be 750 frames, not {}", v.name, played);
    }
}

/// A program written from scratch (sdk/examples/hello), built with an
/// SH-4 GCC if there is one, put in the loader's place on the disc and
/// started the way the boot ROM starts the loader: it brings the board
/// up, prints on the debug serial port, draws and keeps time, on both
/// units.
/// Build an SDK example for a unit, put it in the loader's place and
/// start it.  None when there is no cross compiler or the disc has no
/// software for that unit.
fn sdk_example(image: &[u8], model: rnse::board::Model, example: &str) -> Option<(Machine, String)> {
    use rnse::board::Model;
    let sdk = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("sdk");
    let unit = if model == Model::Rnse192 { "192" } else { "193" };
    let variants = fw::load_variants(image).unwrap();
    let v = variants.iter().find(|v| Model::for_variant(&v.name) == Some(model))?;
    // Builds of the same example for the two tests that use it must
    // not run into each other.
    static BUILD: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let built = {
        let _lock = BUILD.lock().unwrap();
        std::process::Command::new("sh").arg(sdk.join("build.sh")).args([unit, example]).output()
    };
    match built {
        Ok(out) if out.status.success() => {}
        // No cross compiler here: nothing to test.
        _ => return None,
    }
    let program = std::fs::read(sdk.join(format!("out/{}-{}.bin", example, unit))).unwrap();
    let (custom, places) = fw::custom_loader(image, &v.name, &program).unwrap();
    assert_eq!(places.len(), 2, "{}: the loader is on the disc in the container and as a file: {:?}", v.name, places);
    let variants = fw::load_variants(&custom).unwrap();
    let v = variants.iter().find(|x| x.name == v.name).unwrap();
    let ldm = v.module("LDM").unwrap();
    assert!(ldm.checksum_ok());

    let mut m = Machine::for_model(model);
    m.echo_console = false;
    // Whatever was in memory before: not zeroes.
    m.scramble_ram();
    m.install_boot_stub(&v.name);
    m.atapi().load(custom.clone());
    m.start_loader(ldm, [0; 3]);
    Some((m, v.name.clone()))
}

#[test]
fn custom_program_runs() {
    use rnse::board::Model;
    let Some(image) = disc() else { return };
    for model in [Model::Rnse192, Model::Rnse193] {
        let Some((mut m, name)) = sdk_example(&image, model, "hello") else { continue };
        let ms = m.cycles_per_ms();
        assert_eq!(m.run(3_500 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
        let text = String::from_utf8_lossy(&m.console).into_owned();
        assert!(text.contains("rnse sdk: hello"), "{}: console: {:?}", name, text);
        assert!(text.contains("tick 3") && !text.contains("tick 4"), "{}: three seconds have passed: {:?}", name, text);
        assert!(!text.contains("unexpected"), "{}: {:?}", name, text);
        // The picture: a white frame on the panel's edge, colour bars
        // in the top third.
        let (w, h, rgb) = m.render();
        let at = |x: usize, y: usize| [rgb[(y * w + x) * 3], rgb[(y * w + x) * 3 + 1], rgb[(y * w + x) * 3 + 2]];
        let near = |p: [u8; 3], q: [u8; 3]| p.iter().zip(q).all(|(a, b)| a.abs_diff(b) <= 4);
        for (x, y) in [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1), (w / 2, 0), (0, h / 2), (w - 1, h / 2), (w / 2, h - 1)] {
            assert!(near(at(x, y), [255, 255, 255]), "{}: frame at {},{} is {:?}", name, x, y, at(x, y));
        }
        assert!(near(at(w * 3 / 16, h / 6), [255, 255, 0]), "{}: second bar is {:?}", name, at(w * 3 / 16, h / 6));
        assert!(near(at(w * 13 / 16, h / 6), [0, 0, 255]), "{}: seventh bar is {:?}", name, at(w * 13 / 16, h / 6));
        assert!(near(at(w / 2, h * 7 / 8), [0, 0, 0]), "{}: background is {:?}", name, at(w / 2, h * 7 / 8));
    }
}

/// The SDK's faceplate code against the faceplate controller's model:
/// the program releases the controller from reset, takes its start-up
/// frame, sends the stock settings and then follows keys and knobs.
#[test]
fn custom_program_reads_the_faceplate() {
    use rnse::board::Model;
    let Some(image) = disc() else { return };
    for model in [Model::Rnse192, Model::Rnse193] {
        let Some((mut m, name)) = sdk_example(&image, model, "keys") else { continue };
        let ms = m.cycles_per_ms();
        assert!(m.panel.in_reset || !m.panel.online, "{}: the controller starts in reset", name);
        assert_eq!(m.run(500 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
        assert!(m.panel.online, "{}: start-up frame acknowledged", name);
        for cmd in [2, 3, 6, 7, 9, 10] {
            assert!(m.panel.from_unit[cmd].is_some(), "{}: setting {:#x} was sent", name, cmd);
        }
        assert!(m.panel_key("SETUP", true));
        m.run(100 * ms);
        assert!(m.panel_key("SETUP", false));
        assert!(m.panel_knob("main", 3));
        assert!(m.panel_knob("volume", -2));
        assert_eq!(m.run(300 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
        let text = String::from_utf8_lossy(&m.console).into_owned();
        for line in ["faceplate online\r\n", "SETUP down\r\n", "SETUP up\r\n", "select 3 right\r\n", "volume 2 left\r\n"] {
            assert!(text.contains(line), "{}: {:?} in {:?}", name, line, text);
        }
        assert!(!text.contains("unexpected") && !text.contains("lost"), "{}: {:?}", name, text);
        assert_eq!(m.panel.resets, 0, "{}", name);
    }
}

/// The installed OS must get through its three entry points and reach
/// its idle loop with the system tick running.
#[test]
fn os_reaches_idle() {
    let Some(image) = disc() else { return };
    let variants = fw::load_variants(&image).unwrap();
    let v = &variants[0];
    let index = fw::IndexInfo::from_disc(&image, &v.name).unwrap();
    let mut m = Machine::new();
    m.echo_console = false;
    m.install_boot_stub(&v.name);
    m.install(v, &index);
    m.atapi().load(image.clone());
    m.start_os(v).unwrap();
    let stop = m.run(3_400 * m.cycles_per_ms());
    assert_eq!(stop, Stop::Budget, "{:?}", m.cpu.fault);
    assert!(m.cpu.irq_counts[0x400 >> 5] > 100, "system tick is not running");
}

/// With the ignition on, the radio processor answering and the display
/// closed, the OS brings up its user interface, takes keys from the
/// faceplate and draws a different screen for SETUP.
#[test]
fn keys_reach_the_user_interface() {
    let Some(image) = disc() else { return };
    let variants = fw::load_variants(&image).unwrap();
    // The 192 software on this disc: V220 (Europe) or V222 (North
    // America).
    let Some(v) = variants.iter().find(|v| rnse::board::Model::for_variant(&v.name) == Some(rnse::board::Model::Rnse192)) else { return };
    let index = fw::IndexInfo::from_disc(&image, &v.name).unwrap();
    let mut m = Machine::new();
    m.echo_console = false;
    m.install_boot_stub(&v.name);
    m.install(v, &index);
    m.atapi().load(image.clone());
    m.start_os(v).unwrap();
    let printf = v.module("OSM").and_then(|os| Machine::known_printf(&os.version)).expect("printf address for this OS build");
    m.hook_printf(printf);
    m.set_ignition(true);
    let ms = m.cycles_per_ms();
    assert_eq!(m.run(27_000 * ms), Stop::Budget, "{:?}", m.cpu.fault);
    assert!(m.panel.online, "the OS never acknowledged the faceplate controller");
    let log = String::from_utf8_lossy(&m.console).into_owned();
    assert!(log.contains("init_dsp_pos = 2"), "the OS does not see the display as closed");
    let before = m.board.coral.render().2;
    assert!(before.iter().any(|&b| b != 0), "nothing on the screen");

    assert!(m.panel_key("SETUP", true));
    m.run(200 * ms);
    assert!(m.panel_key("SETUP", false));
    assert_eq!(m.run(2_000 * ms), Stop::Budget, "{:?}", m.cpu.fault);
    let log = String::from_utf8_lossy(&m.console).into_owned();
    assert!(log.contains("cmd = 8 para[0] = 0"), "the SETUP key did not arrive");
    assert_ne!(before, m.board.coral.render().2, "the screen did not change");
}

/// Navigation on the 193: the OS takes the navigation DVD for one, the
/// application reads the map from it, and NAV and then the map soft key
/// bring the map onto the screen, drawn by the graphics engine's line
/// and area commands.
#[test]
fn navigation_shows_the_map() {
    let Some(image) = disc() else { return };
    let Some(nav) = std::env::var_os("RNSE_NAV_DISC") else { return };
    let name = std::env::var("RNSE_NAV_VARIANT").unwrap_or_else(|_| "V575".into());
    let variants = fw::load_variants(&image).unwrap();
    let Some(v) = variants.iter().find(|v| v.name == name) else { return };
    let index = fw::IndexInfo::from_disc(&image, &v.name).unwrap();
    let mut m = Machine::for_model(rnse::board::Model::Rnse193);
    m.echo_console = false;
    m.install_boot_stub(&v.name);
    m.install(v, &index);
    if let Ok(serial) = std::env::var("RNSE_SERIAL") {
        m.set_serial(&serial).unwrap();
    }
    m.atapi().load_disc(rnse::dev::atapi::Disc::open(nav.to_str().unwrap()).expect("RNSE_NAV_DISC is not readable"));
    m.hook_debug_output(v, false);
    assert!(m.printf_hook.is_some() && !m.printf_hooks.is_empty(), "the debug printf of the OS and the application");
    // A GPS fix, in Chicago, far from where the software starts.
    let gps = Machine::find_gps_getter(v.module("OSM").unwrap());
    if let Some((getter, context)) = gps {
        m.set_gps(rnse::dev::gps::Gps::at(41.8781, -87.6298), getter, context);
    }
    m.set_ignition(true);
    m.start_os(v).unwrap();
    let ms = m.cycles_per_ms();
    assert_eq!(m.run(45_000 * ms), Stop::Budget, "{:?}", m.cpu.fault);
    let text = String::from_utf8_lossy(&m.console).into_owned();
    assert!(text.contains("media judge = 1"), "the OS did not take the disc for a DVD");

    assert!(text.contains("DISCTYPE:CDDISC_MAP"), "the application did not take the disc for a map disc (is it tied to a serial number? RNSE_SERIAL)");
    assert!(m.atapi().sectors_read > 1000, "only {} sectors read", m.atapi().sectors_read);

    for key in ["NAV", "SOFT_BR"] {
        assert!(m.panel_key(key, true));
        m.run(200 * ms);
        assert!(m.panel_key(key, false));
        assert_eq!(m.run(8_000 * ms), Stop::Budget, "{:?}", m.cpu.fault);
    }
    assert_eq!(m.run(10_000 * ms), Stop::Budget, "{:?}", m.cpu.fault);
    let navi = m.cpu.io.navi.as_ref().unwrap();
    assert!(navi.ge.unknown.is_empty(), "display list commands not understood: {:x?}", navi.ge.unknown);
    // A map is a screen full of light ground with roads in several
    // colours; the menus are light text on black.
    let (_, _, picture) = m.render();
    let light = picture.chunks(3).filter(|p| p.iter().all(|&c| c > 0xc0)).count();
    assert!(light > 150_000, "only {} light pixels: this is not the map", light);
    let colours: std::collections::BTreeSet<&[u8]> = picture.chunks(3).collect();
    assert!(colours.len() > 50, "only {} colours on the screen", colours.len());
    if gps.is_some() {
        assert!(m.gps_fixes > 10, "the application was given a GPS fix {} times", m.gps_fixes);
    }
}
