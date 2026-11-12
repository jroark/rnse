//! End-to-end tests that need a real update disc image.
//!
//! The firmware is not redistributable, so these only run when the
//! environment variable `RNSE_DISC` points at an ISO image of an update
//! CD:
//!
//!     RNSE_DISC=/path/to/disc.iso cargo test --release -- --nocapture

use rnse::fw;
use rnse::machine::Machine;
use rnse::sh4::cpu::Stop;

fn disc() -> Option<Vec<u8>> {
    let path = std::env::var_os("RNSE_DISC")?;
    Some(std::fs::read(path).expect("RNSE_DISC is not readable"))
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
    m.board.atapi.load(image.clone());
    m.start_loader(ldm, [0; 3]);
    let stop = m.run(8_000_000_000);
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
    assert_eq!(&m.board.flash[0xc000..0xc100], &direct.board.flash[0xc000..0xc100]);
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
    m.board.atapi.load(image.clone());
    m.start_os(v).unwrap();
    let stop = m.run(400_000_000);
    assert_eq!(stop, Stop::Budget, "{:?}", m.cpu.fault);
    assert!(m.cpu.irq_counts[0x400 >> 5] > 100, "system tick is not running");
}
