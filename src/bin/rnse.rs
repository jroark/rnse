use rnse::fw;
use rnse::machine::Machine;
use rnse::sh4::cpu::Stop;

fn usage() -> ! {
    eprintln!(
        "usage:\n  rnse info <disc.iso | LOADING.KWI>\n  rnse extract <disc.iso | LOADING.KWI> <out-dir>\n  rnse loader <disc.iso> [options]     run the update loader (LDM)\n\noptions:\n  --variant NAME     hardware variant (default V220)\n  --cycles N         stop after N CPU cycles (default 200000000)\n  --io-log [N]       log device accesses (first N per address, default 4)\n  --onchip-log       include SH-4 on-chip registers in the log\n  --screenshot FILE  write the display contents as a PNG when stopping\n  --trace-exc        log every exception and interrupt\n  --trace-atapi      log CD drive commands\n  --no-disc          start with an empty CD drive\n  --break ADDR       stop when PC reaches ADDR (hex)\n  --tp ADDR          print registers each time PC reaches ADDR, and continue\n  --args A,B,C       r4,r5,r6 passed to the loader (default 0,0,0)"
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
        "loader" if args.len() >= 2 => loader(&args[1], &args[2..]),
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
}

fn parse_opts(args: &[String]) -> Result<Opts, Box<dyn std::error::Error>> {
    let mut o = Opts { variant: "V220".into(), cycles: 200_000_000, io_log: None, onchip_log: false, trace_exc: false, breaks: vec![], args: [0; 3], screenshot: None, trace_atapi: false, no_disc: false, tracepoints: vec![] };
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
            "--trace-atapi" => o.trace_atapi = true,
            "--no-disc" => o.no_disc = true,
            "--screenshot" => o.screenshot = Some(next(&mut i)?.clone()),
            "--trace-exc" => o.trace_exc = true,
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

fn loader(path: &str, rest: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let o = parse_opts(rest)?;
    let image = std::fs::read(path)?;
    let variants = fw::load_variants(&image)?;
    let v = variants.iter().find(|v| v.name == o.variant).ok_or_else(|| format!("no variant {} on this disc", o.variant))?;
    let ldm = v.module("LDM").ok_or("variant has no LDM module")?;
    let mut m = Machine::new();
    if let Some(n) = o.io_log {
        m.board.log = rnse::iolog::IoLog::new(true, n);
        m.board.coral.log = rnse::iolog::IoLog::new(true, n);
        m.board.atapi.log = rnse::iolog::IoLog::new(true, n);
        if o.onchip_log {
            m.cpu.io.log = rnse::iolog::IoLog::new(true, n);
        }
    }
    m.cpu.trace_exceptions = o.trace_exc;
    m.cpu.breakpoints = o.breaks.clone();
    m.cpu.breakpoints.extend(o.tracepoints.iter().copied());
    m.board.atapi.trace = o.trace_atapi;
    if !o.no_disc {
        m.board.atapi.load(image.clone());
    }
    m.install_boot_stub(&v.name);
    m.start_loader(ldm, o.args);
    let end = m.cpu.cycles + o.cycles;
    let mut stop;
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
    if let Some(path) = &o.screenshot {
        let (w, h, rgb) = m.board.coral.render();
        std::fs::write(path, rnse::png::encode_rgb(w, h, &rgb))?;
        eprintln!("screenshot: {} ({}x{})", path, w, h);
    }
    let h = m.cpu.history();
    eprintln!("last PCs: {}", h[h.len() - 24..].iter().map(|p| format!("{:08x}", p)).collect::<Vec<_>>().join(" "));
    Ok(())
}
