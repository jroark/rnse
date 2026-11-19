//! End-to-end tests that need a real update disc image.
//!
//! The firmware is not redistributable, so these only run when the
//! environment variable `RNSE_DISC` points at an ISO image of an update
//! CD:
//!
//!     RNSE_DISC=/path/to/disc.iso cargo test --release -- --nocapture
//!
//! The tests of the SDK (`sdk/`) build its examples and are skipped
//! without an SH-4 GCC on the PATH (`tools/sh4-docker` has one in a
//! container).  Those that compare with a real boot ROM need
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

#[test]
fn custom_program_runs() {
    use rnse::board::Model;
    let Some(image) = disc() else { return };
    for model in [Model::Rnse192, Model::Rnse193] {
        let Some((mut m, name, _)) = sdk_example(&image, model, "hello") else { continue };
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
        let Some((mut m, name, _)) = sdk_example(&image, model, "keys") else { continue };
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

/// The SDK's display driver (sdk/examples/display) against the
/// display's model: the key next to the power button opens the display
/// and closes it again, and the motor is off once it has got there,
/// stopped by the driver because the position no longer changes.  On
/// the 193 this checks the driver against the emulator's assumption of
/// which motor line closes, and nothing more.
#[test]
fn sdk_display_opens_and_closes() {
    use rnse::board::Model;
    let Some(image) = disc() else { return };
    for model in [Model::Rnse192, Model::Rnse193] {
        let Some((mut m, name, _)) = sdk_example(&image, model, "display") else { continue };
        let ms = m.cycles_per_ms();
        assert_eq!(m.run(500 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
        assert!(m.display_pos() >= 894, "{}: starts closed", name);
        let press = |m: &mut Machine, key: &str| {
            assert!(m.panel_key(key, true));
            m.run(200 * ms);
            assert!(m.panel_key(key, false));
        };
        let text = |m: &Machine| String::from_utf8_lossy(&m.console).into_owned();
        press(&mut m, "EJECT");
        assert_eq!(m.display_motor(), -1, "{}: opening: {:?}", name, text(&m));
        assert_eq!(m.run(3_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
        assert!(m.display_pos() <= 94, "{}: open: {} {:?}", name, m.display_pos(), text(&m));
        assert_eq!(m.display_motor(), 0, "{}: the motor stops when open", name);
        assert!(text(&m).contains("display open, "), "{}: {:?}", name, text(&m));
        press(&mut m, "EJECT");
        assert_eq!(m.display_motor(), 1, "{}: closing", name);
        assert_eq!(m.run(3_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
        assert!(m.display_pos() >= 894, "{}: closed: {}", name, m.display_pos());
        assert_eq!(m.display_motor(), 0, "{}: the motor stops when closed", name);
        // Opened again and stopped half-way by SELECT: in between, not
        // stuck, and the motor off.
        press(&mut m, "NEXT");
        m.run(400 * ms);
        press(&mut m, "SELECT");
        m.run(500 * ms);
        assert_eq!(m.display_motor(), 0, "{}: stopped by SELECT", name);
        assert!(m.display_pos() > 94 && m.display_pos() < 894, "{}: in between: {}", name, m.display_pos());
        assert!(text(&m).contains("display between, "), "{}: {:?}", name, text(&m));
        // CD EJECT, behind the display, ejects the disc (panel_service).
        let ejects = m.atapi().ejects;
        press(&mut m, "CD_EJECT");
        m.run(500 * ms);
        let after = m.atapi().ejects;
        assert_eq!(after, ejects + 1, "{}: CD EJECT did not eject: {:?}", name, text(&m));
        // A short RETURN does nothing; held for two seconds it would go
        // back to the SD loader, and with none (started from a disc
        // here) the program says so and goes on, keys still answered.
        press(&mut m, "RETURN");
        m.run(500 * ms);
        assert!(!text(&m).contains("display: done"), "{}: {:?}", name, text(&m));
        assert!(m.panel_key("RETURN", true));
        m.run(2_500 * ms);
        assert!(m.panel_key("RETURN", false));
        m.run(300 * ms);
        let t = text(&m);
        assert!(t.contains("display: done") && t.contains("return unavailable"), "{}: {:?}", name, t);
        press(&mut m, "PREV");
        m.run(3_000 * ms);
        assert!(m.display_pos() >= 894, "{}: closed after all that: {}", name, m.display_pos());
        assert_eq!(m.display_motor(), 0, "{}", name);
        let t = text(&m);
        assert!(t.contains("display closed, "), "{}: {:?}", name, t);
        assert!(!t.contains("unexpected"), "{}: {:?}", name, t);
    }
}

/// The SDK's drive code against the drive's model: the program waits
/// for the disc, reads the volume descriptor, finds a file in the root
/// directory and adds up its first megabyte.
#[test]
fn custom_program_reads_the_disc() {
    use rnse::board::Model;
    let Some(image) = disc() else { return };
    for model in [Model::Rnse192, Model::Rnse193] {
        let Some((mut m, name, custom)) = sdk_example(&image, model, "cd") else { continue };
        let ms = m.cycles_per_ms();
        // The disc "changed": the drive is becoming ready for some
        // seconds and then answers one command with a unit attention,
        // all of which the program has to get past; and it takes its
        // time over seeks and reads.
        m.atapi().times = true;
        m.atapi().insert(Some(custom.clone()));
        // (The 192's drive has to be told to start, as a unit's was
        // seen to; the SDK's wait does that.)
        assert_eq!(m.run(20_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
        let text = String::from_utf8_lossy(&m.console).into_owned();
        // What the program should have found, from the image itself.
        let iso = fw::Iso::new(&custom).unwrap();
        let entry = iso.find("LOADING.KWI").expect("LOADING.KWI on the disc");
        let start = entry.lba as usize * 2048;
        let sum = custom[start..start + (entry.len as usize).min(0x100000)].iter().fold(0u32, |a, b| a.wrapping_add(*b as u32));
        assert!(text.contains(&format!("file at sector {}, {} bytes\r\n", entry.lba, entry.len)), "{}: {:?}", name, text);
        assert!(text.contains(&format!("sum {:08x}\r\n", sum)), "{}: sum {:08x} in {:?}", name, sum, text);
        assert!(!text.contains("unexpected"), "{}: {:?}", name, text);
    }
}

/// The drive log (sdk/examples/drive): the program has the drive push
/// the disc out when the eject key is pressed, the disc goes back in,
/// and the log has what the drive's model says on the way, with the
/// times; then SETUP hands over to the stock loader.  On a real unit
/// the same log tells what the model has to assume.
#[test]
fn drive_program_logs_the_drive() {
    use rnse::board::Model;
    let Some(image) = disc() else { return };
    for model in [Model::Rnse192, Model::Rnse193] {
        let Some((mut m, name, custom)) = sdk_example(&image, model, "drive") else { continue };
        let ms = m.cycles_per_ms();
        m.atapi().load(custom);
        assert_eq!(m.run(3_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
        let press = |m: &mut Machine, key: &str| {
            assert!(m.panel_key(key, true));
            m.run(300 * ms);
            assert!(m.panel_key(key, false));
        };
        press(&mut m, "CD_EJECT");
        // (The 192's drive takes 3.6 s over it and says "tray closed"
        // for three more.)
        assert_eq!(m.run(8_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
        let ejects = m.atapi().ejects;
        assert_eq!(ejects, 1, "{}: {:?}", name, String::from_utf8_lossy(&m.console));
        // Back in: not ready while the drive loads the disc, the notice
        // that the medium may have changed, and ready; on the 192, as
        // a unit's drive was seen to do it, not ready in another way
        // for a while in between.
        assert!(m.atapi().reinsert());
        assert_eq!(m.run(8_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
        if model == Model::Rnse192 {
            // The 192's drive waits to be told to start: PREV.
            press(&mut m, "PREV");
            assert_eq!(m.run(8_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
        }
        let text = String::from_utf8_lossy(&m.console).into_owned();
        let lines: Vec<&str> = text.lines().map(|l| l.trim()).filter(|l| l.len() > 8 && l.as_bytes()[7] == b' ' && l[..7].bytes().all(|b| b.is_ascii_digit())).collect();
        let states: Vec<&str> = lines.iter().map(|l| &l[8..]).collect();
        let time = |i: usize| lines[i][..7].parse::<u64>().unwrap();
        let loading = if model == Model::Rnse192 {
            assert_eq!(states, ["READY", "2/04/00", "6/28/00", "2/3A/01", "2/3A/00", "2/04/00", "6/28/00", "2/04/04", "START", "READY"], "{}: {:?}", name, text);
            assert!((3_500..=3_800).contains(&(time(2) - time(1))), "{}: going out from {} to {}", name, time(1), time(2));
            // Not ready for as long as it is only asked, and 5.65 s
            // more once it has been told to start, as on a unit.
            assert!(time(8) - time(7) > 5_000, "{}: started by itself after {} ms", name, time(8) - time(7));
            assert!((5_500..=5_900).contains(&(time(9) - time(8))), "{}: ready {} ms after the start", name, time(9) - time(8));
            assert!((2_600..=2_900).contains(&(time(6) - time(5))), "{}: not ready from {} to {}", name, time(5), time(6));
            5
        } else {
            assert_eq!(states, ["READY", "2/3A/00", "2/04/00", "6/28/00", "READY"], "{}: {:?}", name, text);
            assert!((5_900..=6_200).contains(&(time(3) - time(2))), "{}: not ready from {} to {}", name, time(2), time(3));
            2
        };
        assert!(!text.contains("unexpected"), "{}: {:?}", name, text);
        // The lines went out on the bus as well.
        let frames: Vec<_> = m.can().tx.iter().filter(|f| f.id == 0x7f5).cloned().collect();
        assert_eq!(frames.len(), states.len(), "{}", name);
        assert_eq!(&frames[loading].data[4..8], &[2, 4, 0, 0], "{}", name);
        // NEXT: the drive's reading speed, 256 sectors timed.
        press(&mut m, "NEXT");
        m.run(3_000 * ms);
        let text = String::from_utf8_lossy(&m.console).into_owned();
        assert!(text.contains(" READ 256 IN 000"), "{}: {:?}", name, text);
        press(&mut m, "SETUP");
        m.run(1_000 * ms);
        let text = String::from_utf8_lossy(&m.console).into_owned();
        assert!(text.contains("starting the stock loader"), "{}: {:?}", name, text);
    }
}

/// The SDK's CAN code against the controller's model: frames from the
/// bus reach the program, its own frame goes out with the stock bit
/// timing, and on a bus with nobody else it gives up as it should.
#[test]
fn custom_program_talks_on_the_can_bus() {
    use rnse::board::Model;
    use rnse::dev::hcan::Frame;
    let Some(image) = disc() else { return };
    for model in [Model::Rnse192, Model::Rnse193] {
        let Some((mut m, name, _)) = sdk_example(&image, model, "can") else { continue };
        let ms = m.cycles_per_ms();
        assert_eq!(m.run(300 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
        // Two frames at once: on the wire one follows the other, and
        // the program has to take the first out of the controller
        // before the second is there.
        m.can_frame(Frame::new(0x351, &[0x45, 0, 0, 0, 0, 0, 0, 0]));
        m.can_frame(Frame::new(0x635, &[0x5a, 0]));
        assert_eq!(m.run(1_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
        let text = String::from_utf8_lossy(&m.console).into_owned();
        for line in ["rx 351: 45 00 00 00 00 00 00 00\r\n", "rx 635: 5A 00\r\n", "tx 661: 83 01 12 A0 00 00 00 00\r\n"] {
            assert!(text.contains(line), "{}: {:?} in {:?}", name, line, text);
        }
        let stock = m.can().stock_timing();
        assert_eq!(m.can().tx_timing, Some(stock), "{}: bit timing", name);
        assert_eq!(m.can().tx.back(), Some(&Frame::new(0x661, &[0x83, 0x01, 0x12, 0xa0, 0, 0, 0, 0])), "{}", name);

        // The same on a bench, with no other node to acknowledge.
        let (mut m, name, _) = sdk_example(&image, model, "can").unwrap();
        m.can().alone = true;
        assert_eq!(m.run(1_500 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
        let text = String::from_utf8_lossy(&m.console).into_owned();
        assert!(text.contains("tx not acknowledged\r\n") && !text.contains("tx 661"), "{}: {:?}", name, text);
        assert_eq!(m.can().tx_count, 0, "{}", name);
    }
}

/// The SDK's report (sdk/examples/report): the program reads the
/// processor's registers before it sets anything up and sends them,
/// the state it was started in and the boot ROM out on the CAN bus;
/// `rnse report` puts the log of frames back together.  Here the boot
/// ROM is the emulator's stand-in, and what comes back is that, byte
/// for byte.
#[test]
fn report_program_sends_the_boot_rom() {
    use rnse::board::Model;
    let Some(image) = disc() else { return };
    for model in [Model::Rnse192, Model::Rnse193] {
        let Some((mut m, name, _)) = sdk_example(&image, model, "report") else { continue };
        let ms = m.cycles_per_ms();
        // A frame takes its time on the bus: one round of the 192's
        // 48 KiB is some 15 seconds.
        let mut frames = Vec::new();
        let mut report = rnse::report::Report::default();
        for _ in 0..600 {
            assert_eq!(m.run(100 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
            frames.extend(m.can().tx.drain(..));
            if frames.len() % 500 < 70 {
                report = rnse::report::Report::from_frames(&frames);
                if report.complete() {
                    break;
                }
            }
        }
        let text = String::from_utf8_lossy(&m.console).into_owned();
        assert!(text.contains("rnse sdk: report"), "{}: console: {:?}", name, text);
        assert!(report.complete(), "{}: after {} frames: {}", name, frames.len(), report.describe());
        let rom = report.rom_bytes().unwrap();
        assert_eq!(report.unit, if model == Model::Rnse192 { 0x92 } else { 0x93 }, "{}", name);
        if model == Model::Rnse192 {
            assert_eq!(rom.len(), 0xc000, "{}", name);
            // The clock register, as the emulator has it, and what the
            // report makes of it.
            assert_eq!(report.value(0xffc0_0000), Some(0x0e0a), "{}", name);
            assert!(report.describe().contains("peripherals 1/4"), "{}: {}", name, report.describe());
            // The two card processors have run the little program the
            // report loads into them: their pins as the emulator
            // straps them, and the two loops timed, which come out as
            // the emulator's clocks.
            for base in [0x10, 0x20] {
                assert_eq!(report.value(base), Some(1), "{}: {}", name, report.describe());
                assert_eq!(report.value(base + 1), Some(0x3f), "{}: {}", name, report.describe());
                // (Clock control: 0x0100 after a reset, as on a unit.)
                assert_eq!(report.value(base + 2), Some(0x100), "{}", name);
                let (t1, t2) = (report.value(base + 4).unwrap() as f64, report.value(base + 5).unwrap() as f64);
                // 1024 x 32768 clocks of 22.5792 MHz, in counts of 14.7 MHz.
                let want = 1024.0 * 32768.0 / 22_579_200.0 * 14_700_000.0;
                assert!((t1 / want - 1.0).abs() < 0.001, "{}: {} counts, {} expected", name, t1, want);
                assert!((t1 / t2 - 3.0).abs() < 0.01, "{}: {} and {} counts", name, t1, t2);
            }
            assert!(report.describe().contains("the 58.8 MHz the emulator has"), "{}: {}", name, report.describe());
            assert!(text.contains("0000003f"), "{}: console: {:?}", name, text);
        } else {
            assert!(report.value(0x10).is_none(), "{}", name);
        }
        assert_eq!(rom[..], m.board.flash[..rom.len()], "{}: the ROM that arrived", name);
        // Not much slower than the bus allows: at least 500 frames a
        // second, and never faster than a frame's 1.2 ms.
        let seconds = m.cpu.cycles as f64 / ms as f64 / 1000.0;
        let rate = frames.len() as f64 / seconds;
        assert!((400.0..850.0).contains(&rate), "{}: {} frames in {:.1} s", name, frames.len(), seconds);
        // As text, the way a log would have them.
        let log: String = frames.iter().map(|f| format!("[can] tx {}\n", f)).collect();
        let again = rnse::report::Report::from_frames(&rnse::report::frames(&log));
        assert_eq!(again.rom_bytes().unwrap(), rom, "{}", name);
        // The start state: started directly here, with interrupts masked.
        assert!(report.value(1).is_some() && report.value(5) == Some(0), "{}: {}", name, report.describe());
    }
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

/// A disc put into the drive while the unit runs, which is how an
/// update is normally begun: the stock software of each unit notices
/// it, takes it for an update and starts its loader (here the SDK's
/// first example in the loader's place).  This depends on what the
/// drive says while it loads the disc (docs/boot-rom.md).
#[test]
fn a_disc_put_in_while_the_unit_runs_starts_the_update() {
    use rnse::board::Model;
    let Some(image) = disc() else { return };
    for (model, at) in [(Model::Rnse192, 10_000), (Model::Rnse193, 20_000)] {
        let Some((mut m, name, _)) = started_by_stock_software_with(&image, model, "hello", Some(at)) else { continue };
        let ms = m.cycles_per_ms();
        assert_eq!(m.run(2_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
        let text = String::from_utf8_lossy(&m.console).into_owned();
        assert!(text.contains("rnse sdk: hello"), "{}: console: {:?}", name, text);
    }
}

/// The stock software of each unit takes a disc from `rnse custom` for
/// an update and starts the SDK's first example, which then has to cope
/// with the machine as a running operating system leaves it.  Nothing
/// may be written to flash on the way.
#[test]
fn stock_software_starts_a_custom_program() {
    use rnse::board::Model;
    let Some(image) = disc() else { return };
    let sdk = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("sdk");
    let variants = fw::load_variants(&image).unwrap();
    for model in [Model::Rnse192, Model::Rnse193] {
        let Some((mut m, name, _)) = started_by_stock_software(&image, model, "hello") else { continue };
        let ms = m.cycles_per_ms();
        // What a loader finds: privileged mode with interrupts open and
        // the operating system's handlers in place, a return address in
        // the application, the language in r4.
        assert_eq!(m.cpu.sr & (rnse::sh4::SR_MD | rnse::sh4::SR_BL | 0xf0), rnse::sh4::SR_MD, "{}: SR {:08x}", name, m.cpu.sr);
        assert_ne!(m.cpu.pr, 0, "{}", name);
        assert!(m.cpu.r[4] < 9, "{}: language {}", name, m.cpu.r[4]);
        let flash = m.board.flash.clone();
        assert_eq!(m.run(3_500 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
        let text = String::from_utf8_lossy(&m.console).into_owned();
        assert!(text.contains("rnse sdk: hello") && text.contains("tick 2\r\n") && !text.contains("unexpected"), "{}: {:?}", name, &text[text.len().saturating_sub(300)..]);
        assert!(m.board.flash == flash, "{}: the program wrote to flash", name);
        // The program's picture, not the installed software's.
        let (w, h, rgb) = m.render();
        let at = |x: usize, y: usize| [rgb[(y * w + x) * 3], rgb[(y * w + x) * 3 + 1], rgb[(y * w + x) * 3 + 2]];
        let near = |p: [u8; 3], q: [u8; 3]| p.iter().zip(q).all(|(a, b)| a.abs_diff(b) <= 4);
        assert!(near(at(w * 3 / 16, h / 6), [255, 255, 0]), "{}: second colour bar is {:?}", name, at(w * 3 / 16, h / 6));
        assert!(near(at(w / 2, h * 7 / 8), [0, 0, 0]), "{}: background is {:?}", name, at(w / 2, h * 7 / 8));

        // The same disc with the index it had is no update: the
        // installed software goes on as usual.
        let v = variants.iter().find(|v| v.name == name).unwrap();
        let index = fw::IndexInfo::from_disc(&image, &name).unwrap();
        let unit = if model == Model::Rnse192 { "192" } else { "193" };
        let program = std::fs::read(sdk.join(format!("out/hello-{}.bin", unit))).unwrap();
        let (same, _) = fw::custom_loader(&image, &name, &program).unwrap();
        let mut m = Machine::for_model(model);
        m.echo_console = false;
        m.install_boot_stub(&name);
        m.install(v, &index);
        m.atapi().load(same);
        m.start_os(v).unwrap();
        m.set_ignition(true);
        assert_eq!(m.run(15_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
        assert!(!String::from_utf8_lossy(&m.console).contains("rnse sdk"), "{}: started without a higher index", name);
    }
}

/// The other examples started the same way: the faceplate, the drive
/// and the CAN controller are then in use by the installed software up
/// to the moment the program takes over.
#[test]
fn stock_software_starts_programs_that_use_its_devices() {
    use rnse::board::Model;
    use rnse::dev::hcan::Frame;
    let Some(image) = disc() else { return };
    for model in [Model::Rnse192, Model::Rnse193] {
        // Keys and knobs.
        let Some((mut m, name, _)) = started_by_stock_software(&image, model, "keys") else { continue };
        let ms = m.cycles_per_ms();
        let resets = m.panel.resets;
        assert_eq!(m.run(600 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
        assert_eq!(m.panel.resets, resets + 1, "{}: the program restarts the faceplate controller", name);
        assert!(m.panel_key("SETUP", true));
        m.run(100 * ms);
        assert!(m.panel_key("SETUP", false));
        assert!(m.panel_knob("main", -1));
        assert_eq!(m.run(300 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
        let text = String::from_utf8_lossy(&m.console).into_owned();
        for line in ["faceplate online\r\n", "SETUP down\r\n", "SETUP up\r\n", "select 1 left\r\n"] {
            assert!(text.contains(line), "{}: {:?} in {:?}", name, line, &text[text.len().saturating_sub(300)..]);
        }

        // The disc.
        let (mut m, name, custom) = started_by_stock_software(&image, model, "cd").unwrap();
        assert_eq!(m.run(4_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
        let text = String::from_utf8_lossy(&m.console).into_owned();
        let entry = fw::Iso::new(&custom).unwrap().find("LOADING.KWI").unwrap();
        let start = entry.lba as usize * 2048;
        let sum = custom[start..start + (entry.len as usize).min(0x100000)].iter().fold(0u32, |a, b| a.wrapping_add(*b as u32));
        assert!(text.contains(&format!("sum {:08x}\r\n", sum)), "{}: sum {:08x} in {:?}", name, sum, &text[text.len().saturating_sub(300)..]);

        // CAN.
        let (mut m, name, _) = started_by_stock_software(&image, model, "can").unwrap();
        m.set_ignition(false); // no more frames but the one below
        assert_eq!(m.run(1_300 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
        m.can_frame(Frame::new(0x635, &[0x5a, 0]));
        assert_eq!(m.run(300 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
        let text = String::from_utf8_lossy(&m.console).into_owned();
        for line in ["rx 635: 5A 00\r\n", "tx 661: 83 01 12 A0 00 00 00 00\r\n"] {
            assert!(text.contains(line), "{}: {:?} in {:?}", name, line, &text[text.len().saturating_sub(300)..]);
        }
        let stock = m.can().stock_timing();
        assert_eq!(m.can().tx_timing, Some(stock), "{}: bit timing", name);
        assert!(!text.contains("unexpected"), "{}: {:?}", name, text);
    }
}

/// The way back.  The stock software starts the SDK's probe from a disc
/// with a higher index; the probe counts down and hands over to the
/// stock loader that `rnse custom` packed into its image; the stock
/// loader updates the unit from the disc and resets it.  Afterwards the
/// flash holds the disc's software with the disc's index, so that the
/// disc is no update any more.
#[test]
fn custom_program_hands_back_to_the_stock_loader() {
    use rnse::board::Model;
    let Some(image) = disc() else { return };
    let variants = fw::load_variants(&image).unwrap();
    for model in [Model::Rnse192, Model::Rnse193] {
        let Some((mut m, name, custom)) = started_by_stock_software(&image, model, "probe") else { continue };
        let ms = m.cycles_per_ms();
        // A minute of counting down, then the update.
        let stop = m.run(240_000 * ms);
        let text = String::from_utf8_lossy(&m.console).into_owned();
        let tail = &text[text.len().saturating_sub(300)..];
        assert!(text.contains("the stock loader is carried along\r\n") && text.contains("starting the stock loader\r\n"), "{}: {:?}", name, tail);
        assert!(!text.contains("did not unpack") && !text.contains("unexpected"), "{}: {:?}", name, tail);
        assert_eq!(stop, Stop::Fault, "{}: the stock loader ends with a reset: {:?}", name, tail);
        assert!(m.reset_requested, "{}: {:?}", name, m.cpu.fault);
        let cache = &m.cpu.cache.trouble;
        assert_eq!((cache.stale_fetches, cache.dirty_discarded), (0, 0), "{}: {:?}", name, cache);
        // What the stock loader installed: the disc's modules, and a
        // directory with the disc's (raised) index.
        let v = variants.iter().find(|v| v.name == name).unwrap();
        for module in v.modules.iter().filter(|x| x.kind != "LDR") {
            let (a, n) = (module.phys_addr() as usize, module.size as usize);
            assert!(m.board.flash[a..a + n] == module.data[..n], "{}: module {} as written by the stock loader", name, module.name);
        }
        let index = fw::IndexInfo::from_disc(&custom, &name).unwrap();
        assert_ne!(index.sofidx, fw::IndexInfo::from_disc(&image, &name).unwrap().sofidx);
        let dir = fw::module_directory_for(v, &index);
        let at = v.module("OSM").unwrap().phys_addr() as usize - dir.len();
        assert!(m.board.flash[at..at + dir.len()] == dir[..], "{}: module directory with index {}", name, index.sofidx);
    }
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

/// The disc `rnse firmware` makes of an SDK program, with a higher
/// index than the stock disc's.
fn firmware_disc(image: &[u8], example: &str) -> Option<Vec<u8>> {
    use rnse::board::Model;
    let sdk = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("sdk");
    let variants = fw::load_variants(image).unwrap();
    let v = variants.iter().find(|v| Model::for_variant(&v.name) == Some(Model::Rnse192))?;
    sdk_example(image, Model::Rnse192, example)?;
    let program = std::fs::read(sdk.join(format!("out/{}-192.bin", example))).unwrap();
    sdk_example(image, Model::Rnse192, "swap")?;
    let swap = std::fs::read(sdk.join("out/swap-192.bin")).unwrap();
    let mut disc = fw::firmware_disc(image, &v.name, &program, Some(&swap)).unwrap();
    let index = fw::IndexInfo::from_disc(image, &v.name).unwrap();
    let middle: u32 = index.sofidx[1..3].parse().unwrap();
    let higher = format!("{}{:02}{}", &index.sofidx[..1], middle + 1, &index.sofidx[3..]);
    fw::set_disc_index(&mut disc, &v.name, &higher).unwrap();
    Some(disc)
}

/// A program in flash (`rnse firmware`, sdk/examples/installed), all the
/// way round under a unit's boot ROM: the stock software takes the disc
/// for an update, its loader writes the program into the operating
/// system's place, the ROM starts it; and when RETURN is held the
/// program takes the directory away, the ROM finds no software and
/// installs from the disc that is in the drive by then, the stock one.
#[test]
fn a_program_is_installed_in_flash_and_taken_out_again() {
    let (Some(image), Some(rom)) = (disc(), boot_rom()) else { return };
    let Some(custom) = firmware_disc(&image, "installed") else { return };
    let Some(mut m) = rom_192(&image, &custom, &rom) else { return };
    let ms = m.cycles_per_ms();
    let tail = |m: &Machine| { let t = String::from_utf8_lossy(&m.console).into_owned(); t[t.len().saturating_sub(300)..].to_string() };
    let jump = until(&mut m, 0, "*** Jump User RESET ***", 600).unwrap_or_else(|| panic!("the stock software did not install the disc: {:?}", tail(&m)));
    let started = until(&mut m, jump, "rnse sdk: installed", 60).unwrap_or_else(|| panic!("the ROM did not start the program: {:?}", tail(&m)));
    // It runs, shows a picture, and counts itself healthy.
    assert_eq!(m.run(12_000 * ms), Stop::Budget, "{:?}", m.cpu.fault);
    let (_, _, pixels) = m.render();
    assert!(pixels.iter().any(|&p| p != 0), "the screen is dark");
    assert_eq!(m.resets, 0, "the watchdog reset the unit");
    // The way out, with the stock disc in the drive.
    m.atapi().load(image.clone());
    assert!(m.panel_key("RETURN", true));
    assert_eq!(m.run(3_000 * ms), Stop::Budget, "{:?}", m.cpu.fault);
    assert!(m.panel_key("RETURN", false));
    let removed = until(&mut m, started, "removed, restarting", 10).unwrap_or_else(|| panic!("not removed: {:?}", tail(&m)));
    let back = until(&mut m, removed, "*** Jump User RESET ***", 600).unwrap_or_else(|| panic!("the ROM did not install the stock disc: {:?}", tail(&m)));
    until(&mut m, back, "[fw] SOUND_TSK START", 60).unwrap_or_else(|| panic!("the stock software did not start: {:?}", tail(&m)));
    stock_is_in_flash(&m, &image);
}

/// The disc that installed a program is still in the drive when the
/// program is taken out.  The boot ROM runs the loader that the disc
/// has as a file, which on a firmware disc is not the installation but
/// sdk/examples/swap: the disc is pushed out, and when the stock disc
/// has gone in the unit restarts and the ROM installs that.
#[test]
fn a_firmware_disc_left_in_the_drive_does_not_install_again() {
    let (Some(image), Some(rom)) = (disc(), boot_rom()) else { return };
    let Some(custom) = firmware_disc(&image, "installed") else { return };
    let Some(mut m) = rom_192(&custom, &custom, &rom) else { return };
    let ms = m.cycles_per_ms();
    let tail = |m: &Machine| { let t = String::from_utf8_lossy(&m.console).into_owned(); t[t.len().saturating_sub(300)..].to_string() };
    assert_eq!(m.run(12_000 * ms), Stop::Budget, "{:?}", m.cpu.fault);
    assert!(m.panel_key("RETURN", true));
    assert_eq!(m.run(3_000 * ms), Stop::Budget, "{:?}", m.cpu.fault);
    assert!(m.panel_key("RETURN", false));
    let swap = until(&mut m, 0, "rnse sdk: swap", 60).unwrap_or_else(|| panic!("the ROM did not run the disc's other loader: {:?}", tail(&m)));
    until(&mut m, swap, "ejected", 20).unwrap_or_else(|| panic!("the disc was not pushed out: {:?}", tail(&m)));
    // It waits, for as long as it takes; the program is not installed
    // again and nothing is erased.
    let erased = m.board.flash_ctl.erased_sectors;
    assert_eq!(m.run(20_000 * ms), Stop::Budget, "{:?}", m.cpu.fault);
    assert_eq!(m.board.flash_ctl.erased_sectors, erased, "something was erased while it waited");
    assert_eq!(String::from_utf8_lossy(&m.console).matches("rnse sdk: installed").count(), 1);
    // The stock disc goes in.
    m.atapi().insert(Some(image.clone()));
    let again = until(&mut m, swap, "a disc is ready, restarting", 60).unwrap_or_else(|| panic!("the new disc was not noticed: {:?}", tail(&m)));
    let back = until(&mut m, again, "*** Jump User RESET ***", 600).unwrap_or_else(|| panic!("the ROM did not install the stock disc: {:?}", tail(&m)));
    until(&mut m, back, "[fw] SOUND_TSK START", 60).unwrap_or_else(|| panic!("the stock software did not start: {:?}", tail(&m)));
    stock_is_in_flash(&m, &image);
}

/// A firmware of one's own (sdk/examples/base), started by a unit's
/// boot ROM from flash with nothing of the stock software run before
/// it: it starts the radio processor and has the tuner as the source,
/// tunes, starts a card by itself and plays an MP3 file of it through
/// the amplifier, and goes back to the tuner.
#[test]
fn a_firmware_from_flash_is_a_radio_and_a_player() {
    use rnse::dev::radio::Radio;
    let (Some(image), Some(rom)) = (disc(), boot_rom()) else { return };
    let Some(custom) = firmware_disc(&image, "base") else { return };
    let Some(mut m) = rom_192(&custom, &image, &rom) else { return };
    let ms = m.cycles_per_ms();
    let song = include_bytes!("data/tones.mp3").to_vec();
    let card = rnse::fat::card("MUSIC", &[rnse::fat::Entry::file("TONES.MP3", song)], None).unwrap();
    assert!(m.insert_card(0, rnse::dev::sdhi::Card::from_bytes(card).unwrap()));
    let tail = |m: &Machine| { let t = String::from_utf8_lossy(&m.console).into_owned(); t[t.len().saturating_sub(300)..].to_string() };
    until(&mut m, 0, "radio started", 30).unwrap_or_else(|| panic!("the radio processor was not started: {:?}", tail(&m)));
    assert_eq!(m.run(2_000 * ms), Stop::Budget, "{:?}", m.cpu.fault);
    assert_eq!((m.radio.source, m.radio.volume), (Radio::SOURCE_TUNER, 8), "the tuner as the source");
    let press = |m: &mut Machine, key: &str| {
        assert!(m.panel_key(key, true));
        m.run(200 * ms);
        assert!(m.panel_key(key, false));
        m.run(800 * ms);
    };
    press(&mut m, "NEXT");
    assert_eq!(m.radio.frequency, Some(2062), "one megahertz up from 102.1");
    // MEDIA: the card is started from nothing, and its file plays.
    press(&mut m, "MEDIA");
    until(&mut m, 0, "playing", 30).unwrap_or_else(|| panic!("no file was played: {:?}", tail(&m)));
    assert_eq!(m.run(2_000 * ms), Stop::Budget, "{:?}", m.cpu.fault);
    assert_eq!(m.board.media.dsps[0].fault, None);
    assert!(m.board.media.dsps[0].loud_frames > 0, "the card processor sends its converter only silence");
    assert_eq!(m.radio.source, Radio::SOURCE_DECODER, "the amplifier's source");
    let (_, _, pixels) = m.render();
    assert!(pixels.iter().any(|&p| p != 0), "the screen is dark");
    assert!(m.panel_knob("volume", 3));
    assert_eq!(m.run(1_500 * ms), Stop::Budget, "{:?}", m.cpu.fault);
    assert_eq!(m.radio.volume, 11);
    press(&mut m, "RADIO");
    assert_eq!(m.radio.source, Radio::SOURCE_TUNER, "back to the tuner");
    assert_eq!(m.resets, 0, "the watchdog reset the unit");
}

/// The guard of a program in flash (sdk/src/flash.c): starts that end
/// before the program has called itself healthy are counted, and the
/// fifth in a row takes the program out, after which the boot ROM
/// installs the stock disc.  That is the way out of a program that
/// hangs, and the one by hand: off and on four times.
#[test]
fn a_program_in_flash_that_never_gets_going_takes_itself_out() {
    let (Some(image), Some(rom)) = (disc(), boot_rom()) else { return };
    let Some(custom) = firmware_disc(&image, "installed") else { return };
    // The program installed already, the stock disc in the drive.
    let Some(mut m) = rom_192(&custom, &image, &rom) else { return };
    let ms = m.cycles_per_ms();
    let count = |m: &Machine| String::from_utf8_lossy(&m.console).matches("rnse sdk: installed").count();
    // Four starts that are cut short, three seconds in.
    for n in 1..=4 {
        assert_eq!(m.run(3_000 * ms), Stop::Budget, "{:?}", m.cpu.fault);
        assert_eq!(count(&m), n, "start {}", n);
        m.cpu.reset();
    }
    // The fifth does not get to the program's main.
    let back = until(&mut m, 0, "*** Jump User RESET ***", 600).expect("the ROM did not install the stock disc");
    assert_eq!(count(&m), 4, "the program was started a fifth time");
    until(&mut m, back, "[fw] SOUND_TSK START", 60).expect("the stock software did not start");
    stock_is_in_flash(&m, &image);
    // And one that does get going is not counted out: installed anew,
    // it runs through any number of starts once it has been healthy.
    let Some(mut m) = rom_192(&custom, &image, &rom) else { return };
    for n in 1..=6 {
        assert_eq!(m.run(13_000 * ms), Stop::Budget, "{:?}", m.cpu.fault);
        assert_eq!(count(&m), n, "start {}", n);
        m.cpu.reset();
    }
}

/// The beacon (sdk/examples/beacon), the probe that says on the CAN bus
/// how far it has got: its frames in order, the faceplate controller
/// taking its settings, and the hand-over at the end.
#[test]
fn beacon_reports_its_progress_on_the_bus() {
    use rnse::board::Model;
    let Some(image) = disc() else { return };
    let Some((mut m, name, _)) = sdk_example(&image, Model::Rnse192, "beacon") else { return };
    let ms = m.cycles_per_ms();
    assert_eq!(m.run(24_500 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    let text = String::from_utf8_lossy(&m.console).into_owned();
    assert!(text.contains("starting the stock loader"), "{}: {:?}", name, text);
    assert!(!text.contains("unexpected"), "{}: {:?}", name, text);
    let frames: Vec<_> = m.can().tx.iter().filter(|f| f.id == 0x7f6).cloned().collect();
    let stages: Vec<u8> = frames.iter().map(|f| f.data[0]).collect();
    // Up; eight seconds without the faceplate; its start; fourteen with.
    let mut want = vec![1u8];
    want.extend([2; 8]);
    want.push(5);
    want.extend([2; 14]);
    // And the one it leaves in the controller for the hand-over.
    want.push(3);
    assert_eq!(stages, want, "{}", name);
    // A second apart, by the program's own clock in the frames.
    let at = |i: usize| u32::from(frames[i].data[2]) << 8 | u32::from(frames[i].data[3]);
    assert_eq!(at(22) - at(12), 1000, "{}", name);
    // The faceplate controller has its settings a second after it was started.
    assert_eq!(frames[9].data[7] & 2, 0, "{}", name);
    assert_eq!(frames[12].data[7] & 2, 2, "{}", name);
    // The state it was started in, and the latch and port A as found.
    assert_eq!(m.can().tx.iter().filter(|f| f.id == 0x7f7).count(), 9, "{}", name);
}

/// The suite (sdk/examples/suite): a row of measurements in one run,
/// every answer a record on the CAN bus.  Here against the emulator's
/// own models, which shows that the program asks properly; on a unit
/// the answers are what the models are corrected by.
#[test]
fn suite_runs_its_measurements() {
    use rnse::board::Model;
    let Some(image) = disc() else { return };
    let Some((mut m, name, custom)) = sdk_example(&image, Model::Rnse192, "suite") else { return };
    let ms = m.cycles_per_ms();
    m.atapi().load(custom);
    m.atapi().times = true;
    // Up to the thirty seconds for hands.
    let mut waited = 0;
    while !String::from_utf8_lossy(&m.console).contains("KEYS AND KNOBS") {
        assert_eq!(m.run(1_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
        waited += 1;
        assert!(waited < 60, "{}: {:?}", name, String::from_utf8_lossy(&m.console));
    }
    m.run(3_500 * ms);
    assert!(m.panel_key("NAV", true));
    m.run(300 * ms);
    assert!(m.panel_key("NAV", false));
    assert!(m.panel_knob("main", 3));
    m.run(500 * ms);
    assert!(m.panel_key("RETURN", true));
    m.run(300 * ms);
    assert!(m.panel_key("RETURN", false));
    // No disc change (RETURN leaves it out), and of the battery-backed
    // memory only the beginning (RETURN held down).
    let until = |m: &mut Machine, what: &str| {
        let mut waited = 0;
        while !String::from_utf8_lossy(&m.console).contains(what) {
            assert_eq!(m.run(200 * ms), Stop::Budget, "{:?}", m.cpu.fault);
            waited += 1;
            assert!(waited < 300, "{}: {:?}", what, String::from_utf8_lossy(&m.console));
        }
    };
    until(&mut m, "A DISC CHANGE");
    m.run(500 * ms);
    assert!(m.panel_key("SETUP", true));
    until(&mut m, "BATTERY-BACKED MEMORY");
    until(&mut m, "BYTES SENT");
    assert!(m.panel_key("SETUP", false));
    // The watchdog test ends the run: the emulator's bites as a unit's does.
    let stop = m.run(8_000 * ms);
    let text = String::from_utf8_lossy(&m.console).into_owned();
    assert_eq!(stop, Stop::Fault, "{}: {:?}", name, text);
    assert!(m.reset_requested, "{}: {:?}", name, m.cpu.fault);
    assert!(!text.contains("unexpected"), "{}: {:?}", name, text);
    let records: Vec<(u8, u8, u32, u16)> = m.can().tx.iter().filter(|f| f.id == 0x7f8).map(|f| {
        let d = f.data;
        (d[0], d[1], u32::from_be_bytes([d[2], d[3], d[4], d[5]]), u16::from_be_bytes([d[6], d[7]]))
    }).collect();
    let value = |test: u8, item: u8| records.iter().find(|r| r.0 == test && r.1 == item).map(|r| r.2);
    // Every test was opened, in order.
    let opened: Vec<u8> = records.iter().filter(|r| r.1 == 0xfe).map(|r| r.0).collect();
    assert_eq!(opened, [1, 2, 3, 4, 5, 6, 7, 8, 11, 12, 13, 14, 15, 16, 9, 19, 17, 18, 10], "{}", name);
    // The display folded down, stopped by the driver at its end and
    // open (state 1), its switch saying so; and up again (state 0).
    let second = |test: u8, item: u8| records.iter().find(|r| r.0 == test && r.1 == item).map(|r| r.3);
    assert_eq!(second(19, 2), Some(1), "{}: {:?}", name, records.iter().filter(|r| r.0 == 19).collect::<Vec<_>>());
    assert!(value(19, 3).is_some_and(|p| p <= 94) && second(19, 3) == Some(1), "{}: open at {:?}", name, value(19, 3));
    assert!(value(19, 2).is_some_and(|t| (1_000..3_000).contains(&t)), "{}: the motor ran {:?} ms", name, value(19, 2));
    assert!(records.iter().filter(|r| r.0 == 19 && r.1 == 1).count() >= 10, "{}: samples while it moved", name);
    assert_eq!(second(19, 5), Some(0), "{}", name);
    assert!(value(19, 6).is_some_and(|p| p >= 894) && second(19, 6) == Some(0), "{}: closed at {:?}", name, value(19, 6));
    assert_eq!(m.display_motor(), 0, "{}", name);
    // The radio processor was asked its two questions.  (Its line is
    // the installed software's to set up; started like this, without
    // that software, the program hears nothing on it.)  And the first
    // flash chip said what it is.
    assert!(value(12, 62).is_some() && value(12, 126).is_some(), "{}", name);
    assert_eq!((value(13, 0), value(13, 1)), (Some(0x04), Some(0x2222_227e)), "{}", name);
    // "QRY", and a sector erased in 2^8 ms.
    assert_eq!((value(13, 2), value(13, 3), value(13, 4), value(13, 19)), (Some(0x51), Some(0x52), Some(0x59), Some(8)), "{}", name);
    // The drive's name once more, in the full answer.
    assert_eq!(value(14, 2), Some(u32::from_be_bytes(*b"ALPI")), "{}", name);
    // Some of the battery-backed memory went out on its own frames.
    assert!(m.can().tx.iter().any(|f| f.id == 0x7f9), "{}", name);
    // The loop takes some 14 million instructions' time at 235.2 MHz.
    let spin = value(1, 0).unwrap();
    assert!((40_000..90_000).contains(&spin), "{}: the loop took {} us", name, spin);
    assert!(value(1, 1).unwrap() > 2_000, "{}: adding up took {:?} us", name, value(1, 1));
    // The display is closed: input 3 at its top.
    assert_eq!(value(2, 3), Some(1023), "{}", name);
    // Port 0 is the strobe's, an output; the latch is as the ROM and
    // board_init leave it.
    assert_eq!(records.iter().find(|r| r.0 == 3 && r.1 == 0).map(|r| r.3), Some(1), "{}", name);
    assert_eq!(value(3, 17), Some(0x3f9f), "{}", name);
    // The clock chip is not running, as on a unit.
    assert_eq!(value(4, 0), Some(0), "{}", name);
    assert!(value(5, 0).unwrap() < 1_000, "{}: faceplate after {:?} ms", name, value(5, 0));
    // The drive: the disc's size, and 64 sectors at 5.5 ms each.
    assert_eq!(value(7, 9), Some(image.len() as u32 / 2048), "{}", name);
    for item in 10..=13 {
        let t = value(7, item).unwrap();
        assert!((330_000..650_000).contains(&t), "{}: read {} took {} us", name, item, t);
    }
    // Its name, as a unit's drive gave it.
    assert_eq!(value(7, 2), Some(u32::from_be_bytes(*b"ALPI")), "{}", name);
    // The key and the knob were seen.
    assert!(records.iter().any(|r| r.0 == 9 && r.1 == 0 && r.2 & 1 << 2 != 0), "{}: NAV not seen", name);
    let knob: Vec<i32> = records.iter().filter(|r| r.0 == 9 && r.1 == 2).map(|r| r.2 as i32).collect();
    assert!(knob.last().is_some_and(|v| v.abs() == 3), "{}: the knob's three steps not seen: {:?}", name, knob);
    // And the watchdog: records every 10 ms until it bites, after some
    // 110 ms as on a unit.
    let last = records.iter().filter(|r| r.0 == 10 && r.1 == 0).map(|r| r.2).max().unwrap();
    assert!((90..=130).contains(&last), "{}: reset {} ms after the last strobe", name, last);
}

/// Sound from a program of the SDK (sdk/examples/player): started from
/// a card by the loader, it has the card processor play an MP3 file of
/// that card and the radio processor's amplifier take the converter as
/// its source, and what leaves the speaker is the file's two tones.
#[test]
fn sdk_player_plays_an_mp3() {
    use rnse::board::Model;
    use rnse::dev::radio::Radio;
    let Some(image) = disc() else { return };
    let sdk = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("sdk");
    let Some((mut m, name, _)) = started_by_stock_software(&image, Model::Rnse192, "sdboot") else { return };
    if sdk_example(&image, Model::Rnse192, "player").is_none() { return }
    let player = std::fs::read(sdk.join("out/player-192.bin")).unwrap();
    let song = include_bytes!("data/tones.mp3").to_vec();
    let entries = vec![rnse::fat::Entry::file("RNSE.BIN", player), rnse::fat::Entry::file("TONES.MP3", song)];
    let card = rnse::fat::card("MUSIC", &entries, None).unwrap();
    assert!(m.insert_card(0, rnse::dev::sdhi::Card::from_bytes(card).unwrap()));
    let ms = m.cycles_per_ms();
    m.cpu.breakpoints.clear();
    assert_eq!(m.run(12_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    let text = String::from_utf8_lossy(&m.console).into_owned();
    let tail = &text[text.len().saturating_sub(400)..];
    assert!(text.contains("rnse sdk: player"), "{}: {:?}", name, tail);
    assert!(text.contains("playing: TONES   MP3"), "{}: {:?}", name, tail);
    assert!(!text.contains("not played"), "{}: {:?}", name, tail);
    let dsp = &m.board.media.dsps[0];
    assert_eq!(dsp.fault, None, "{}", name);
    assert!(dsp.loud_frames > 0, "{}: the card processor sends its converter only silence: {:?}", name, tail);
    assert_eq!((m.radio.source, m.radio.volume), (Radio::SOURCE_DECODER, 8), "{}: the amplifier's source and volume", name);
    // A second of it, and the tones where they belong.
    m.speaker = Some(Vec::new());
    assert_eq!(m.run(1_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    let heard = m.speaker.take().unwrap();
    assert!(heard.len() > 80_000, "{}: {} samples in a second", name, heard.len());
    for (channel, want) in [(0usize, 440.0f64), (1, 880.0)] {
        let samples: Vec<f64> = heard.iter().skip(channel).step_by(2).map(|&v| f64::from(v)).collect();
        let crossings = samples.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count() as f64;
        let pitch = crossings / (samples.len() as f64 / 44_100.0);
        assert!((pitch - want).abs() < want * 0.05, "{}: channel {} sounds at {} Hz, not {}", name, channel, pitch, want);
    }
    // The volume knob is the amplifier's.
    assert!(m.panel_knob("volume", 2));
    assert_eq!(m.run(1_500 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    assert_eq!(m.radio.volume, 10, "{}: two detents louder", name);
}

/// The SD loader's menu (sdk/examples/sdboot): a card with several
/// programs (NAME.BIN) offers them, a loader among them as RNSE.BIN
/// left out; the select knob chooses, SELECT runs, the program comes
/// back to the menu with it chosen, and RETURN runs it again.  And a
/// loader chosen from the menu takes over from the one on the disc, as
/// it does on a unit whose disc has an older loader that runs RNSE.BIN.
#[test]
fn sd_loader_offers_a_menu_of_programs() {
    use rnse::board::Model;
    let Some(image) = disc() else { return };
    let sdk = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("sdk");
    let Some((mut m, name, _)) = started_by_stock_software(&image, Model::Rnse192, "sdboot") else { return };
    let loader = std::fs::read(sdk.join("out/sdboot-192.bin")).unwrap();
    assert!(sdk_example(&image, Model::Rnse192, "returntest").is_some());
    let r17 = std::fs::read(sdk.join("out/returntest-192.bin")).unwrap();
    assert!(sdk_example(&image, Model::Rnse192, "returnapi").is_some());
    let r23 = std::fs::read(sdk.join("out/returnapi-192.bin")).unwrap();
    let flash = m.board.flash.clone();
    let ms = m.cycles_per_ms();
    m.cpu.breakpoints.clear();
    assert_eq!(m.run(2000 * ms), Stop::Budget);
    let press = |m: &mut Machine, key: &str| {
        assert!(m.panel_key(key, true));
        assert_eq!(m.run(200 * m.cycles_per_ms()), Stop::Budget);
        assert!(m.panel_key(key, false));
        assert_eq!(m.run(300 * m.cycles_per_ms()), Stop::Budget);
    };
    let card = rnse::fat::card("TESTS", &[
        rnse::fat::Entry::file("RNSE.BIN", loader.clone()),
        rnse::fat::Entry::file("A17.BIN", r17.clone()),
        rnse::fat::Entry::file("B23.BIN", r23),
    ], None).unwrap();
    let at = m.console.len();
    assert!(m.insert_card(0, rnse::dev::sdhi::Card::from_bytes(card).unwrap()));
    let menu = until(&mut m, at, "> A17.BIN", 30).unwrap_or_else(|| panic!("{}: no menu: {}", name, String::from_utf8_lossy(&m.console[at..])));
    let shown = String::from_utf8_lossy(&m.console[at..]).into_owned();
    assert!(shown.contains("  B23.BIN") && !shown.contains("RNSE.BIN"), "{}: {}", name, shown);
    assert!(!shown.contains("rnse sdk: returntest"), "{}: nothing runs by itself", name);
    // The display key works with the menu up: open, then closed again
    // (CLAUDE.md, "Mandatory hardware behavior").
    press(&mut m, "EJECT");
    assert_eq!(m.run(3000 * ms), Stop::Budget);
    assert!(m.board.display_pos <= 94, "{}: the display did not open from the menu: {}", name, m.board.display_pos);
    press(&mut m, "EJECT");
    assert_eq!(m.run(3000 * ms), Stop::Budget);
    assert!(m.board.display_pos >= 894, "{}: the display did not close: {}", name, m.board.display_pos);
    // The knob to the second, SELECT: it runs and comes back to the
    // menu, with it chosen.
    assert!(m.panel_knob("main", 1));
    assert_eq!(m.run(300 * ms), Stop::Budget);
    press(&mut m, "SELECT");
    let back = until(&mut m, menu, "TEST RETURNED: 23", 60).unwrap_or_else(|| panic!("{}: {}", name, String::from_utf8_lossy(&m.console[menu..])));
    until(&mut m, back, "> B23.BIN", 30).unwrap_or_else(|| panic!("{}: not back at the menu: {}", name, String::from_utf8_lossy(&m.console[back..])));
    // RETURN: the same again.
    assert_eq!(m.run(1500 * ms), Stop::Budget);
    let at = m.console.len();
    press(&mut m, "RETURN");
    until(&mut m, at, "TEST RETURNED: 23", 60).unwrap_or_else(|| panic!("{}: RETURN does not rerun: {}", name, String::from_utf8_lossy(&m.console[at..])));
    // Back to the first.
    let at = m.console.len();
    assert_eq!(m.run(1500 * ms), Stop::Budget);
    assert!(m.panel_knob("main", -1));
    assert_eq!(m.run(300 * ms), Stop::Budget);
    press(&mut m, "SELECT");
    until(&mut m, at, "TEST RETURNED: 17", 60).unwrap_or_else(|| panic!("{}: {}", name, String::from_utf8_lossy(&m.console[at..])));
    let text = String::from_utf8_lossy(&m.console).into_owned();
    assert_eq!(text.matches("rnse sdk: returntest").count(), 3, "{}", name);
    m.eject_card(0);
    assert_eq!(m.run(500 * ms), Stop::Budget);
    // The card out: the menu goes, all of it (a blank of the wrong
    // height left the glyphs' bottom row behind, on a unit).
    let (w, _, rgb) = m.render();
    let w = w as usize;
    for y in 30 + 14 * 2..30 + 14 * 13 {
        assert!(rgb[3 * w * y..3 * w * (y + 1)].iter().all(|&c| c == 0), "{}: the menu left something on line {} of the screen", name, y);
    }

    // A loader under another name, chosen from the menu, becomes the
    // one programs return to.
    let card = rnse::fat::card("CHAIN", &[
        rnse::fat::Entry::file("LOADER.BIN", loader),
        rnse::fat::Entry::file("A17.BIN", r17),
    ], None).unwrap();
    let at = m.console.len();
    assert!(m.insert_card(0, rnse::dev::sdhi::Card::from_bytes(card).unwrap()));
    // (A17, run last, is chosen: this card has it too.)
    until(&mut m, at, "> A17.BIN", 30).unwrap_or_else(|| panic!("{}: {}", name, String::from_utf8_lossy(&m.console[at..])));
    assert!(m.panel_knob("main", -1));
    assert_eq!(m.run(300 * ms), Stop::Budget);
    let menu = until(&mut m, at, "> LOADER.BIN", 30).unwrap_or_else(|| panic!("{}: {}", name, String::from_utf8_lossy(&m.console[at..])));
    press(&mut m, "SELECT");
    // The loader from the card starts, finds the card in, and offers
    // its menu; from there, A17.
    let started = until(&mut m, menu, "rnse sdk: sdboot", 30).expect("the loader from the card starts");
    until(&mut m, started, "> LOADER.BIN", 30).unwrap_or_else(|| panic!("{}: {}", name, String::from_utf8_lossy(&m.console[started..])));
    assert!(m.panel_knob("main", 1));
    assert_eq!(m.run(300 * ms), Stop::Budget);
    press(&mut m, "SELECT");
    until(&mut m, started, "TEST RETURNED: 17", 60).unwrap_or_else(|| panic!("{}: {}", name, String::from_utf8_lossy(&m.console[started..])));
    assert_eq!(m.board.flash, flash, "{}", name);
    let text = String::from_utf8_lossy(&m.console).into_owned();
    assert!(!text.contains("unexpected") && !text.contains("LOAD ERROR"), "{}: {}", name, &text[text.len().saturating_sub(1500)..]);
    let cache = &m.cpu.cache.trouble;
    assert_eq!((cache.stale_fetches, cache.dirty_discarded), (0, 0), "{}: {:?}", name, cache);
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

/// Sound as samples from a program of our own (sdk/src/pcm.c,
/// sdk/examples/pcmtest): each card processor in turn gets the SDK's
/// player and plays two tones, fed by one DMA transfer and by DMA in
/// blocks, and the amplifier gives out each at its pitch, for its two
/// seconds, at the converter's 44.1 kHz, without a break.
#[test]
fn sdk_pcm_plays_through_both_card_processors() {
    use rnse::board::Model;
    use rnse::dev::radio::Radio;
    let Some(image) = disc() else { return };
    let sdk = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("sdk");
    let Some((mut m, name, _)) = started_by_stock_software(&image, Model::Rnse192, "sdboot") else { return };
    if sdk_example(&image, Model::Rnse192, "pcmtest").is_none() { return }
    let program = std::fs::read(sdk.join("out/pcmtest-192.bin")).unwrap();
    let card = rnse::fat::card("SOUND", &[rnse::fat::Entry::file("RNSE.BIN", program)], None).unwrap();
    assert!(m.insert_card(0, rnse::dev::sdhi::Card::from_bytes(card).unwrap()));
    let flash = m.board.flash.clone();
    let ms = m.cycles_per_ms();
    m.cpu.breakpoints.clear();
    m.speaker = Some(Vec::new());
    let text = loop {
        assert_eq!(m.run(1_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
        let text = String::from_utf8_lossy(&m.console).into_owned();
        if text.contains("DONE.") || m.cpu.cycles > 60_000 * ms {
            break text;
        }
    };
    // (What the converters sent last reaches the speaker a little later.)
    assert_eq!(m.run(1_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    let tail = &text[text.len().saturating_sub(1500)..];
    assert!(text.contains("rnse sdk: pcmtest"), "{}: {:?}", name, tail);
    assert!(!text.contains("FAILED") && !text.contains("DID NOT START"), "{}: {:?}", name, tail);
    let rates: Vec<u32> = text.lines().filter_map(|l| l.trim().strip_prefix("SAMPLES A SECOND ")).filter_map(|v| v.trim().parse().ok()).collect();
    assert_eq!(rates.len(), 4, "{}: {:?}", name, tail);
    assert!(rates.iter().all(|r| r.abs_diff(44_100) <= 20), "{}: every way at 44.1 kHz: {:?}", name, rates);
    assert_eq!(m.radio.source, Radio::SOURCE_DECODER, "{}", name);
    for d in &m.board.media.dsps {
        assert_eq!(d.fault, None, "{}: {}", name, d.name);
    }
    // The pitch of each half second of the right channel: the six
    // tones, in their order, each for its two seconds.
    let heard = m.speaker.take().unwrap();
    let right: Vec<i16> = heard.iter().skip(1).step_by(2).copied().collect();
    let pitches: Vec<u32> = right.chunks(22_050).map(|c| 2 * c.windows(2).filter(|w| w[0] < 0 && w[1] >= 0).count() as u32).collect();
    for want in [440, 550, 880, 1100] {
        let halves = pitches.iter().filter(|&&p| p.abs_diff(want) <= 4).count();
        assert!(halves >= 3, "{}: {} Hz heard for {} half seconds: {:?}", name, want, halves, pitches);
    }
    // No break in a tone: within each half second that is one tone
    // throughout, no step between two samples larger than that tone's
    // sine makes.  (Where one tone ends and the next starts, a step is
    // what it is.)
    let chunks: Vec<&[i16]> = right.chunks(22_050).collect();
    for k in 1..chunks.len().saturating_sub(1) {
        let (chunk, pitch) = (chunks[k], pitches[k]);
        let Some(&hz) = [440u32, 550, 880, 1100].iter().find(|&&f| pitch.abs_diff(f) <= 4) else { continue };
        // (The same tone on both sides: not where one starts or ends.)
        if pitches[k - 1].abs_diff(hz) > 4 || pitches[k + 1].abs_diff(hz) > 4 {
            continue;
        }
        let peak = chunk.iter().map(|v| i32::from(*v).abs()).max().unwrap();
        let largest = chunk.windows(2).map(|w| (i32::from(w[1]) - i32::from(w[0])).abs()).max().unwrap();
        // (pcmtest's sine takes eight bits of its phase: its steps are
        // up to a fifth or so uneven.  A break is a step of the order of
        // the whole amplitude.)
        let slope = (1.4 * 2.0 * std::f64::consts::PI * f64::from(hz) / 44_100.0 * f64::from(peak)) as i32;
        assert!(peak > 0 && largest <= slope + 2, "{}: {} Hz: a step of {} where its sine of {} has at most {}", name, hz, largest, peak, slope);
    }
    assert!(!text.contains("unexpected"), "{}: {:?}", name, tail);
    // And back to the SD loader, with the flash as it was.
    let at = text.find("DONE.").unwrap();
    until(&mut m, at, "TEST RETURNED: 0", 60).unwrap_or_else(|| panic!("{}: not back at the loader: {:?}", name, String::from_utf8_lossy(&m.console[at..])));
    assert_eq!(m.board.flash, flash, "{}", name);
    let cache = &m.cpu.cache.trouble;
    assert_eq!((cache.stale_fetches, cache.dirty_discarded), (0, 0), "{}: {:?}", name, cache);
}

/// RETURN held for two seconds (CLAUDE.md, "Mandatory hardware
/// behavior"): in a program the SD loader started, back to the loader
/// (and a RETURN still held then is not taken for a new one); in the
/// loader, back to the stock software: the display opens, the disc is
/// ejected, and the unit starts anew, its flash untouched.
#[test]
fn return_held_goes_back_to_the_loader_and_from_there_to_stock() {
    use rnse::board::Model;
    let Some(image) = disc() else { return };
    let sdk = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("sdk");
    let Some((mut m, name, _)) = started_by_stock_software(&image, Model::Rnse192, "sdboot") else { return };
    assert!(sdk_example(&image, Model::Rnse192, "returntest2").is_some());
    let second = std::fs::read(sdk.join("out/returntest2-192.bin")).unwrap();
    let flash = m.board.flash.clone();
    let ms = m.cycles_per_ms();
    m.cpu.breakpoints.clear();
    assert_eq!(m.run(2000 * ms), Stop::Budget);
    let card = rnse::fat::card("HOLD", &[rnse::fat::Entry::file("SECOND.BIN", second)], None).unwrap();
    let at = m.console.len();
    assert!(m.insert_card(0, rnse::dev::sdhi::Card::from_bytes(card).unwrap()));
    let started = until(&mut m, at, "rnse sdk: returntest2", 30).unwrap_or_else(|| panic!("{}: {}", name, String::from_utf8_lossy(&m.console[at..])));
    // Held within the program's five seconds: the loader again, before
    // the program would have returned by itself (status 23).
    assert!(m.panel_key("RETURN", true));
    assert_eq!(m.run(2600 * ms), Stop::Budget);
    let text = String::from_utf8_lossy(&m.console[started..]).into_owned();
    assert!(text.contains("TEST ENDED BY HOLDING RETURN"), "{}: {}", name, text);
    assert!(!text.contains("TEST RETURNED: 23"), "{}: {}", name, text);
    // Still held, and for longer than two seconds: not the loader's own
    // hold, which would leave for the stock software.
    assert_eq!(m.run(2500 * ms), Stop::Budget);
    assert!(m.panel_key("RETURN", false));
    assert_eq!(m.run(1500 * ms), Stop::Budget);
    assert!(!String::from_utf8_lossy(&m.console[started..]).contains("BACK TO THE STOCK SOFTWARE"), "{}", name);
    assert!(m.atapi().image.is_some(), "{}: the disc is still in", name);
    // A new hold in the loader: display open, disc out, a new start.
    let at = m.console.len();
    assert!(m.panel_key("RETURN", true));
    let stop = m.run(2600 * ms);
    assert!(m.panel_key("RETURN", false));
    let stop = if stop == Stop::Budget { m.run(15_000 * ms) } else { stop };
    let text = String::from_utf8_lossy(&m.console[at..]).into_owned();
    assert!(text.contains("BACK TO THE STOCK SOFTWARE"), "{}: {}", name, text);
    assert!(text.contains("EJECTING THE DISC"), "{}: {}", name, text);
    assert!(m.board.display_pos <= 94, "{}: the display is not open: {}", name, m.board.display_pos);
    assert!(m.atapi().image.is_none(), "{}: the disc was not ejected", name);
    assert!(stop != Stop::Budget && m.reset_requested, "{}: no new start: {:?} {:?}", name, stop, m.cpu.fault);
    assert_eq!(m.board.flash, flash, "{}", name);
    let cache = &m.cpu.cache.trouble;
    assert_eq!((cache.stale_fetches, cache.dirty_discarded), (0, 0), "{}: {:?}", name, cache);
}

/// The radio processor's keep-alive: a unit left in the SD loader's menu
/// (and under Linux) was restarted into the stock software after about
/// seven minutes, the stock software's 81 [06] of every minute not
/// coming.  The loader sends it now (panel_service, radio_keepalive);
/// with the emulated radio processor's patience cut to 70 s, the menu is
/// still there after 150 s, the keep-alives coming once a minute.
#[test]
fn the_loader_keeps_the_radio_processor_content() {
    use rnse::board::Model;
    let Some(image) = disc() else { return };
    let Some((mut m, name, _)) = started_by_stock_software(&image, Model::Rnse192, "sdboot") else { return };
    m.radio_keepalive_limit_ms = 70_000;
    let ms = m.cycles_per_ms();
    m.cpu.breakpoints.clear();
    let before = m.radio.seen.get(&(0x81, 0x06)).copied().unwrap_or(0);
    assert_eq!(m.run(150_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
    assert!(!m.reset_requested, "{}: the unit was restarted", name);
    let sent = m.radio.seen.get(&(0x81, 0x06)).copied().unwrap_or(0) - before;
    assert!((2..=4).contains(&sent), "{}: {} keep-alives in 150 s", name, sent);
    assert!(String::from_utf8_lossy(&m.console).contains("WAITING FOR CARD"), "{}", name);
}

/// Programs from an SD card (sdk/examples/sdboot): the stock software
/// starts the loader from the disc, the loader starts the card
/// processor of a slot with the installed software's program out of
/// the flash, reads RNSE.BIN from the card's FAT and starts it in its
/// own place; and with the card in the second slot likewise.
#[test]
fn programs_come_from_an_sd_card() {
    use rnse::board::Model;
    let Some(image) = disc() else { return };
    let sdk = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("sdk");
    for slot in [0, 1] {
        let Some((mut m, name, _)) = started_by_stock_software(&image, Model::Rnse192, "sdboot") else { return };
        // (Built by sdk_example for the two tests of it.)
        // The probe, because it draws and leaves the faceplate
        // controller alone: the screen's light is the controller's, and
        // a loader that starts the controller anew and then reads a
        // card instead of talking to it hands over a dark screen (as a
        // unit showed).
        if sdk_example(&image, Model::Rnse192, "probe").is_none() { return }
        let probe = std::fs::read(sdk.join("out/probe-192.bin")).unwrap();
        let entries = vec![rnse::fat::Entry::file("RNSE.BIN", probe)];
        let card = rnse::fat::card("RNSE", &entries, None).unwrap();
        // In the second slot an old card, of version 1 of the
        // specification: no answer to CMD8, and busy for a while; and
        // one that is not awake at the first start, which the driver
        // has to make as the stock driver does.
        m.board.media.old_cards = slot == 1;
        m.board.media.deaf_cards = if slot == 1 { 2 } else { 0 };
        assert!(m.insert_card(slot, rnse::dev::sdhi::Card::from_bytes(card).unwrap()));
        let ms = m.cycles_per_ms();
        m.cpu.breakpoints.clear();
        if let Some(address) = Machine::known_printf("V2220371") { m.hook_printf(address); }
        assert_eq!(m.run(40_000 * ms), Stop::Budget, "{}: {:?}", name, m.cpu.fault);
        let text = String::from_utf8_lossy(&m.console).into_owned();
        let tail = &text[text.len().saturating_sub(400)..];
        for d in &m.board.media.dsps {
            assert_eq!(d.fault, None, "{}, slot {}: {} stopped", name, slot, d.name);
        }
        assert!(text.contains("rnse sdk: sdboot"), "{}: {:?}", name, tail);
        assert!(text.contains("STARTING IT"), "{}, slot {}: {:?}", name, slot, tail);
        assert!(text.contains("rnse sdk: probe"), "{}, slot {}: {:?}", name, slot, tail);
        assert!(!text.contains("unexpected"), "{}: {:?}", name, tail);
        let (_, _, pixels) = m.render();
        assert!(pixels.iter().any(|&p| p != 0), "{}, slot {}: the screen is dark", name, slot);
        let cache = &m.cpu.cache.trouble;
        assert_eq!((cache.stale_fetches, cache.dirty_discarded), (0, 0), "{}: {:?}", name, cache);
    }
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

/// Three cooperative tests run with one disc launch and no flash writes.
#[test]
fn sd_loader_returns_and_rearms_on_reinsertion() {
    use rnse::board::Model;
    let Some(image) = disc() else { return };
    for slot in [0, 1] {
        let Some((mut m, _, _)) = started_by_stock_software(&image, Model::Rnse192, "sdboot") else { return };
        assert!(sdk_example(&image, Model::Rnse192, "returntest").is_some(), "returntest build failed");
        let bin = std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("sdk/out/returntest-192.bin")).unwrap();
        assert!(sdk_example(&image, Model::Rnse192, "returnapi").is_some());
        let api = std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("sdk/out/returnapi-192.bin")).unwrap();
        let cards: Vec<_> = [bin, api].into_iter().map(|b| rnse::fat::card("TEST", &[rnse::fat::Entry::file("RNSE.BIN", b)], None).unwrap()).collect();
        let flash = m.board.flash.clone();
        let ms = m.cycles_per_ms();
        let mut from = 0;
        assert_eq!(m.run(2000 * ms), Stop::Budget);
        assert!(String::from_utf8_lossy(&m.console).contains("WAITING FOR CARD"));
        for cycle in 0..3 {
            assert!(m.insert_card(slot, rnse::dev::sdhi::Card::from_bytes(cards[cycle % 2].clone()).unwrap()));
            let expected = if cycle % 2 == 0 { "TEST RETURNED: 17" } else { "TEST RETURNED: 23" };
            let _end = until(&mut m, from, expected, 60).unwrap_or_else(|| panic!("slot {} cycle {}: {}", slot, cycle, String::from_utf8_lossy(&m.console)));
            assert_eq!(m.run(1000 * ms), Stop::Budget);
            let text = String::from_utf8_lossy(&m.console);
            assert_eq!(text.matches("rnse sdk: returntest").count(), cycle + 1);
            assert_eq!(text.matches("rnse sdk: sdboot").count(), cycle + 2);
            assert!(!text[text.find("rnse sdk: sdboot").unwrap()..].contains("[fw]"));
            assert_eq!(m.board.flash, flash);
            assert!(!text.contains("unexpected"));
            assert_eq!(m.cpu.cache.trouble.stale_fetches, 0);
            assert_eq!(m.cpu.cache.trouble.dirty_discarded, 0);
            if cycle == 2 {
                let at = m.console.len();
                assert!(m.panel_key("RETURN", true));
                assert_eq!(m.run(200 * ms), Stop::Budget);
                assert!(m.panel_key("RETURN", false));
                until(&mut m, at, "TEST RETURNED: 17", 60).expect("RETURN reruns the card");
                assert_eq!(String::from_utf8_lossy(&m.console).matches("rnse sdk: returntest").count(), 4);
            }
            from = m.console.len();
            m.eject_card(slot);
            assert_eq!(m.run(500 * ms), Stop::Budget);
        }
        // Bad cards leave the loader available for the next insertion.
        for bad in [None, Some(vec![0; 32]), Some(vec![0; 0x1c001])] {
            let entries = bad.map(|b| vec![rnse::fat::Entry::file("RNSE.BIN", b)]).unwrap_or_default();
            let card = rnse::fat::card("BAD", &entries, None).unwrap();
            let at = m.console.len();
            assert!(m.insert_card(slot, rnse::dev::sdhi::Card::from_bytes(card).unwrap()));
            until(&mut m, at, "LOAD ERROR:", 60).expect("invalid card must not execute");
            assert_eq!(m.board.flash, flash);
            m.eject_card(slot);
            assert_eq!(m.run(500 * ms), Stop::Budget);
        }
        let at = m.console.len();
        assert!(m.insert_card(slot, rnse::dev::sdhi::Card::from_bytes(cards[0].clone()).unwrap()));
        assert_eq!(m.run(500 * ms), Stop::Budget);
        m.eject_card(slot);
        until(&mut m, at, "LOAD ERROR:", 60).expect("removal during initialization must fail");
        assert!(!String::from_utf8_lossy(&m.console[at..]).contains("rnse sdk: returntest"));
        assert_eq!(m.run(500 * ms), Stop::Budget);
        let at = m.console.len();
        for both in [0, 1] {
            assert!(m.insert_card(both, rnse::dev::sdhi::Card::from_bytes(cards[0].clone()).unwrap()));
        }
        let first = until(&mut m, at, "TEST RETURNED: 17", 60).expect("first simultaneous card");
        until(&mut m, first + 1, "TEST RETURNED: 17", 60).expect("second simultaneous card");
        assert_eq!(m.run(2000 * ms), Stop::Budget);
        let log = String::from_utf8_lossy(&m.console[at..]);
        assert_eq!(log.matches("rnse sdk: returntest").count(), 2, "cards must not alternate forever");
        assert!(log.find("SLOT 1").unwrap() < log.find("SLOT 2").unwrap());
        assert_eq!(m.board.flash, flash);
        assert_eq!(m.cpu.cache.trouble.stale_fetches, 0);
        assert_eq!(m.cpu.cache.trouble.dirty_discarded, 0);
    }
}

#[test]
fn second_sd_test_returns_status_23() {
    use rnse::board::Model;
    let Some(image) = disc() else { return };
    let Some((mut m, _, _)) = started_by_stock_software(&image, Model::Rnse192, "sdboot") else { return };
    assert!(sdk_example(&image, Model::Rnse192, "returntest2").is_some(), "second test must build");
    let bin = std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("sdk/out/returntest2-192.bin")).unwrap();
    let card = rnse::fat::card("SECOND", &[rnse::fat::Entry::file("RNSE.BIN", bin)], None).unwrap();
    let flash = m.board.flash.clone();
    assert!(m.insert_card(0, rnse::dev::sdhi::Card::from_bytes(card).unwrap()));
    until(&mut m, 0, "TEST RETURNED: 23", 60).expect("second test must return directly");
    assert!(String::from_utf8_lossy(&m.console).contains("rnse sdk: returntest2"));
    assert_eq!(m.board.flash, flash);
    assert_eq!(m.cpu.cache.trouble.stale_fetches, 0);
    assert_eq!(m.cpu.cache.trouble.dirty_discarded, 0);
}

/// Resident shell: actual SH-4 app code, audio, persistence and malformed input.
#[test]
fn modern_resident_shell() {
    use rnse::board::Model;
    let Some(image) = disc() else { return };
    let (mut m, name, _) = sdk_example(&image, Model::Rnse192, "modern")
        .expect("modern test requires the SH-4 compiler");
    let variants = fw::load_variants(&image).unwrap();
    let v = variants.iter().find(|v| v.name == name).unwrap();
    m.install(v, &fw::IndexInfo::from_disc(&image, &name).unwrap());
    let sdk = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("sdk");
    assert!(std::process::Command::new("sh").arg(sdk.join("apps/build.sh")).arg("trip").status().unwrap().success());
    let good = std::fs::read(sdk.join("out/app-trip.RNA")).unwrap();
    let mut incompatible = good.clone();
    incompatible[7] = 99;
    let mut oversized = good.clone();
    oversized[16..20].copy_from_slice(&0x80001u32.to_be_bytes());
    let mut entry = good.clone();
    entry[28..32].copy_from_slice(&0xffffffffu32.to_be_bytes());
    let mut bad = good.clone();
    *bad.last_mut().unwrap() ^= 1;
    let entries = vec![
        rnse::fat::Entry::file("TRIP.RNA", good),
        rnse::fat::Entry::file("BAD.RNA", bad),
        rnse::fat::Entry::file("ABI.RNA", incompatible),
        rnse::fat::Entry::file("SIZE.RNA", oversized),
        rnse::fat::Entry::file("ENTRY.RNA", entry),
        rnse::fat::Entry::file("TONES.MP3", include_bytes!("data/tones.mp3").to_vec()),
    ];
    let card = rnse::fat::card("MODERN", &entries, None).unwrap();
    m.insert_card(0, rnse::dev::sdhi::Card::from_bytes(card).unwrap());
    let ms = m.cycles_per_ms();
    let run = |m: &mut Machine, n| assert_eq!(m.run(n * ms), Stop::Budget, "{:?} {}", m.cpu.fault, String::from_utf8_lossy(&m.console));
    let press = |m: &mut Machine, key| {
        assert!(m.panel_key(key, true)); run(m, 150);
        assert!(m.panel_key(key, false)); run(m, 150);
    };
    run(&mut m, 2000);
    assert!(String::from_utf8_lossy(&m.console).contains("modern: ready"));
    let shot = |m: &mut Machine, name| {
        let (w,h,rgb)=m.render();
        std::fs::create_dir_all("out/modern").unwrap();
        std::fs::write(format!("out/modern/{name}.png"), rnse::png::encode_rgb(w,h,&rgb)).unwrap();
        rgb
    };
    let radio = shot(&mut m, "radio");
    assert!(radio.chunks(3).any(|p| p[0]>100&&p[1]<50));
    m.panel_knob("volume", 2); run(&mut m, 200);
    assert_eq!(m.radio.volume, 10);
    press(&mut m, "MEDIA"); press(&mut m, "SOFT_BL"); run(&mut m, 8000);
    assert!(String::from_utf8_lossy(&m.console).contains("modern: card ready"));
    shot(&mut m, "media");
    press(&mut m, "SELECT"); run(&mut m, 2000);
    assert!(m.board.media.dsps[0].loud_frames > 0);
    press(&mut m, "INFO"); press(&mut m, "SELECT");
    assert!(String::from_utf8_lossy(&m.console).contains("modern: app started"));
    let before=shot(&mut m, "app");
    m.panel_knob("main", 3); run(&mut m, 200);
    assert_ne!(before, shot(&mut m, "app-adjusted"));
    press(&mut m, "RETURN");
    assert!(String::from_utf8_lossy(&m.console).contains("modern: app exited"));
    m.panel_knob("main", 1); run(&mut m, 200);
    let starts=String::from_utf8_lossy(&m.console).matches("modern: app started").count();
    press(&mut m, "SELECT");
    assert_eq!(starts, String::from_utf8_lossy(&m.console).matches("modern: app started").count());
    shot(&mut m, "bad-app");
    for _ in 0..3 {
        m.panel_knob("main", 1); run(&mut m, 200);
        press(&mut m, "SELECT");
        assert_eq!(starts, String::from_utf8_lossy(&m.console).matches("modern: app started").count());
    }
    // Empty second slot is recoverable through physical controls.
    press(&mut m, "SOFT_TR");
    run(&mut m, 8000);
    shot(&mut m, "no-card");
    press(&mut m, "RADIO");
    assert_eq!(m.radio.source, rnse::dev::radio::Radio::SOURCE_TUNER);
    let saved=m.board.nvram.clone();
    let (mut reboot,_,_)=sdk_example(&image, Model::Rnse192, "modern").unwrap();
    reboot.install(v, &fw::IndexInfo::from_disc(&image, &name).unwrap());
    reboot.board.nvram.copy_from_slice(&saved);
    run(&mut reboot,2000);
    assert_eq!(reboot.radio.volume,10);
    let (mut invalid,_,_)=sdk_example(&image, Model::Rnse192, "modern").unwrap();
    invalid.install(v, &fw::IndexInfo::from_disc(&image, &name).unwrap());
    invalid.board.nvram.fill(0xa5);
    run(&mut invalid,2000);
    assert_eq!(invalid.radio.volume,8, "corrupt settings restore defaults");
    assert_eq!(m.resets,0);
}

#[test]
fn modern_installed_recovery() {
    let (Some(image), Some(rom)) = (disc(), boot_rom()) else { return };
    let custom = firmware_disc(&image, "modern").expect("SH-4 toolchain required");
    let mut m = rom_192(&image, &custom, &rom).unwrap();
    until(&mut m,0,"modern: ready",600).expect("stock loader must install and boot modern");
    let ms=m.cycles_per_ms();
    assert_eq!(m.run(12_000*ms),Stop::Budget);
    assert_eq!(m.resets,0);
    assert_eq!(&m.board.nvram[0x3ff04..0x3ff08], &[0,0,0,0], "boot guard healthy");
    m.atapi().load(image.clone());
    for key in ["SETUP", "SELECT"] {
        if key=="SELECT" {m.panel_knob("main",1);m.run(200*ms);}
        m.panel_key(key,true);m.run(200*ms);m.panel_key(key,false);m.run(200*ms);
    }
    // RETURN from recovery cancels, never erases.
    let directory=m.board.flash[0xc000..0xe000].to_vec();
    m.panel_key("RETURN",true);m.run(200*ms);m.panel_key("RETURN",false);m.run(200*ms);
    assert_eq!(directory,m.board.flash[0xc000..0xe000]);
    m.panel_knob("main",1);m.run(200*ms);
    m.panel_key("SELECT",true);m.run(200*ms);m.panel_key("SELECT",false);m.run(200*ms);
    m.panel_key("SELECT",true);m.run(3000*ms);m.panel_key("SELECT",false);
    until(&mut m,0,"[fw] SOUND_TSK START",600).expect("stock firmware restored");
    stock_is_in_flash(&m,&image);
}

/// Built-ins exercise real firmware editing, SRAM and factory CAN transport.
/// The peer below is a protocol fixture, not real Audi-module compatibility evidence.
#[test]
fn modern_builtins() {
    use rnse::board::Model;
    use rnse::dev::{hcan::Frame, tp20::Peer};
    let Some(image) = disc() else { return };
    let (mut m, name, _) = sdk_example(&image, Model::Rnse192, "modern")
        .expect("modern built-in test requires SH-4 compiler");
    let variants = fw::load_variants(&image).unwrap();
    let v = variants.iter().find(|v| v.name == name).unwrap();
    m.install(v, &fw::IndexInfo::from_disc(&image, &name).unwrap());
    let ms=m.cycles_per_ms();
    let run=|m:&mut Machine,n|assert_eq!(m.run(n*ms),Stop::Budget,"{:?}",m.cpu.fault);
    let press=|m:&mut Machine,key|{m.panel_key(key,true);run(m,150);m.panel_key(key,false);run(m,150);};
    let turn=|m:&mut Machine,n|{m.panel_knob("main",n);run(m,100);};
    let shot=|m:&mut Machine,name|{let(w,h,rgb)=m.render();std::fs::create_dir_all("out/modern").unwrap();std::fs::write(format!("out/modern/{name}.png"),rnse::png::encode_rgb(w,h,&rgb)).unwrap();rgb};
    run(&mut m,2000);
    press(&mut m,"CAR");
    let unknown=shot(&mut m,"vehicle-no-data");
    m.can_send_cyclic(Frame::new(0x271,&[3]),100);
    run(&mut m,1200);
    assert!(unknown != shot(&mut m,"vehicle"), "vehicle data must refresh");
    // Six persistent local contacts, empty by default. Write AB / 01 through rotary editor.
    press(&mut m,"NAME");press(&mut m,"SELECT");press(&mut m,"SOFT_TL");
    turn(&mut m,1);press(&mut m,"SELECT");turn(&mut m,1);press(&mut m,"SELECT");press(&mut m,"SOFT_BR");
    press(&mut m,"SOFT_TR");press(&mut m,"SELECT");turn(&mut m,1);press(&mut m,"SELECT");press(&mut m,"SOFT_BR");
    shot(&mut m,"contact");
    let latest=|m:&Machine| {let a=&m.board.nvram[0x3fc00..0x3fd80];let b=&m.board.nvram[0x3fd80..0x3ff00];if u32::from_be_bytes(a[4..8].try_into().unwrap())>u32::from_be_bytes(b[4..8].try_into().unwrap()){a.to_vec()}else{b.to_vec()}};
    let book=latest(&m);
    assert_eq!(&book[8..11],b"AB\0");assert_eq!(&book[28..31],b"01\0");
    // Cancel leaves the original name. Delete requires explicit soft-key confirmation.
    press(&mut m,"SOFT_TL");press(&mut m,"SOFT_TR");press(&mut m,"RETURN");
    assert_eq!(book,latest(&m));
    press(&mut m,"SOFT_BL");press(&mut m,"RETURN");assert_eq!(book,latest(&m));
    // Saved latitude and longitude, with bounds checked before saving.
    let enter=|m:&mut Machine,text:&str| {let chars="0123456789-.";let mut at=0;press(m,"SOFT_TR");for c in text.chars(){let next=chars.find(c).unwrap() as i32;turn(m,next-at);press(m,"SELECT");at=next;}press(m,"SOFT_BR");};
    press(&mut m,"NAV");press(&mut m,"SOFT_BR");press(&mut m,"SOFT_TR");
    enter(&mut m,"90.0001");assert_eq!(book,latest(&m));
    shot(&mut m,"invalid-coordinate");
    press(&mut m,"RETURN");press(&mut m,"SOFT_TR");enter(&mut m,"37.7749");
    press(&mut m,"SOFT_BR");enter(&mut m,"-122.4194");
    let location=latest(&m);
    assert_eq!(i32::from_be_bytes(location[268..272].try_into().unwrap()),377749);
    assert_eq!(i32::from_be_bytes(location[272..276].try_into().unwrap()),-1224194);
    shot(&mut m,"destination");
    // Original fixed CAN IDs, acknowledged segmented commands and actual module replies.
    m.can_peers.push(Peer::new(0x6c4,0x6c5));
    let mut handled=0usize;
    let mut call=-1i32;
    let pump=|m:&mut Machine,handled:&mut usize,call:&mut i32,n:u64|{
        for _ in 0..n/100 {
            run(m,100);
            let blocks=m.can_peers[0].received[*handled..].to_vec();
            *handled+=blocks.len();
            for block in blocks {
                let cmd=String::from_utf8(block).unwrap();
                let response=if cmd=="AT\r" {"\r\nOK\r\n".to_string()}
                    else if cmd=="AT+CLCC\r" {if *call<0 {"\r\nOK\r\n".to_string()} else {format!("\r\n+CLCC: 1,0,{},0,0,\"01\",129\r\nOK\r\n",*call)}}
                    else if cmd=="ATD01;\r" {*call=2;"\r\nOK\r\n".to_string()}
                    else if cmd=="ATA\r" {*call=0;"\r\nOK\r\n".to_string()}
                    else if cmd=="AT+CHUP\r" {*call=-1;"\r\nOK\r\n".to_string()}
                    else {panic!("unexpected telephone command {cmd:?}")};
                let frames=m.can_peers[0].send(response.as_bytes());
                for f in frames {m.can_frame(f);}
            }
        }
    };
    press(&mut m,"TEL");press(&mut m,"SOFT_TL");pump(&mut m,&mut handled,&mut call,800);
    assert!(m.can_peers[0].open);
    assert!(m.can_peers[0].received.iter().any(|c|c==b"AT\r"));
    press(&mut m,"NAME");press(&mut m,"SELECT");press(&mut m,"SOFT_BR");
    pump(&mut m,&mut handled,&mut call,500);
    // Let the current query finish, then make one explicit request.
    press(&mut m,"SOFT_BR");pump(&mut m,&mut handled,&mut call,1000);
    assert!(m.can_peers[0].received.iter().any(|c|c==b"ATD01;\r"));
    assert_eq!(call,2);
    shot(&mut m,"telephone-dialling");
    call=0;pump(&mut m,&mut handled,&mut call,1500);shot(&mut m,"telephone-active");
    pump(&mut m,&mut handled,&mut call,200);press(&mut m,"SOFT_BL");pump(&mut m,&mut handled,&mut call,1500);
    assert!(m.can_peers[0].received.iter().any(|c|c==b"AT+CHUP\r"));
    // Incoming call and answer use replies, rather than local connected flags.
    call=4;pump(&mut m,&mut handled,&mut call,1500);shot(&mut m,"telephone-ringing");
    press(&mut m,"SOFT_BR");pump(&mut m,&mut handled,&mut call,1500);
    assert!(m.can_peers[0].received.iter().any(|c|c==b"ATA\r"));
    assert_eq!(call,0);
    // Fresh boot restores notebook independently of shell settings and guard.
    let saved=m.board.nvram.clone();
    let(mut reboot,_,_)=sdk_example(&image,Model::Rnse192,"modern").unwrap();
    reboot.board.nvram.copy_from_slice(&saved);run(&mut reboot,2000);
    press(&mut reboot,"NAME");press(&mut reboot,"SELECT");shot(&mut reboot,"contact-restored");
    assert_eq!(latest(&reboot),location);
    assert_eq!(m.resets,0);
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
