//! Firmware container formats found on an RNS-E update disc.
//!
//! An update CD is a plain ISO 9660 volume.  `LOADING.KWI` holds the
//! software for every hardware variant the disc supports; each variant is
//! a set of *modules* (loader, OS, application, fonts, ...) and every
//! module starts with the same 0x80 byte header:
//!
//! ```text
//! 0x00  ".LDR" / ".OS " / ".APL" / ".FNT" / ".ETC"  CR LF
//! 0x06  version, 8 characters                       CR LF
//! 0x10  build date  "Mmm dd yyyy"
//! 0x20  build time  "hh:mm:ss"
//! 0x30  "AISIN AW co.LTD"
//! 0x70  load address (of the header itself)
//! 0x74  image size in bytes, header included
//! 0x78  32-bit sum of the big-endian words from 0x80 to the image end
//! 0x80  four entry points: init1, init2, start, (unused)
//! ```
//!
//! Nothing in this file is specific to one firmware version.

use std::fmt;

#[derive(Debug)]
pub enum FwError {
    Io(std::io::Error),
    Format(String),
}

impl fmt::Display for FwError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FwError::Io(e) => write!(f, "{}", e),
            FwError::Format(s) => write!(f, "{}", s),
        }
    }
}

impl std::error::Error for FwError {}

impl From<std::io::Error> for FwError {
    fn from(e: std::io::Error) -> Self {
        FwError::Io(e)
    }
}

fn bad<T>(msg: impl Into<String>) -> Result<T, FwError> {
    Err(FwError::Format(msg.into()))
}

fn be32(d: &[u8], off: usize) -> u32 {
    u32::from_be_bytes([d[off], d[off + 1], d[off + 2], d[off + 3]])
}

fn cstr(d: &[u8]) -> String {
    let end = d.iter().position(|&b| b == 0).unwrap_or(d.len());
    String::from_utf8_lossy(&d[..end]).trim_end().to_string()
}

// ---- ISO 9660 ------------------------------------------------------------

/// Minimal read-only ISO 9660 access (no Joliet / Rock Ridge needed).
pub struct Iso<'a> {
    data: &'a [u8],
    root_lba: u32,
    root_len: u32,
}

#[derive(Clone, Debug)]
pub struct IsoEntry {
    pub name: String,
    pub lba: u32,
    pub len: u32,
    pub is_dir: bool,
}

impl<'a> Iso<'a> {
    pub fn new(data: &'a [u8]) -> Result<Self, FwError> {
        if data.len() < 17 * 2048 || &data[0x8001..0x8006] != b"CD001" {
            return bad("not an ISO 9660 image (no CD001 volume descriptor)");
        }
        let root = &data[0x8000 + 156..0x8000 + 190];
        Ok(Iso { data, root_lba: u32::from_le_bytes([root[2], root[3], root[4], root[5]]), root_len: u32::from_le_bytes([root[10], root[11], root[12], root[13]]) })
    }

    pub fn volume_label(&self) -> String {
        String::from_utf8_lossy(&self.data[0x8028..0x8048]).trim_end().to_string()
    }

    fn read_dir(&self, lba: u32, len: u32) -> Vec<IsoEntry> {
        let mut out = Vec::new();
        let start = lba as usize * 2048;
        let end = (start + len as usize).min(self.data.len());
        let mut p = start;
        while p < end {
            let rec_len = self.data[p] as usize;
            if rec_len == 0 {
                // Records do not span sectors; skip to the next one.
                p = (p / 2048 + 1) * 2048;
                continue;
            }
            if p + rec_len > end {
                break;
            }
            let r = &self.data[p..p + rec_len];
            let name_len = r[32] as usize;
            let raw = &r[33..33 + name_len];
            if !(name_len == 1 && (raw[0] == 0 || raw[0] == 1)) {
                let mut name = String::from_utf8_lossy(raw).to_string();
                if let Some(i) = name.find(';') {
                    name.truncate(i);
                }
                if name.ends_with('.') {
                    name.pop();
                }
                out.push(IsoEntry {
                    name,
                    lba: u32::from_le_bytes([r[2], r[3], r[4], r[5]]),
                    len: u32::from_le_bytes([r[10], r[11], r[12], r[13]]),
                    is_dir: r[25] & 2 != 0,
                });
            }
            p += rec_len;
        }
        out
    }

    /// Look up a path such as `574/LDM.BIN` (case-insensitive).
    pub fn find(&self, path: &str) -> Option<IsoEntry> {
        let mut dir = (self.root_lba, self.root_len);
        let mut found = None;
        for part in path.split('/').filter(|s| !s.is_empty()) {
            let e = self.read_dir(dir.0, dir.1).into_iter().find(|e| e.name.eq_ignore_ascii_case(part))?;
            dir = (e.lba, e.len);
            found = Some(e);
        }
        found
    }

    pub fn file(&self, path: &str) -> Option<&'a [u8]> {
        let e = self.find(path)?;
        if e.is_dir {
            return None;
        }
        let start = e.lba as usize * 2048;
        self.data.get(start..start + e.len as usize)
    }

    /// Every file on the volume as (path, entry).
    pub fn walk(&self) -> Vec<(String, IsoEntry)> {
        let mut out = Vec::new();
        let mut stack = vec![(String::new(), self.root_lba, self.root_len)];
        while let Some((prefix, lba, len)) = stack.pop() {
            for e in self.read_dir(lba, len) {
                let path = if prefix.is_empty() { e.name.clone() } else { format!("{}/{}", prefix, e.name) };
                if e.is_dir {
                    stack.push((path, e.lba, e.len));
                } else {
                    out.push((path, e));
                }
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }
}

// ---- modules -------------------------------------------------------------

/// One firmware module with its parsed header.
#[derive(Clone)]
pub struct Module {
    /// Name in the KWI directory: LDM, OSM, APM, AUM, APFM, FONT, VRM, DIG.
    pub name: String,
    /// Header tag without the dot: LDR, OS, APL, FNT, ETC.
    pub kind: String,
    pub version: String,
    pub build_date: String,
    pub build_time: String,
    pub load_addr: u32,
    pub size: u32,
    pub checksum: u32,
    pub entries: [u32; 4],
    /// Raw image as stored (padded to a sector multiple).
    pub data: Vec<u8>,
}

impl Module {
    pub fn parse(name: &str, data: &[u8]) -> Result<Module, FwError> {
        if data.len() < 0x90 || data[0] != b'.' || &data[4..6] != b"\r\n" {
            return bad(format!("module {}: missing header", name));
        }
        let m = Module {
            name: name.to_string(),
            kind: cstr(&data[1..4]),
            version: cstr(&data[6..14]),
            build_date: cstr(&data[0x10..0x1e]),
            build_time: cstr(&data[0x20..0x2e]),
            load_addr: be32(data, 0x70),
            size: be32(data, 0x74),
            checksum: be32(data, 0x78),
            entries: [be32(data, 0x80), be32(data, 0x84), be32(data, 0x88), be32(data, 0x8c)],
            data: data.to_vec(),
        };
        if m.size as usize > data.len() {
            return bad(format!("module {}: header size {:#x} exceeds data {:#x}", name, m.size, data.len()));
        }
        Ok(m)
    }

    /// Checksum computed over the image, to compare with the header.
    pub fn compute_checksum(&self) -> u32 {
        let body = &self.data[0x80..self.size as usize];
        let mut sum = 0u32;
        let mut chunks = body.chunks_exact(4);
        for c in &mut chunks {
            sum = sum.wrapping_add(u32::from_be_bytes([c[0], c[1], c[2], c[3]]));
        }
        let rem = chunks.remainder();
        if !rem.is_empty() {
            let mut last = [0u8; 4];
            last[..rem.len()].copy_from_slice(rem);
            sum = sum.wrapping_add(u32::from_be_bytes(last));
        }
        sum
    }

    pub fn checksum_ok(&self) -> bool {
        self.compute_checksum() == self.checksum
    }

    /// Physical (29-bit) load address.
    pub fn phys_addr(&self) -> u32 {
        self.load_addr & 0x1fff_ffff
    }

    /// The address the module's code runs at for a place in its image:
    /// the load address in the address region of its entry points.
    pub fn code_addr(&self, offset: usize) -> u32 {
        (self.entries[0] & 0xe000_0000) | (self.phys_addr() + offset as u32)
    }

    /// Big-endian long word at a code address, if it is in the image.
    pub fn long_at(&self, addr: u32) -> Option<u32> {
        let o = (addr & 0x1fff_ffff).checked_sub(self.phys_addr())? as usize;
        let b = self.data.get(o..o + 4)?;
        Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// Where a sequence of instruction words occurs in the image, as
    /// code addresses.  Each word is given with a mask of the bits that
    /// must match.
    pub fn find_code(&self, pattern: &[(u16, u16)]) -> Vec<u32> {
        let end = (self.size as usize).min(self.data.len());
        let mut found = Vec::new();
        let mut o = 0;
        while o + 2 * pattern.len() <= end {
            let hit = pattern.iter().enumerate().all(|(i, &(word, mask))| u16::from_be_bytes([self.data[o + 2 * i], self.data[o + 2 * i + 1]]) & mask == word);
            if hit {
                found.push(self.code_addr(o));
            }
            o += 2;
        }
        found
    }
}

/// The software for one hardware variant.
#[derive(Clone)]
pub struct Variant {
    /// "V220", "V574", "VB48", ...
    pub name: String,
    /// Software version string from the KWI directory, e.g. "0815".
    pub version: String,
    pub modules: Vec<Module>,
}

impl Variant {
    pub fn module(&self, name: &str) -> Option<&Module> {
        self.modules.iter().find(|m| m.name == name)
    }
}

/// Parse a `LOADING.KWI` container.
///
/// Layout (all big-endian):
/// * 0x10: number of variants (16 bit, stored little-endian in the first
///   byte pair as seen on every disc so far)
/// * 0x14: one 0x40 byte summary per variant (name at +4, version at +0x38)
/// * then one 0x100 byte block per variant: name at +4, module count at
///   +0x44, 16 byte module records from +0x48 (name[8], offset, size),
///   start sector at +0xf8 (5 bytes) and sector count at +0xfd (3 bytes).
///   Module offsets are in units of two bytes from the variant start.
pub fn parse_kwi(d: &[u8]) -> Result<Vec<Variant>, FwError> {
    if d.len() < 0x800 {
        return bad("KWI file too short");
    }
    let n = d[0x11] as usize;
    if n == 0 || n > 8 {
        return bad(format!("KWI: implausible variant count {}", n));
    }
    let mut out = Vec::new();
    for i in 0..n {
        let summary = &d[0x14 + i * 0x40..0x14 + (i + 1) * 0x40];
        let b = 0x14 + n * 0x40 + i * 0x100;
        let blk = &d[b..b + 0x100];
        let name = cstr(&blk[4..12]);
        let version = cstr(&summary[0x38..0x3c]);
        let count = u16::from_be_bytes([blk[0x44], blk[0x45]]) as usize;
        let start = blk[0xf8..0xfd].iter().fold(0u64, |a, &x| (a << 8) | x as u64) as usize * 0x800;
        if count > 11 {
            return bad(format!("KWI: variant {} has {} modules", name, count));
        }
        let mut modules = Vec::new();
        for j in 0..count {
            let e = &blk[0x48 + j * 16..0x58 + j * 16];
            let mname = cstr(&e[..8]);
            let off = start + be32(e, 8) as usize * 2;
            let size = be32(e, 12) as usize;
            let data = d.get(off..off + size).ok_or_else(|| FwError::Format(format!("KWI: module {} of {} lies outside the file", mname, name)))?;
            modules.push(Module::parse(&mname, data)?);
        }
        out.push(Variant { name, version, modules });
    }
    Ok(out)
}

/// Values from the `INDEX.TXT` file that accompanies a variant on the
/// disc (`KEY:value$` lines).
#[derive(Clone, Debug, Default)]
pub struct IndexInfo {
    pub sofidx: String,
    pub navver: String,
    pub audver: String,
    /// Version of the text/speech data (193-family discs only).
    pub txtver: String,
}

impl IndexInfo {
    pub fn parse(text: &[u8]) -> IndexInfo {
        let mut info = IndexInfo::default();
        for line in String::from_utf8_lossy(text).lines() {
            let line = line.trim_end_matches('$');
            if let Some((k, v)) = line.split_once(':') {
                match k {
                    "SOFIDX" => info.sofidx = v.to_string(),
                    "NAVVER" => info.navver = v.to_string(),
                    "AUDVER" => info.audver = v.to_string(),
                    "TXTVER" => info.txtver = v.to_string(),
                    _ => {}
                }
            }
        }
        info
    }

    /// The index file for a variant: `INDEX.TXT` for the first one on the
    /// disc, `<xxx>INDEX.TXT` (variant name without the V) for others.
    pub fn from_disc(image: &[u8], variant: &str) -> Option<IndexInfo> {
        let iso = Iso::new(image).ok()?;
        let named = format!("{}INDEX.TXT", &variant[1..]);
        iso.file(&named).or_else(|| iso.file("INDEX.TXT")).map(IndexInfo::parse)
    }
}

/// Build the 256 byte module directory V220's update loader writes to
/// flash just below the OS module once everything is installed.  The OS walks
/// it at start-up to find and initialise the other modules.
///
/// ```text
/// 0x00  ".OS "  version[8]  address          the OS module
/// 0x10  project code "00", ' ', software version "0815"
/// 0x1c  SOFIDX "0650"
/// 0x20  NAVVER[8]  AUDVER[8]
/// 0x30  ".IDX"
/// 0x5c  number of further modules
/// 0x60  tag[4] version[8] address            one per module
/// ```
pub fn module_directory(v: &Variant, index: &IndexInfo) -> [u8; 0x100] {
    let mut d = [0u8; 0x100];
    fn put(d: &mut [u8], off: usize, s: &[u8], max: usize) {
        let n = s.len().min(max);
        d[off..off + n].copy_from_slice(&s[..n]);
    }
    fn entry(d: &mut [u8], off: usize, m: &Module) {
        d[off] = b'.';
        put(d, off + 1, format!("{:<3}", m.kind).as_bytes(), 3);
        put(d, off + 4, m.version.as_bytes(), 8);
        d[off + 12..off + 16].copy_from_slice(&m.load_addr.to_be_bytes());
    }
    if let Some(os) = v.module("OSM") {
        entry(&mut d, 0, os);
    }
    put(&mut d, 0x10, format!("00 {}", v.version).as_bytes(), 8);
    put(&mut d, 0x1c, index.sofidx.as_bytes(), 4);
    put(&mut d, 0x20, index.navver.as_bytes(), 8);
    put(&mut d, 0x28, index.audver.as_bytes(), 8);
    put(&mut d, 0x30, b".IDX", 4);
    // The loader registers modules in this fixed order.
    let order = ["APM", "APFM", "AUM", "DIG", "FONT", "VRM"];
    let mut n = 0u32;
    for name in order {
        if let Some(m) = v.module(name) {
            entry(&mut d, 0x60 + n as usize * 16, m);
            n += 1;
        }
    }
    d[0x5c..0x60].copy_from_slice(&n.to_be_bytes());
    d
}

/// The two-digit code in front of the software version in a module
/// directory.  It follows the variant name (V220 "00", V222 "02"; V574
/// "00", V575 "01"), so it is presumably a market code that the loader
/// derives from the project number.
pub fn market_code(variant: &str) -> &'static str {
    match variant {
        "V222" => "02",
        "V575" => "01",
        _ => "00",
    }
}

/// The compact module directory, written by every loader except V220's:
/// in the 256 bytes below the OS on the 192 (V222), in the 512 bytes
/// below it on the 193.  Whoever starts the modules walks it.
///
/// ```text
/// 0x00  ".OS "  version[8]  address          the OS module
/// 0x10  market code, ' ', software version, e.g. "00 1040"
/// 0x18  number of further modules
/// 0x1c  tag[4] version[8] address            one per module
///       then SOFIDX[4] NAVVER[8] AUDVER[8] (TXTVER[4]) ".IDX"
/// ```
pub fn module_directory_compact(v: &Variant, index: &IndexInfo, size: usize) -> Vec<u8> {
    let mut d = vec![0u8; size];
    fn put(d: &mut [u8], off: usize, s: &[u8], max: usize) {
        let n = s.len().min(max);
        d[off..off + n].copy_from_slice(&s[..n]);
    }
    fn entry(d: &mut [u8], off: usize, m: &Module) {
        d[off] = b'.';
        put(d, off + 1, format!("{:<3}", m.kind).as_bytes(), 3);
        put(d, off + 4, m.version.as_bytes(), 8);
        d[off + 12..off + 16].copy_from_slice(&m.load_addr.to_be_bytes());
    }
    if let Some(os) = v.module("OSM") {
        entry(&mut d, 0, os);
    }
    put(&mut d, 0x10, format!("{} {}", market_code(&v.name), v.version).as_bytes(), 8);
    let order = ["APM", "APFM", "AUM", "FONT", "VRM", "DIG"];
    let mut n = 0usize;
    for name in order {
        if let Some(m) = v.module(name) {
            entry(&mut d, 0x1c + n * 16, m);
            n += 1;
        }
    }
    d[0x18..0x1c].copy_from_slice(&(n as u32).to_be_bytes());
    let mut o = 0x1c + n * 16;
    put(&mut d, o, index.sofidx.as_bytes(), 4);
    put(&mut d, o + 4, index.navver.as_bytes(), 8);
    put(&mut d, o + 12, index.audver.as_bytes(), 8);
    o += 20;
    if !index.txtver.is_empty() {
        put(&mut d, o, index.txtver.as_bytes(), 4);
        o += 4;
    }
    put(&mut d, o, b".IDX", 4);
    d
}

/// The module directory for a variant, as its loader writes it, and its
/// distance below the OS module.
pub fn module_directory_for(v: &Variant, index: &IndexInfo) -> Vec<u8> {
    match v.name.as_str() {
        "V220" => module_directory(v, index).to_vec(),
        n if n.starts_with("V22") => module_directory_compact(v, index, 0x100),
        _ => module_directory_compact(v, index, 0x200),
    }
}

/// Load the variants from either an ISO image of the update disc or a
/// bare `LOADING.KWI` file.
pub fn load_variants(image: &[u8]) -> Result<Vec<Variant>, FwError> {
    if image.len() > 0x8006 && &image[0x8001..0x8006] == b"CD001" {
        let iso = Iso::new(image)?;
        let kwi = iso.file("LOADING.KWI").ok_or_else(|| FwError::Format("disc image has no LOADING.KWI".into()))?;
        parse_kwi(kwi)
    } else {
        parse_kwi(image)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_module(tag: &[u8; 4], version: &[u8; 8], load: u32, body: &[u8]) -> Vec<u8> {
        let mut d = vec![0u8; 0x90];
        d[..4].copy_from_slice(tag);
        d[4..6].copy_from_slice(b"\r\n");
        d[6..14].copy_from_slice(version);
        d[14..16].copy_from_slice(b"\r\n");
        d[0x10..0x1b].copy_from_slice(b"Jan  1 2000");
        d[0x20..0x28].copy_from_slice(b"12:00:00");
        d[0x80..0x84].copy_from_slice(&(load + 0x90).to_be_bytes());
        d.extend_from_slice(body);
        let size = d.len() as u32;
        d[0x70..0x74].copy_from_slice(&load.to_be_bytes());
        d[0x74..0x78].copy_from_slice(&size.to_be_bytes());
        let mut sum = 0u32;
        for c in d[0x80..].chunks(4) {
            let mut w = [0u8; 4];
            w[..c.len()].copy_from_slice(c);
            sum = sum.wrapping_add(u32::from_be_bytes(w));
        }
        d[0x78..0x7c].copy_from_slice(&sum.to_be_bytes());
        d.resize((d.len() + 0x7ff) & !0x7ff, 0);
        d
    }

    #[test]
    fn module_header_and_checksum() {
        let raw = fake_module(b".OS ", b"V9990001", 0x0000_c100, &[1, 2, 3, 4, 5, 6, 7]);
        let m = Module::parse("OSM", &raw).unwrap();
        assert_eq!(m.kind, "OS");
        assert_eq!(m.version, "V9990001");
        assert_eq!(m.load_addr, 0xc100);
        assert_eq!(m.size, 0x97);
        assert_eq!(m.entries[0], 0xc190);
        assert!(m.checksum_ok());
        let mut bad = raw.clone();
        bad[0x92] ^= 1;
        assert!(!Module::parse("OSM", &bad).unwrap().checksum_ok());
    }

    #[test]
    fn kwi_container_round_trip() {
        let os = fake_module(b".OS ", b"V9990001", 0x0000_c100, b"operating system");
        let apm = fake_module(b".APL", b"V9990002", 0x0028_0000, b"application");
        // One variant, two modules, data from sector 1.
        let mut kwi = vec![0u8; 0x800];
        kwi[0x11] = 1;
        kwi[0x14 + 4..0x14 + 8].copy_from_slice(b"V999");
        kwi[0x14 + 0x38..0x14 + 0x3c].copy_from_slice(b"0001");
        let b = 0x14 + 0x40;
        kwi[b + 4..b + 8].copy_from_slice(b"V999");
        kwi[b + 0x45] = 2;
        let mut off = 0u32;
        for (j, (name, data)) in [("OSM", &os), ("APM", &apm)].iter().enumerate() {
            let e = b + 0x48 + j * 16;
            kwi[e..e + name.len()].copy_from_slice(name.as_bytes());
            kwi[e + 8..e + 12].copy_from_slice(&(off / 2).to_be_bytes());
            kwi[e + 12..e + 16].copy_from_slice(&(data.len() as u32).to_be_bytes());
            off += data.len() as u32;
        }
        kwi[b + 0xfc] = 1; // start sector
        kwi.extend_from_slice(&os);
        kwi.extend_from_slice(&apm);

        let variants = parse_kwi(&kwi).unwrap();
        assert_eq!(variants.len(), 1);
        let v = &variants[0];
        assert_eq!((v.name.as_str(), v.version.as_str()), ("V999", "0001"));
        assert_eq!(v.modules.len(), 2);
        assert!(v.modules.iter().all(|m| m.checksum_ok()));
        assert_eq!(v.module("APM").unwrap().load_addr, 0x0028_0000);

        let index = IndexInfo::parse(b"SOFIDX:0650$\r\nNAVVER:V9990001$\r\nAUDVER:A9990001$\r\n");
        let dir = module_directory(v, &index);
        assert_eq!(&dir[..16], b".OS V9990001\x00\x00\xc1\x00");
        assert_eq!(&dir[0x10..0x17], b"00 0001");
        assert_eq!(&dir[0x1c..0x20], b"0650");
        assert_eq!(dir[0x5f], 1);
        assert_eq!(&dir[0x60..0x70], b".APLV9990002\x00\x28\x00\x00");
    }

    #[test]
    fn rejects_garbage() {
        assert!(Iso::new(&[0u8; 100]).is_err());
        assert!(parse_kwi(&[0u8; 0x1000]).is_err());
        assert!(Module::parse("X", &[0u8; 0x200]).is_err());
    }
}
