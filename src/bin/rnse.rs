use rnse::fw;
use rnse::machine::Machine;
use rnse::sh4::cpu::Stop;

fn usage() -> ! {
    eprintln!(
        "usage:\n  rnse info <disc.iso | LOADING.KWI>\n  rnse extract <disc.iso | LOADING.KWI> <out-dir>\n  rnse loader <disc.iso> [options]     run the update loader (LDM)\n  rnse boot <disc.iso> [options]       install the firmware and start it\n  rnse ui <disc.iso> [ui options]      run it with the faceplate in a browser\n\nui options:\n  --port N           port to serve on (default 8080)\n  --bind ADDR        address to listen on (default 127.0.0.1)\n  --loader           run the update loader instead of the installed firmware\n  --no-ignition      start with the ignition off\n  --no-disc          start with an empty CD drive\n  --variant NAME     hardware variant (default V220)\n\noptions:\n  --variant NAME     hardware variant (default V220)\n  --cycles N         stop after N CPU cycles (default 200000000)\n  --io-log [N]       log device accesses (first N per address, default 4)\n  --onchip-log       include SH-4 on-chip registers in the log\n  --log-after N      start the device log only after N cycles\n  --gpio PORT=HEX    level on input port 2, 3 or 4 (experimental)\n  --screenshot FILE  write the display contents as a PNG when stopping\n  --save-flash FILE  write the flash contents to FILE when stopping\n  --trace-exc        log every exception and interrupt\n  --ignition         send the ignition lock status frame (271: key in, ignition on)\n  --can ID:HEX[@MS]  send a CAN frame every MS milliseconds (default 100),\n                     e.g. --can 635:6400@200\n  --trace-can        log CAN frames in both directions\n  --trace-serial     log the serial links (radio processor, faceplate)\n  --no-radio         leave the radio processor's link unanswered\n  --no-panel         leave the faceplate controller's link unanswered\n  --key KEY@MS[+HOLD]  press a faceplate key at MS milliseconds for HOLD ms\n                     (default 200).  KEY is RADIO, MEDIA, NAME, TEL, NAV,\n                     INFO, CAR, SETUP, RETURN, PREV, NEXT, SELECT, EJECT,\n                     SOFT_TL, SOFT_TR, SOFT_BL, SOFT_BR, CD_EJECT, or a\n                     key state bit number (0-47)\n  --knob K:N@MS      turn knob K (0 volume, 1 select) by N detents at MS\n                     milliseconds, positive = clockwise\n  --shot MS:FILE     also write a screenshot at MS milliseconds\n  --peek ADDR[,LEN]  hex dump of LEN bytes of memory (default 16) at the end\n  --trace-calls A:B  list the subroutines called between A and B ms\n  --trace-coral      dump the display lists sent to the graphics chip\n  --only-layer N     screenshot shows display layer N (0-5) alone\n  --surface A,S,W,H,FILE  save a 16 bpp off-screen surface at video memory\n                     address A (hex), S pixels per line, as a PNG\n  --trace-atapi      log CD drive commands\n  --trace-bios       log boot ROM service calls (boot only)\n  --flash FILE       boot from a flash image saved by `loader --save-flash`\n  --no-disc          start with an empty CD drive\n  --break ADDR       stop when PC reaches ADDR (hex)\n  --printf ADDR      address of the OS's printf; its messages are printed as\n                     \"[fw] ...\".  Known for V220 software 0815 and then on\n                     by default; --no-printf turns it off\n  --tp ADDR          print registers each time PC reaches ADDR, and continue\n  --args A,B,C       r4,r5,r6 passed to the loader (default 0,0,0)"
    );
    std::process::exit(2);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        usage();
    }
    let res = match args[0].as_str() {
        "info" if args.len() == 2 => info(&args[1]),
        "extract" if args.len() == 3 => extract(&args[1], &args[2]),
        "loader" if args.len() >= 2 => run(&args[1], &args[2..], true),
        "boot" if args.len() >= 2 => run(&args[1], &args[2..], false),
        "ui" if args.len() >= 2 => ui(&args[1], &args[2..]),
        _ => usage(),
    };
    if let Err(e) = res {
        eprintln!("rnse: {}", e);
        std::process::exit(1);
    }
}

fn info(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let image = std::fs::read(path)?;
    if let Ok(iso) = fw::Iso::new(&image) {
        println!("ISO 9660 volume \"{}\", {} files", iso.volume_label(), iso.walk().len());
    }
    for v in fw::load_variants(&image)? {
        println!("\nvariant {}  software {}", v.name, v.version);
        println!("  {:<5} {:<4} {:<9} {:<12} {:<9} {:>10} {:>9}  {:<8} entry points", "name", "kind", "version", "built", "", "load", "size", "checksum");
        for m in &v.modules {
            println!(
                "  {:<5} {:<4} {:<9} {:<12} {:<9} {:>#10x} {:>#9x}  {:<8} {:08x} {:08x} {:08x}",
                m.name,
                m.kind,
                m.version,
                m.build_date,
                m.build_time,
                m.load_addr,
                m.size,
                if m.checksum_ok() { "ok" } else { "BAD" },
                m.entries[0],
                m.entries[1],
                m.entries[2]
            );
        }
    }
    Ok(())
}

fn extract(path: &str, out: &str) -> Result<(), Box<dyn std::error::Error>> {
    let image = std::fs::read(path)?;
    for v in fw::load_variants(&image)? {
        let dir = std::path::Path::new(out).join(&v.name);
        std::fs::create_dir_all(&dir)?;
        for m in &v.modules {
            let p = dir.join(format!("{}.BIN", m.name));
            std::fs::write(&p, &m.data)?;
            println!("{}", p.display());
        }
    }
    Ok(())
}

struct Opts {
    variant: String,
    cycles: u64,
    io_log: Option<u32>,
    onchip_log: bool,
    trace_exc: bool,
    breaks: Vec<u32>,
    args: [u32; 3],
    screenshot: Option<String>,
    trace_atapi: bool,
    no_disc: bool,
    tracepoints: Vec<u32>,
    save_flash: Option<String>,
    flash: Option<String>,
    trace_bios: bool,
    log_after: u64,
    gpio: Vec<(usize, u8)>,
    printf_hook: Option<u32>,
    no_printf: bool,
    can: Vec<(rnse::dev::hcan::Frame, u64)>,
    trace_can: bool,
    trace_coral: bool,
    surfaces: Vec<(u32, usize, usize, usize, String)>,
    ignition: bool,
    trace_serial: bool,
    no_radio: bool,
    no_panel: bool,
    panel_script: Vec<(u64, rnse::machine::PanelAction)>,
    shots: Vec<(u64, String)>,
    call_windows: Vec<(u64, u64)>,
    peeks: Vec<(u32, u32)>,
    only_layer: Option<usize>,
}

fn parse_opts(args: &[String]) -> Result<Opts, Box<dyn std::error::Error>> {
    let mut o = Opts { variant: "V220".into(), cycles: 200_000_000, io_log: None, onchip_log: false, trace_exc: false, breaks: vec![], args: [0; 3], screenshot: None, trace_atapi: false, no_disc: false, tracepoints: vec![], save_flash: None, flash: None, trace_bios: false, log_after: 0, gpio: vec![], printf_hook: None, no_printf: false, can: vec![], trace_can: false, trace_coral: false, surfaces: vec![], ignition: false, trace_serial: false, no_radio: false, only_layer: None, no_panel: false, panel_script: vec![], shots: vec![], call_windows: vec![], peeks: vec![] };
    let mut i = 0;
    while i < args.len() {
        let next = |i: &mut usize| -> Result<&String, Box<dyn std::error::Error>> {
            *i += 1;
            args.get(*i).ok_or_else(|| format!("{} needs a value", args[*i - 1]).into())
        };
        match args[i].as_str() {
            "--variant" => o.variant = next(&mut i)?.to_uppercase(),
            "--cycles" => o.cycles = next(&mut i)?.replace('_', "").parse()?,
            "--io-log" => {
                o.io_log = Some(4);
                if let Some(n) = args.get(i + 1).and_then(|s| s.parse().ok()) {
                    o.io_log = Some(n);
                    i += 1;
                }
            }
            "--onchip-log" => o.onchip_log = true,
            "--gpio" => {
                let v = next(&mut i)?;
                let (p, val) = v.split_once('=').ok_or("--gpio wants PORT=HEX")?;
                o.gpio.push((p.parse()?, u8::from_str_radix(val, 16)?));
            }
            "--log-after" => o.log_after = next(&mut i)?.replace('_', "").parse()?,
            "--trace-atapi" => o.trace_atapi = true,
            "--no-disc" => o.no_disc = true,
            "--screenshot" => o.screenshot = Some(next(&mut i)?.clone()),
            "--flash" => o.flash = Some(next(&mut i)?.clone()),
            "--trace-bios" => o.trace_bios = true,
            "--save-flash" => o.save_flash = Some(next(&mut i)?.clone()),
            "--trace-exc" => o.trace_exc = true,
            "--printf" => o.printf_hook = Some(u32::from_str_radix(next(&mut i)?.trim_start_matches("0x"), 16)?),
            "--no-printf" => o.no_printf = true,
            "--trace-can" => o.trace_can = true,
            "--trace-coral" => o.trace_coral = true,
            "--trace-serial" => o.trace_serial = true,
            "--only-layer" => o.only_layer = Some(next(&mut i)?.parse()?),
            "--no-radio" => o.no_radio = true,
            "--no-panel" => o.no_panel = true,
            "--key" => {
                // KEY@MS[+HOLD_MS]: key name or raw state bit, pressed at MS.
                let v = next(&mut i)?;
                let (bit, when) = v.split_once('@').ok_or("--key wants BIT@MS[+HOLD_MS]")?;
                let (at, hold) = match when.split_once('+') {
                    Some((a, h)) => (a.parse()?, h.parse()?),
                    None => (when.parse::<u64>()?, 200u64),
                };
                let bit: usize = match rnse::dev::panel::key_bit(bit) {
                    Some(b) => b,
                    None => bit.parse().map_err(|_| format!("--key: {} is neither a key name nor a bit number", bit))?,
                };
                o.panel_script.push((at, rnse::machine::PanelAction::Key(bit, true)));
                o.panel_script.push((at + hold, rnse::machine::PanelAction::Key(bit, false)));
            }
            "--knob" => {
                // KNOB:DELTA@MS, knob 0 or 1.
                let v = next(&mut i)?;
                let (what, at) = v.split_once('@').ok_or("--knob wants KNOB:DELTA@MS")?;
                let (knob, delta) = what.split_once(':').ok_or("--knob wants KNOB:DELTA@MS")?;
                o.panel_script.push((at.parse()?, rnse::machine::PanelAction::Knob(knob.parse()?, delta.parse()?)));
            }
            "--peek" => {
                // ADDR[,LEN]: hex dump of memory when stopping.
                let v = next(&mut i)?;
                let (a, n) = v.split_once(',').unwrap_or((v.as_str(), "16"));
                o.peeks.push((u32::from_str_radix(a.trim_start_matches("0x"), 16)?, n.parse()?));
            }
            "--trace-calls" => {
                // FROM:TO in ms: list the subroutines called in that window.
                let v = next(&mut i)?;
                let (a, b) = v.split_once(':').ok_or("--trace-calls wants FROM_MS:TO_MS")?;
                o.call_windows.push((a.parse()?, b.parse()?));
            }
            "--shot" => {
                // MS:FILE, a screenshot on the way.
                let v = next(&mut i)?;
                let (at, file) = v.split_once(':').ok_or("--shot wants MS:FILE")?;
                o.shots.push((at.parse()?, file.to_string()));
            }
            "--surface" => {
                // ADDR,STRIDE,W,H,FILE
                let v: Vec<&str> = next(&mut i)?.split(',').collect();
                if v.len() != 5 {
                    return Err("--surface wants ADDR,STRIDE,W,H,FILE".into());
                }
                o.surfaces.push((u32::from_str_radix(v[0].trim_start_matches("0x"), 16)?, v[1].parse()?, v[2].parse()?, v[3].parse()?, v[4].to_string()));
            }
            "--ignition" => o.ignition = true,
            "--can" => {
                // ID:HEXBYTES[@PERIOD_MS]
                let v = next(&mut i)?;
                let (spec, period) = match v.split_once('@') {
                    Some((a, b)) => (a, b.parse()?),
                    None => (v.as_str(), 100),
                };
                let (id, hex) = spec.split_once(':').ok_or("--can wants ID:HEXBYTES[@MS]")?;
                let bytes: Vec<u8> = (0..hex.len() / 2).map(|k| u8::from_str_radix(&hex[k * 2..k * 2 + 2], 16)).collect::<Result<_, _>>()?;
                o.can.push((rnse::dev::hcan::Frame::new(u16::from_str_radix(id, 16)?, &bytes), period));
            }
            "--tp" => o.tracepoints.push(u32::from_str_radix(next(&mut i)?.trim_start_matches("0x"), 16)?),
            "--break" => o.breaks.push(u32::from_str_radix(next(&mut i)?.trim_start_matches("0x"), 16)?),
            "--args" => {
                let v: Vec<u32> = next(&mut i)?.split(',').map(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16)).collect::<Result<_, _>>()?;
                for (k, x) in v.iter().take(3).enumerate() {
                    o.args[k] = *x;
                }
            }
            other => return Err(format!("unknown option {}", other).into()),
        }
        i += 1;
    }
    Ok(o)
}

fn run(path: &str, rest: &[String], loader: bool) -> Result<(), Box<dyn std::error::Error>> {
    let o = parse_opts(rest)?;
    let image = std::fs::read(path)?;
    let variants = fw::load_variants(&image)?;
    let v = variants.iter().find(|v| v.name == o.variant).ok_or_else(|| format!("no variant {} on this disc", o.variant))?;
    let ldm = v.module("LDM").ok_or("variant has no LDM module")?;
    let model = rnse::board::Model::for_variant(&v.name).ok_or_else(|| format!("variant {} is for hardware this emulator does not know", v.name))?;
    if let Some(bad) = v.modules.iter().find(|m| !model.can_load(m.phys_addr(), m.size as usize)) {
        return Err(format!("module {} of variant {} loads at {:#x}, outside the {:?} memory map", bad.name, v.name, bad.load_addr, model).into());
    }
    let mut m = Machine::for_model(model);
    let enable_logs = |m: &mut Machine| {
        if let Some(n) = o.io_log {
            m.board.log = rnse::iolog::IoLog::new(true, n);
            m.board.coral.log = rnse::iolog::IoLog::new(true, n);
            m.atapi().log = rnse::iolog::IoLog::new(true, n);
            m.can().log = rnse::iolog::IoLog::new(true, n);
            if o.onchip_log {
                m.cpu.io.log = rnse::iolog::IoLog::new(true, n);
            }
            if let Some(navi) = m.cpu.io.navi.as_mut() {
                // On the 193 nearly all hardware is on-chip.
                navi.log = rnse::iolog::IoLog::new(true, n);
            }
        }
    };
    if o.log_after == 0 {
        enable_logs(&mut m);
    }
    for (p, v) in &o.gpio {
        m.board.gpio_in[p & 7] = *v;
    }
    m.cpu.trace_exceptions = o.trace_exc;
    m.cpu.breakpoints = o.breaks.clone();
    m.cpu.breakpoints.extend(o.tracepoints.iter().copied());
    let printf = o.printf_hook.or_else(|| {
        if o.no_printf {
            None
        } else {
            v.module(if loader { "LDM" } else { "OSM" }).and_then(|m| Machine::known_printf(&m.version))
        }
    });
    if let Some(a) = printf {
        m.hook_printf(a);
    }
    m.atapi().trace = o.trace_atapi;
    m.can().trace = o.trace_can;
    m.board.coral.trace = o.trace_coral;
    m.board.coral.only_layer = o.only_layer;
    m.trace_serial = o.trace_serial;
    m.radio.trace = o.trace_serial;
    m.radio_present = !o.no_radio;
    m.panel.trace = o.trace_serial;
    m.panel_present = !o.no_panel;
    m.panel_script = o.panel_script.clone();
    if !o.no_disc {
        m.atapi().load(image.clone());
    }
    m.install_boot_stub(&v.name);
    m.trace_bios = o.trace_bios;
    if loader {
        m.start_loader(ldm, o.args);
    } else {
        if let Some(path) = &o.flash {
            let img = std::fs::read(path)?;
            let n = img.len().min(m.board.flash.len());
            m.board.flash[..n].copy_from_slice(&img[..n]);
            m.install_boot_stub(&v.name);
        } else {
            let index = fw::IndexInfo::from_disc(&image, &v.name).unwrap_or_default();
            m.install(v, &index);
        }
        m.start_os(v)?;
    }
    if o.ignition {
        m.set_ignition(true);
    }
    for (frame, period) in &o.can {
        m.can_send_cyclic(frame.clone(), *period);
    }
    let end = m.cpu.cycles + o.cycles;
    let mut stop;
    if o.log_after > 0 {
        let stop = m.run(o.log_after.min(o.cycles));
        if stop == Stop::Budget {
            eprintln!("--- device log starts at cycle {}", m.cpu.cycles);
            enable_logs(&mut m);
        }
    }
    let mut shots = o.shots.clone();
    shots.sort();
    let per_ms = m.cycles_per_ms();
    // Times at which a call-trace window opens (true) or closes.
    let mut windows: Vec<(u64, bool)> = o.call_windows.iter().flat_map(|&(a, b)| [(a, true), (b, false)]).collect();
    windows.sort();
    loop {
        let mut target = shots.first().map_or(end, |(ms, _)| (ms * per_ms).min(end));
        if let Some((ms, _)) = windows.first() {
            target = target.min(ms * per_ms);
        }
        stop = m.run(target.saturating_sub(m.cpu.cycles).max(1));
        while windows.first().is_some_and(|(ms, _)| ms * per_ms <= m.cpu.cycles) {
            let (ms, open) = windows.remove(0);
            if open {
                m.cpu.calls = Some(Default::default());
            } else if let Some(calls) = m.cpu.calls.take() {
                let mut list: Vec<_> = calls.into_iter().collect();
                list.sort_by_key(|(_, (_, first, _))| *first);
                eprintln!("--- calls up to {} ms: {} subroutines", ms, list.len());
                for (target, (n, first, caller)) in list {
                    eprintln!("[call] {:08x} x{:<6} first at {} ms from {:08x}", target, n, first / per_ms, caller);
                }
            }
        }
        while shots.first().is_some_and(|(ms, _)| ms * per_ms <= m.cpu.cycles) {
            let (ms, path) = shots.remove(0);
            let (w, h, rgb) = m.render();
            std::fs::write(&path, rnse::png::encode_rgb(w, h, &rgb))?;
            eprintln!("screenshot at {} ms: {}", ms, path);
        }
        if stop == Stop::Budget && m.cpu.cycles < end {
            continue;
        }
        if stop == Stop::Breakpoint && o.tracepoints.contains(&m.cpu.pc) {
            let c = &m.cpu;
            let mut line = format!("[tp] {:08x} pr={:08x}", c.pc, c.pr);
            for i in 0..16 {
                line += &format!(" r{}={:08x}", i, c.r[i]);
            }
            eprintln!("{}", line);
            for i in [4usize, 5, 6] {
                if let Some(t) = m.peek_str(c.r[i], 48) {
                    eprintln!("[tp]    r{} -> {:?}", i, t);
                }
            }
            // First stack argument (the firmware's printf takes its
            // format string there).
            if let Some(t) = m.peek32(c.r[15]).and_then(|p| m.peek_str(p, 96)) {
                eprintln!("[tp]    [sp] -> {:?}", t);
            }
            // Step off the tracepoint and carry on.
            let saved = std::mem::take(&mut m.cpu.breakpoints);
            m.run(1);
            m.cpu.breakpoints = saved;
            if m.cpu.cycles < end {
                continue;
            }
        }
        break;
    }
    eprintln!("\n--- stopped: {:?} after {} cycles", stop, m.cpu.cycles);
    if let Some(f) = &m.cpu.fault {
        eprintln!("fault: {}", f);
    }
    eprint!("{}", m.cpu.dump());
    eprint!("{}", m.describe_display());
    {
        let io = &m.cpu.io;
        eprintln!(
            "intc: ICR={:04x} IPRA={:04x} IPRB={:04x} IPRC={:04x} IPRD={:04x} pending={:08x} irl={} level={}",
            io.intc.icr, io.intc.ipra, io.intc.iprb, io.intc.iprc, io.intc.iprd, io.intc.pending, io.intc.irl, io.intc.level
        );
        for (i, t) in io.tmu.iter().enumerate() {
            eprintln!("tmu{}: running={} TCOR={:08x} TCNT={:08x} TCR={:04x}", i, t.running, t.tcor, t.tcnt, t.tcr);
        }
    }
    let irqs: Vec<String> = m.cpu.irq_counts.iter().enumerate().filter(|(_, &n)| n > 0).map(|(i, n)| format!("{:03x}:{}", i * 0x20, n)).collect();
    eprintln!("interrupts taken (INTEVT:count): {}", irqs.join(" "));
    {
        let can = &*m.can();
        let boxes: Vec<String> = can.mailboxes().iter().filter(|(_, id, _)| *id != 0).map(|(n, id, mode)| format!("{}:{:03x}{}", n, id, match mode { 0 => "t", 2 | 3 => "r", 7 => "-", _ => "?" })).collect();
        eprintln!("can: {} frames sent by the unit, {} delivered to it ({} matched no mailbox); mailboxes {}", can.tx_count, can.rx_count, can.rx_dropped, boxes.join(" "));
        if o.trace_can {
            eprint!("{}", can.describe());
        }
        let mut seen = std::collections::BTreeMap::new();
        for f in &can.tx {
            *seen.entry(f.id).or_insert((0u32, f.clone())) = (seen.get(&f.id).map(|x: &(u32, rnse::dev::hcan::Frame)| x.0).unwrap_or(0) + 1, f.clone());
        }
        for (_, (n, f)) in seen {
            eprintln!("  tx x{:<5} last {}", n, f);
        }
    }
    for (i, u) in m.board.uart.iter().enumerate() {
        if u.tx_total > 0 || u.rx_total > 0 {
            let shown: Vec<String> = u.tx.iter().take(48).map(|b| format!("{:02x}", b)).collect();
            eprintln!("serial channel {}: {} bytes sent, {} received; first unsent-to-host bytes: {}{}", i, u.tx_total, u.rx_total, shown.join(" "), if u.tx.len() > 48 { " ..." } else { "" });
        }
    }
    if !m.cpu.io.sci_tx.is_empty() {
        eprintln!("SCI: {} bytes sent", m.cpu.io.sci_tx.len());
    }
    if !m.bios_calls.is_empty() {
        eprintln!("boot ROM services called:");
        for ((n, code), count) in &m.bios_calls {
            eprintln!("  trap {:02x} fn {:04x}: {} calls", n, code, count);
        }
    }
    if let Some(path) = &o.save_flash {
        std::fs::write(path, &m.board.flash)?;
        eprintln!("flash image: {} ({} sectors erased, {} words programmed)", path, m.board.flash_ctl.erased_sectors, m.board.flash_ctl.programmed_words);
    }
    for (addr, stride, w, h, path) in &o.surfaces {
        let rgb = m.board.coral.render_surface(*addr, *stride, *w, *h);
        std::fs::write(path, rnse::png::encode_rgb(*w, *h, &rgb))?;
        eprintln!("surface {:x}: {}", addr, path);
    }
    for (addr, len) in &o.peeks {
        let bytes: Vec<String> = (0..*len).map(|k| m.peek8(addr + k).map_or("??".into(), |b| format!("{:02x}", b))).collect();
        eprintln!("peek {:08x}: {}", addr, bytes.join(" "));
    }
    if let Some(path) = &o.screenshot {
        let (w, h, rgb) = m.render();
        std::fs::write(path, rnse::png::encode_rgb(w, h, &rgb))?;
        eprintln!("screenshot: {} ({}x{})", path, w, h);
    }
    let h = m.cpu.history();
    eprintln!("last PCs: {}", h[h.len() - 24..].iter().map(|p| format!("{:08x}", p)).collect::<Vec<_>>().join(" "));
    Ok(())
}

fn json_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out += "\\\"",
            '\\' => out += "\\\\",
            '\n' => out += "\\n",
            c if (c as u32) < 0x20 => out += " ",
            c => out.push(c),
        }
    }
    out + "\""
}

/// Run the machine behind the browser front end.
fn ui(path: &str, rest: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    use rnse::ui::{encode_frame, serve, Shared, UiEvent};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    let (mut port, mut bind, mut loader, mut ignition, mut disc, mut variant) = (8080u16, "127.0.0.1".to_string(), false, true, true, "V220".to_string());
    let mut i = 0;
    while i < rest.len() {
        let opt = rest[i].as_str();
        let mut value = || -> Result<String, Box<dyn std::error::Error>> {
            i += 1;
            rest.get(i).cloned().ok_or_else(|| format!("{} needs a value", opt).into())
        };
        match opt {
            "--port" => port = value()?.parse()?,
            "--bind" => bind = value()?,
            "--variant" => variant = value()?.to_uppercase(),
            "--loader" => loader = true,
            "--no-ignition" => ignition = false,
            "--no-disc" => disc = false,
            other => return Err(format!("unknown option {}", other).into()),
        }
        i += 1;
    }

    let image = std::fs::read(path)?;
    let variants = fw::load_variants(&image)?;
    let v = variants.iter().find(|v| v.name == variant).ok_or_else(|| format!("no variant {} on this disc", variant))?;
    let model = rnse::board::Model::for_variant(&v.name).ok_or_else(|| format!("variant {} is for hardware this emulator does not know", v.name))?;
    if model != rnse::board::Model::Rnse192 {
        return Err(format!("variant {} is for the 193 hardware, which cannot run in the browser front end yet; try `rnse loader --variant {}`", v.name, v.name).into());
    }
    let what = if loader { format!("{} update loader", v.name) } else { format!("{} software {}", v.name, v.version) };

    let build = |ignition: bool, disc: bool| -> Result<Machine, Box<dyn std::error::Error>> {
        let mut m = Machine::new();
        m.echo_console = false;
        m.install_boot_stub(&v.name);
        if disc {
            m.atapi().load(image.clone());
        }
        if loader {
            m.start_loader(v.module("LDM").ok_or("variant has no LDM module")?, [0; 3]);
        } else {
            let index = fw::IndexInfo::from_disc(&image, &v.name).unwrap_or_default();
            m.install(v, &index);
            if let Some(a) = v.module("OSM").and_then(|os| Machine::known_printf(&os.version)) {
                m.hook_printf(a);
            }
            m.start_os(v)?;
            m.set_ignition(ignition);
        }
        Ok(m)
    };

    let shared = Arc::new(Mutex::new(Shared::default()));
    let addr = serve(&format!("{}:{}", bind, port), shared.clone())?;
    println!("rnse: {} running; open http://{}/ in a browser (Ctrl-C to quit)", what, addr);

    let mut m = build(ignition, disc)?;
    let mut stopped: Option<String> = None;
    let mut last_rgb: Vec<u8> = Vec::new();
    let mut seq = 0u32;
    let mut log_sent = 0usize;
    let mut note = String::new();
    let mut disc_in = disc;
    let (mut wall, mut cycles_mark, mut speed) = (Instant::now(), 0u64, 0.0f64);
    let start = Instant::now();
    // Emulated time is `emu_base_ms` (machines before the last restart)
    // plus the current machine's clock.  It is held to wall-clock time
    // minus `paused_ms`, the time spent stopped or given up as lag.
    let (mut emu_base_ms, mut paused_ms) = (0u64, 0u64);
    loop {
        // Input from the page.
        let events: Vec<UiEvent> = shared.lock().unwrap().events.drain(..).collect();
        for ev in events {
            match ev {
                UiEvent::Key { name, down } => {
                    let delivered = m.panel_key(&name, down);
                    if down {
                        note = format!("{} pressed{}", name, if delivered { "" } else { " (this button is not wired to the main processor; nothing to emulate yet)" });
                    }
                }
                UiEvent::Knob { name, delta } => {
                    let delivered = m.panel_knob(&name, delta);
                    note = format!("{} knob turned {}{}", name, if delta > 0 { "right" } else { "left" }, if delivered { "" } else { " (unknown knob)" });
                }
                UiEvent::Ignition(on) => {
                    ignition = on;
                    if !loader {
                        m.set_ignition(on);
                    }
                    note = format!("ignition {}", if on { "on" } else { "off" });
                }
                UiEvent::Disc(present) => {
                    disc_in = present;
                    m.atapi().insert(if present { Some(image.clone()) } else { None });
                    note = if present { "disc inserted".into() } else { "disc removed".into() };
                }
                UiEvent::Reset => {
                    emu_base_ms += m.cpu.cycles / m.cycles_per_ms();
                    m = build(ignition, disc_in)?;
                    stopped = None;
                    log_sent = 0;
                    note = "restarted".into();
                    shared.lock().unwrap().push_log(b"\n--- restart ---\n");
                }
            }
        }

        // One display frame of emulated time.
        let frame_cycles = m.frame_cycles;
        if stopped.is_none() {
            let stop = m.run(frame_cycles);
            if stop != Stop::Budget {
                stopped = Some(match &m.cpu.fault {
                    Some(f) if m.reset_requested => format!("the firmware asked for a restart ({})", f),
                    Some(f) => f.clone(),
                    None => format!("{:?}", stop),
                });
            }
        }

        // Publish.
        let (w, h, rgb) = m.render();
        {
            let mut s = shared.lock().unwrap();
            if rgb != last_rgb || s.frame.is_empty() {
                seq = seq.wrapping_add(1);
                s.frame = encode_frame(w, h, &rgb, seq);
                s.frame_seq = seq;
                last_rgb = rgb;
            }
            if m.console.len() > log_sent {
                s.push_log(&m.console[log_sent..]);
                log_sent = m.console.len();
            }
            s.status = format!(
                "{{\"what\":{},\"seconds\":{:.2},\"speed\":{:.0},\"ignition\":{},\"disc\":{},\"display\":{},\"stopped\":{},\"note\":{}}}",
                json_str(&what),
                m.cpu.cycles as f64 / m.cycles_per_ms() as f64 / 1000.0,
                speed,
                ignition,
                disc_in,
                m.board.display_pos,
                stopped.as_deref().map(json_str).unwrap_or_else(|| "null".into()),
                json_str(&note)
            );
        }

        // Keep to real time, and measure how close we get.
        let emulated_ms_total = emu_base_ms + m.cpu.cycles / m.cycles_per_ms();
        let target = Duration::from_millis(emulated_ms_total);
        let elapsed = start.elapsed() - Duration::from_millis(paused_ms);
        if stopped.is_some() {
            std::thread::sleep(Duration::from_millis(30));
            paused_ms += 30;
        } else if target > elapsed {
            std::thread::sleep(target - elapsed);
        } else if elapsed - target > Duration::from_millis(500) {
            // Too slow to keep up: do not try to catch up later.
            paused_ms += (elapsed - target).as_millis() as u64;
        }
        if wall.elapsed() >= Duration::from_secs(1) {
            let ms = (m.cpu.cycles.saturating_sub(cycles_mark)) / m.cycles_per_ms();
            speed = ms as f64 * 100.0 / wall.elapsed().as_millis().max(1) as f64;
            wall = Instant::now();
            cycles_mark = m.cpu.cycles;
        }
    }
}
