//! The emulator as a WebAssembly module, for the browser front end in
//! `web/`.
//!
//! No bindings generator: a handful of C functions, and text passed
//! through one buffer.  The page's worker hands over the image of an
//! update disc the user picked (it never leaves their machine), starts a
//! variant from it and then runs the machine a display frame at a time.
//! A navigation disc, which is far too large to copy, is read on demand
//! through the one function the module imports.
//!
//! | function | |
//! |----------|---|
//! | `rnse_alloc(len)` | memory for the caller to fill |
//! | `rnse_text()`, `rnse_text_len()` | the text buffer: results, and (after `rnse_alloc`-free writing by the caller up to 64 KiB) arguments |
//! | `rnse_firmware(ptr, len)` | take the update disc image; lists its variants in the text buffer, one `name version model` a line |
//! | `rnse_boot(len)` | start the machine by the settings in the text buffer (`key=value` lines) |
//! | `rnse_run()` | one display frame; 1 once the machine has stopped |
//! | `rnse_render()` | bring the picture up to date; 1 if it changed |
//! | `rnse_pixels()`, `rnse_width()`, `rnse_height()` | the picture, RGBA |
//! | `rnse_input(len)` | an event from the page, as text |
//! | `rnse_log()` | new debug output of the firmware, into the text buffer |
//! | `rnse_status()` | the state as JSON, into the text buffer |
//! | `rnse_ms()` | emulated time in milliseconds |
//!
//! All return a negative number on error, with the message in the text
//! buffer.

use crate::dev::atapi::Disc;
use crate::dev::gps::Gps;
use crate::fw::{self, Variant};
use crate::machine::{Drive, Machine};
use crate::sh4::cpu::Stop;
use std::collections::BTreeMap;
use std::sync::Arc;

#[link(wasm_import_module = "env")]
extern "C" {
    /// Read `len` bytes at `offset` of the file the page knows as `id`
    /// into memory at `ptr`; returns how many it got.
    fn rnse_host_read(id: u32, offset: f64, ptr: *mut u8, len: u32) -> u32;
}

struct State {
    image: Vec<u8>,
    variants: Vec<Variant>,
    settings: BTreeMap<String, String>,
    machine: Option<Machine>,
    drive_disc: Option<Disc>,
    what: String,
    rgb: Vec<u8>,
    rgba: Vec<u8>,
    size: (usize, usize),
    log_sent: usize,
    stopped: Option<String>,
    ignition: bool,
    disc_in: bool,
    note: String,
    base_ms: f64,
    text: Vec<u8>,
}

static mut STATE: Option<State> = None;

fn state() -> &'static mut State {
    // One thread, and nothing re-enters: the module is driven by a
    // single worker.
    unsafe {
        let s = &mut *core::ptr::addr_of_mut!(STATE);
        s.get_or_insert_with(|| State {
            image: Vec::new(),
            variants: Vec::new(),
            settings: BTreeMap::new(),
            machine: None,
            drive_disc: None,
            what: String::new(),
            rgb: Vec::new(),
            rgba: Vec::new(),
            size: (0, 0),
            log_sent: 0,
            stopped: None,
            ignition: true,
            disc_in: true,
            note: String::new(),
            base_ms: 0.0,
            text: vec![0; 65536],
        })
    }
}

/// Put `text` into the text buffer; returns its length.
fn say(text: &str) -> i32 {
    let s = state();
    s.text.clear();
    s.text.extend_from_slice(text.as_bytes());
    if s.text.capacity() < 65536 {
        s.text.reserve(65536 - s.text.len());
    }
    text.len() as i32
}

fn fail(text: impl AsRef<str>) -> i32 {
    say(text.as_ref());
    -1
}

/// What the caller wrote into the text buffer.
fn heard(len: u32) -> String {
    let s = state();
    let len = (len as usize).min(s.text.capacity());
    // The caller wrote `len` bytes at `rnse_text()`.
    unsafe { String::from_utf8_lossy(core::slice::from_raw_parts(s.text.as_ptr(), len)).into_owned() }
}

fn json_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out += "\\\"",
            '\\' => out += "\\\\",
            c if (c as u32) < 0x20 => out += " ",
            c => out.push(c),
        }
    }
    out + "\""
}

#[no_mangle]
pub extern "C" fn rnse_alloc(len: u32) -> *mut u8 {
    let mut v = Vec::<u8>::with_capacity(len as usize);
    let p = v.as_mut_ptr();
    core::mem::forget(v);
    p
}

#[no_mangle]
pub extern "C" fn rnse_text() -> *const u8 {
    let s = state();
    if s.text.capacity() < 65536 {
        s.text.reserve(65536);
    }
    s.text.as_ptr()
}

#[no_mangle]
pub extern "C" fn rnse_text_len() -> u32 {
    state().text.len() as u32
}

/// Take the update disc image the caller wrote into memory from
/// `rnse_alloc(len)`.
///
/// # Safety
/// `ptr` must come from `rnse_alloc(len)` and hold `len` bytes.
#[no_mangle]
pub unsafe extern "C" fn rnse_firmware(ptr: *mut u8, len: u32) -> i32 {
    let image = Vec::from_raw_parts(ptr, len as usize, len as usize);
    let variants = match fw::load_variants(&image) {
        Ok(v) => v,
        Err(e) => return fail(format!("this is not an RNS-E update disc image: {}", e)),
    };
    let mut list = String::new();
    for v in &variants {
        let model = match crate::board::Model::for_variant(&v.name) {
            Some(crate::board::Model::Rnse192) => "192",
            Some(crate::board::Model::Rnse193) => "193",
            None => "unknown",
        };
        list += &format!("{} {} {}\n", v.name, v.version, model);
    }
    let s = state();
    s.image = image;
    s.variants = variants;
    say(&list)
}

fn build(s: &mut State) -> Result<Machine, String> {
    let get = |k: &str| s.settings.get(k).map(String::as_str).filter(|v| !v.is_empty());
    let name = get("variant").ok_or("no variant chosen")?;
    let v = s.variants.iter().find(|v| v.name == name).ok_or_else(|| format!("no variant {} on this disc", name))?;
    let model = crate::board::Model::for_variant(&v.name).ok_or_else(|| format!("variant {} is for hardware this emulator does not know", v.name))?;
    let loader = get("loader") == Some("1");
    let mut m = Machine::for_model(model);
    m.echo_console = false;
    m.install_boot_stub(&v.name);
    if s.disc_in {
        if let Some(disc) = &s.drive_disc {
            m.atapi().load_disc(disc.clone());
        }
    }
    if loader {
        let ldm = v.module("LDM").ok_or("variant has no LDM module")?;
        m.start_loader(ldm, [0; 3]);
        if model == crate::board::Model::Rnse193 {
            m.hook_debug_output(v, true);
        }
        s.what = format!("{} update loader", v.name);
    } else {
        let index = fw::IndexInfo::from_disc(&s.image, &v.name).unwrap_or_default();
        m.install(v, &index);
        m.hook_debug_output(v, false);
        if let Some(serial) = get("serial") {
            m.set_serial(&serial.to_uppercase())?;
        }
        let found = v.module("OSM").and_then(Machine::find_gps_getter);
        let fix = match get("gps") {
            Some("none") => None,
            Some(text) => Some(Gps::parse(text)?),
            // Without a position the unit is in San Francisco.
            None => Some(Gps::at(37.7749, -122.4194)),
        };
        if let (Some(fix), Some((getter, context))) = (fix, found) {
            m.set_gps(fix, getter, context);
        }
        let drive = get("drive").map(Drive::parse).transpose()?;
        let car_type = get("cartype").map(|t| t.parse::<u8>().map_err(|_| format!("{} is not a car type", t))).transpose()?;
        if model == crate::board::Model::Rnse193 {
            if let Some(t) = car_type.or(drive.as_ref().map(|_| 5)) {
                m.set_car_type(t)?;
            }
            m.drive = drive;
        }
        m.start_os(v)?;
        m.set_ignition(s.ignition);
        s.what = format!("{} software {}", v.name, v.version);
    }
    Ok(m)
}

/// Start (or restart) the machine by the settings in the text buffer:
/// `variant`, `loader` (1 for the update loader), `serial`, `gps`
/// (`LAT,LON[,COURSE]` or `none`), `drive`, `cartype`, and `nav` with
/// `navlen` for a navigation disc the page holds (its number for
/// `rnse_host_read` and its length in bytes).
#[no_mangle]
pub extern "C" fn rnse_boot(len: u32) -> i32 {
    let text = heard(len);
    let s = state();
    if s.variants.is_empty() {
        return fail("no update disc image yet");
    }
    s.settings = text.lines().filter_map(|l| l.split_once('=')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect();
    s.drive_disc = match (s.settings.get("nav").and_then(|v| v.parse::<u32>().ok()), s.settings.get("navlen").and_then(|v| v.parse::<u64>().ok())) {
        (Some(id), Some(bytes)) => Some(Disc::from_reader(
            bytes,
            // The page's file, read where the worker can do so at once.
            Arc::new(move |at, buf: &mut [u8]| unsafe { rnse_host_read(id, at as f64, buf.as_mut_ptr(), buf.len() as u32) as usize }),
        )),
        _ => Some(Disc::Mem(Arc::new(s.image.clone()))),
    };
    restart(s)
}

fn restart(s: &mut State) -> i32 {
    if let Some(m) = &s.machine {
        s.base_ms += m.cpu.cycles as f64 / m.cycles_per_ms() as f64;
    }
    match build(s) {
        Ok(m) => {
            s.machine = Some(m);
            s.stopped = None;
            s.log_sent = 0;
            s.rgb.clear();
            say("")
        }
        Err(e) => fail(e),
    }
}

#[no_mangle]
pub extern "C" fn rnse_run() -> i32 {
    let s = state();
    let Some(m) = s.machine.as_mut() else { return fail("not started") };
    if s.stopped.is_none() {
        let stop = m.run(m.frame_cycles);
        if stop != Stop::Budget {
            s.stopped = Some(match &m.cpu.fault {
                Some(f) if m.reset_requested => format!("the firmware asked for a restart ({})", f),
                Some(f) => f.clone(),
                None => format!("{:?}", stop),
            });
        }
    }
    s.stopped.is_some() as i32
}

#[no_mangle]
pub extern "C" fn rnse_render() -> i32 {
    let s = state();
    let Some(m) = s.machine.as_ref() else { return 0 };
    let (w, h, rgb) = m.render();
    if rgb == s.rgb && s.size == (w, h) {
        return 0;
    }
    s.rgba.clear();
    s.rgba.reserve(w * h * 4);
    for p in rgb.chunks_exact(3) {
        s.rgba.extend_from_slice(&[p[0], p[1], p[2], 255]);
    }
    s.rgb = rgb;
    s.size = (w, h);
    1
}

#[no_mangle]
pub extern "C" fn rnse_pixels() -> *const u8 {
    state().rgba.as_ptr()
}

#[no_mangle]
pub extern "C" fn rnse_width() -> u32 {
    state().size.0 as u32
}

#[no_mangle]
pub extern "C" fn rnse_height() -> u32 {
    state().size.1 as u32
}

#[no_mangle]
pub extern "C" fn rnse_ms() -> f64 {
    let s = state();
    s.base_ms + s.machine.as_ref().map_or(0.0, |m| m.cpu.cycles as f64 / m.cycles_per_ms() as f64)
}

/// An event from the page: `key NAME 0|1`, `knob NAME DELTA`,
/// `ignition 0|1`, `disc 0|1` or `reset`.
#[no_mangle]
pub extern "C" fn rnse_input(len: u32) -> i32 {
    let text = heard(len);
    let s = state();
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.first() == Some(&"reset") {
        s.note = "restarted".into();
        return restart(s);
    }
    let loader = s.settings.get("loader").map(String::as_str) == Some("1");
    let Some(m) = s.machine.as_mut() else { return fail("not started") };
    match words[..] {
        ["key", name, down] => {
            let delivered = m.panel_key(name, down == "1");
            if down == "1" {
                s.note = format!("{} pressed{}", name, if delivered { "" } else { " (this button is not wired to the main processor; nothing to emulate yet)" });
            }
        }
        ["knob", name, delta] => {
            let delta: i32 = delta.parse().unwrap_or(0);
            let delivered = m.panel_knob(name, delta);
            s.note = format!("{} knob turned {}{}", name, if delta > 0 { "right" } else { "left" }, if delivered { "" } else { " (unknown knob)" });
        }
        ["ignition", on] => {
            s.ignition = on == "1";
            if !loader {
                m.set_ignition(s.ignition);
            }
            s.note = format!("ignition {}", if s.ignition { "on" } else { "off" });
        }
        ["disc", present] => {
            s.disc_in = present == "1";
            let disc = if s.disc_in { s.drive_disc.clone() } else { None };
            m.atapi().insert_disc(disc);
            s.note = if s.disc_in { "disc inserted".into() } else { "disc removed".into() };
        }
        _ => return fail(format!("not an event: {}", text)),
    }
    0
}

#[no_mangle]
pub extern "C" fn rnse_log() -> i32 {
    let s = state();
    let Some(m) = s.machine.as_ref() else { return 0 };
    let new = &m.console[s.log_sent.min(m.console.len())..];
    let new = &new[..new.len().min(60000)];
    s.log_sent += new.len();
    s.text.clear();
    s.text.extend_from_slice(new);
    new.len() as i32
}

#[no_mangle]
pub extern "C" fn rnse_status() -> i32 {
    let s = state();
    let display = s.machine.as_ref().map_or(1023, |m| m.display_pos());
    let json = format!(
        "{{\"what\":{},\"seconds\":{:.2},\"ignition\":{},\"disc\":{},\"display\":{},\"stopped\":{},\"note\":{}}}",
        json_str(&s.what),
        rnse_ms() / 1000.0,
        s.ignition,
        s.disc_in,
        display,
        s.stopped.as_deref().map(json_str).unwrap_or_else(|| "null".into()),
        json_str(&s.note)
    );
    say(&json)
}
