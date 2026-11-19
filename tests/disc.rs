//! End-to-end tests that need a real update disc image.
//!
//! The firmware is not redistributable, so these only run when the
//! environment variable `RNSE_DISC` points at an ISO image of an update
//! CD:
//!
//!     RNSE_DISC=/path/to/disc.iso cargo test --release -- --nocapture
//!
//! The tests here that run the SDK's examples (`sdk/`: the programs
//! that come from an SD card, Linux) build them and are skipped without
//! an SH-4 GCC on the PATH (`sdk/tools/sh4-docker` has one in a
//! container).  The SDK's own tests are in its crate,
//! `sdk/tests/emulated.rs` (cargo test --release -p rnse-sdk).  Those that compare with a real boot ROM need
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

/// A program written from scratch (sdk/examples/hello), built with an
/// SH-4 GCC if there is one, put in the loader's place on the disc and
/// started the way the boot ROM starts the loader: it brings the board
/// up, prints on the debug serial port, draws and keeps time, on both
/// units.
/// Build an SDK example for a unit, put it in the loader's place and
/// start it.  None when there is no cross compiler or the disc has no
/// software for that unit.
fn sdk_example(image: &[u8], model: rnse::board::Model, example: &str) -> Option<(Machine, String, Vec<u8>)> {
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
    // As unforgiving as the emulator gets (`--strict`).
    m.busy_start();
    m.watchdog_always = true;
    m.cache_model();
    Some((m, v.name.clone(), custom))
}

/// An SDK example on a disc with a higher software index than the
/// installed one, and the installed (stock) software run until its
/// application has checked the disc, copied the program to RAM and is
/// about to call its first entry point.  That is how a real unit would
/// come to run it.  None when there is no cross compiler or the disc
/// has no software for the unit.
fn started_by_stock_software(image: &[u8], model: rnse::board::Model, example: &str) -> Option<(Machine, String, Vec<u8>)> {
    started_by_stock_software_with(image, model, example, None)
}

/// The same, with the disc in the drive from the start (None) or put
/// in after so many milliseconds, as someone would do it.
fn started_by_stock_software_with(image: &[u8], model: rnse::board::Model, example: &str, insert_at: Option<u64>) -> Option<(Machine, String, Vec<u8>)> {
    use rnse::board::Model;
    let sdk = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("sdk");
    let variants = fw::load_variants(image).unwrap();
    let v = variants.iter().find(|v| Model::for_variant(&v.name) == Some(model))?;
    sdk_example(image, model, example)?;
    let unit = if model == Model::Rnse192 { "192" } else { "193" };
    let program = std::fs::read(sdk.join(format!("out/{}-{}.bin", example, unit))).unwrap();
    let (mut custom, _) = fw::custom_loader(image, &v.name, &program).unwrap();
    // The disc's index, its two middle digits one up: those are what
    // the application compares.
    let index = fw::IndexInfo::from_disc(image, &v.name).unwrap();
    let middle: u32 = index.sofidx[1..3].parse().unwrap();
    let higher = format!("{}{:02}{}", &index.sofidx[..1], middle + 1, &index.sofidx[3..]);
    assert_eq!(fw::set_disc_index(&mut custom, &v.name, &higher).unwrap(), index.sofidx);

    // The stock software's own start of a loader can fail: on the
    // European 192 the copy of the loader goes over a variable of the
    // module APFM, whose timer handler (every 100 ms) then meets an
    // address error if it runs in the 15 ms before the loader has
    // taken the unit over (docs/boot-rom.md).  Whether it does is a
    // matter of timing, which on a real unit the drive makes vary and
    // which here is the same in every run.  So the start is tried at
    // the nominal ratio of CPU speed to timers and, when exactly that
    // happens, at the two next to it.
    let apfm = v.module("APFM").map(|a| a.load_addr | 0x8000_0000..(a.load_addr | 0x8000_0000) + a.size);
    let nominal = if model == Model::Rnse192 { 4 } else { 8 };
    let mut started = None;
    for ratio in [nominal, nominal + 1, nominal - 1] {
        let mut m = Machine::for_model(model);
        m.set_cpu_ratio(ratio);
        m.echo_console = false;
        // The CPU's caches are modelled all the way: the stock software has
        // to live with them as on a real unit, and so has the program.
        m.scramble_ram();
        m.cache_model();
        m.install_boot_stub(&v.name);
        m.install(v, &index);
        if insert_at.is_none() {
            m.atapi().load(custom.clone());
        }
        m.start_os(v).unwrap();
        m.set_ignition(true);
        let ms = m.cycles_per_ms();
        let entry = fw::load_variants(&custom).unwrap().iter().find(|x| x.name == v.name).unwrap().module("LDM").unwrap().entries[0];
        m.cpu.breakpoints.push(entry);
        let mut stop = Stop::Budget;
        if let Some(at) = insert_at {
            stop = m.run(at * ms);
            assert_eq!(stop, Stop::Budget, "{}: {:?}", v.name, m.cpu.fault);
            m.atapi().insert(Some(custom.clone()));
        }
        for _ in 0..40 {
            if stop != Stop::Budget {
                break;
            }
            stop = m.run(1_000 * ms);
        }
        let collision = stop == Stop::Fault && m.cpu.fault.as_deref().is_some_and(|f| {
            let spc = f.split("SPC=").nth(1).and_then(|t| u32::from_str_radix(&t[..8.min(t.len())], 16).ok());
            f.contains("EXPEVT=0e0") && spc.zip(apfm.clone()).is_some_and(|(spc, range)| range.contains(&spc))
        });
        if collision {
            eprintln!("{}: at CPU ratio {} the stock software's start of the loader collides with its own timer handler", v.name, ratio);
            continue;
        }
        assert_eq!(stop, Stop::Breakpoint, "{}: the installed software did not start the program: {:?}", v.name, m.cpu.fault);
        started = Some(m);
        break;
    }
    let mut m = started.unwrap_or_else(|| panic!("{}: the stock software's start of the loader failed at every speed tried", v.name));
    m.cpu.breakpoints.clear();
    Some((m, v.name.clone(), custom))
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

/// A 192 as `started_192` makes it, under a unit's boot ROM, with the
/// software of `flash` installed and `drive` in the drive.
fn rom_192(flash: &[u8], drive: &[u8], rom: &[u8]) -> Option<Machine> {
    use rnse::board::Model;
    let variants = fw::load_variants(flash).unwrap();
    let v = variants.iter().find(|v| Model::for_variant(&v.name) == Some(Model::Rnse192))?;
    let index = fw::IndexInfo::from_disc(flash, &v.name).unwrap();
    let mut m = Machine::new();
    m.echo_console = false;
    m.scramble_ram();
    m.cache_model();
    m.install_boot_stub(&v.name);
    m.install(v, &index);
    m.atapi().load(drive.to_vec());
    m.set_ignition(true);
    if let Some(address) = Machine::known_printf("V2220371") {
        m.hook_printf(address);
    }
    m.start_from_reset(rom).unwrap();
    Some(m)
}

/// Run until the log has `what` after `from`, in steps of five seconds.
fn until(m: &mut Machine, from: usize, what: &str, seconds: u64) -> Option<usize> {
    let ms = m.cycles_per_ms();
    for _ in 0..seconds / 5 {
        assert_eq!(m.run(5_000 * ms), Stop::Budget, "{:?}", m.cpu.fault);
        let text = String::from_utf8_lossy(&m.console);
        if let Some(at) = text.get(from..).and_then(|t| t.find(what)) {
            return Some(from + at);
        }
    }
    None
}

fn stock_is_in_flash(m: &Machine, image: &[u8]) {
    use rnse::board::Model;
    let variants = fw::load_variants(image).unwrap();
    let v = variants.iter().find(|v| Model::for_variant(&v.name) == Some(Model::Rnse192)).unwrap();
    for module in v.modules.iter().filter(|x| x.kind != "LDR") {
        let at = module.phys_addr() as usize;
        assert!(m.board.flash[at..at + module.size as usize] == module.data[..module.size as usize], "{} is not the stock module", module.name);
    }
}

/// Android Auto's video on the 192 (sdk/examples/h264bench): the
/// stock software starts the SD loader, the loader the benchmark, which
/// decodes an H.264 stream from the card as a phone would send it
/// (tests/data/margin.264: twelve synthetic frames of 800x480 made by
/// tools/aa-streams.sh) and finds every frame as ffmpeg decodes it
/// (margin.crc, the CRC-32s ffmpeg's frames have), twice: with the
/// caches as the stock software leaves them and the decoder as it came,
/// and with the caches set up anew (`cache_setup`: index modes off, P1
/// copy-back) and the decoder's direct copy, under the modelled caches.
#[test]
fn sdk_decodes_h264_from_a_card() {
    use rnse::board::Model;
    let Some(image) = disc() else { return };
    let sdk = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("sdk");
    let Some((mut m, name, _)) = started_by_stock_software(&image, Model::Rnse192, "sdboot") else { return };
    if sdk_example(&image, Model::Rnse192, "h264bench").is_none() { return }
    let bench = std::fs::read(sdk.join("out/h264bench-192.bin")).unwrap();
    let entries = vec![
        rnse::fat::Entry::file("RNSE.BIN", bench),
        rnse::fat::Entry::file("MARGIN.264", include_bytes!("data/margin.264").to_vec()),
        rnse::fat::Entry::file("MARGIN.CRC", include_bytes!("data/margin.crc").to_vec()),
    ];
    let card = rnse::fat::card("VIDEO", &entries, None).unwrap();
    assert!(m.insert_card(0, rnse::dev::sdhi::Card::from_bytes(card).unwrap()));
    let flash = m.board.flash.clone();
    let ms = m.cycles_per_ms();
    m.cpu.breakpoints.clear();
    let text = loop {
        assert_eq!(m.run(1_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
        let text = String::from_utf8_lossy(&m.console).into_owned();
        if text.contains("DONE.") || m.cpu.cycles > 120_000 * ms {
            break text;
        }
    };
    let tail = &text[text.len().saturating_sub(1200)..];
    assert!(text.contains("rnse sdk: h264bench"), "{}: {:?}", name, tail);
    // Each frame's line is the CRC-32 of the frames up to it, which is
    // what each line of the .CRC file has.
    let want: Vec<&str> = std::str::from_utf8(include_bytes!("data/margin.crc")).unwrap().lines().collect();
    assert_eq!(want.len(), 12);
    for crc in &want {
        assert_eq!(text.matches(&format!("{} ", crc)).count(), 2, "{}: frame CRC {} not seen once a pass: {:?}", name, crc, tail);
    }
    let rows: Vec<&str> = text.lines().filter(|l| l.contains("MARGIN ") && (l.starts_with("STOCK ") || l.starts_with("COPY+CB+NOIX "))).collect();
    assert_eq!(rows.len(), 2, "{}: {:?}", name, tail);
    for row in rows {
        assert!(row.ends_with(" OK"), "{}: {}", name, row);
    }
    assert!(!text.contains("unexpected"), "{}: {:?}", name, tail);
    // And back to the SD loader, with the caches as the program found
    // them and the flash as it was.
    let at = text.find("DONE.").unwrap();
    until(&mut m, at, "TEST RETURNED: 0", 90).unwrap_or_else(|| panic!("{}: not back at the loader: {:?}", name, String::from_utf8_lossy(&m.console[at..])));
    assert_eq!(m.board.flash, flash, "{}", name);
    let cache = &m.cpu.cache.trouble;
    assert_eq!((cache.stale_fetches, cache.dirty_discarded), (0, 0), "{}: {:?}", name, cache);
}

/// Linux from an SD card (docs/linux.md): the stock software starts
/// sdboot from the disc, sdboot starts linuxboot (RNSE.BIN) from the
/// card, and linuxboot loads the kernel (VMLINUX.BIN, as linux/build.sh
/// makes it; RNSE_LINUX names the file) and enters it.  The kernel comes
/// up on the 192's own processor with its caches modelled, starts
/// BusyBox and the GUI (rnse-gui), and keeps the board's watchdog served
/// with room to spare.  The GUI's "Shell" goes to the shell on the second
/// console, where a command typed with nothing but the knob, SELECT and
/// the soft keys ("rotary typing") runs; INFO comes back.
/// The controls work throughout (CLAUDE.md,
/// "Mandatory hardware behavior"): EJECT folds the display down and up
/// while linuxboot reads the card and under Linux, CD EJECT pushes the
/// disc out, a short RETURN does nothing of the kind, and RETURN held for
/// two seconds goes back to sdboot, which does not start Linux again by
/// itself.  Without RNSE_LINUX nothing is tested.
#[test]
fn linux_boots_from_an_sd_card() {
    use rnse::board::Model;
    let Some(image) = disc() else { return };
    let Some(kernel) = std::env::var_os("RNSE_LINUX") else { return };
    let kernel = std::fs::read(kernel).expect("RNSE_LINUX is not readable");
    let sdk = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("sdk");
    let Some((mut m, name, _)) = started_by_stock_software(&image, Model::Rnse192, "sdboot") else { return };
    if sdk_example(&image, Model::Rnse192, "linuxboot").is_none() { return }
    let linuxboot = std::fs::read(sdk.join("out/linuxboot-192.bin")).unwrap();
    let entries = vec![rnse::fat::Entry::file("RNSE.BIN", linuxboot), rnse::fat::Entry::file("VMLINUX.BIN", kernel)];
    let card = rnse::fat::card("LINUX", &entries, None).unwrap();
    assert!(m.insert_card(0, rnse::dev::sdhi::Card::from_bytes(card).unwrap()));
    let ms = m.cycles_per_ms();
    m.cpu.breakpoints.clear();
    // The watchdog from here on: the stock software's own start (some
    // 33 ms to its first edge, with the watchdog armed from power-on)
    // is not what this test judges.
    m.watchdog.longest = 0;
    let console = |m: &Machine| String::from_utf8_lossy(&m.console).into_owned();
    let press = |m: &mut Machine, key: &str, hold: u64| {
        assert!(m.panel_key(key, true));
        assert_eq!(m.run(hold * ms), Stop::Budget, "{:?}", m.cpu.fault);
        assert!(m.panel_key(key, false));
    };

    // While the card is read: the display folds down and up again.
    for _ in 0..200 {
        if console(&m).contains("READING VMLINUX.BIN") { break }
        assert_eq!(m.run(100 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    }
    assert!(console(&m).contains("READING VMLINUX.BIN"), "{}: {:?}", name, console(&m));
    press(&mut m, "EJECT", 200);
    assert_eq!(m.run(2_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    assert!(m.board.display_pos <= 94, "{}: the display is at {} while the card is read", name, m.board.display_pos);
    assert!(!console(&m).contains("STARTING LINUX"), "{}: the card was read too fast to try the keys", name);
    press(&mut m, "EJECT", 200);
    assert_eq!(m.run(2_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    assert!(m.board.display_pos >= 894, "{}: the display is at {}", name, m.board.display_pos);

    // The card's start, the kernel from it, the kernel's start, BusyBox
    // and the GUI.
    until(&mut m, 0, "rnse: Linux on the RNS-E 192 (sd start)", 90).unwrap_or_else(|| panic!("{}: Linux did not come up: {:?}", name, console(&m)));
    assert_eq!(m.run(6_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    let text = console(&m);
    let tail = &text[text.len().saturating_sub(600)..];
    assert!(text.contains("rnse sdk: linuxboot"), "{}: {:?}", name, tail);
    assert!(text.contains("Booting machvec: Audi RNS-E 192"), "{}: {:?}", name, tail);
    assert!(text.contains("CPU: SH7750R"), "{}: {:?}", name, tail);
    assert!(text.contains("fb0: 400x235"), "{}: {:?}", name, tail);
    assert!(text.contains("return to the loader with RETURN held"), "{}: {:?}", name, tail);
    assert!(!text.contains("Kernel panic") && !text.contains("Oops"), "{}: {:?}", name, tail);
    let (_, _, pixels) = m.render();
    assert!(pixels.iter().any(|&p| p != 0), "{}: the GUI's screen is dark", name);

    // The GUI's "Shell", the sixth of its list, and there, typed with the
    // knob, a command whose answer comes out on the serial console, the
    // kernel's log.
    let ready = until(&mut m, 0, "rnse-gui: ready", 30).unwrap_or_else(|| panic!("{}: the GUI did not come up: {:?}", name, console(&m)));
    assert!(m.panel_knob("main", 5));
    assert_eq!(m.run(300 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    press(&mut m, "SELECT", 100);
    until(&mut m, ready, "rnse-gui: shell", 10).unwrap_or_else(|| panic!("{}: the GUI did not go to the shell: {:?}", name, console(&m)));
    assert_eq!(m.run(500 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    const RING: &str = "abcdefghijklmnopqrstuvwxyz0123456789-./_|>&;=*~$'\"<()[]{}!?#@%+:,\\^`ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let edges = m.board.watchdog_edges;
    let mut at = 0;
    for c in "echo rotary>/dev/kmsg".chars() {
        if c == ' ' {
            press(&mut m, "SOFT_BL", 100);
        } else {
            let target = RING.find(c).unwrap();
            // The first detent shows the character where the ring stands.
            let detents = (target + RING.len() - at) % RING.len() + 1;
            assert!(m.panel_knob("main", detents as i32));
            assert_eq!(m.run(100 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
            press(&mut m, "SELECT", 100);
            at = target;
        }
        assert_eq!(m.run(50 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    }
    press(&mut m, "SOFT_BR", 100);
    assert_eq!(m.run(2_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    let text = console(&m);
    assert!(text.lines().any(|l| l.trim_end().ends_with("] rotary")), "{}: the typed command did not run: {:?}", name, &text[text.len().saturating_sub(600)..]);
    assert!(m.board.watchdog_edges > edges + 500, "{}: {} edges while typing", name, m.board.watchdog_edges - edges);
    let (_, _, pixels) = m.render();
    assert!(pixels.iter().any(|&p| p != 0), "{}: the screen is dark", name);
    let shell = console(&m).len();
    press(&mut m, "INFO", 100);
    until(&mut m, shell, "rnse-gui: back", 10).unwrap_or_else(|| panic!("{}: INFO did not bring the GUI back", name));

    // Under Linux: a short RETURN stays in Linux; the display folds down,
    // the disc comes out, the display folds up.
    press(&mut m, "RETURN", 300);
    assert_eq!(m.run(1_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    assert!(!console(&m).contains("back to the SD loader"), "{}: a short RETURN left Linux", name);
    press(&mut m, "EJECT", 200);
    assert_eq!(m.run(2_500 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    assert!(m.board.display_pos <= 94, "{}: the display is at {} under Linux", name, m.board.display_pos);
    let ejects = m.atapi().ejects;
    press(&mut m, "CD_EJECT", 200);
    assert_eq!(m.run(2_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    let text = console(&m);
    assert_eq!(m.atapi().ejects, ejects + 1, "{}: no eject: {:?}", name, &text[text.len().saturating_sub(300)..]);
    press(&mut m, "EJECT", 200);
    assert_eq!(m.run(2_500 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    assert!(m.board.display_pos >= 894, "{}: the display is at {}", name, m.board.display_pos);

    // RETURN held: back to sdboot, which stays there.
    press(&mut m, "RETURN", 2_500);
    assert_eq!(m.run(3_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    let text = console(&m);
    let back = text.find("back to the SD loader").unwrap_or_else(|| panic!("{}: RETURN held did not leave Linux", name));
    assert!(text[back..].contains("rnse sdk: sdboot") && text[back..].contains("TEST RETURNED: 0"), "{}: {:?}", name, &text[back..]);
    assert!(!text[back..].contains("rnse sdk: linuxboot"), "{}: Linux was started again", name);

    // The unit resets 105 ms after the last edge: keep well inside.
    // The emulator counts an instruction as a cycle, which is kind to
    // loops of cache operations: a pause of 50 ms here, on the way back
    // (the purge of all RAM from the cache, unstrobed), was a reset on a
    // unit, which then came up in the stock software.  Normal is the
    // tick's spacing, some 16 ms; under Linux the console's scrolling
    // (written with interrupts off) makes 33, which is the next thing
    // to bring down.
    let longest = m.watchdog.longest / ms;
    assert!(longest < 40, "{}: the watchdog waited {} ms, until {} ms, PC {:08x}", name, longest, m.watchdog.longest_end / ms, m.watchdog.longest_pc);
    let cache = &m.cpu.cache.trouble;
    assert_eq!((cache.stale_fetches, cache.dirty_discarded), (0, 0), "{}: {:?}", name, cache);
}

/// The SD path to Linux (as `linux_boots_from_an_sd_card`) with a card
/// that also holds files and a script, AUTORUN.SH, which /etc/init.d/rcS
/// runs: every driver of docs/linux.md from user space.  The flash and
/// the battery-backed memory read, the A/D converter, the card itself, the
/// radio processor answering, CAN frames out on the bus (and the bus's
/// frames in), the disc in the drive mounted and read, sound from a WAV
/// and an MP3 file through a card processor's converter.  Then the GUI's
/// pages, with the keys; and the disc comes out and the display folds
/// down while sound plays.
#[test]
fn linux_drivers_and_gui_from_an_sd_card() {
    use rnse::board::Model;
    let Some(image) = disc() else { return };
    let Some(kernel) = std::env::var_os("RNSE_LINUX") else { return };
    let kernel = std::fs::read(kernel).expect("RNSE_LINUX is not readable");
    let sdk = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("sdk");
    let Some((mut m, name, _)) = started_by_stock_software(&image, Model::Rnse192, "sdboot") else { return };
    if sdk_example(&image, Model::Rnse192, "linuxboot").is_none() { return }
    let linuxboot = std::fs::read(sdk.join("out/linuxboot-192.bin")).unwrap();
    let mut wav = Vec::new();
    let samples: Vec<i16> = (0..22050).map(|i| ((i as f64 * 440.0 * std::f64::consts::TAU / 22050.0).sin() * 8000.0) as i16).collect();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + 2 * samples.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    for v in [16u32, 0x0001_0001, 22050, 44100, 0x0010_0002] {
        wav.extend_from_slice(&v.to_le_bytes());
    }
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(2 * samples.len() as u32).to_le_bytes());
    for v in &samples {
        wav.extend_from_slice(&v.to_le_bytes());
    }
    let script = b"echo \"== T: mode $(cat /sys/kernel/rnse/mode)\"
echo \"== T: mtd $(grep -c mtd /proc/mtd)\"
echo \"== T: sram $(dd if=/sys/bus/nvmem/devices/rnse-sram/nvmem bs=1 count=4 skip=261888 2>/dev/null | wc -c)\"
echo \"== T: adc $(cat /sys/bus/iio/devices/iio:device0/in_voltage3_raw)\"
echo \"== T: card $(cat /sys/block/rnsesd0/size)\"
echo \"== T: radio $(rnse-radio version)\"
rnse-can send 7f0#0102 && echo \"== T: can sent $(cat /sys/class/net/can0/statistics/tx_packets)\"
echo \"== T: can received $(rnse-can dump 2 | wc -l)\"
mount -t iso9660 -o ro /dev/sr0 /media/cd && echo \"== T: cd $(ls /media/cd | grep -c LOADING.KWI)\"
umount /media/cd
rnse-radio source decoder 12
rnse-play /media/sd0/TONE.WAV /media/sd0/TONES.MP3 && echo \"== T: played\"
echo \"== T: done\"
".to_vec();
    let entries = vec![
        rnse::fat::Entry::file("RNSE.BIN", linuxboot),
        rnse::fat::Entry::file("VMLINUX.BIN", kernel),
        rnse::fat::Entry::file("TONE.WAV", wav),
        rnse::fat::Entry::file("TONES.MP3", include_bytes!("data/tones.mp3").to_vec()),
        rnse::fat::Entry::file("AUTORUN.SH", script),
    ];
    let card = rnse::fat::card("LINUX", &entries, None).unwrap();
    assert!(m.insert_card(0, rnse::dev::sdhi::Card::from_bytes(card).unwrap()));
    let ms = m.cycles_per_ms();
    m.cpu.breakpoints.clear();
    m.watchdog.longest = 0;
    let console = |m: &Machine| String::from_utf8_lossy(&m.console).into_owned();
    let press = |m: &mut Machine, key: &str, hold: u64| {
        assert!(m.panel_key(key, true));
        assert_eq!(m.run(hold * ms), Stop::Budget, "{:?}", m.cpu.fault);
        assert!(m.panel_key(key, false));
    };
    let start = until(&mut m, 0, "rnse: running /media/sd0/AUTORUN.SH", 120).unwrap_or_else(|| panic!("{}: the card's script did not run: {:?}", name, console(&m)));
    let played = until(&mut m, start, "== T: played", 90);
    let text = console(&m);
    let log = &text[start..];
    assert!(played.is_some(), "{}: {:?}", name, log);
    for (what, want) in [("mode", "sd"), ("mtd", "7"), ("sram", "4"), ("adc", "1023"), ("can sent", "1"), ("can received", "2"), ("cd", "1")] {
        let line = log.lines().find(|l| l.starts_with(&format!("== T: {} ", what))).unwrap_or_else(|| panic!("{}: no {}: {:?}", name, what, log));
        assert_eq!(line[format!("== T: {} ", what).len()..].trim(), want, "{}: {}", name, line);
    }
    assert!(log.contains("== T: radio A2220141"), "{}: the radio processor did not answer: {:?}", name, log);
    assert!(log.lines().any(|l| l.starts_with("== T: card ") && l[11..].trim().parse::<u64>().unwrap_or(0) > 0), "{}: no card: {:?}", name, log);
    assert!(m.can().tx.iter().any(|f| f.id == 0x7f0 && f.data[..2] == [1, 2]), "{}: 7f0 not on the bus", name);
    let loud = m.board.media.dsps[0].loud_frames + m.board.media.dsps[1].loud_frames;
    assert!(loud > 44_100, "{}: the converters played {} frames that were not silent", name, loud);
    assert!(m.radio.source == 4, "{}: the amplifier's source is {}, not the card processors", name, m.radio.source);

    // The GUI's pages, by their keys.
    for key in ["RADIO", "MEDIA", "CAR", "SETUP", "INFO"] {
        press(&mut m, key, 100);
        assert_eq!(m.run(1_500 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
        let (_, _, pixels) = m.render();
        assert!(pixels.iter().any(|&p| p != 0), "{}: the screen is dark after {}", name, key);
    }
    // Sound from the GUI (Media, the first card, its first file), and
    // while it plays the disc comes out and the display folds down.
    press(&mut m, "MEDIA", 100);
    assert_eq!(m.run(1_000 * ms), Stop::Budget);
    press(&mut m, "SELECT", 100);
    assert_eq!(m.run(1_000 * ms), Stop::Budget);
    let before = m.board.media.dsps[0].loud_frames + m.board.media.dsps[1].loud_frames;
    press(&mut m, "SELECT", 100);
    assert_eq!(m.run(2_000 * ms), Stop::Budget);
    let after = m.board.media.dsps[0].loud_frames + m.board.media.dsps[1].loud_frames;
    assert!(after > before, "{}: nothing played from the GUI", name);
    let ejects = m.atapi().ejects;
    press(&mut m, "CD_EJECT", 200);
    assert_eq!(m.run(4_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    assert!(m.board.display_pos <= 94, "{}: the display is at {}", name, m.board.display_pos);
    assert_eq!(m.atapi().ejects, ejects + 1, "{}: the disc did not come out while sound played", name);
    // RETURN held: playback stops and sdboot is back.
    press(&mut m, "RETURN", 2_500);
    assert_eq!(m.run(3_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    let text = console(&m);
    let back = text.find("back to the SD loader").unwrap_or_else(|| panic!("{}: RETURN held did not leave Linux", name));
    assert!(text[back..].contains("TEST RETURNED: 0"), "{}: {:?}", name, &text[back..]);
    assert!(!text.contains("Kernel panic") && !text.contains("Oops") && !text.contains("BUG:"), "{}", name);
    let longest = m.watchdog.longest / ms;
    assert!(longest < 40, "{}: the watchdog waited {} ms, until {} ms, PC {:08x}", name, longest, m.watchdog.longest_end / ms, m.watchdog.longest_pc);
    let cache = &m.cpu.cache.trouble;
    assert_eq!((cache.stale_fetches, cache.dirty_discarded), (0, 0), "{}: {:?}", name, cache);
}

/// The disc `rnse firmware ... --kernel` makes: linuxboot in the
/// operating system's place, the kernel (RNSE_LINUX) in the application's,
/// the swap loader, and a higher index than the stock disc's.  With
/// `spoil`, one byte of the kernel changed after its sum was taken.
fn linux_firmware_disc(image: &[u8], spoil: bool) -> Option<Vec<u8>> {
    use rnse::board::Model;
    let kernel = std::fs::read(std::env::var_os("RNSE_LINUX")?).expect("RNSE_LINUX is not readable");
    let sdk = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("sdk");
    let variants = fw::load_variants(image).unwrap();
    let v = variants.iter().find(|v| Model::for_variant(&v.name) == Some(Model::Rnse192))?;
    sdk_example(image, Model::Rnse192, "linuxboot")?;
    let program = std::fs::read(sdk.join("out/linuxboot-192.bin")).unwrap();
    sdk_example(image, Model::Rnse192, "swap")?;
    let swap = std::fs::read(sdk.join("out/swap-192.bin")).unwrap();
    let mut disc = fw::firmware_disc_with_kernel(image, &v.name, &program, Some(&swap), Some(&kernel)).unwrap();
    if spoil {
        let apm = v.module("APM").unwrap();
        let mut spoilt = fw::kernel_module(&apm.data, &kernel).unwrap();
        spoilt[fw::KERNEL_AT + 0x3000] ^= 0x40;
        let name = v.name.clone();
        disc = fw::repack(&disc, &|var, module| (var == name && module == "APM").then(|| spoilt.clone())).unwrap().0;
    }
    let index = fw::IndexInfo::from_disc(image, &v.name).unwrap();
    let middle: u32 = index.sofidx[1..3].parse().unwrap();
    let higher = format!("{}{:02}{}", &index.sofidx[..1], middle + 1, &index.sofidx[3..]);
    fw::set_disc_index(&mut disc, &v.name, &higher).unwrap();
    Some(disc)
}

/// Linux installed in flash (docs/linux.md, "Linux in flash"), all the
/// way under a unit's boot ROM (RNSE_BOOT_ROM) with the caches modelled
/// and RAM arbitrary: the stock software takes the firmware disc for an
/// update and its loader writes linuxboot and the kernel; the ROM starts
/// linuxboot, which counts the start for the guard, checks the kernel in
/// the flash and starts it; the GUI comes up and, ten seconds in, counts
/// the start as one that worked.  The controls work, RETURN held says
/// there is no loader to go back to, the watchdog is served, and after a
/// power cycle the unit comes up in Linux again.
#[test]
fn linux_is_installed_in_flash() {
    let (Some(image), Some(rom)) = (disc(), boot_rom()) else { return };
    let Some(custom) = linux_firmware_disc(&image, false) else { return };
    let Some(mut m) = rom_192(&image, &custom, &rom) else { return };
    let ms = m.cycles_per_ms();
    let tail = |m: &Machine| { let t = String::from_utf8_lossy(&m.console).into_owned(); t[t.len().saturating_sub(600)..].to_string() };
    let press = |m: &mut Machine, key: &str, hold: u64| {
        assert!(m.panel_key(key, true));
        assert_eq!(m.run(hold * ms), Stop::Budget, "{:?}", m.cpu.fault);
        assert!(m.panel_key(key, false));
    };
    let jump = until(&mut m, 0, "*** Jump User RESET ***", 600).unwrap_or_else(|| panic!("the stock software did not install the disc: {:?}", tail(&m)));
    let started = until(&mut m, jump, "rnse sdk: linuxboot from flash", 60).unwrap_or_else(|| panic!("the ROM did not start linuxboot: {:?}", tail(&m)));
    m.watchdog.longest = 0;
    // The cache's record from here on: what the stock loader left dirty
    // when it jumped to the reset is lost on a unit too.
    let before = (m.cpu.cache.trouble.stale_fetches, m.cpu.cache.trouble.dirty_discarded);
    eprintln!("cache trouble when linuxboot starts: {:?}", m.cpu.cache.trouble);
    let up = until(&mut m, started, "rnse: Linux on the RNS-E 192 (flash start)", 60).unwrap_or_else(|| panic!("Linux did not come up from flash: {:?}", tail(&m)));
    until(&mut m, up, "rnse: this start counted as one that worked", 30).unwrap_or_else(|| panic!("the GUI did not call the start healthy: {:?}", tail(&m)));
    let text = String::from_utf8_lossy(&m.console).into_owned();
    assert!(text[started..].contains("rnse.flash=1"), "{:?}", tail(&m));
    assert!(text[started..].contains("return to the loader not available (started from flash)"), "{:?}", tail(&m));
    let (_, _, pixels) = m.render();
    assert!(pixels.iter().any(|&p| p != 0), "the screen is dark");
    // The controls: the disc out (the firmware disc is still in), the
    // display down and up; RETURN held leaves nothing.
    let ejects = m.atapi().ejects;
    press(&mut m, "CD_EJECT", 200);
    assert_eq!(m.run(4_000 * ms), Stop::Budget, "{:?}", m.cpu.fault);
    let now = m.atapi().ejects;
    assert_eq!(now, ejects + 1, "no eject: {:?}", tail(&m));
    press(&mut m, "EJECT", 200);
    assert_eq!(m.run(2_500 * ms), Stop::Budget, "{:?}", m.cpu.fault);
    assert!(m.board.display_pos >= 894, "the display is at {}", m.board.display_pos);
    press(&mut m, "RETURN", 2_500);
    assert_eq!(m.run(2_000 * ms), Stop::Budget, "{:?}", m.cpu.fault);
    assert!(String::from_utf8_lossy(&m.console).contains("return to the SD loader is not available here (started from flash)"));
    assert_eq!(m.resets, 0, "the watchdog reset the unit");
    let longest = m.watchdog.longest / ms;
    assert!(longest < 40, "the watchdog waited {} ms, until {} ms, PC {:08x}", longest, m.watchdog.longest_end / ms, m.watchdog.longest_pc);
    let erased = m.board.flash_ctl.erased_sectors;
    let cache = &m.cpu.cache.trouble;
    assert_eq!((cache.stale_fetches, cache.dirty_discarded), before, "{:?}", cache);
    // A power cycle: Linux again, nothing erased.
    m.cpu.reset();
    let again = until(&mut m, up, "rnse sdk: linuxboot from flash", 30).expect("not started again");
    until(&mut m, again, "rnse: Linux on the RNS-E 192 (flash start)", 60).expect("Linux did not come up again");
    assert_eq!(m.board.flash_ctl.erased_sectors, erased, "something was erased");
}

/// The guard (sdk/src/flash.c) is the way out of Linux in flash: with a
/// kernel that is not whole, linuxboot says so and keeps the controls
/// going but never counts a start as one that worked, and at the fifth
/// start in a row the guard takes the program out; the boot ROM installs
/// the stock disc that is in the drive.  Nothing else erases anything.
/// Each start's CD EJECT is tried while linuxboot waits; the stock disc
/// goes in before the power cycle that follows.
#[test]
fn linux_in_flash_with_a_spoilt_kernel_is_taken_out_by_the_guard() {
    let (Some(image), Some(rom)) = (disc(), boot_rom()) else { return };
    let Some(spoilt) = linux_firmware_disc(&image, true) else { return };
    let Some(mut m) = rom_192(&spoilt, &image, &rom) else { return };
    let ms = m.cycles_per_ms();
    let count = |m: &Machine| String::from_utf8_lossy(&m.console).matches("rnse sdk: linuxboot from flash").count();
    for n in 1..=4 {
        assert_eq!(m.run(4_000 * ms), Stop::Budget, "{:?}", m.cpu.fault);
        assert_eq!(count(&m), n, "start {}", n);
        let text = String::from_utf8_lossy(&m.console).into_owned();
        assert!(text.matches("NO KERNEL IN THE FLASH").count() == n, "start {}: {:?}", n, &text[text.len().saturating_sub(400)..]);
        assert!(!text.contains("Linux version"), "a spoilt kernel was started");
        assert_eq!(m.resets, 0, "the watchdog reset the unit while linuxboot waited");
        // The controls while it waits.
        let ejects = m.atapi().ejects;
        assert!(m.panel_key("CD_EJECT", true));
        assert_eq!(m.run(200 * ms), Stop::Budget);
        assert!(m.panel_key("CD_EJECT", false));
        assert_eq!(m.run(3_000 * ms), Stop::Budget);
        assert_eq!(m.atapi().ejects, ejects + 1, "start {}: CD EJECT did nothing", n);
        // Off and on with the stock disc put in: the drive comes up with
        // it, as at power-on.  (m.cpu.reset() resets the processor only;
        // a disc inserted while the unit ran would leave the emulated
        // drive with its "medium changed" still to tell, which a power
        // cut clears on a unit, and the boot ROM does not get past it.)
        m.atapi().load(image.clone());
        m.cpu.reset();
    }
    let tail = |m: &Machine| { let t = String::from_utf8_lossy(&m.console).into_owned(); t[t.len().saturating_sub(1500)..].to_string() };
    let back = until(&mut m, 0, "*** Jump User RESET ***", 600).unwrap_or_else(|| panic!("the ROM did not install the stock disc: {:?}", tail(&m)));
    assert_eq!(count(&m), 4, "linuxboot was started a fifth time");
    until(&mut m, back, "[fw] SOUND_TSK START", 60).expect("the stock software did not start");
    stock_is_in_flash(&m, &image);
}

/// Doom (docs/doom.md) from an SD card, as on a unit: the stock software
/// starts `sdboot`, which starts `DOOM.BIN` (sdk/examples/doomboot) from
/// the card, which reads the game, DOOMGAME.BIN, and the shareware WAD
/// and enters the game.  The controls while the card is read; the title
/// screen and the demo; sound effects out of a card processor's
/// converter; the menu with the faceplate's keys and a game begun; the
/// display and the disc while it plays; RETURN held back to `sdboot`.
/// A PC on the 192 (win311/, Tiny386): sdboot starts pcboot
/// (WIN311.BIN) from the card, which starts the emulator (PCEMU.BIN),
/// which boots MS-DOS and Windows 3.11 from the disk image on the same
/// card, and the knobs move the mouse.  With the caches modelled and RAM
/// arbitrary.  Needs RNSE_WIN311, a directory with what win311/build.sh
/// writes (win311/out) and a disk image WIN311.IMG with DOS and Windows
/// installed (docs/win311.md: not in the repository).  Screenshots go to
/// out/win311/.
#[test]
fn win311_runs_from_an_sd_card() {
    use rnse::board::Model;
    let Some(image) = disc() else { return };
    let Some(dir) = std::env::var_os("RNSE_WIN311") else { return };
    let dir = std::path::PathBuf::from(dir);
    let file = |name: &str| std::fs::read(dir.join(name)).unwrap_or_else(|_| panic!("RNSE_WIN311 has no {}", name));
    let Some((mut m, name, _)) = started_by_stock_software(&image, Model::Rnse192, "sdboot") else { return };
    let entries = vec![
        rnse::fat::Entry::file("WIN311.BIN", file("WIN311.BIN")),
        rnse::fat::Entry::file("PCEMU.BIN", file("PCEMU.BIN")),
        rnse::fat::Entry::file("BIOS.ROM", file("BIOS.ROM")),
        rnse::fat::Entry::file("VGABIOS.ROM", file("VGABIOS.ROM")),
        rnse::fat::Entry::file("WIN311.IMG", file("WIN311.IMG")),
    ];
    let card = rnse::fat::card("WIN311", &entries, None).unwrap();
    assert!(m.insert_card(0, rnse::dev::sdhi::Card::from_bytes(card).unwrap()));
    let ms = m.cycles_per_ms();
    m.watchdog.longest = 0;
    let console = |m: &Machine| String::from_utf8_lossy(&m.console).into_owned();
    let tail = |m: &Machine| { let t = console(m); t[t.len().saturating_sub(1500)..].to_string() };
    std::fs::create_dir_all("out/win311").unwrap();
    let shot = |m: &Machine, file: &str| {
        let (w, h, rgb) = m.render();
        std::fs::write(format!("out/win311/{file}.png"), rnse::png::encode_rgb(w, h, &rgb)).unwrap();
    };
    // Windows' desktop: the share of the screen in its grey (192, 192,
    // 192), and whether a title bar's dark blue (0, 0, 128) is there.
    let desktop = |m: &Machine| {
        let (_, _, rgb) = m.render();
        let px = rgb.len() / 3;
        let grey = rgb.chunks(3).filter(|p| p == &[192, 192, 192]).count();
        let blue = rgb.chunks(3).filter(|p| p == &[0, 0, 128]).count();
        (grey * 100 / px, blue)
    };

    until(&mut m, 0, "rnse sdk: pcemu", 60).unwrap_or_else(|| panic!("{}: the emulator did not start: {:?}", name, tail(&m)));
    until(&mut m, 0, "pcemu: WIN311.IMG", 30).unwrap_or_else(|| panic!("{}: no disk image: {:?}", name, tail(&m)));
    // Where the SH-4 spends its time, sampled (addresses to
    // out/win311/profile.txt, for nm).
    // RNSE_WIN311_PROFILE=S: after S seconds (Windows starting at about
    // 60), a second of samples.
    if let Some(skip) = std::env::var("RNSE_WIN311_PROFILE").ok().and_then(|s| s.parse::<u64>().ok()) {
        assert_eq!(m.run(skip * 1_000 * ms), Stop::Budget, "{:?}", m.cpu.fault);
        shot(&m, "profiled");
        let mut hist = std::collections::HashMap::<u32, u32>::new();
        for _ in 0..120_000 {
            assert_eq!(m.run(2_000 + (hist.len() as u64 % 7)), Stop::Budget, "{:?}", m.cpu.fault);
            *hist.entry(m.cpu.pc).or_default() += 1;
        }
        let mut v: Vec<_> = hist.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        let text: String = v.iter().map(|(pc, n)| format!("{:08x} {}\n", pc, n)).collect();
        std::fs::write("out/win311/profile.txt", text).unwrap();
        return;
    }
    let started = std::time::Instant::now();
    let mut seconds = 0;
    let mut up = None;
    while seconds < 240 {
        assert_eq!(m.run(5_000 * ms), Stop::Budget, "{}: {:?} {}", name, m.cpu.fault, tail(&m));
        seconds += 5;
        shot(&m, &format!("boot-{:03}", seconds));
        std::fs::write("out/win311/console.txt", &m.console).unwrap();
        let (grey, blue) = desktop(&m);
        eprintln!("win311: {} s emulated ({:.0} s here): grey {}%, blue {}", seconds, started.elapsed().as_secs_f64(), grey, blue);
        if grey > 15 && blue > 100 {
            up = Some(seconds);
            break;
        }
    }
    let up = up.unwrap_or_else(|| panic!("{}: no Windows desktop after {} s: {:?}", name, seconds, tail(&m)));
    eprintln!("win311: Program Manager after {} s of the unit's time", up);
    let longest = m.watchdog.longest / ms;
    assert!(longest < 40, "{}: the watchdog waited {} ms, until {} ms, PC {:08x}", name, longest, m.watchdog.longest_end / ms, m.watchdog.longest_pc);

    // Until the picture stands still (Program Manager's icons drawn).
    let mut last = m.render().2;
    for _ in 0..30 {
        assert_eq!(m.run(1_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
        let now = m.render().2;
        if now == last { break }
        last = now;
    }
    shot(&m, "desktop");
    eprintln!("win311: {}", tail(&m).lines().filter(|l| l.starts_with("pcemu:")).last().unwrap_or(""));

    // The pointer: what changes between two pictures, as a box.
    let w = m.render().0;
    let changed = |a: &[u8], b: &[u8]| {
        let mut bx = (usize::MAX, usize::MAX, 0, 0);
        for (i, (p, q)) in a.chunks(3).zip(b.chunks(3)).enumerate() {
            if p != q {
                let (x, y) = (i % w, i / w);
                bx = (bx.0.min(x), bx.1.min(y), bx.2.max(x), bx.3.max(y));
            }
        }
        bx
    };
    let turn = |m: &mut Machine, knob: &str, n: i32| -> Vec<u8> {
        for _ in 0..n.abs() {
            assert!(m.panel_knob(knob, n.signum()));
            assert_eq!(m.run(60 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
        }
        assert_eq!(m.run(1_500 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
        m.render().2
    };
    // Slowly, a detent at a time: the volume knob louder moves it right,
    // the select knob clockwise down, a few pixels a detent.
    let right = turn(&mut m, "volume", 10);
    shot(&m, "pointer-right");
    let (x0, y0, x1, y1) = changed(&last, &right);
    eprintln!("win311: right: changed ({}, {})..({}, {})", x0, y0, x1, y1);
    assert!(x0 != usize::MAX, "{}: the volume knob did not move the pointer", name);
    assert!(x1 - x0 >= 8 && x1 - x0 <= 40 && y1 - y0 <= 24, "{}: moved ({}, {})..({}, {}) for 10 detents across", name, x0, y0, x1, y1);
    let down = turn(&mut m, "main", 10);
    shot(&m, "pointer-down");
    let (x0, y0, x1, y1) = changed(&right, &down);
    eprintln!("win311: down: changed ({}, {})..({}, {})", x0, y0, x1, y1);
    assert!(x0 != usize::MAX, "{}: the select knob did not move the pointer", name);
    assert!(y1 - y0 >= 6 && y1 - y0 <= 40 && x1 - x0 <= 24, "{}: moved ({}, {})..({}, {}) for 10 detents down", name, x0, y0, x1, y1);
}

/// With the caches modelled and RAM arbitrary.  Needs RNSE_DOOM, the
/// directory doom/build.sh writes (doom/out: DOOMGAME.BIN, DOOM1.WAD).
#[test]
fn doom_runs_from_an_sd_card() {
    use rnse::board::Model;
    let Some(image) = disc() else { return };
    let Some(dir) = std::env::var_os("RNSE_DOOM") else { return };
    let dir = std::path::PathBuf::from(dir);
    let game = std::fs::read(dir.join("DOOMGAME.BIN")).expect("RNSE_DOOM has no DOOMGAME.BIN");
    let wad = std::fs::read(dir.join("DOOM1.WAD")).expect("RNSE_DOOM has no DOOM1.WAD");
    let sdk = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("sdk");
    let Some((mut m, name, _)) = started_by_stock_software(&image, Model::Rnse192, "sdboot") else { return };
    if sdk_example(&image, Model::Rnse192, "doomboot").is_none() { return }
    let doomboot = std::fs::read(sdk.join("out/doomboot-192.bin")).unwrap();
    let entries = vec![
        rnse::fat::Entry::file("DOOM.BIN", doomboot),
        rnse::fat::Entry::file("DOOMGAME.BIN", game),
        rnse::fat::Entry::file("DOOM1.WAD", wad),
    ];
    let card = rnse::fat::card("DOOM", &entries, None).unwrap();
    assert!(m.insert_card(0, rnse::dev::sdhi::Card::from_bytes(card).unwrap()));
    let ms = m.cycles_per_ms();
    m.watchdog.longest = 0;
    let console = |m: &Machine| String::from_utf8_lossy(&m.console).into_owned();
    let tail = |m: &Machine| { let t = console(m); t[t.len().saturating_sub(800)..].to_string() };
    let press = |m: &mut Machine, key: &str, hold: u64| {
        assert!(m.panel_key(key, true));
        assert_eq!(m.run(hold * ms), Stop::Budget, "{:?}", m.cpu.fault);
        assert!(m.panel_key(key, false));
    };
    // Doom's 320x200, centred on the 400x235 screen: how many colours,
    // and the pixels themselves.
    let window = |m: &Machine| {
        let (w, h, pixels) = m.render();
        let bytes = pixels.len() / (w * h);
        let mut out = Vec::new();
        for y in 17..217 {
            out.extend_from_slice(&pixels[(y * w + 40) * bytes..(y * w + 360) * bytes]);
        }
        let colours: std::collections::HashSet<&[u8]> = out.chunks(bytes).collect();
        (colours.len(), out.clone())
    };

    // While the card is read: the display folds down and up.
    until(&mut m, 0, "READING DOOM1.WAD", 60).unwrap_or_else(|| panic!("{}: doomboot did not read the card: {:?}", name, tail(&m)));
    press(&mut m, "EJECT", 200);
    assert_eq!(m.run(2_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    assert!(m.board.display_pos <= 94, "{}: the display is at {} while the card is read", name, m.board.display_pos);
    press(&mut m, "EJECT", 200);
    assert_eq!(m.run(2_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    assert!(m.board.display_pos >= 894, "{}: the display is at {}", name, m.board.display_pos);

    // The game: its start, then the title and the demo, which plays
    // (the picture changes) and is heard.
    until(&mut m, 0, "ST_Init: Init status bar.", 60).unwrap_or_else(|| panic!("{}: Doom did not start: {:?}", name, tail(&m)));
    let text = console(&m);
    assert!(text.lines().any(|l| l.trim_end() == "rnse sdk: doom") && text.contains("adding DOOM1.WAD") && text.contains("DOOM Shareware"), "{}: {:?}", name, tail(&m));
    assert!(text.contains("sound: card processor 0"), "{}: no sound: {:?}", name, tail(&m));
    assert_eq!(m.run(12_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    let (colours, first) = window(&m);
    assert!(colours > 40, "{}: {} colours in Doom's window", name, colours);
    assert_eq!(m.run(1_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    let (_, second) = window(&m);
    assert_ne!(first, second, "{}: the demo does not move", name);
    let dsp = &m.board.media.dsps[0];
    assert!(dsp.fault.is_none(), "{}: {:?}", name, dsp.fault);
    assert!(dsp.loud_frames > 0, "{}: the card processor sends its converter only silence ({} samples)", name, dsp.frames);

    // The menu (SETUP), New Game, the episode, the skill: E1M1.  Then
    // forward, a turn, the door key.
    let before = console(&m).len();
    press(&mut m, "SETUP", 150);
    assert_eq!(m.run(800 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    for _ in 0..3 {
        press(&mut m, "SELECT", 150);
        assert_eq!(m.run(800 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    }
    until(&mut m, before, "doom: playing E1M1", 10).unwrap_or_else(|| panic!("{}: no game began: {:?}", name, tail(&m)));
    let (_, standing) = window(&m);
    press(&mut m, "NEXT", 1_000);
    assert!(m.panel_knob("main", 4));
    assert_eq!(m.run(500 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    let (_, moved) = window(&m);
    assert_ne!(standing, moved, "{}: the view did not change with NEXT and the knob", name);

    // While it plays: the display folds down, the disc comes out, the
    // display folds up; a short RETURN is Doom's (the menu), not the way
    // back.
    press(&mut m, "EJECT", 200);
    assert_eq!(m.run(2_500 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    assert!(m.board.display_pos <= 94, "{}: the display is at {} in the game", name, m.board.display_pos);
    let ejects = m.atapi().ejects;
    press(&mut m, "CD_EJECT", 200);
    assert_eq!(m.run(2_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    assert_eq!(m.atapi().ejects, ejects + 1, "{}: no eject in the game", name);
    press(&mut m, "EJECT", 200);
    assert_eq!(m.run(2_500 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    assert!(m.board.display_pos >= 894, "{}: the display is at {}", name, m.board.display_pos);
    press(&mut m, "RETURN", 300);
    assert_eq!(m.run(1_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    assert!(!console(&m)[before..].contains("rnse sdk: sdboot"), "{}: a short RETURN left the game", name);

    // RETURN held: back to sdboot, with the status of a RETURN held.
    let before = console(&m).len();
    press(&mut m, "RETURN", 2_500);
    assert_eq!(m.run(3_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    let text = console(&m);
    assert!(text[before..].contains("rnse sdk: sdboot") && text[before..].contains("TEST ENDED BY HOLDING RETURN"), "{}: {:?}", name, &text[before..]);
    assert!(!text[before..].lines().any(|l| l.trim_end() == "rnse sdk: doom"), "{}: Doom was started again", name);

    // The watchdog was served throughout (it resets the unit 105 ms after
    // the last edge), and the caches never handed out anything stale.
    let longest = m.watchdog.longest / ms;
    assert!(longest < 40, "{}: the watchdog waited {} ms, until {} ms, PC {:08x}", name, longest, m.watchdog.longest_end / ms, m.watchdog.longest_pc);
    let cache = &m.cpu.cache.trouble;
    assert_eq!((cache.stale_fetches, cache.dirty_discarded), (0, 0), "{}: {:?}", name, cache);
}

/// umac, a Mac Plus emulator, on the 192 from an SD card (docs/umac.md):
/// the stock software starts sdboot, sdboot's only program is macboot
/// (MAC.BIN), which reads umac (MACEMU.BIN), the Mac Plus ROM and a disk
/// image and starts umac.  Needs RNSE_UMAC=umac/out (umac/build.sh) and,
/// since neither may be in the repository, RNSE_MAC_ROM (the Mac Plus's
/// third ROM, 128 KiB) and RNSE_MAC_DISK (a raw image of a disk with a
/// System that boots).  Screenshots go to out/umac/.
#[test]
fn mac_runs_from_an_sd_card() {
    use rnse::board::Model;
    let Some(image) = disc() else { return };
    let (Some(dir), Some(rom), Some(disk)) =
        (std::env::var_os("RNSE_UMAC"), std::env::var_os("RNSE_MAC_ROM"), std::env::var_os("RNSE_MAC_DISK"))
    else {
        return;
    };
    let umac = std::fs::read(std::path::Path::new(&dir).join("MACEMU.BIN")).expect("RNSE_UMAC has no MACEMU.BIN");
    let rom = std::fs::read(rom).expect("RNSE_MAC_ROM is not readable");
    let disk = std::fs::read(disk).expect("RNSE_MAC_DISK is not readable");
    let sdk = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("sdk");
    let Some((mut m, name, _)) = started_by_stock_software(&image, Model::Rnse192, "sdboot") else { return };
    if sdk_example(&image, Model::Rnse192, "macboot").is_none() { return }
    let macboot = std::fs::read(sdk.join("out/macboot-192.bin")).unwrap();
    let entries = vec![
        rnse::fat::Entry::file("MAC.BIN", macboot),
        rnse::fat::Entry::file("MACEMU.BIN", umac),
        rnse::fat::Entry::file("MACPLUS.ROM", rom),
        rnse::fat::Entry::file("MAC.DSK", disk),
    ];
    let card = rnse::fat::card("MAC", &entries, None).unwrap();
    assert!(m.insert_card(0, rnse::dev::sdhi::Card::from_bytes(card).unwrap()));
    let ms = m.cycles_per_ms();
    m.watchdog.longest = 0;
    let console = |m: &Machine| String::from_utf8_lossy(&m.console).into_owned();
    let tail = |m: &Machine| { let t = console(m); t[t.len().saturating_sub(800)..].to_string() };
    let press = |m: &mut Machine, key: &str, hold: u64| {
        assert!(m.panel_key(key, true));
        assert_eq!(m.run(hold * ms), Stop::Budget, "{:?}", m.cpu.fault);
        assert!(m.panel_key(key, false));
    };
    // The Mac's 384x235 at x = 8 of the 400x235 screen, black (true) or
    // white, one bool a pixel, saved as a PNG on the way.
    let shot = |m: &Machine, file: &str| -> Vec<Vec<bool>> {
        let (w, h, rgb) = m.render();
        std::fs::create_dir_all("out/umac").unwrap();
        std::fs::write(format!("out/umac/{file}.png"), rnse::png::encode_rgb(w, h, &rgb)).unwrap();
        let bytes = rgb.len() / (w * h);
        (0..h).map(|y| (8..392).map(|x| rgb[(y * w + x) * bytes] < 128).collect()).collect()
    };

    until(&mut m, 0, "READING MAC.DSK", 60).unwrap_or_else(|| panic!("{}: macboot did not read the card: {:?}", name, tail(&m)));
    until(&mut m, 0, "umac: 384x235", 60).unwrap_or_else(|| panic!("{}: umac did not start: {:?}", name, tail(&m)));
    let text = console(&m);
    assert!(text.lines().any(|l| l.trim_end() == "rnse sdk: umac"), "{}: {:?}", name, tail(&m));

    // The Mac boots from the disk into the Finder: its menu bar is a
    // white band across the top over a black line (the pointer may
    // cross it), with the desktop's pattern below.
    let mut finder = None;
    for second in (5..=90).step_by(5) {
        assert_eq!(m.run(5_000 * ms), Stop::Budget, "{}: {:?} {:?}", name, m.cpu.fault, tail(&m));
        let pixels = shot(&m, &format!("boot-{second:02}s"));
        let line = (15..24).find(|&y| pixels[y].iter().filter(|&&b| b).count() >= 370);
        if let Some(y) = line {
            if pixels[y + 4].iter().any(|&b| b) && pixels[y + 4].iter().any(|&b| !b) {
                finder = Some((second, y));
                break;
            }
        }
    }
    let (second, menu_line) = finder.unwrap_or_else(|| panic!("{}: no Finder after 90 s: {:?}", name, tail(&m)));
    eprintln!("{}: the Finder after about {} s, the menu bar's line at y = {}", name, second, menu_line);
    assert!(!console(&m).contains("Attempted to read"), "{}: {:?}", name, tail(&m));

    // The knobs move the pointer: the picture changes where it was and
    // where it goes, and nothing else does.
    assert_eq!(m.run(2_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    let before = shot(&m, "pointer-before");
    assert!(m.panel_knob("main", 6));
    assert!(m.panel_knob("volume", 6));
    assert_eq!(m.run(1_500 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    let after = shot(&m, "pointer-after");
    let changed: Vec<(usize, usize)> = (0..235).flat_map(|y| (0..384).map(move |x| (x, y)))
        .filter(|&(x, y)| before[y][x] != after[y][x]).collect();
    assert!(!changed.is_empty(), "{}: the knobs did not move the pointer", name);
    let (x0, x1) = (changed.iter().map(|c| c.0).min().unwrap(), changed.iter().map(|c| c.0).max().unwrap());
    let (y0, y1) = (changed.iter().map(|c| c.1).min().unwrap(), changed.iter().map(|c| c.1).max().unwrap());
    assert!(x1 - x0 < 120 && y1 - y0 < 120, "{}: more than the pointer changed: {}..{} x {}..{}", name, x0, x1, y0, y1);

    // The display folds down and up while the Mac runs.
    press(&mut m, "EJECT", 200);
    assert_eq!(m.run(2_500 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    assert!(m.board.display_pos <= 94, "{}: the display is at {} under the Mac", name, m.board.display_pos);
    press(&mut m, "EJECT", 200);
    assert_eq!(m.run(2_500 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    assert!(m.board.display_pos >= 894, "{}: the display is at {}", name, m.board.display_pos);

    // RETURN held: back to sdboot, with the status of a RETURN held.
    let before = console(&m).len();
    press(&mut m, "RETURN", 2_500);
    assert_eq!(m.run(3_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    let text = console(&m);
    assert!(text[before..].contains("rnse sdk: sdboot") && text[before..].contains("TEST ENDED BY HOLDING RETURN"), "{}: {:?}", name, &text[before..]);

    // The watchdog was served throughout, and the caches never handed
    // out anything stale.
    let longest = m.watchdog.longest / ms;
    assert!(longest < 40, "{}: the watchdog waited {} ms, until {} ms, PC {:08x}", name, longest, m.watchdog.longest_end / ms, m.watchdog.longest_pc);
    let cache = &m.cpu.cache.trouble;
    assert_eq!((cache.stale_fetches, cache.dirty_discarded), (0, 0), "{}: {:?}", name, cache);
}

/// The slot benchmark (`sdk/examples/linkbench`) as it was run on a unit
/// on 2026-11-16: the SD loader started by the stock software, a card
/// with LINKTEST.BIN in the first slot (there the RP2040 bridge) and the
/// benchmark as LINKBENC.BIN in the second.  The stock way runs first,
/// then the benchmark's own card processor program takes over from the
/// stock one with its pins (which tests/linkbench.rs, with no stock
/// software, never does).
#[test]
fn link_benchmark_from_the_sd_loader() {
    use rnse::board::Model;
    let Some(image) = disc() else { return };
    let sdk = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("sdk");
    let Some((mut m, name, _)) = started_by_stock_software(&image, Model::Rnse192, "sdboot") else { return };
    if sdk_example(&image, Model::Rnse192, "linkbench").is_none() { return }
    let bench = std::fs::read(sdk.join("out/linkbench-192.bin")).unwrap();
    let ms = m.cycles_per_ms();
    m.cpu.breakpoints.clear();
    assert_eq!(m.run(2000 * ms), Stop::Budget);
    // LINKTEST.BIN as tools/linktest.py writes it.
    let file: Vec<u8> = (0..(2u32 << 20) / 4).flat_map(|j| {
        let mut x = j.wrapping_add(1).wrapping_mul(0x9e37_79b1);
        x ^= x >> 16;
        x = x.wrapping_mul(0x85eb_ca6b);
        x ^= x >> 13;
        x.to_be_bytes()
    }).collect();
    let link = rnse::fat::card("LINK", &[rnse::fat::Entry::file("LINKTEST.BIN", file)], None).unwrap();
    // An old card, as the 32 MB one tried on a unit: no answer to CMD8,
    // busy for a few rounds of ACMD41.
    m.board.media.old_cards = true;
    assert!(m.insert_card(0, rnse::dev::sdhi::Card::from_bytes(link).unwrap()));
    m.board.media.old_cards = false;
    assert_eq!(m.run(2000 * ms), Stop::Budget);
    let card = rnse::fat::card("RNSE", &[rnse::fat::Entry::file("LINKBENC.BIN", bench.clone())], None).unwrap();
    let at = m.console.len();
    // The only program on the card: the loader starts it by itself.
    assert!(m.insert_card(1, rnse::dev::sdhi::Card::from_bytes(card).unwrap()));
    let started = until(&mut m, at, "rnse sdk: linkbench", 30).unwrap_or_else(|| panic!("{}: {}", name, String::from_utf8_lossy(&m.console[at..])));
    until(&mut m, started, "DONE.", 400).unwrap_or_else(|| panic!("{}: {}", name, String::from_utf8_lossy(&m.console[started..])));
    let text = String::from_utf8_lossy(&m.console[started..]).into_owned();
    println!("{}", text);
    assert!(text.contains("SLOT 1, STOCK PROGRAM"), "{}: {}", name, text);
    assert!(text.contains("CARD STARTED") && !text.contains("OWN PROGRAM: STEP"), "{}: {}", name, text);
    let summary = &text[text.find("LINKBENCH 1: KB/S").unwrap()..];
    // (Not the comparison for a unit with commands started the old way,
    // "OLD", which on a unit and here gets no response interrupt.)
    for line in summary.lines().filter(|l| l.contains(" KB/S ") && !l.contains(" OLD ")) {
        assert!(line.contains(" B0 T0 L0") && !line.contains("FAIL"), "{}: {}", name, line);
    }
    assert!(!summary.contains("NOT RUN"), "{}: {}", name, summary);
    // The card in the other slot has the program: the results go through
    // it (as through the RP2040 bridge, whose log has them), and the
    // program waits for a new one there and runs it (a bridge gets it
    // over USB from sdbridge/link.py; here the card is changed).
    assert!(text.contains("SLOT 2: LINKBENC.BIN ") && text.contains("RESULTS THROUGH THE OTHER CARD"), "{}: {}", name, text);
    until(&mut m, started, "WAITING FOR A NEW LINKBENC.BIN IN SLOT 2", 60).unwrap_or_else(|| panic!("{}: {}", name, String::from_utf8_lossy(&m.console[started..])));
    assert_eq!(m.run(5000 * ms), Stop::Budget);
    assert!(!String::from_utf8_lossy(&m.console[started..]).contains("A NEW ONE"));
    let mut newer = bench;
    newer.extend_from_slice(&[0; 4]);
    let card = rnse::fat::card("RNSE", &[rnse::fat::Entry::file("LINKBENC.BIN", newer)], None).unwrap();
    m.eject_card(1);
    assert!(m.insert_card(1, rnse::dev::sdhi::Card::from_bytes(card).unwrap()));
    let again = until(&mut m, started, "A NEW ONE: STARTING IT", 60).unwrap_or_else(|| panic!("{}: {}", name, String::from_utf8_lossy(&m.console[started..])));
    let again = until(&mut m, again, "rnse sdk: linkbench", 30).unwrap_or_else(|| panic!("{}: {}", name, String::from_utf8_lossy(&m.console[again..])));
    until(&mut m, again, "RESULTS THERE", 60).unwrap_or_else(|| panic!("{}: {}", name, String::from_utf8_lossy(&m.console[again..])));
}

/// The phone's video on the 192 as a unit runs it (sdbridge/,
/// SDLINK.UF2; sdk/examples/aaview and aaviewer): the SD loader started by
/// the stock software, the bridge in the second slot (its card made up by
/// the bridge's own code, sdbridge/tests/host.c, with tests/data/margin.264
/// laid into AALINK.BIN as a phone's messages), a card in the first with
/// AAVIEW.BIN and AAVIEWER.IMG.  The loader starts AAVIEW.BIN, that loads
/// the viewer and enters it, the viewer finds the bridge through the
/// stock card program, takes the slot over with its own, and decodes
/// every picture, each frame's CRC-32 as ffmpeg's (the viewer built with
/// -DAA_CRC).  RETURN held goes back to the loader.  Needs a host C
/// compiler as well (tests/aaviewer.rs has the viewer with no firmware).
#[test]
fn android_auto_viewer_from_the_sd_loader() {
    use rnse::board::Model;
    let Some(image) = disc() else { return };
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let sdk = root.join("sdk");
    let Some((mut m, name, _)) = started_by_stock_software(&image, Model::Rnse192, "sdboot") else { return };
    if sdk_example(&image, Model::Rnse192, "aaview").is_none() { return }
    let first = std::fs::read(sdk.join("out/aaview-192.bin")).unwrap();
    let built = std::process::Command::new("sh").arg(sdk.join("build.sh")).args(["192", "aaviewer"])
        .env("EXTRA_CFLAGS", "-DAA_CRC").output().unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let viewer = std::fs::read(sdk.join("out/aaviewer-192.bin")).unwrap();
    let _ = std::process::Command::new("sh").arg(sdk.join("build.sh")).args(["192", "aaviewer"]).output();
    // The bridge's card, from its host test program.
    let dir = std::env::temp_dir().join(format!("rnse-aaview-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let tool = dir.join("host");
    let src = root.join("sdbridge/src");
    let Ok(cc) = std::process::Command::new("cc").args(["-O2", "-o"]).arg(&tool).arg(root.join("sdbridge/tests/host.c"))
        .arg(src.join("card.c")).arg(src.join("volume.c")).arg(src.join("crc.c")).arg(src.join("aap.c"))
        .arg(src.join("aastream.c")).output() else { return };
    assert!(cc.status.success(), "{}", String::from_utf8_lossy(&cc.stderr));
    let bridge_image = dir.join("bridge.img");
    assert!(std::process::Command::new(&tool).arg(&bridge_image).arg(root.join("tests/data/margin.264")).status().unwrap().success());
    let bridge = std::fs::read(&bridge_image).unwrap();
    std::fs::remove_dir_all(&dir).ok();
    let want: Vec<String> = std::fs::read_to_string(root.join("tests/data/margin.crc")).unwrap()
        .lines().map(|l| l.trim().to_ascii_lowercase()).filter(|l| !l.is_empty()).collect();

    let ms = m.cycles_per_ms();
    m.cpu.breakpoints.clear();
    assert_eq!(m.run(2000 * ms), Stop::Budget);
    assert!(m.insert_card(1, rnse::dev::sdhi::Card::from_bytes(bridge).unwrap()));
    assert_eq!(m.run(2000 * ms), Stop::Budget);
    let card = rnse::fat::card("RNSE", &[rnse::fat::Entry::file("AAVIEW.BIN", first), rnse::fat::Entry::file("AAVIEWER.IMG", viewer)], None).unwrap();
    let at = m.console.len();
    // The only program on the card: the loader starts it by itself.
    assert!(m.insert_card(0, rnse::dev::sdhi::Card::from_bytes(card).unwrap()));
    let started = until(&mut m, at, "rnse sdk: aaviewer", 60).unwrap_or_else(|| panic!("{}: {}", name, String::from_utf8_lossy(&m.console[at..])));
    let crcs = |m: &Machine| -> Vec<String> {
        String::from_utf8_lossy(&m.console[started..]).lines().filter_map(|l| l.strip_prefix("aaviewer: frame "))
            .filter_map(|l| l.split(" crc ").nth(1)).map(|c| c.trim().to_ascii_lowercase()).collect()
    };
    for _ in 0..1200 {
        if crcs(&m).len() >= want.len() {
            break;
        }
        assert_eq!(m.run(100 * ms), Stop::Budget, "{}: {:?}: {}", name, m.cpu.fault, String::from_utf8_lossy(&m.console[started..]));
    }
    let text = String::from_utf8_lossy(&m.console[started..]).into_owned();
    println!("{}", text);
    assert!(text.contains("BRIDGE IN SLOT 2"), "{}: {}", name, text);
    let got = crcs(&m);
    assert_eq!(got.len(), want.len(), "{}: {}", name, text);
    for (i, (g, w)) in got.iter().zip(want.iter()).enumerate() {
        assert_eq!(g.trim_start_matches('0'), w.trim_start_matches('0'), "{}: frame {}: {}", name, i, text);
    }
    // RETURN held: back to the loader, through aaview.
    let before = m.console.len();
    assert!(m.panel_key("RETURN", true));
    assert_eq!(m.run(2500 * ms), Stop::Budget);
    assert!(m.panel_key("RETURN", false));
    until(&mut m, before, "TEST ENDED BY HOLDING RETURN", 30).unwrap_or_else(|| panic!("{}: {}", name, String::from_utf8_lossy(&m.console[before..])));
}

/// The DVD player (`sdk/examples/dvd`) as a unit starts it: the stock
/// software starts the SD loader from the disc, the loader the player
/// from the card (DVD.BIN), CD EJECT takes the loader's disc out (the
/// player says it is no DVD-Video disc), a DVD goes in, SELECT plays
/// its first title with the caches modelled, and RETURN held goes back
/// to the loader.  Needs `RNSE_DVD` as well (`sdk/tests/check-dvd.sh`
/// makes one, and `tests/dvd.rs` has the player's own tests, which need
/// no firmware).
#[test]
fn dvd_player_from_the_sd_loader() {
    use rnse::board::Model;
    let Some(image) = disc() else { return };
    let Some(dvd) = std::env::var_os("RNSE_DVD") else { return };
    let dvd = std::fs::read(dvd).expect("RNSE_DVD is not readable");
    let sdk = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("sdk");
    let Some((mut m, name, _)) = started_by_stock_software(&image, Model::Rnse192, "sdboot") else { return };
    if sdk_example(&image, Model::Rnse192, "dvd").is_none() { return }
    let player = std::fs::read(sdk.join("out/dvd-192.bin")).unwrap();
    let flash = m.board.flash.clone();
    let ms = m.cycles_per_ms();
    m.cpu.breakpoints.clear();
    assert_eq!(m.run(2000 * ms), Stop::Budget);
    let card = rnse::fat::card("DVD", &[rnse::fat::Entry::file("DVD.BIN", player)], None).unwrap();
    let at = m.console.len();
    assert!(m.insert_card(0, rnse::dev::sdhi::Card::from_bytes(card).unwrap()));
    let started = until(&mut m, at, "rnse sdk: dvd", 30).unwrap_or_else(|| panic!("{}: {}", name, String::from_utf8_lossy(&m.console[at..])));
    until(&mut m, started, "NOT A DVD-VIDEO DISC", 30).unwrap_or_else(|| panic!("{}: {}", name, String::from_utf8_lossy(&m.console[started..])));
    // The loader's disc out, and the DVD in.
    assert!(m.panel_key("CD_EJECT", true));
    assert_eq!(m.run(300 * ms), Stop::Budget);
    assert!(m.panel_key("CD_EJECT", false));
    assert_eq!(m.run(8000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    assert!(m.atapi().image.is_none(), "{}: the loader's disc was not ejected", name);
    let at = m.console.len();
    m.atapi().insert(Some(dvd));
    until(&mut m, at, "READING THE DISC", 40).unwrap_or_else(|| panic!("{}: {}", name, String::from_utf8_lossy(&m.console[at..])));
    assert_eq!(m.run(3000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    assert!(m.panel_key("SELECT", true));
    assert_eq!(m.run(200 * ms), Stop::Budget);
    assert!(m.panel_key("SELECT", false));
    assert_eq!(m.run(5000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    let (_, _, rgb) = m.render();
    let colour = rgb.chunks(3).filter(|p| {
        let (r, g, b) = (p[0] as i32, p[1] as i32, p[2] as i32);
        (r - g).abs() > 40 || (g - b).abs() > 40 || (r - b).abs() > 40
    }).count();
    let text = String::from_utf8_lossy(&m.console[at..]).into_owned();
    assert!(colour > 20_000, "{}: {} colourful pixels: {}", name, colour, text);
    assert!(text.contains("fps*10"), "{}: {}", name, text);
    // RETURN held: the player stops its sound and puts the caches back,
    // and the loader is there again.
    let at = m.console.len();
    assert!(m.panel_key("RETURN", true));
    assert_eq!(m.run(2600 * ms), Stop::Budget);
    assert!(m.panel_key("RETURN", false));
    until(&mut m, at, "TEST ENDED BY HOLDING RETURN", 30).unwrap_or_else(|| panic!("{}: {}", name, String::from_utf8_lossy(&m.console[at..])));
    let text = String::from_utf8_lossy(&m.console[started..]).into_owned();
    assert!(!text.contains("unexpected"), "{}: {}", name, text);
    assert_eq!(m.board.flash, flash, "{}", name);
    let cache = &m.cpu.cache.trouble;
    assert_eq!((cache.stale_fetches, cache.dirty_discarded), (0, 0), "{}: {:?}", name, cache);
}

/// Quake (quake/build.sh, docs/quake.md) from an SD card: sdboot starts
/// quakeboot (QUAKE.BIN), which reads the game (QUAKEGAM.BIN) and enters
/// it; the game reads PAK0.PAK from the card as it needs it and plays
/// its sound out of the other card processor.  Needs RNSE_QUAKE=quake/out
/// besides RNSE_DISC and a cross compiler.  RNSE_QUAKE_SHOTS=DIR keeps
/// screenshots of the way; RNSE_QUAKE_ARG=TEXT puts a QUAKE.ARG on the
/// card (with `+timedemo demo1` the test prints the frame rate and ends
/// there).
#[test]
fn quake_runs_from_an_sd_card() {
    use rnse::board::Model;
    let Some(image) = disc() else { return };
    let Some(dir) = std::env::var_os("RNSE_QUAKE") else { return };
    let dir = std::path::PathBuf::from(dir);
    let game = std::fs::read(dir.join("QUAKEGAM.BIN")).expect("RNSE_QUAKE has no QUAKEGAM.BIN");
    let pak = std::fs::read(dir.join("PAK0.PAK")).expect("RNSE_QUAKE has no PAK0.PAK");
    let shots = std::env::var_os("RNSE_QUAKE_SHOTS").map(std::path::PathBuf::from);
    let sdk = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("sdk");
    let Some((mut m, name, _)) = started_by_stock_software(&image, Model::Rnse192, "sdboot") else { return };
    if sdk_example(&image, Model::Rnse192, "quakeboot").is_none() { return }
    let quakeboot = std::fs::read(sdk.join("out/quakeboot-192.bin")).unwrap();
    let mut entries = vec![
        rnse::fat::Entry::file("QUAKE.BIN", quakeboot),
        rnse::fat::Entry::file("QUAKEGAM.BIN", game),
        rnse::fat::Entry::file("PAK0.PAK", pak),
    ];
    // RNSE_QUAKE_ARG: QUAKE.ARG for a run by hand (`+timedemo demo1`).
    let arg = std::env::var("RNSE_QUAKE_ARG").ok();
    if let Some(arg) = &arg {
        entries.push(rnse::fat::Entry::file("QUAKE.ARG", arg.as_bytes().to_vec()));
    }
    let card = rnse::fat::card("QUAKE", &entries, None).unwrap();
    assert!(m.insert_card(0, rnse::dev::sdhi::Card::from_bytes(card).unwrap()));
    let ms = m.cycles_per_ms();
    m.watchdog.longest = 0;
    let console = |m: &Machine| String::from_utf8_lossy(&m.console).into_owned();
    let tail = |m: &Machine| { let t = console(m); t[t.len().saturating_sub(1500)..].to_string() };
    let shot = |m: &Machine, what: &str| {
        if let Some(dir) = &shots {
            let (w, h, rgb) = m.render();
            std::fs::write(dir.join(format!("{}.png", what)), rnse::png::encode_rgb(w, h, &rgb)).unwrap();
        }
    };

    // Until the console says `what`, a second at a time, the console's
    // end shown when the unit stops.
    let wait_for = |m: &mut Machine, from: usize, what: &str, seconds: u64| -> Option<usize> {
        for _ in 0..seconds {
            let stop = m.run(1_000 * ms);
            let text = String::from_utf8_lossy(&m.console).into_owned();
            assert_eq!(stop, Stop::Budget, "{:?} at {} ms: {}", m.cpu.fault, m.cpu.cycles / ms, &text[text.len().saturating_sub(1500)..]);
            if let Some(at) = text.get(from..).and_then(|t| t.find(what)) {
                return Some(from + at);
            }
        }
        None
    };

    wait_for(&mut m, 0, "quake: PAK0.PAK", 60).unwrap_or_else(|| panic!("{}: the game did not start: {:?}", name, tail(&m)));
    wait_for(&mut m, 0, "Quake Initialized", 120).unwrap_or_else(|| { shot(&m, "fail"); panic!("{}: Quake did not come up: {:?}", name, tail(&m)) });
    if arg.as_deref().is_some_and(|a| a.contains("timedemo")) {
        let at = wait_for(&mut m, 0, " fps", 600).unwrap_or_else(|| panic!("{}: no timedemo result: {:?}", name, tail(&m)));
        eprintln!("timedemo: {}", console(&m)[at.saturating_sub(60)..at + 4].trim());
        return;
    }
    let press = |m: &mut Machine, key: &str, hold: u64| {
        assert!(m.panel_key(key, true));
        assert_eq!(m.run(hold * ms), Stop::Budget, "{:?}", m.cpu.fault);
        assert!(m.panel_key(key, false));
    };
    // Quake's 320x200, centred on the 400x235 screen: how many colours,
    // and the pixels themselves.
    let window = |m: &Machine| {
        let (w, h, pixels) = m.render();
        let bytes = pixels.len() / (w * h);
        let mut out = Vec::new();
        for y in 17..217 {
            out.extend_from_slice(&pixels[(y * w + 40) * bytes..(y * w + 360) * bytes]);
        }
        let colours: std::collections::HashSet<&[u8]> = out.chunks(bytes).collect();
        (colours.len(), out.clone())
    };
    let text = console(&m);
    let silent = arg.as_deref().is_some_and(|a| a.contains("-nosound"));
    assert!(silent || text.contains("quake: sound from card processor 1"), "{}: no sound: {:?}", name, tail(&m));

    // The first demo, read from the card as it plays: the picture moves
    // and is heard.
    wait_for(&mut m, 0, "Playing demo from demo1.dem", 30).unwrap_or_else(|| panic!("{}: no demo: {:?}", name, tail(&m)));
    assert_eq!(m.run(10_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    shot(&m, "demo");
    let (colours, first) = window(&m);
    assert!(colours > 40, "{}: {} colours in Quake's window", name, colours);
    assert_eq!(m.run(1_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    let (_, second) = window(&m);
    assert_ne!(first, second, "{}: the demo does not move", name);
    let dsp = &m.board.media.dsps[1];
    assert!(dsp.fault.is_none(), "{}: {:?}", name, dsp.fault);
    assert!(silent || dsp.loud_frames > 0, "{}: the other card processor sends its converter only silence ({} samples)", name, dsp.frames);

    // While the card is read and the demo plays: the display folds down
    // and up.
    press(&mut m, "EJECT", 200);
    assert_eq!(m.run(2_500 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    assert!(m.board.display_pos <= 94, "{}: the display is at {} in the demo", name, m.board.display_pos);
    press(&mut m, "EJECT", 200);
    assert_eq!(m.run(2_500 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    assert!(m.board.display_pos >= 894, "{}: the display is at {}", name, m.board.display_pos);

    // The menu (SETUP), Single Player, New Game: the start map.  Then
    // forward and a turn.
    let before = console(&m).len();
    press(&mut m, "SETUP", 150);
    assert_eq!(m.run(800 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    shot(&m, "menu");
    for _ in 0..2 {
        press(&mut m, "SELECT", 150);
        assert_eq!(m.run(800 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    }
    wait_for(&mut m, before, "quake: playing start", 60).unwrap_or_else(|| { shot(&m, "fail"); panic!("{}: no game began: {:?}", name, tail(&m)) });
    assert_eq!(m.run(3_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    shot(&m, "start");
    let (_, standing) = window(&m);
    press(&mut m, "NEXT", 1_000);
    assert!(m.panel_knob("main", 4));
    assert_eq!(m.run(500 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    shot(&m, "moved");
    let (_, moved) = window(&m);
    assert_ne!(standing, moved, "{}: the view did not change with NEXT and the knob", name);

    // While it plays: the disc comes out; a short RETURN is Quake's (the
    // menu), not the way back.
    let ejects = m.atapi().ejects;
    press(&mut m, "CD_EJECT", 200);
    assert_eq!(m.run(2_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    assert_eq!(m.atapi().ejects, ejects + 1, "{}: no eject in the game", name);
    let before = console(&m).len();
    press(&mut m, "RETURN", 300);
    assert_eq!(m.run(1_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    assert!(!console(&m)[before..].contains("rnse sdk: sdboot"), "{}: a short RETURN left the game", name);

    // RETURN held: back to sdboot, with the status of a RETURN held.
    let before = console(&m).len();
    press(&mut m, "RETURN", 2_500);
    assert_eq!(m.run(3_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    let text = console(&m);
    assert!(text[before..].contains("rnse sdk: sdboot") && text[before..].contains("TEST ENDED BY HOLDING RETURN"), "{}: {:?}", name, &text[before..]);
    assert!(!text[before..].lines().any(|l| l.trim_end() == "rnse sdk: quake"), "{}: Quake was started again", name);

    // (A game built with -DRNSE_DEBUG_IDLE says how long the faceplate
    // went unanswered at most.)
    for line in console(&m).lines().filter(|l| l.starts_with("idle:")) {
        eprintln!("{}: {}", name, line);
    }

    // The watchdog was served throughout, card reads included (it resets
    // the unit 105 ms after the last edge), and the caches never handed
    // out anything stale.
    let longest = m.watchdog.longest / ms;
    assert!(longest < 40, "{}: the watchdog waited {} ms, until {} ms, PC {:08x}", name, longest, m.watchdog.longest_end / ms, m.watchdog.longest_pc);
    let cache = &m.cpu.cache.trouble;
    assert_eq!((cache.stale_fetches, cache.dirty_discarded), (0, 0), "{}: {:?}", name, cache);
}

/// Generated homebrew cartridge: colored background, pulse tone, no copyrighted ROM.
#[test]
fn nes_bare_metal_sd_video_audio() {
    use rnse::board::Model;
    let Some(image) = disc() else { return };
    let sdk = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("sdk");
    assert!(
        sdk.join("out/nes/NESCORE.BIN").exists(),
        "run sdk/nes-build.sh first"
    );
    let (mut m, _, _) = sdk_example(&image, Model::Rnse192, "sdboot")
        .expect("SH4 compiler and 192 firmware required");
    let variants = fw::load_variants(&image).unwrap();
    let v = variants
        .iter()
        .find(|v| v.name == "V222")
        .expect("V222 firmware required");
    m.install(v, &fw::IndexInfo::from_disc(&image, "V222").unwrap());
    m.cpu.breakpoints.clear();
    let mut rom = vec![0; 16 + 16384 + 8192];
    rom[..6].copy_from_slice(&[b'N', b'E', b'S', 26, 1, 1]);
    let program: &[u8] = &[
        0x78, 0xd8, 0xa2, 0xff, 0x9a, // SEI, CLD, stack
        0xa9, 0x00, 0x8d, 0x00, 0x20, 0x8d, 0x01, 0x20, 0xa9, 0x3f, 0x8d, 0x06, 0x20, 0xa9, 0x00,
        0x8d, 0x06, 0x20, 0xa9, 0x21, 0x8d, 0x07, 0x20, // blue universal background
        0xa9, 0x08, 0x8d, 0x01, 0x20, 0xa9, 0x01, 0x8d, 0x15, 0x40, 0xa9, 0xbf, 0x8d, 0x00, 0x40,
        0xa9, 0xfd, 0x8d, 0x02, 0x40, 0xa9, 0x08, 0x8d, 0x03, 0x40, 0x4c, 0x35, 0x80,
    ];
    rom[16..16 + program.len()].copy_from_slice(program);
    for offset in [0x3ffa, 0x3ffc, 0x3ffe] {
        rom[16 + offset] = 0;
        rom[16 + offset + 1] = 0x80;
    }
    let card = rnse::fat::card(
        "NES",
        &[
            rnse::fat::Entry::file(
                "NESCORE.BIN",
                std::fs::read(sdk.join("out/nes/NESCORE.BIN")).unwrap(),
            ),
            rnse::fat::Entry::file(
                "RNSE.BIN",
                std::fs::read(sdk.join("out/nes/RNSE.BIN")).unwrap(),
            ),
            rnse::fat::Entry::file("DEMO.NES", rom),
        ],
        None,
    )
    .unwrap();
    assert!(m.insert_card(0, rnse::dev::sdhi::Card::from_bytes(card).unwrap()));
    let flash = m.board.flash.clone();
    m.speaker = Some(Vec::new());
    until(&mut m, 0, "DEMO", 60)
        .unwrap_or_else(|| panic!("{}", String::from_utf8_lossy(&m.console)));
    let ms = m.cycles_per_ms();
    m.panel_key("SELECT", true);
    assert_eq!(m.run(200 * ms), Stop::Budget);
    m.panel_key("SELECT", false);
    until(&mut m, 0, "NES PLAYING", 60)
        .unwrap_or_else(|| panic!("{:?}: {}", m.cpu.fault, String::from_utf8_lossy(&m.console)));
    assert_eq!(m.run(1000 * ms), Stop::Budget, "{:?}", m.cpu.fault);
    assert_eq!(m.board.flash, flash);
    assert_eq!(m.cpu.cache.trouble.stale_fetches, 0);
    assert_eq!(m.cpu.cache.trouble.dirty_discarded, 0);
    assert!(!String::from_utf8_lossy(&m.console).contains("unexpected"));
    let picture = m.render();
    let center = (117 * picture.0 + 200) * 3;
    assert!(
        picture.2[center + 2] > picture.2[center],
        "blue cartridge background"
    );
    std::fs::create_dir_all(sdk.join("out/nes")).unwrap();
    std::fs::write(
        sdk.join("out/nes/demo.png"),
        rnse::png::encode_rgb(picture.0, picture.1, &picture.2),
    )
    .unwrap();
    let heard = m.speaker.take().unwrap();
    assert!(
        heard.len() > 44100 && heard.iter().any(|v| v.unsigned_abs() > 1),
        "cartridge pulse audio: {} samples, peak {}", heard.len(), heard.iter().map(|v|v.unsigned_abs()).max().unwrap_or(0)
    );
    let symbols = std::fs::read_to_string(sdk.join("out/nes/nes.sym")).unwrap();
    let inputs = u32::from_str_radix(
        symbols
            .lines()
            .find(|l| l.ends_with(" inputs"))
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap(),
        16,
    )
    .unwrap();
    // With no adapter, faceplate holds become player-one buttons.
    m.panel_key("SOFT_TR", true);
    m.panel_key("NEXT", true);
    m.panel_key("INFO", true);
    assert_eq!(m.run(100 * ms), Stop::Budget);
    assert_eq!(m.peek32(inputs + 4), Some(0x85));
    assert_eq!(m.peek32(inputs + 12), Some(0));
    m.can_frame(rnse::dev::hcan::Frame::new(
        0x6e3,
        &[1, 0, 1, 128, 0, 0, 0, 0],
    ));
    assert_eq!(m.run(80 * ms), Stop::Budget);
    assert_eq!(m.peek32(inputs + 4), Some(1));
    assert_eq!(m.peek32(inputs + 12), Some(128));
    assert_eq!(m.run(250 * ms), Stop::Budget);
    assert_eq!(m.peek32(inputs + 4), Some(0x85)); // held faceplate resumes on timeout
    assert_eq!(m.peek32(inputs + 12), Some(0));
    m.panel_key("SOFT_TR", false);
    m.panel_key("NEXT", false);
    m.panel_key("INFO", false);
    assert_eq!(m.run(100 * ms), Stop::Budget);
    assert_eq!(m.peek32(inputs + 4), Some(0));
    let at = m.console.len();
    m.panel_key("RETURN", true);
    assert_eq!(m.run(200 * ms), Stop::Budget);
    m.panel_key("RETURN", false);
    until(&mut m, at, "DEMO", 30).expect("return to ROM browser restores SD");
    m.panel_key("SELECT", true);
    assert_eq!(m.run(200 * ms), Stop::Budget);
    m.panel_key("SELECT", false);
    until(&mut m, at, "NES PLAYING", 30).expect("second game launch");
    assert_eq!(m.board.flash, flash);
    m.panel_key("RETURN", true);
    assert_eq!(m.run(2500 * ms), Stop::Budget);
    m.panel_key("RETURN", false);
    until(&mut m, at, "TEST ENDED BY HOLDING RETURN", 30)
        .unwrap_or_else(||panic!("long RETURN: {}",String::from_utf8_lossy(&m.console[at..])));
    assert_eq!(m.board.flash, flash);
    assert_eq!(m.cpu.cache.trouble.stale_fetches, 0);
    assert_eq!(m.cpu.cache.trouble.dirty_discarded, 0);
}

/// Optional user-provided cartridge smoke test; never stores the ROM in the repo.
#[test]
fn nes_user_rom_from_sd_loader() {
    use rnse::board::Model;
    let Some(path) = std::env::var_os("RNSE_NES_ROM") else {
        return;
    };
    let image = disc().expect("RNSE_DISC required for user-ROM test");
    let sdk = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("sdk");
    let rom = std::fs::read(path).expect("RNSE_NES_ROM unreadable");
    assert!(rom.starts_with(b"NES\x1a"));
    let (mut m, _, _) =
        sdk_example(&image, Model::Rnse192, "sdboot").expect("SH4 compiler required");
    let variants = fw::load_variants(&image).unwrap();
    let v = variants
        .iter()
        .find(|v| v.name == "V222")
        .expect("V222 required");
    m.install(v, &fw::IndexInfo::from_disc(&image, "V222").unwrap());
    m.cpu.breakpoints.clear();
    let card = rnse::fat::card(
        "NES",
        &[
            rnse::fat::Entry::file(
                "NESCORE.BIN",
                std::fs::read(sdk.join("out/nes/NESCORE.BIN")).unwrap(),
            ),
            rnse::fat::Entry::file(
                "NES.BIN",
                std::fs::read(sdk.join("out/nes/RNSE.BIN")).unwrap(),
            ),
            rnse::fat::Entry::file("METROID.NES", rom),
        ],
        None,
    )
    .unwrap();
    assert!(m.insert_card(0, rnse::dev::sdhi::Card::from_bytes(card).unwrap()));
    let flash = m.board.flash.clone();
    until(&mut m, 0, "METROID", 60)
        .unwrap_or_else(|| panic!("{}", String::from_utf8_lossy(&m.console)));
    let ms = m.cycles_per_ms();
    m.panel_key("SELECT", true);
    assert_eq!(m.run(200 * ms), Stop::Budget);
    m.panel_key("SELECT", false);
    until(&mut m, 0, "NES PLAYING", 30)
        .unwrap_or_else(|| panic!("{}", String::from_utf8_lossy(&m.console)));
    assert_eq!(m.run(5000 * ms), Stop::Budget, "{:?}", m.cpu.fault);
    let save = |m: &Machine, name: &str| {
        let (w, h, p) = m.render();
        std::fs::write(
            sdk.join("out/nes").join(name),
            rnse::png::encode_rgb(w, h, &p),
        )
        .unwrap();
        assert!(
            p.chunks_exact(3)
                .any(|v| v[0] > 60 || v[1] > 60 || v[2] > 60),
            "visible cartridge output"
        );
    };
    save(&m, "metroid-title.png");
    // Faceplate Start, then A + Right, with no CAN adapter attached.
    m.panel_key("SELECT", true);
    assert_eq!(m.run(200 * ms), Stop::Budget);
    m.panel_key("SELECT", false);
    assert_eq!(m.run(5000 * ms), Stop::Budget, "{:?}", m.cpu.fault);
    save(&m, "metroid-start.png");
    m.panel_key("SELECT", true);
    assert_eq!(m.run(200 * ms), Stop::Budget);
    m.panel_key("SELECT", false);
    assert_eq!(m.run(5000 * ms), Stop::Budget, "{:?}", m.cpu.fault);
    save(&m, "metroid-gameplay.png");
    m.panel_key("SOFT_TR", true);
    m.panel_key("NEXT", true);
    assert_eq!(m.run(1000 * ms), Stop::Budget, "{:?}", m.cpu.fault);
    m.panel_key("SOFT_TR", false);
    m.panel_key("NEXT", false);
    save(&m, "metroid-controls.png");
    assert_eq!(m.board.flash, flash);
    assert_eq!(m.cpu.cache.trouble.stale_fetches, 0);
    assert_eq!(m.cpu.cache.trouble.dirty_discarded, 0);
    assert!(!String::from_utf8_lossy(&m.console).contains("unexpected"));
}
