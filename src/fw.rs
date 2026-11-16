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

/// Where the modules of a `LOADING.KWI` container lie in it: variant,
/// module name, offset and stored size in bytes (see [`parse_kwi`]).
pub fn kwi_modules(d: &[u8]) -> Result<Vec<(String, String, usize, usize)>, FwError> {
    if d.len() < 0x800 {
        return bad("KWI file too short");
    }
    let n = d[0x11] as usize;
    if n == 0 || n > 8 {
        return bad(format!("KWI: implausible variant count {}", n));
    }
    let mut out = Vec::new();
    for i in 0..n {
        let b = 0x14 + n * 0x40 + i * 0x100;
        let blk = &d[b..b + 0x100];
        let name = cstr(&blk[4..12]);
        let count = (u16::from_be_bytes([blk[0x44], blk[0x45]]) as usize).min(11);
        let start = blk[0xf8..0xfd].iter().fold(0u64, |a, &x| (a << 8) | x as u64) as usize * 0x800;
        for j in 0..count {
            let e = &blk[0x48 + j * 16..0x58 + j * 16];
            out.push((name.clone(), cstr(&e[..8]), start + be32(e, 8) as usize * 2, be32(e, 12) as usize));
        }
    }
    Ok(out)
}

/// Give a module image the checksum its header should have (the sum of
/// the big-endian long words from 0x80 to the size in the header).
/// Returns the old and the new one.
pub fn fix_checksum(module: &mut [u8]) -> Result<(u32, u32), FwError> {
    let parsed = Module::parse("module", module)?;
    let new = parsed.compute_checksum();
    module[0x78..0x7c].copy_from_slice(&new.to_be_bytes());
    Ok((parsed.checksum, new))
}

/// A copy of an update disc image (or of a bare `LOADING.KWI`) with some
/// modules replaced.  `replacement(variant, module)` supplies the new
/// image of a module, which must be as long as the one stored, with the
/// same load address and size in its header: nothing moves, and no
/// other part of the disc is touched.  The module's checksum is put
/// right.  Returns the new image and, for each module that changed, its
/// variant, name, old and new checksum.
///
/// The update loader has its own copies on the disc outside the
/// container (`LDM.BIN`), which the boot ROM loads; replacing it here
/// would leave the two different, and is refused.
#[allow(clippy::type_complexity)]
pub fn repack(image: &[u8], replacement: &dyn Fn(&str, &str) -> Option<Vec<u8>>) -> Result<(Vec<u8>, Vec<(String, String, u32, u32)>), FwError> {
    let base = if image.len() > 0x8006 && &image[0x8001..0x8006] == b"CD001" {
        let kwi = Iso::new(image)?.file("LOADING.KWI").ok_or_else(|| FwError::Format("disc image has no LOADING.KWI".into()))?;
        kwi.as_ptr() as usize - image.as_ptr() as usize
    } else {
        0
    };
    let mut out = image.to_vec();
    let mut changed = Vec::new();
    for (variant, name, offset, size) in kwi_modules(&image[base..])? {
        let Some(mut new) = replacement(&variant, &name) else { continue };
        let old = image.get(base + offset..base + offset + size).ok_or_else(|| FwError::Format(format!("module {} of {} lies outside the file", name, variant)))?;
        if new.len() != size {
            return bad(format!("{} of {}: the replacement is {:#x} bytes, the module on the disc {:#x}", name, variant, new.len(), size));
        }
        if new[0x70..0x78] != old[0x70..0x78] {
            return bad(format!("{} of {}: load address or size in the header differ from the disc's", name, variant));
        }
        let (_, sum) = fix_checksum(&mut new)?;
        if new == old {
            continue;
        }
        if name == "LDM" {
            return bad(format!("{} of {}: the loader is on the disc a second time outside the container; not replaced", name, variant));
        }
        changed.push((variant, name, be32(old, 0x78), sum));
        out[base + offset..base + offset + size].copy_from_slice(&new);
    }
    Ok((out, changed))
}

/// Pack `data` for the SDK's unpacker (`sdk/src/wayback.c`): LZSS with a
/// window of 4095 bytes and matches of 3 to 18.  Tokens come in groups
/// of eight behind a byte of flags, lowest bit first: 1 = a byte as it
/// is, 0 = two bytes `b0 b1` meaning "copy (b1 & 15) + 3 bytes from
/// b0 + (b1 >> 4) * 256 bytes back".
pub fn lzss_pack(data: &[u8]) -> Vec<u8> {
    const WINDOW: usize = 4095;
    const MAX: usize = 18;
    let n = data.len();
    let hash = |i: usize| ((data[i] as usize) << 8 ^ (data[i + 1] as usize) << 4 ^ data[i + 2] as usize) & 0xffff;
    // Chains of earlier positions with the same three bytes' hash.
    let mut head = vec![usize::MAX; 0x10000];
    let mut prev = vec![usize::MAX; n];
    let mut out = Vec::with_capacity(n / 2);
    let mut i = 0;
    while i < n {
        let flags_at = out.len();
        out.push(0u8);
        for bit in 0..8 {
            if i >= n {
                break;
            }
            let (mut best, mut dist) = (0, 0);
            if i + 3 <= n {
                let mut p = head[hash(i)];
                let mut tries = 64;
                while p != usize::MAX && i - p <= WINDOW && tries > 0 {
                    let mut l = 0;
                    while l < MAX && i + l < n && data[p + l] == data[i + l] {
                        l += 1;
                    }
                    if l > best {
                        (best, dist) = (l, i - p);
                        if l == MAX {
                            break;
                        }
                    }
                    p = prev[p];
                    tries -= 1;
                }
            }
            let step = if best >= 3 {
                out.push(dist as u8);
                out.push(((dist >> 8) << 4) as u8 | (best - 3) as u8);
                best
            } else {
                out[flags_at] |= 1 << bit;
                out.push(data[i]);
                1
            };
            for k in i..i + step {
                if k + 3 <= n {
                    let h = hash(k);
                    prev[k] = head[h];
                    head[h] = k;
                }
            }
            i += step;
        }
    }
    out
}

/// The reverse of [`lzss_pack`]; `None` when the data is not sound.
pub fn lzss_unpack(packed: &[u8], len: usize) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(len);
    let mut i = 0;
    while i < packed.len() {
        let flags = packed[i];
        i += 1;
        for bit in 0..8 {
            if i >= packed.len() {
                break;
            }
            if flags >> bit & 1 != 0 {
                out.push(packed[i]);
                i += 1;
            } else {
                let (b0, b1) = (packed[i] as usize, *packed.get(i + 1)? as usize);
                i += 2;
                let (dist, n) = (b0 | (b1 >> 4) << 8, (b1 & 15) + 3);
                if dist == 0 || dist > out.len() {
                    return None;
                }
                for _ in 0..n {
                    out.push(out[out.len() - dist]);
                }
            }
        }
    }
    (out.len() == len).then_some(out)
}

/// Mark at offset 0x10 of a program built with the SDK: `RNSE`, the size
/// of the memory the program uses from its load address on, and two
/// words for the offsets of the two copies of the packed stock loader's
/// directory ([`custom_loader`]).
pub const SDK_MARK: &[u8; 4] = b"RNSE";
/// What a directory of the packed stock loader starts with.
pub const WAY_BACK_MARK: &[u8; 8] = b"RNSEBACK";
/// Unpacked bytes to a block of the packed stock loader.
pub const WAY_BACK_BLOCK: usize = 0x4000;

/// The stock loader `stock`, packed for the SDK's `stock_update`, to be
/// placed at offset `at` of the module.  Returns the bytes and the
/// offsets (in the module) of the two copies of the directory.
///
/// The loader is cut into blocks of [`WAY_BACK_BLOCK`] bytes, each
/// packed by itself and stored twice, all first copies before all
/// second copies; a directory (mark, length, block size, number of
/// blocks, sum of what follows as long words, then for either copy and
/// each block its offset in the module, packed length and the sum of
/// its unpacked bytes) comes first and again last.  The redundancy is
/// there because the memory a loader is copied to is not all its own:
/// the North American 192 software keeps a few variables of its
/// operating system in the middle of it and goes on changing them
/// until the loader has taken the machine over.  The stock loader has
/// part of a picture there.
pub fn way_back(stock: &[u8], at: usize) -> (Vec<u8>, [usize; 2]) {
    let blocks: Vec<(Vec<u8>, u32)> = stock
        .chunks(WAY_BACK_BLOCK)
        .map(|c| (lzss_pack(c), c.iter().fold(0u32, |a, b| a.wrapping_add(*b as u32))))
        .collect();
    let n = blocks.len();
    let dir_len = 24 + n * 2 * 12;
    let data_len: usize = blocks.iter().map(|(p, _)| p.len()).sum();
    let pad = |x: usize| (x + 3) & !3;
    let second_dir = pad(dir_len + 2 * data_len);
    let mut entries = Vec::new();
    let mut off = at + dir_len;
    for _ in 0..2 {
        for (packed, sum) in &blocks {
            entries.extend([off as u32, packed.len() as u32, *sum]);
            off += packed.len();
        }
    }
    let mut dir = WAY_BACK_MARK.to_vec();
    for word in [stock.len() as u32, WAY_BACK_BLOCK as u32, n as u32, entries.iter().fold(0u32, |a, b| a.wrapping_add(*b))] {
        dir.extend(word.to_be_bytes());
    }
    for word in &entries {
        dir.extend(word.to_be_bytes());
    }
    let mut out = dir.clone();
    for _ in 0..2 {
        for (packed, _) in &blocks {
            out.extend(packed);
        }
    }
    out.resize(second_dir, 0);
    out.extend(dir);
    (out, [at, at + second_dir])
}

/// A copy of an update disc image with a program of one's own in the
/// place of a variant's update loader.
///
/// `payload` is the image from offset 0x80 of the module on: the four
/// entry points, of which the boot ROM calls the first, then code and
/// data, linked for the loader's load address.  The loader's 0x80-byte
/// header is kept as it is but for the checksum, and the image keeps the
/// loader's length (the rest is zero), so that the file the boot ROM
/// reads differs from one it is known to accept in content only.  The
/// loader is on the disc twice, in `LOADING.KWI` and as a file of its
/// own; both are replaced.
///
/// A program built with the SDK (it carries [`SDK_MARK`]) also gets the
/// stock loader it replaces, packed ([`way_back`]), at the end of the
/// image, above the memory the program says it uses, and the offsets of
/// its directories written into its mark.  With it the program can end by unpacking the stock loader
/// over itself and starting it: the unit is then updated from the disc
/// as from any update disc, ends up with the disc's software index and
/// no longer takes the disc for an update.  Without that way back a
/// unit starts the program again at every start, for as long as the
/// disc is in the drive.
///
/// Returns the new image and where it put the program.
pub fn custom_loader(image: &[u8], variant: &str, payload: &[u8]) -> Result<(Vec<u8>, Vec<String>), FwError> {
    let iso = Iso::new(image)?;
    let kwi = iso.file("LOADING.KWI").ok_or_else(|| FwError::Format("disc image has no LOADING.KWI".into()))?;
    let base = kwi.as_ptr() as usize - image.as_ptr() as usize;
    let (offset, size) = kwi_modules(kwi)?
        .into_iter()
        .find(|(v, name, _, _)| v == variant && name == "LDM")
        .map(|(_, _, offset, size)| (offset, size))
        .ok_or_else(|| FwError::Format(format!("no loader for variant {} on this disc", variant)))?;
    let old = &kwi[offset..offset + size];
    let used = be32(old, 0x74) as usize;
    if payload.len() < 16 || 0x80 + payload.len() > used.min(size) {
        return bad(format!("the program is {:#x} bytes; the loader's place holds {:#x}", payload.len(), used.min(size).saturating_sub(0x80)));
    }
    let load = be32(old, 0x70);
    let entry = be32(payload, 0);
    if !(load + 0x90..load + used as u32).contains(&entry) {
        return bad(format!("the program's first entry point, {:08x}, is not inside the loader's place at {:08x}: was it linked for this unit?", entry, load));
    }
    let mut new = old.to_vec();
    new[0x80..].fill(0);
    new[0x80..0x80 + payload.len()].copy_from_slice(payload);
    if payload.len() >= 0x1c && &payload[0x10..0x14] == SDK_MARK {
        let room = used.min(size);
        let uses = be32(payload, 0x14) as usize;
        let stock = &old[..room];
        // Its length does not depend on where it goes.
        let len = way_back(stock, 0).0.len();
        let at = room.saturating_sub(len) & !3;
        if at < uses.max(0x80 + payload.len()) || payload.len() < 0x20 {
            return bad(format!("no room for the stock loader ({:#x} bytes packed, twice) above the {:#x} bytes the program uses, in a place of {:#x}", len, uses, room));
        }
        let (blob, dirs) = way_back(stock, at);
        new[at..at + blob.len()].copy_from_slice(&blob);
        new[0x80 + 0x18..0x80 + 0x1c].copy_from_slice(&(dirs[0] as u32).to_be_bytes());
        new[0x80 + 0x1c..0x80 + 0x20].copy_from_slice(&(dirs[1] as u32).to_be_bytes());
    }
    fix_checksum(&mut new)?;

    let mut out = image.to_vec();
    let mut places = vec![format!("LOADING.KWI, module LDM of {}", variant)];
    out[base + offset..base + offset + size].copy_from_slice(&new);
    // The loader's own files: every file on the disc with the same
    // contents as the module in the container.
    for (path, entry) in iso.walk() {
        let start = entry.lba as usize * 2048;
        if entry.is_dir {
            continue;
        }
        let Some(file) = image.get(start..start + entry.len as usize) else { continue };
        if start != base && file.len() >= used && file[..used] == old[..used] {
            out[start..start + used].copy_from_slice(&new[..used]);
            places.push(path);
        }
    }
    Ok((out, places))
}

/// A module to go in the operating system's place in flash: the stock
/// module's header and length, and inside it a program of the SDK (the
/// image `sdk/build.sh` writes, linked for the loader's place in RAM)
/// behind a few instructions that copy it there and start it.  All
/// three entry points lead to those instructions; the boot ROM calls
/// the first and nothing comes back from it, so no other module is
/// called.  The 192 only.
///
/// What the instructions do, at the module's offset 0x90: exceptions
/// and interrupts off (the ROM's own handlers are at work when it
/// calls), one edge on the watchdog's strobe, the copy through the
/// uncached address of RAM, a mark in the first of the three arguments
/// a loader gets ("FLSH": the SDK's start-up code counts the starts of
/// a program that comes with it, `sdk/src/flash.c`) and zeroes in the
/// others, and a jump to the program's first entry point.
pub fn firmware_module(stock: &[u8], payload: &[u8], ram_load: u32) -> Result<Vec<u8>, FwError> {
    const AT: usize = 0x90;
    const PROGRAM: usize = 0x100;
    let size = be32(stock, 0x74) as usize;
    let load = be32(stock, 0x70) & 0x1fff_ffff;
    if size > stock.len() || PROGRAM + payload.len() > size {
        return bad(format!("the program is {:#x} bytes; the module's place holds {:#x}", payload.len(), size.saturating_sub(PROGRAM)));
    }
    if payload.len() < 0x20 || &payload[0x10..0x14] != SDK_MARK {
        return bad("not a program of the SDK (no mark behind its entry points)");
    }
    let entry = be32(payload, 0);
    if !(ram_load + 0x90..ram_load + 0x80 + payload.len() as u32).contains(&entry) {
        return bad(format!("the program's first entry point, {:08x}, is not inside the place it is linked for at {:08x}", entry, ram_load));
    }
    let words = payload.len().div_ceil(4) as u32;
    // Instructions, then the long words they load: (register, value).
    let literals: [(u16, u32); 7] = [
        (0, 0x5000_00f0),                              // SR
        (5, 0xb230_0001),                              // port 0: the watchdog's strobe
        (1, 0x8000_0000 | (load + PROGRAM as u32)),    // from: the program in flash
        (2, 0xa000_0000 | ((ram_load & 0x1fff_ffff) + 0x80)), // to: RAM, uncached
        (3, words),
        (1, ram_load + 0x80),                          // where its entry points are then
        (4, 0x464c_5348),                              // "FLSH": started from flash (sdk/src/flash.c)
    ];
    let mut code: Vec<u16> = Vec::new();
    let mut fix: Vec<(usize, usize)> = Vec::new();    // (instruction index, literal index)
    let mut load_literal = |code: &mut Vec<u16>, n: usize| {
        fix.push((code.len(), n));
        code.push(0xd000 | literals[n].0 << 8);
    };
    load_literal(&mut code, 0);
    code.push(0x400e);              // ldc r0, sr
    load_literal(&mut code, 1);
    code.push(0x6050);              // mov.b @r5, r0
    code.push(0xca01);              // xor #1, r0
    code.push(0x2500);              // mov.b r0, @r5
    load_literal(&mut code, 2);
    load_literal(&mut code, 3);
    load_literal(&mut code, 4);
    let top = code.len();
    code.push(0x6016);              // mov.l @r1+, r0
    code.push(0x2202);              // mov.l r0, @r2
    code.push(0x7204);              // add #4, r2
    code.push(0x4310);              // dt r3
    let back = top as i32 - (code.len() as i32 + 2);
    code.push(0x8b00 | (back as u16 & 0xff)); // bf top
    load_literal(&mut code, 5);
    code.push(0x6012);              // mov.l @r1, r0
    load_literal(&mut code, 6);
    code.push(0xe500);              // mov #0, r5
    code.push(0xe600);              // mov #0, r6
    code.push(0x402b);              // jmp @r0
    code.push(0x0009);              // nop
    if code.len() % 2 == 1 {
        code.push(0x0009);
    }
    let pool = AT + 2 * code.len();
    for (at, n) in fix {
        // mov.l @(disp, PC): the long word at (PC & ~3) + 4 + 4 * disp.
        let pc = AT + 2 * at;
        let disp = (pool + 4 * n - ((pc & !3) + 4)) / 4;
        code[at] |= disp as u16;
    }
    if pool + 4 * literals.len() > PROGRAM {
        return bad("the start-up instructions do not fit below the program");
    }
    // Everything else of the stock module stays as it is: nothing of it
    // runs, but the card processors' program is in it (at 0x200000 of
    // the flash on the 192), which is where the SDK's card driver
    // fetches it from, as the stock driver does.
    const KEPT_FROM: usize = 0x10_0000;
    if PROGRAM + payload.len() > KEPT_FROM.min(size) {
        return bad(format!("the program is {:#x} bytes; more than {:#x} would go over what is kept of the stock module", payload.len(), KEPT_FROM - PROGRAM));
    }
    let mut new = stock[..size].to_vec();
    new[0x80..KEPT_FROM.min(size)].fill(0);
    let start = (0x8000_0000 | load) + AT as u32;
    for i in 0..3 {
        new[0x80 + 4 * i..0x84 + 4 * i].copy_from_slice(&start.to_be_bytes());
    }
    for (i, w) in code.iter().enumerate() {
        new[AT + 2 * i..AT + 2 * i + 2].copy_from_slice(&w.to_be_bytes());
    }
    for (i, (_, v)) in literals.iter().enumerate() {
        new[pool + 4 * i..pool + 4 * i + 4].copy_from_slice(&v.to_be_bytes());
    }
    new[PROGRAM..PROGRAM + payload.len()].copy_from_slice(payload);
    // (The module on the disc may be stored longer than its size says.)
    let mut out = stock.to_vec();
    out[..size].copy_from_slice(&new);
    fix_checksum(&mut out)?;
    Ok(out)
}

/// A copy of an update disc on which a variant's operating system
/// module is a program of the SDK ([`firmware_module`]).  The disc's
/// own loader installs it like any software; every other module is the
/// disc's, and stays in flash unused.  (So does the stock operating
/// system from its second megabyte on, for the card processors'
/// program that is in it.)
///
/// A disc has its loader twice: in the container, for the installed
/// software, and as a file of its own, which the boot ROM runs when the
/// unit has no software (tried: each runs its own).  With `swap`, a
/// program of the SDK goes in the file's place and the container keeps
/// the stock loader: the installed software's update installs the
/// program, and a unit without software that finds this disc runs
/// `swap` instead of installing the program again (`sdk/examples/swap`
/// pushes the disc out and waits for another).
pub fn firmware_disc(image: &[u8], variant: &str, payload: &[u8], swap: Option<&[u8]>) -> Result<Vec<u8>, FwError> {
    firmware_disc_with_kernel(image, variant, payload, swap, None)
}

/// Where a kernel image starts in the module that [`kernel_module`]
/// makes, and the mark of its header (at 0x100): `RNSELNX1`, its length
/// and its CRC-32 (IEEE, as zlib's), big-endian, then where it is loaded
/// and entered.  `sdk/examples/linuxboot` checks all of it.
pub const KERNEL_AT: usize = 0x200;
pub const KERNEL_MARK: &[u8; 8] = b"RNSELNX1";

/// CRC-32 (IEEE 802.3, as zlib's crc32).
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// The application module's place (APM) with a Linux kernel image in it,
/// for a firmware disc whose operating system's place holds
/// `sdk/examples/linuxboot` (docs/linux.md, "Linux in flash").  Header,
/// load address and length are the stock module's, so that its loader
/// writes it where the stock application was; nothing of the stock
/// application is left.  The three entry points go to an `rts` of their
/// own: the boot ROM calls the operating system's first (V222, which
/// never returns for a program installed there), but should it ever call
/// this module's, they return at once.  The kernel image, as
/// `linux/build.sh` makes VMLINUX.BIN, is at [`KERNEL_AT`].
pub fn kernel_module(stock: &[u8], kernel: &[u8]) -> Result<Vec<u8>, FwError> {
    const STUB: usize = 0x90;
    let size = be32(stock, 0x74) as usize;
    let load = be32(stock, 0x70);
    if size > stock.len() || KERNEL_AT + kernel.len() > size {
        return bad(format!("the kernel is {:#x} bytes; the module's place holds {:#x}", kernel.len(), size.saturating_sub(KERNEL_AT)));
    }
    if kernel.len() < 0x2000 || kernel[0x1000..0x1004] == [0; 4] {
        return bad("not a kernel image as linux/build.sh makes it (nothing at _stext, 0x1000 in)");
    }
    let mut out = stock.to_vec();
    out[0x80..size].fill(0xff);
    let stub = load + STUB as u32;
    for i in 0..4 {
        out[0x80 + 4 * i..0x84 + 4 * i].copy_from_slice(&stub.to_be_bytes());
    }
    out[STUB..STUB + 4].copy_from_slice(&[0x00, 0x0b, 0x00, 0x09]); // rts; nop
    out[0x100..0x108].copy_from_slice(KERNEL_MARK);
    out[0x108..0x10c].copy_from_slice(&(kernel.len() as u32).to_be_bytes());
    out[0x10c..0x110].copy_from_slice(&crc32(kernel).to_be_bytes());
    out[0x110..0x114].copy_from_slice(&0x8c00_1000u32.to_be_bytes());
    out[0x114..0x118].copy_from_slice(&0x8c00_2000u32.to_be_bytes());
    out[KERNEL_AT..KERNEL_AT + kernel.len()].copy_from_slice(kernel);
    fix_checksum(&mut out)?;
    Ok(out)
}

/// [`firmware_disc`], and with `kernel` a Linux kernel in the
/// application module's place ([`kernel_module`]) for the program in the
/// operating system's place to start.
pub fn firmware_disc_with_kernel(image: &[u8], variant: &str, payload: &[u8], swap: Option<&[u8]>, kernel: Option<&[u8]>) -> Result<Vec<u8>, FwError> {
    let variants = load_variants(image)?;
    let v = variants.iter().find(|v| v.name == variant).ok_or_else(|| FwError::Format(format!("no variant {} on this disc", variant)))?;
    let loader = v.module("LDM").ok_or_else(|| FwError::Format("the variant has no loader".into()))?;
    if loader.load_addr != 0x8df0_0000 {
        return bad("a program can be installed on the 192 only, so far");
    }
    let ram_load = loader.load_addr;
    let error = std::cell::RefCell::new(None);
    let (out, changed) = repack(image, &|var, name| {
        if var != variant {
            return None;
        }
        if name == "APM" {
            let kernel = kernel?;
            return match kernel_module(&v.module("APM")?.data, kernel) {
                Ok(m) => Some(m),
                Err(e) => {
                    error.borrow_mut().get_or_insert(e);
                    None
                }
            };
        }
        if name != "OSM" {
            return None;
        }
        let stock = &v.module("OSM")?.data;
        match firmware_module(stock, payload, ram_load) {
            Ok(m) => Some(m),
            Err(e) => {
                error.borrow_mut().get_or_insert(e);
                None
            }
        }
    })?;
    if let Some(e) = error.into_inner() {
        return Err(e);
    }
    if !changed.iter().any(|c| c.1 == "OSM") {
        return bad("the disc's operating system module was not replaced");
    }
    if kernel.is_some() && !changed.iter().any(|c| c.1 == "APM") {
        return bad("the disc's application module was not replaced by the kernel");
    }
    let mut out = out;
    if let Some(swap) = swap {
        // The loader's file as `custom_loader` makes it, and none of
        // what that does to the container.
        let (custom, places) = custom_loader(image, variant, swap)?;
        if places.len() < 2 {
            return bad("the loader's own file was not found on the disc");
        }
        let iso = Iso::new(image)?;
        let kwi = iso.file("LOADING.KWI").ok_or_else(|| FwError::Format("disc image has no LOADING.KWI".into()))?;
        let start = kwi.as_ptr() as usize - image.as_ptr() as usize;
        let container = start..start + kwi.len();
        for (i, (new, old)) in custom.iter().zip(image).enumerate() {
            if new != old && !container.contains(&i) {
                out[i] = *new;
            }
        }
    }
    Ok(out)
}

/// Give a variant's software on the disc another software index: the
/// four digits after `SOFIDX:` in its index file.  The installed
/// application takes a program disc for an update only when this number
/// is greater than the one it has in flash (it logs `ON ROM IDX`, `ON
/// DISC IDX` and, for an update, `IDX VersionUP`).  Returns the old
/// index.
pub fn set_disc_index(image: &mut [u8], variant: &str, index: &str) -> Result<String, FwError> {
    if index.len() != 4 || !index.bytes().all(|b| b.is_ascii_digit()) {
        return bad("a software index is four digits");
    }
    let named = format!("{}INDEX.TXT", &variant[1..]);
    let entry = {
        let iso = Iso::new(image)?;
        match iso.find(&named).or_else(|| iso.find("INDEX.TXT")) {
            Some(e) if !e.is_dir => e,
            _ => return bad("the disc has no index file for this variant"),
        }
    };
    let start = entry.lba as usize * 2048;
    let text = &mut image[start..start + entry.len as usize];
    let Some(at) = text.windows(7).position(|w| w == b"SOFIDX:") else { return bad("the index file has no SOFIDX line") };
    if at + 11 > text.len() {
        return bad("the index file's SOFIDX line is cut short");
    }
    let old = String::from_utf8_lossy(&text[at + 7..at + 11]).into_owned();
    text[at + 7..at + 11].copy_from_slice(index.as_bytes());
    Ok(old)
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

    #[test]
    fn lzss_round_trip() {
        // Code-like data: runs, repeats at several distances, noise.
        let mut data = Vec::new();
        let mut x = 12345u32;
        for i in 0..40_000u32 {
            x = x.wrapping_mul(1103515245).wrapping_add(12345);
            data.push(match i % 97 {
                0..=40 => (i % 7) as u8,
                41..=70 => 0,
                _ => (x >> 16) as u8,
            });
        }
        data.extend(std::iter::repeat(0xff).take(5000));
        for sample in [&data[..], &data[..1], &data[..2], &[][..], b"abcabcabcabcabcabcabcabcabcabcabcabc"] {
            let packed = lzss_pack(sample);
            assert_eq!(lzss_unpack(&packed, sample.len()).as_deref(), Some(sample));
        }
        assert!(lzss_pack(&data).len() < data.len() * 2 / 3);
        // Damaged data is refused, not unpacked wrongly.
        assert_eq!(lzss_unpack(&[0x00, 0x05, 0x00], 3), None, "a copy from before the start");
        assert_eq!(lzss_unpack(&lzss_pack(&data), data.len() + 1), None);

        // The packed loader: two directories that agree, and either
        // copy of every block unpacks to the block.
        let (blob, dirs) = way_back(&data, 0x1000);
        let word = |o: usize| u32::from_be_bytes(blob[o..o + 4].try_into().unwrap()) as usize;
        let (a, b) = (dirs[0] - 0x1000, dirs[1] - 0x1000);
        let n = word(a + 16);
        assert_eq!((word(a + 8), word(a + 12), n), (data.len(), WAY_BACK_BLOCK, data.len().div_ceil(WAY_BACK_BLOCK)));
        assert_eq!(blob[a..a + 24 + n * 24], blob[b..b + 24 + n * 24]);
        assert_eq!(blob.len(), b + 24 + n * 24);
        for copy in 0..2 {
            for (k, block) in data.chunks(WAY_BACK_BLOCK).enumerate() {
                let e = a + 24 + (copy * n + k) * 12;
                let at = word(e) - 0x1000;
                assert_eq!(lzss_unpack(&blob[at..at + word(e + 4)], block.len()).as_deref(), Some(block), "copy {} block {}", copy, k);
            }
        }
    }

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

    /// Repacking with the modules as they are gives the same image;
    /// with one changed, only that module and its checksum change.
    #[test]
    fn crc32_is_zlibs() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn a_kernel_goes_into_the_application_module() {
        let mut body = vec![0u8; 0x4000];
        body[0x1000] = 0x12;
        let mut apm = fake_module(b".APL", b"V9990002", 0x0021_0000, &vec![0x55u8; 0x5000]);
        let size = be32(&apm, 0x74) as usize;
        let m = kernel_module(&apm, &body).unwrap();
        assert_eq!(m.len(), apm.len());
        assert_eq!(&m[0x70..0x78], &apm[0x70..0x78], "load address and size kept");
        assert_eq!(be32(&m, 0x80), 0x0021_0090);
        assert_eq!(&m[0x90..0x94], &[0x00, 0x0b, 0x00, 0x09]);
        assert_eq!(&m[0x100..0x108], KERNEL_MARK);
        assert_eq!(be32(&m, 0x108), 0x4000);
        assert_eq!(be32(&m, 0x10c), crc32(&body));
        assert_eq!(&m[KERNEL_AT..KERNEL_AT + 0x4000], &body[..]);
        assert!(Module::parse("APM", &m).unwrap().checksum_ok());
        apm.truncate(size);
        assert!(kernel_module(&apm, &vec![1u8; size]).is_err(), "too long");
    }

    #[test]
    fn repack_replaces_a_module() {
        let os = fake_module(b".OS ", b"V9990001", 0x0000_c100, b"operating system");
        let apm = fake_module(b".APL", b"V9990002", 0x0028_0000, b"application");
        let mut kwi = vec![0u8; 0x800];
        kwi[0x11] = 1;
        let b = 0x14 + 0x40;
        kwi[b + 4..b + 8].copy_from_slice(b"V999");
        kwi[b + 0x45] = 2;
        for (j, (name, off)) in [("OSM", 0u32), ("APM", os.len() as u32)].iter().enumerate() {
            let e = b + 0x48 + j * 16;
            kwi[e..e + 3].copy_from_slice(name.as_bytes());
            kwi[e + 8..e + 12].copy_from_slice(&(off / 2).to_be_bytes());
            kwi[e + 12..e + 16].copy_from_slice(&0x800u32.to_be_bytes());
        }
        kwi[b + 0xfc] = 1;
        kwi.extend_from_slice(&os);
        kwi.extend_from_slice(&apm);

        let same = repack(&kwi, &|_, name| Some(if name == "OSM" { os.clone() } else { apm.clone() })).unwrap();
        assert_eq!(same.0, kwi);
        assert!(same.1.is_empty());

        let mut patched = apm.clone();
        patched[0x90..0x9b].copy_from_slice(b"APPLICATION");
        let (out, changed) = repack(&kwi, &|v, name| (v == "V999" && name == "APM").then(|| patched.clone())).unwrap();
        assert_eq!(changed.len(), 1);
        assert_eq!((changed[0].0.as_str(), changed[0].1.as_str()), ("V999", "APM"));
        assert_ne!(changed[0].2, changed[0].3);
        let variants = parse_kwi(&out).unwrap();
        assert!(variants[0].modules.iter().all(|m| m.checksum_ok()));
        assert_eq!(&variants[0].module("APM").unwrap().data[0x90..0x9b], b"APPLICATION");
        assert_eq!(out[..0x1000], kwi[..0x1000], "header and the other module untouched");

        // A replacement of another length, or a different loader, is refused.
        assert!(repack(&kwi, &|_, _| Some(vec![0; 0x1000])).is_err());
    }

    #[test]
    fn rejects_garbage() {
        assert!(Iso::new(&[0u8; 100]).is_err());
        assert!(parse_kwi(&[0u8; 0x1000]).is_err());
        assert!(Module::parse("X", &[0u8; 0x200]).is_err());
    }
}
