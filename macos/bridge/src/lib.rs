use rnse::{
    board::Model,
    dev::{
        atapi::{AudioDisc, Disc},
        sdhi::Card,
    },
    fw,
    machine::Machine,
    sh4::cpu::Stop,
};
use std::{
    ffi::{c_char, CStr},
    panic::{catch_unwind, AssertUnwindSafe},
};

pub struct Session {
    image: Vec<u8>,
    variants: Vec<fw::Variant>,
    machine: Option<Machine>,
    buffers: [Vec<u8>; 5],
    width: u32,
    height: u32,
    log_at: usize,
    pressed: std::collections::BTreeMap<String, u64>,
    releases: std::collections::BTreeMap<String, u64>,
}
impl Default for Session {
    fn default() -> Self {
        Self {
            image: vec![],
            variants: vec![],
            machine: None,
            buffers: Default::default(),
            width: 0,
            height: 0,
            log_at: 0,
            pressed: Default::default(),
            releases: Default::default(),
        }
    }
}
impl Session {
    fn command(&mut self, cmd: &str, value: &str) -> Result<(), String> {
        match cmd {
            "firmware" => {
                let image = std::fs::read(value).map_err(|e| e.to_string())?;
                let variants = fw::load_variants(&image).map_err(|e| e.to_string())?;
                let listing = variants
                    .iter()
                    .filter_map(|v| {
                        Model::for_variant(&v.name).map(|m| {
                            format!(
                                "{}\t{}\t{:?}\t{}",
                                v.name,
                                v.version,
                                m,
                                if v.module("OSM").and_then(Machine::find_gps_getter).is_some() {
                                    "GPS"
                                } else {
                                    ""
                                }
                            )
                        })
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if listing.is_empty() {
                    return Err("No supported firmware variants".into());
                }
                self.image = image;
                self.variants = variants;
                self.buffers[0] = listing.into_bytes();
            }
            "boot" => {
                // variant, serial, GPS enabled, latitude, longitude, course.
                let args: Vec<_> = value.split('\n').collect();
                let v = self
                    .variants
                    .iter()
                    .find(|v| v.name == args[0])
                    .ok_or("Choose a firmware variant")?;
                let mut m =
                    Machine::for_model(Model::for_variant(&v.name).ok_or("Unsupported hardware")?);
                m.echo_console = false;
                m.install_boot_stub(&v.name);
                m.install(
                    v,
                    &fw::IndexInfo::from_disc(&self.image, &v.name).unwrap_or_default(),
                );
                m.hook_debug_output(v, false);
                if let Some(serial) = args.get(1).filter(|s| !s.is_empty()) {
                    m.set_serial(serial)?;
                }
                if args.get(2) == Some(&"1") {
                    let (getter, context) = v
                        .module("OSM")
                        .and_then(Machine::find_gps_getter)
                        .ok_or("GPS is not supported by this firmware")?;
                    let fix = rnse::dev::gps::Gps::parse(
                        &args.iter().skip(3).copied().collect::<Vec<_>>().join(","),
                    )?;
                    m.set_gps(fix, getter, context);
                }
                m.atapi().load_disc(self.image.clone().into());
                m.start_os(v)?;
                m.set_ignition(true);
                m.speaker = Some(vec![]);
                self.machine = Some(m);
                self.log_at = 0;
                self.pressed.clear();
                self.releases.clear();
                for b in &mut self.buffers[1..] {
                    b.clear();
                }
            }
            "stop" => {
                self.machine = None;
            }
            _ => {
                let m = self.machine.as_mut().ok_or("Boot the emulator first")?;
                match cmd {
                    "key" => {
                        let (name, state) = value.split_once(':').ok_or("Invalid key")?;
                        if rnse::dev::panel::key_bit(name).is_none() {
                            return Err("Unsupported key".into());
                        }
                        if state == "1" {
                            self.releases.remove(name);
                            self.pressed.insert(name.into(), m.cpu.cycles);
                            m.panel_key(name, true);
                        } else {
                            let start = self.pressed.remove(name).unwrap_or(0);
                            self.releases
                                .insert(name.into(), start + 200 * m.cycles_per_ms());
                        }
                    }
                    "knob" => {
                        let (name, delta) = value.split_once(':').ok_or("Invalid knob")?;
                        if !m.panel_knob(name, delta.parse().map_err(|_| "Invalid detents")?) {
                            return Err("Unknown knob".into());
                        }
                    }
                    "ignition" => m.set_ignition(value == "1"),
                    "disc" if value.is_empty() => m.atapi().insert_disc(None),
                    "disc" => {
                        let disc = Disc::open(value).map_err(|e| e.to_string())?;
                        m.atapi().insert_disc(Some(disc));
                    }
                    "eject" => m.atapi().insert_disc(None),
                    "audio" => {
                        let tracks = value
                            .lines()
                            .map(|p| {
                                std::fs::read(p)
                                    .map_err(|e| e.to_string())
                                    .and_then(|b| rnse::wav::decode_cd(&b))
                            })
                            .collect::<Result<Vec<_>, _>>()?;
                        if tracks.is_empty() {
                            return Err("Choose at least one WAV track".into());
                        }
                        m.atapi()
                            .insert_audio(AudioDisc::from_tracks(&tracks), true);
                    }
                    "sd0" | "sd1" => {
                        let slot = usize::from(cmd == "sd1");
                        let card = Card::open(value).map_err(|e| e.to_string())?;
                        if !m.insert_card(slot, card) {
                            return Err("This model has no second SD slot".into());
                        }
                    }
                    "eject0" => m.eject_card(0),
                    "eject1" => m.eject_card(1),
                    _ => return Err("Unknown command".into()),
                }
            }
        }
        Ok(())
    }
    fn step(&mut self) -> Result<(), String> {
        let m = self.machine.as_mut().ok_or("No running machine")?;
        // At most 5 ms per call so stop and input remain responsive.
        let stop = m.run(m.cycles_per_ms() * 5);
        let due: Vec<_> = self
            .releases
            .iter()
            .filter(|(_, at)| **at <= m.cpu.cycles)
            .map(|(k, _)| k.clone())
            .collect();
        for key in due {
            m.panel_key(&key, false);
            self.releases.remove(&key);
        }
        let (w, h, rgb) = m.render();
        self.width = w as u32;
        self.height = h as u32;
        self.buffers[1] = rgb;
        self.buffers[2] = m
            .speaker
            .as_mut()
            .unwrap()
            .drain(..)
            .flat_map(i16::to_le_bytes)
            .collect();
        self.buffers[3] = m.console[self.log_at..].to_vec();
        m.console.clear();
        self.log_at = 0;
        self.buffers[4] = format!(
            "{:.2} s · {:?}",
            m.cpu.cycles as f64 / m.cycles_per_ms() as f64 / 1000.,
            m.board.model
        )
        .into_bytes();
        if stop != Stop::Budget {
            return Err(m
                .cpu
                .fault
                .clone()
                .unwrap_or_else(|| format!("Firmware stopped: {stop:?}")));
        }
        Ok(())
    }
}
fn guard(s: &mut Session, f: impl FnOnce(&mut Session) -> Result<(), String>) -> i32 {
    match catch_unwind(AssertUnwindSafe(|| f(s))) {
        Ok(Ok(())) => 0,
        other => {
            s.buffers[0] = match other {
                Ok(Err(e)) => e,
                _ => "Emulator panic; reset the session".into(),
            }
            .into_bytes();
            -1
        }
    }
}
#[no_mangle]
pub extern "C" fn rnse_create() -> *mut Session {
    Box::into_raw(Box::new(Session::default()))
}
#[no_mangle]
pub unsafe extern "C" fn rnse_destroy(s: *mut Session) {
    if !s.is_null() {
        drop(Box::from_raw(s));
    }
}
#[no_mangle]
pub unsafe extern "C" fn rnse_command(s: *mut Session, c: *const c_char, v: *const c_char) -> i32 {
    if s.is_null() || c.is_null() || v.is_null() {
        return -1;
    }
    let s = &mut *s;
    guard(s, |s| {
        let c = CStr::from_ptr(c).to_str().map_err(|_| "Invalid UTF-8")?;
        let v = CStr::from_ptr(v).to_str().map_err(|_| "Invalid UTF-8")?;
        s.command(c, v)
    })
}
#[no_mangle]
pub unsafe extern "C" fn rnse_step(s: *mut Session) -> i32 {
    if s.is_null() {
        return -1;
    }
    guard(&mut *s, Session::step)
}
#[no_mangle]
pub unsafe extern "C" fn rnse_buffer(s: *mut Session, k: u32, len: *mut usize) -> *const u8 {
    if s.is_null() || len.is_null() {
        return std::ptr::null();
    }
    match (*s).buffers.get(k as usize) {
        Some(b) => {
            *len = b.len();
            b.as_ptr()
        }
        None => {
            *len = 0;
            std::ptr::null()
        }
    }
}
#[no_mangle]
pub unsafe extern "C" fn rnse_width(s: *mut Session) -> u32 {
    (*s).width
}
#[no_mangle]
pub unsafe extern "C" fn rnse_height(s: *mut Session) -> u32 {
    (*s).height
}
#[no_mangle]
pub unsafe extern "C" fn rnse_time(s: *mut Session) -> f64 {
    (*s).machine
        .as_ref()
        .map(|m| m.cpu.cycles as f64 / m.cycles_per_ms() as f64 / 1000.)
        .unwrap_or(0.)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_operations_are_errors() {
        let mut s = Session::default();
        assert!(s.command("firmware", "/does/not/exist").is_err());
        assert!(s.command("boot", "V220").is_err());
        assert!(s.command("key", "RADIO:1").is_err());
        assert!(s.step().is_err());
    }
    #[test]
    fn quick_clicks_have_an_emulated_hold() {
        let mut s = Session::default();
        s.machine = Some(Machine::new());
        s.command("key", "SETUP:1").unwrap();
        s.command("key", "SETUP:0").unwrap();
        assert_eq!(
            s.releases["SETUP"],
            200 * s.machine.as_ref().unwrap().cycles_per_ms()
        );
        s.command("key", "SETUP:1").unwrap();
        assert!(!s.releases.contains_key("SETUP"));
        assert!(s.command("key", "INVALID:1").is_err());
        assert!(s.command("disc", "/missing/image").is_err());
        assert!(s.command("audio", "/missing/track").is_err());
        assert!(s.command("sd0", "/missing/card").is_err());
        s.command("disc", "").unwrap();
    }
    #[test]
    fn panic_is_contained() {
        let mut s = Session::default();
        assert_eq!(guard(&mut s, |_| panic!("test")), -1);
        assert!(!s.buffers[0].is_empty());
    }
    #[test]
    fn abi_lifecycle() {
        unsafe {
            let s = rnse_create();
            assert_eq!(rnse_step(s), -1);
            let mut len = 0;
            let p = rnse_buffer(s, 0, &mut len);
            assert!(!p.is_null());
            assert!(len > 0);
            rnse_destroy(s);
        }
    }
}
