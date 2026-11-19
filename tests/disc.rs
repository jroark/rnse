//! End-to-end tests that need a real update disc image.
//!
//! The firmware is not redistributable, so these only run when the
//! environment variable `RNSE_DISC` points at an ISO image of an update
//! CD:
//!
//!     RNSE_DISC=/path/to/disc.iso cargo test --release -- --nocapture
//!
//! These are the emulator's own: the stock software on both units.  The
//! programs of one's own run in their crates' tests, each with its own
//! `tests/disc.rs`: the kit's (`sdk/tests/emulated.rs`, cargo test
//! --release -p rnse-sdk), the SD bridge's (`-p rnse-sdbridge`), the
//! ports' (`-p rnse-ports`) and Linux's (`-p rnse-linux`); they build
//! their programs and are skipped without an SH-4 GCC on the PATH
//! (`sdk/tools/sh4-docker` has one in a container).  Those here that
//! compare with a real boot ROM need
//! `RNSE_BOOT_ROM`, the file `rnse report --rom` writes: firmware, like
//! the disc, and not in the repository.
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

/// The 192's two card processors, emulated, under the real driver: it
/// downloads their program, which starts, answers the first commands
/// and sets its audio output going.
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
    assert_eq!(m.run(12_000 * m.cycles_per_ms()), Stop::Budget, "{:?}", m.cpu.fault);
    let log = String::from_utf8_lossy(&m.console);
    for (ch, dsp) in m.board.media.dsps.iter().enumerate() {
        assert!(dsp.fault.is_none(), "DSP{ch}: {:?}", dsp.fault);
        // The program is there, both parts of it.
        assert!(dsp.imem[0x200] != 0 && dsp.imem[0x8000] != 0, "DSP{ch}: no program");
        // "System initialisation" has set the audio interface up for
        // 44.1 kHz and started it; silence goes out.
        assert_eq!(dsp.sample_rate(), 44_100, "DSP{ch}");
        assert!(dsp.frames > 44_100 && dsp.loud_frames == 0, "DSP{ch}: {} samples sent, {} not silent", dsp.frames, dsp.loud_frames);
        if printf.is_some() {
            assert!(log.contains(&format!("SDC{ch}-DSPC_SYS_INIT Success")), "driver rejected DSP{ch}:\n{log}");
            assert!(log.contains(&format!("SDC{ch}-CARD_EJECT")), "DSP{ch}: no card should be seen:\n{log}");
        }
    }
    assert!(m.cpu.irq_counts[0x7e0 >> 5] > 0, "DSP0 DMA completion never interrupted");
    assert!(!log.contains("BOOT ERROR"), "DSP boot failure:\n{log}");
    assert!(!log.contains("ERR_"), "the driver reports an error:\n{log}");
}

/// The 192 plays an MP3 file from an SD card: its software finds the
/// card by the slot's switch, runs the card's start-up by commands
/// passed through the slot's processor, reads the file system, and has
/// the processor fetch and decode the file.  All of it is the unit's
/// own code, the processor's program included.
#[test]
fn mp3_from_a_card_plays_on_the_192() {
    use rnse::board::Model;
    use rnse::dev::radio::Radio;
    let Some(image) = disc() else { return };
    let variants = fw::load_variants(&image).unwrap();
    let Some(v) = variants.iter().find(|v| Model::for_variant(&v.name) == Some(Model::Rnse192)) else { return };
    let index = fw::IndexInfo::from_disc(&image, &v.name).unwrap();
    let song = include_bytes!("data/tones.mp3").to_vec();
    let entries = vec![rnse::fat::Entry::folder("Tones", vec![rnse::fat::Entry::file("left 440 right 880.mp3", song.clone())]),
        rnse::fat::Entry::file("TONES.MP3", song)];
    let card = rnse::fat::card("MUSIC", &entries, None).unwrap();
    let mut m = Machine::new();
    m.echo_console = false;
    m.install_boot_stub(&v.name);
    m.install(v, &index);
    m.set_ignition(true);
    let printf = Machine::known_printf(&v.module("OSM").unwrap().version);
    if let Some(address) = printf { m.hook_printf(address); }
    m.start_os(v).unwrap();
    let ms = m.cycles_per_ms();
    // The card goes in while the unit runs.  (And the unit takes the
    // MEDIA key only some 14 s after its start.)
    assert_eq!(m.run(9_000 * ms), Stop::Budget, "{}: {:?}", v.name, m.cpu.fault);
    assert!(m.insert_card(0, rnse::dev::sdhi::Card::from_bytes(card).unwrap()));
    assert_eq!(m.run(7_000 * ms), Stop::Budget, "{}: {:?}", v.name, m.cpu.fault);
    if printf.is_some() {
        let log = String::from_utf8_lossy(&m.console);
        assert!(log.contains("SDC0-CARD_INSERT"), "{}: the card was not noticed:\n{}", v.name, log);
    }
    {
        let dsp = &m.board.media.dsps[0];
        assert!(dsp.fault.is_none(), "{}: {:?}", v.name, dsp.fault);
        let card = dsp.card.as_ref().unwrap();
        assert!(card.sectors_read > 20, "{}: the card was not read ({} commands, {} sectors)", v.name, card.commands, card.sectors_read);
    }
    let press = |m: &mut Machine, key: &str| {
        assert!(m.panel_key(key, true));
        m.run(200 * ms);
        assert!(m.panel_key(key, false));
    };
    // MEDIA, the list of sources, "MP3" (second in it, after the CD
    // drive), and it plays the card's first file.
    press(&mut m, "MEDIA");
    assert_eq!(m.run(3_000 * ms), Stop::Budget, "{}: {:?}", v.name, m.cpu.fault);
    press(&mut m, "SOFT_TR");
    assert_eq!(m.run(2_000 * ms), Stop::Budget, "{}: {:?}", v.name, m.cpu.fault);
    assert!(m.panel_knob("main", 1));
    assert_eq!(m.run(1_500 * ms), Stop::Budget, "{}: {:?}", v.name, m.cpu.fault);
    press(&mut m, "SELECT");
    for _ in 0..40 {
        assert_eq!(m.run(250 * ms), Stop::Budget, "{}: {:?}", v.name, m.cpu.fault);
        if m.board.media.dsps[0].loud_frames > 0 {
            break;
        }
    }
    assert_eq!(m.run(1_000 * ms), Stop::Budget, "{}: {:?}", v.name, m.cpu.fault);
    let log = String::from_utf8_lossy(&m.console).into_owned();
    if printf.is_some() {
        assert!(log.contains("Play-OK[/TONES.MP3]"), "{}: not playing:\n{}", v.name, log);
        assert!(!log.contains("SDC_ERROR"), "{}: the player reports an error:\n{}", v.name, log);
    }
    assert!(m.board.media.dsps[0].fault.is_none(), "{}: {:?}", v.name, m.board.media.dsps[0].fault);
    assert!(m.board.media.dsps[0].loud_frames > 0, "{}: the processor sends its converter only silence:\n{}", v.name, log);
    assert_eq!(m.board.media.dsps[1].loud_frames, 0, "{}: the other processor has nothing to play", v.name);
    assert_eq!(m.radio.source, Radio::SOURCE_DECODER, "{}: the amplifier's source", v.name);
    // Two seconds of it: no gaps, the two tones where they belong, and
    // as loud as the amplifier's volume makes it.
    m.speaker = Some(Vec::new());
    assert_eq!(m.run(2_000 * ms), Stop::Budget, "{}: {:?}", v.name, m.cpu.fault);
    let heard = m.speaker.take().unwrap();
    let seconds = heard.len() as f64 / 88_200.0;
    assert!((1.95..=2.05).contains(&seconds), "{}: {} s of sound", v.name, seconds);
    let heard = &heard[8820..];
    let seconds = heard.len() as f64 / 88_200.0;
    for (channel, want) in [(0, 440.0), (1, 880.0)] {
        let side: Vec<i16> = heard.chunks(2).map(|p| p[channel]).collect();
        let crossings = side.windows(2).filter(|w| (w[0] < 0) != (w[1] < 0)).count() as f64;
        let hz = crossings / 2.0 / seconds;
        assert!((hz - want).abs() < 3.0, "{}: channel {} should sound at {} Hz, not {:.1}", v.name, channel, want, hz);
        let peak = side.iter().map(|x| x.unsigned_abs() as f32).fold(0.0, f32::max);
        let full = 0.25 * 32768.0 * m.radio.gain(Radio::SOURCE_DECODER);
        assert!(peak > 0.7 * full && peak < 1.3 * full, "{}: channel {} peak {} at volume {} (expected about {})", v.name, channel, peak, m.radio.volume, full);
    }
    // The card is pulled out: the processor hears of it by its
    // interrupt pin, the main software by the slot's switch, and it is
    // quiet.
    let mark = m.console.len();
    m.eject_card(0);
    assert_eq!(m.run(2_000 * ms), Stop::Budget, "{}: {:?}", v.name, m.cpu.fault);
    let loud = m.board.media.dsps[0].loud_frames;
    assert_eq!(m.run(1_000 * ms), Stop::Budget, "{}: {:?}", v.name, m.cpu.fault);
    assert_eq!(m.board.media.dsps[0].loud_frames, loud, "{}: still playing without a card", v.name);
    assert!(m.board.media.dsps[0].fault.is_none(), "{}: {:?}", v.name, m.board.media.dsps[0].fault);
    if printf.is_some() {
        let log = String::from_utf8_lossy(&m.console[mark..]).into_owned();
        assert!(log.contains("SDC0-CARD_EJECT"), "{}: the removal was not noticed:\n{}", v.name, log);
    }
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
/// the position moves in real time, and what the drive plays is the
/// track.
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
        // The unit has told the amplifier: the CD drive is the source,
        // at some volume.
        assert_eq!(m.radio.source, rnse::dev::radio::Radio::SOURCE_CD, "{}: source", v.name);
        assert!((1..=30).contains(&m.radio.volume), "{}: volume {}", v.name, m.radio.volume);
        // Listen to the drive, and to what comes out of the amplifier,
        // from here on.
        m.sound = Some(Vec::new());
        m.speaker = Some(Vec::new());
        assert_eq!(m.run(10_000 * ms), Stop::Budget, "{}: {:?}", v.name, m.cpu.fault);
        let played = m.atapi().audio_frame() - before;
        assert!((745..=755).contains(&played), "{}: ten seconds should be 750 frames, not {}", v.name, played);
        // Ten seconds of sound, and it is the tone of the track that is
        // playing (a disc without recordings has a pitch per track).
        let sound = m.sound.take().unwrap();
        let seconds = sound.len() as f64 / 88_200.0;
        assert!((9.9..=10.1).contains(&seconds), "{}: {} s of sound", v.name, seconds);
        let track = 1 + (m.atapi().audio_frame() / (180 * 75)) as usize;
        let left: Vec<i16> = sound.chunks(2).map(|p| p[0]).collect();
        let crossings = left.windows(2).filter(|w| (w[0] < 0) != (w[1] < 0)).count() as f64;
        let hz = crossings / 2.0 / seconds;
        let want = AudioDisc::tone_hz(track) as f64;
        assert!((hz - want).abs() < 2.0, "{}: track {} should sound at {} Hz, not {:.1}", v.name, track, want, hz);
        // Through the amplifier: the same, quieter by the volume.
        let speaker = m.speaker.take().unwrap();
        let peak = |s: &[i16]| s.iter().map(|x| x.unsigned_abs() as f32).fold(0.0, f32::max);
        let ratio = peak(&speaker) / peak(&sound);
        assert!((ratio - m.radio.cd_gain()).abs() < 0.01 && ratio > 0.0, "{}: gain {} at volume {}", v.name, ratio, m.radio.volume);
        // Louder when the volume knob is turned up, and silent when the
        // unit goes to another source.
        let quiet = m.radio.volume;
        m.speaker = Some(Vec::new());
        assert!(m.panel_knob("volume", 4));
        assert_eq!(m.run(2_000 * ms), Stop::Budget, "{}: {:?}", v.name, m.cpu.fault);
        assert_eq!(m.radio.volume, quiet + 4, "{}: four steps up", v.name);
        assert!(m.panel_key("RADIO", true));
        m.run(200 * ms);
        assert!(m.panel_key("RADIO", false));
        assert_eq!(m.run(4_000 * ms), Stop::Budget, "{}: {:?}", v.name, m.cpu.fault);
        assert_ne!(m.radio.source, rnse::dev::radio::Radio::SOURCE_CD, "{}: the tuner is the source again", v.name);
        m.speaker = Some(Vec::new());
        m.run(1_000 * ms);
        assert!(m.speaker.take().unwrap().iter().all(|s| *s == 0), "{}: nothing of the CD comes through", v.name);
    }
}

/// MP3 from an SD card on the 193, all of it done by the unit's own
/// software: the card is found and read (host interface and its DMA
/// channel), its files are listed, one is chosen through MEDIA and the
/// list of sources, the software decodes it and sends the samples out
/// through the serial sound interface.  The file has 440 Hz on the left
/// and 880 Hz on the right.
#[test]
fn mp3_from_a_card_plays() {
    use rnse::board::Model;
    use rnse::dev::radio::Radio;
    let Some(image) = disc() else { return };
    let variants = fw::load_variants(&image).unwrap();
    let Some(v) = variants.iter().find(|v| Model::for_variant(&v.name) == Some(Model::Rnse193)) else { return };
    let index = fw::IndexInfo::from_disc(&image, &v.name).unwrap();
    let song = include_bytes!("data/tones.mp3").to_vec();
    let entries = vec![rnse::fat::Entry::folder("Tones", vec![rnse::fat::Entry::file("left 440 right 880.mp3", song.clone())]),
        rnse::fat::Entry::file("TONES.MP3", song)];
    let card = rnse::fat::card("MUSIC", &entries, None).unwrap();
    let mut m = Machine::for_model(Model::Rnse193);
    m.echo_console = false;
    m.install_boot_stub(&v.name);
    m.install(v, &index);
    m.cpu.io.navi.as_mut().unwrap().sdhi.cards[0] = Some(rnse::dev::sdhi::Card::from_bytes(card).unwrap());
    m.set_ignition(true);
    let printf = Machine::known_printf(&v.module("OSM").unwrap().version);
    if let Some(address) = printf { m.hook_printf(address); }
    m.start_os(v).unwrap();
    let ms = m.cycles_per_ms();
    assert_eq!(m.run(30_000 * ms), Stop::Budget, "{}: {:?}", v.name, m.cpu.fault);
    {
        // The card has been read: partition table, FAT, directories and
        // the start of each file, by DMA.
        let host = &m.cpu.io.navi.as_ref().unwrap().sdhi;
        assert!(host.commands.iter().filter(|c| c.0 == 17).count() > 30, "{}: the card was not read: {:?}", v.name, host.commands);
    }
    let press = |m: &mut Machine, key: &str| {
        assert!(m.panel_key(key, true));
        m.run(200 * ms);
        assert!(m.panel_key(key, false));
    };
    // MEDIA, the list of sources, the card (second in it, after the CD
    // drive), and it plays its first file.
    press(&mut m, "MEDIA");
    assert_eq!(m.run(3_000 * ms), Stop::Budget, "{}: {:?}", v.name, m.cpu.fault);
    press(&mut m, "SOFT_TR");
    assert_eq!(m.run(3_000 * ms), Stop::Budget, "{}: {:?}", v.name, m.cpu.fault);
    assert!(m.panel_knob("main", 1));
    assert_eq!(m.run(1_500 * ms), Stop::Budget, "{}: {:?}", v.name, m.cpu.fault);
    press(&mut m, "SELECT");
    // It starts a few seconds after this, sooner or later with the
    // software; the file lasts six.
    for _ in 0..40 {
        assert_eq!(m.run(250 * ms), Stop::Budget, "{}: {:?}", v.name, m.cpu.fault);
        if m.cpu.io.navi.as_ref().unwrap().audio.running {
            break;
        }
    }
    assert_eq!(m.run(1_000 * ms), Stop::Budget, "{}: {:?}", v.name, m.cpu.fault);
    let log = String::from_utf8_lossy(&m.console).into_owned();
    if printf.is_some() {
        assert!(log.contains("finf.filename=[TONES.MP3]"), "{}: not playing:\n{}", v.name, log);
        assert!(!log.contains("SDC_ERROR"), "{}: the player reports an error:\n{}", v.name, log);
    }
    assert_eq!(m.radio.source, Radio::SOURCE_DECODER, "{}: the amplifier's source", v.name);
    let navi = m.cpu.io.navi.as_ref().unwrap();
    assert!(navi.audio.running, "{}: nothing is being sent to the sound interface", v.name);
    assert_eq!(navi.audio_rate(), 44_100, "{}: sample rate", v.name);
    // Two seconds of it: no gaps, the two tones where they belong,
    // and as loud as the amplifier's volume makes it.
    m.speaker = Some(Vec::new());
    assert_eq!(m.run(2_000 * ms), Stop::Budget, "{}: {:?}", v.name, m.cpu.fault);
    let heard = m.speaker.take().unwrap();
    let seconds = heard.len() as f64 / 88_200.0;
    assert!((1.95..=2.05).contains(&seconds), "{}: {} s of sound", v.name, seconds);
    // (Without the first tenth of a second: listening began in the
    // middle of a display frame, and the recording is kept by frames.)
    let heard = &heard[8820..];
    let seconds = heard.len() as f64 / 88_200.0;
    for (channel, want) in [(0, 440.0), (1, 880.0)] {
        let side: Vec<i16> = heard.chunks(2).map(|p| p[channel]).collect();
        let crossings = side.windows(2).filter(|w| (w[0] < 0) != (w[1] < 0)).count() as f64;
        let hz = crossings / 2.0 / seconds;
        assert!((hz - want).abs() < 3.0, "{}: channel {} should sound at {} Hz, not {:.1}", v.name, channel, want, hz);
        let peak = side.iter().map(|x| x.unsigned_abs() as f32).fold(0.0, f32::max);
        let full = 0.25 * 32768.0 * m.radio.gain(Radio::SOURCE_DECODER);
        assert!(peak > 0.7 * full && peak < 1.3 * full, "{}: channel {} peak {} at volume {} (expected about {})", v.name, channel, peak, m.radio.volume, full);
    }
    // No interrupt of the sound interface itself: it never ran dry.
    assert_eq!(m.cpu.irq_counts[0xca0 >> 5], 0, "{}: the sound interface underflowed", v.name);
    assert!(m.cpu.irq_counts[0x980 >> 5] > 20, "{}: blocks finished: {}", v.name, m.cpu.irq_counts[0x980 >> 5]);
}

/// The transport channels to the instrument cluster and to the
/// telephone module: with somebody at the other end the stock software
/// of both units gets them open, keeps them alive, and sends the
/// cluster its first block.
#[test]
fn transport_channels_open() {
    use rnse::board::Model;
    use rnse::dev::tp20::Peer;
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
        m.can_peers = vec![Peer::new(0x6c0, 0x6c1), Peer::new(0x6c4, 0x6c5)];
        // The telephone's status message, as the earlier investigation
        // had it: without it the 192 does not ask for that channel.
        m.can_send_cyclic(rnse::dev::hcan::Frame::new(0x665, &[0; 7]), 200);
        m.start_os(v).unwrap();
        assert_eq!(m.run(30_000 * m.cycles_per_ms()), Stop::Budget, "{}: {:?}", v.name, m.cpu.fault);
        assert!(m.can_peers[0].open, "{}: channel to the cluster", v.name);
        assert!(m.can_peers[1].open, "{}: channel to the telephone", v.name);
        assert_eq!(m.can_peers[0].received.first().map(|b| &b[..]), Some(&[0x15, 0x01, 0x01, 0x02, 0x00, 0x00][..]), "{}: first block to the cluster", v.name);
        // No more requests for a channel once it is there.
        let asked = m.can().tx.iter().filter(|f| f.id == 0x6c4 && f.data[0] == 0xa0).count();
        assert!(asked <= 3, "{}: {} channel requests to the telephone", v.name, asked);
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

/// A boot ROM read from a 192 (`rnse report --rom`; firmware, so not in
/// the repository): `RNSE_BOOT_ROM` names the file.
fn boot_rom() -> Option<Vec<u8>> {
    let path = std::env::var_os("RNSE_BOOT_ROM")?;
    Some(std::fs::read(path).expect("RNSE_BOOT_ROM is not readable"))
}

/// The 192 with its installed software, started the stand-in's way or,
/// with a ROM, from reset with nothing done for it.
fn started_192(image: &[u8], rom: Option<&[u8]>) -> Option<Machine> {
    use rnse::board::Model;
    let variants = fw::load_variants(image).unwrap();
    let v = variants.iter().find(|v| Model::for_variant(&v.name) == Some(Model::Rnse192))?;
    let index = fw::IndexInfo::from_disc(image, &v.name).unwrap();
    let mut m = Machine::new();
    m.echo_console = false;
    m.scramble_ram();
    m.install_boot_stub(&v.name);
    m.install(v, &index);
    m.atapi().load(image.to_vec());
    m.set_ignition(true);
    if let Some(address) = Machine::known_printf(&v.module("OSM").unwrap().version) {
        m.hook_printf(address);
    }
    match rom {
        Some(rom) => m.start_from_reset(rom).unwrap(),
        None => m.start_os(v).unwrap(),
    }
    Some(m)
}

/// The stand-in against the real thing: the same software, started by
/// a unit's boot ROM from reset and by the stand-in, says the same in
/// its log, line for line, and shows the same picture, with arbitrary
/// data in RAM at the start.  What the stand-in does for the 192 was
/// taken from that ROM (docs/boot-rom.md, "The real one").
#[test]
fn stand_in_starts_the_software_as_a_real_boot_rom_does() {
    let (Some(image), Some(rom)) = (disc(), boot_rom()) else { return };
    let Some(mut real) = started_192(&image, Some(&rom)) else { return };
    let mut stand_in = started_192(&image, None).unwrap();
    let ms = real.cycles_per_ms();
    for m in [&mut real, &mut stand_in] {
        assert_eq!(m.run(17_000 * ms), Stop::Budget, "{:?}", m.cpu.fault);
    }
    let log = |m: &Machine| String::from_utf8_lossy(&m.console).lines().filter(|l| l.starts_with("[fw]")).map(String::from).collect::<Vec<_>>();
    let (a, b) = (log(&real), log(&stand_in));
    assert!(a.len() > 100, "the software said little under the real ROM: {:?}", a);
    assert_eq!(a, b);
    assert!(real.render() == stand_in.render(), "the pictures differ");
    // What the ROM leaves and the stand-in is to leave as well.
    for m in [&real, &stand_in] {
        assert_eq!(m.cpu.io.intc.ipra, 0x7050);
        assert_eq!(m.cpu.io.dmaor & 0xc001, 0xc001);
    }
}

/// A 192 with nothing in its flash but the boot ROM, and an update
/// disc in the drive: the ROM starts the loader from the disc by
/// itself, the loader installs the software and jumps to the ROM's
/// reset code, and the software starts.
#[test]
fn real_boot_rom_brings_an_empty_unit_back_from_a_disc() {
    use rnse::board::Model;
    let (Some(image), Some(rom)) = (disc(), boot_rom()) else { return };
    let variants = fw::load_variants(&image).unwrap();
    let Some(v) = variants.iter().find(|v| Model::for_variant(&v.name) == Some(Model::Rnse192)) else { return };
    let mut m = Machine::new();
    m.echo_console = false;
    m.install_boot_stub(&v.name);
    m.board.flash.fill(0xff);
    m.atapi().load(image.clone());
    m.set_ignition(true);
    if let Some(address) = Machine::known_printf(&v.module("OSM").unwrap().version) {
        m.hook_printf(address);
    }
    m.start_from_reset(&rom).unwrap();
    let ms = m.cycles_per_ms();
    let mut done = false;
    for _ in 0..40 {
        assert_eq!(m.run(5_000 * ms), Stop::Budget, "{:?}", m.cpu.fault);
        let text = String::from_utf8_lossy(&m.console);
        if let Some(at) = text.find("*** Jump User RESET ***") {
            // And then the software that was just written.
            if text[at..].contains("[fw] ") {
                done = true;
                break;
            }
        }
    }
    let text = String::from_utf8_lossy(&m.console).into_owned();
    assert!(done, "no update, or nothing after it: {:?}", &text[text.len().saturating_sub(400)..]);
    for module in v.modules.iter().filter(|x| x.kind != "LDR") {
        let at = module.phys_addr() as usize;
        assert!(m.board.flash[at..at + module.size as usize] == module.data[..module.size as usize], "{} is not in the flash", module.name);
    }
}
