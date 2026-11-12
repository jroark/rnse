use rnse::fw;
use rnse::machine::Machine;
use rnse::sh4::cpu::Stop;

fn usage() -> ! {
    eprintln!(
        "usage:\n  rnse info <disc.iso | LOADING.KWI>\n  rnse extract <disc.iso | LOADING.KWI> <out-dir>\n  rnse loader <disc.iso> [options]     run the update loader (LDM)\n  rnse boot <disc.iso> [options]       install the firmware and start it\n\noptions:\n  --variant NAME     hardware variant (default V220)\n  --cycles N         stop after N CPU cycles (default 200000000)\n  --io-log [N]       log device accesses (first N per address, default 4)\n  --onchip-log       include SH-4 on-chip registers in the log\n  --log-after N      start the device log only after N cycles\n  --gpio PORT=HEX    level on input port 2, 3 or 4 (experimental)\n  --screenshot FILE  write the display contents as a PNG when stopping\n  --save-flash FILE  write the flash contents to FILE when stopping\n  --trace-exc        log every exception and interrupt\n  --ignition         send the ignition lock status frame (271: key in, ignition on)\n  --can ID:HEX[@MS]  send a CAN frame every MS milliseconds (default 100),\n                     e.g. --can 635:6400@200\n  --trace-can        log CAN frames in both directions\n  --trace-coral      dump the display lists sent to the graphics chip\n  --surface A,S,W,H,FILE  save a 16 bpp off-screen surface at video memory\n                     address A (hex), S pixels per line, as a PNG\n  --trace-atapi      log CD drive commands\n  --trace-bios       log boot ROM service calls (boot only)\n  --flash FILE       boot from a flash image saved by `loader --save-flash`\n  --no-disc          start with an empty CD drive\n  --break ADDR       stop when PC reaches ADDR (hex)\n  --printf ADDR      address of the OS's printf; its messages are printed as\n                     \"[fw] ...\".  Known for V220 software 0815 and then on\n                     by default; --no-printf turns it off\n  --tp ADDR          print registers each time PC reaches ADDR, and continue\n  --args A,B,C       r4,r5,r6 passed to the loader (default 0,0,0)"
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
}

fn parse_opts(args: &[String]) -> Result<Opts, Box<dyn std::error::Error>> {
    let mut o = Opts { variant: "V220".into(), cycles: 200_000_000, io_log: None, onchip_log: false, trace_exc: false, breaks: vec![], args: [0; 3], screenshot: None, trace_atapi: false, no_disc: false, tracepoints: vec![], save_flash: None, flash: None, trace_bios: false, log_after: 0, gpio: vec![], printf_hook: None, no_printf: false, can: vec![], trace_can: false, trace_coral: false, surfaces: vec![] };
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
            "--surface" => {
                // ADDR,STRIDE,W,H,FILE
                let v: Vec<&str> = next(&mut i)?.split(',').collect();
                if v.len() != 5 {
                    return Err("--surface wants ADDR,STRIDE,W,H,FILE".into());
                }
                o.surfaces.push((u32::from_str_radix(v[0].trim_start_matches("0x"), 16)?, v[1].parse()?, v[2].parse()?, v[3].parse()?, v[4].to_string()));
            }
            "--ignition" => {
                // Ignition lock status: key in (bit 0) and ignition on (bit 1).
                o.can.push((rnse::dev::hcan::Frame::new(0x271, &[0x03]), 100));
            }
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
    if let Some(bad) = v.modules.iter().find(|m| !rnse::board::Board::can_load(m.phys_addr(), m.size as usize)) {
        return Err(format!(
            "variant {} is built for a different memory map (module {} loads at {:#x}); only the V220 hardware is emulated so far",
            v.name, bad.name, bad.load_addr
        )
        .into());
    }
    let mut m = Machine::new();
    let enable_logs = |m: &mut Machine| {
        if let Some(n) = o.io_log {
            m.board.log = rnse::iolog::IoLog::new(true, n);
            m.board.coral.log = rnse::iolog::IoLog::new(true, n);
            m.board.atapi.log = rnse::iolog::IoLog::new(true, n);
            m.board.can.log = rnse::iolog::IoLog::new(true, n);
            if o.onchip_log {
                m.cpu.io.log = rnse::iolog::IoLog::new(true, n);
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
        if loader || o.no_printf {
            None
        } else {
            v.module("OSM").and_then(|os| Machine::known_printf(&os.version))
        }
    });
    if let Some(a) = printf {
        m.hook_printf(a);
    }
    m.board.atapi.trace = o.trace_atapi;
    m.board.can.trace = o.trace_can;
    m.board.coral.trace = o.trace_coral;
    if !o.no_disc {
        m.board.atapi.load(image.clone());
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
    loop {
        stop = m.run(end.saturating_sub(m.cpu.cycles).max(1));
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
    eprint!("{}", m.board.coral.describe());
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
        let can = &m.board.can;
        let boxes: Vec<String> = can.mailboxes().iter().filter(|(_, id, _)| *id != 0).map(|(n, id, mode)| format!("{}:{:03x}{}", n, id, match mode { 0 => "t", 2 | 3 => "r", 7 => "-", _ => "?" })).collect();
        eprintln!("can: {} frames sent by the unit, {} delivered to it ({} matched no mailbox); mailboxes {}", can.tx_count, can.rx_count, can.rx_dropped, boxes.join(" "));
        let mut seen = std::collections::BTreeMap::new();
        for f in &can.tx {
            *seen.entry(f.id).or_insert((0u32, f.clone())) = (seen.get(&f.id).map(|x: &(u32, rnse::dev::hcan::Frame)| x.0).unwrap_or(0) + 1, f.clone());
        }
        for (_, (n, f)) in seen {
            eprintln!("  tx x{:<5} last {}", n, f);
        }
    }
    for (i, tx) in m.board.uart_tx.iter().enumerate() {
        if !tx.is_empty() {
            let shown: Vec<String> = tx.iter().take(24).map(|b| format!("{:02x}", b)).collect();
            eprintln!("serial channel {}: {} bytes sent: {}{}", i, tx.len(), shown.join(" "), if tx.len() > 24 { " ..." } else { "" });
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
    if let Some(path) = &o.screenshot {
        let (w, h, rgb) = m.board.coral.render();
        std::fs::write(path, rnse::png::encode_rgb(w, h, &rgb))?;
        eprintln!("screenshot: {} ({}x{})", path, w, h);
    }
    let h = m.cpu.history();
    eprintln!("last PCs: {}", h[h.len() - 24..].iter().map(|p| format!("{:08x}", p)).collect::<Vec<_>>().join(" "));
    Ok(())
}
