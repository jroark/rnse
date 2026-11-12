use rnse::fw;

fn usage() -> ! {
    eprintln!(
        "usage:\n  rnse info <disc.iso | LOADING.KWI>\n  rnse extract <disc.iso | LOADING.KWI> <out-dir>"
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
