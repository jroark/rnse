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
