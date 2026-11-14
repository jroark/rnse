//! SD card images written from scratch: a partition table, a FAT16
//! volume with files in it and, if wanted, a second partition with
//! other contents (the 193 takes a disc's volume in one of type 0x96).
//!
//! This is the writing side only, and only what a card for the unit
//! needs: FAT16 with long file names, files and folders, one fixed
//! date.  The unit's software is the reader.

/// A file, or a folder if it has `children`.
pub struct Entry {
    pub name: String,
    pub data: Vec<u8>,
    pub children: Option<Vec<Entry>>,
}

impl Entry {
    pub fn file(name: &str, data: Vec<u8>) -> Entry {
        Entry { name: name.into(), data, children: None }
    }

    pub fn folder(name: &str, children: Vec<Entry>) -> Entry {
        Entry { name: name.into(), data: Vec::new(), children: Some(children) }
    }

    /// A file or a folder from the host's file system, folders with
    /// all they hold, in name order.
    pub fn from_path(path: &std::path::Path) -> std::io::Result<Entry> {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if path.is_dir() {
            Ok(Entry::folder(&name, Self::dir(path)?))
        } else {
            Ok(Entry::file(&name, std::fs::read(path)?))
        }
    }

    /// What a folder of the host's file system holds.
    pub fn dir(path: &std::path::Path) -> std::io::Result<Vec<Entry>> {
        let mut paths: Vec<_> = std::fs::read_dir(path)?.collect::<Result<Vec<_>, _>>()?.iter().map(|e| e.path()).collect();
        paths.sort();
        paths.iter().map(|p| Entry::from_path(p)).collect()
    }
}

const SECTOR: usize = 512;
/// Where the FAT partition starts, in sectors.
pub const FAT_START: u32 = 2048;
const ROOT_ENTRIES: usize = 512;
/// 1 January 2008, the date every entry gets.
const DATE: u16 = (28 << 9) | (1 << 5) | 1;

/// The 8.3 name for a directory entry and whether it says everything
/// (if not, long-name entries go in front of it).
fn short_name(name: &str, taken: &mut Vec<[u8; 11]>) -> ([u8; 11], bool) {
    let ok = |c: char| c.is_ascii_uppercase() || c.is_ascii_digit() || "_-~!#$%&'()@^`{}".contains(c);
    let (stem, ext) = match name.rfind('.') {
        Some(dot) if dot > 0 => (&name[..dot], &name[dot + 1..]),
        _ => (name, ""),
    };
    let plain = !stem.is_empty() && stem.len() <= 8 && ext.len() <= 3 && stem.chars().all(ok) && ext.chars().all(ok);
    let clean = |s: &str, n: usize| -> Vec<u8> {
        s.chars().filter(|c| *c != ' ' && *c != '.').map(|c| c.to_ascii_uppercase())
            .map(|c| if ok(c) { c as u8 } else { b'_' }).take(n).collect()
    };
    let mut short = [b' '; 11];
    let e = clean(ext, 3);
    short[8..8 + e.len()].copy_from_slice(&e);
    if plain {
        short[..stem.len()].copy_from_slice(stem.as_bytes());
        taken.push(short);
        return (short, true);
    }
    for n in 1.. {
        let tail = format!("~{}", n);
        let mut s = clean(stem, 8 - tail.len());
        s.extend_from_slice(tail.as_bytes());
        short[..8].fill(b' ');
        short[..s.len()].copy_from_slice(&s);
        if !taken.contains(&short) {
            break;
        }
    }
    taken.push(short);
    (short, false)
}

/// The directory entries for one name: long-name entries, last part
/// first, and the 8.3 entry.
fn dir_entries(name: &str, attr: u8, cluster: u16, size: u32, taken: &mut Vec<[u8; 11]>) -> Vec<u8> {
    let (short, plain) = short_name(name, taken);
    let mut out = Vec::new();
    if !plain {
        let sum = short.iter().fold(0u8, |s, &b| s.rotate_right(1).wrapping_add(b));
        let mut units: Vec<u16> = name.encode_utf16().collect();
        if units.len() % 13 != 0 {
            units.push(0);
            while units.len() % 13 != 0 {
                units.push(0xffff);
            }
        }
        let parts = units.len() / 13;
        for part in (0..parts).rev() {
            let u = &units[part * 13..part * 13 + 13];
            let mut e = [0u8; 32];
            e[0] = (part + 1) as u8 | if part == parts - 1 { 0x40 } else { 0 };
            e[11] = 0x0f;
            e[13] = sum;
            for (i, at) in [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30].iter().enumerate() {
                e[*at..*at + 2].copy_from_slice(&u[i].to_le_bytes());
            }
            out.extend_from_slice(&e);
        }
    }
    let mut e = [0u8; 32];
    e[..11].copy_from_slice(&short);
    e[11] = attr;
    for at in [16, 18, 24] {
        e[at..at + 2].copy_from_slice(&DATE.to_le_bytes());
    }
    e[26..28].copy_from_slice(&cluster.to_le_bytes());
    e[28..32].copy_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&e);
    out
}

struct Volume {
    cluster_bytes: usize,
    /// Chains: `fat[n]` is the cluster after `n`.
    fat: Vec<u16>,
    /// The data area, from cluster 2.
    data: Vec<u8>,
}

impl Volume {
    /// Store `bytes` in a chain of new clusters and return the first
    /// (0 for nothing, as an empty file has it).
    fn store(&mut self, bytes: &[u8]) -> u16 {
        if bytes.is_empty() {
            return 0;
        }
        let first = self.fat.len();
        let n = bytes.len().div_ceil(self.cluster_bytes);
        for i in 0..n {
            self.fat.push(if i + 1 == n { 0xffff } else { (first + i + 1) as u16 });
        }
        self.data.extend_from_slice(bytes);
        self.data.resize((self.fat.len() - 2) * self.cluster_bytes, 0);
        first as u16
    }

    /// The entries of a directory, its files and folders stored.
    /// `own` and `parent` are the clusters for `.` and `..` (none in
    /// the root directory).
    fn directory(&mut self, entries: &[Entry], dots: Option<(u16, u16)>) -> Vec<u8> {
        let mut out = Vec::new();
        let mut taken = Vec::new();
        if let Some((own, parent)) = dots {
            for (name, cluster) in [(".", own), ("..", parent)] {
                let mut e = [0u8; 32];
                e[..11].fill(b' ');
                e[..name.len()].copy_from_slice(name.as_bytes());
                e[11] = 0x10;
                for at in [16, 18, 24] {
                    e[at..at + 2].copy_from_slice(&DATE.to_le_bytes());
                }
                e[26..28].copy_from_slice(&cluster.to_le_bytes());
                out.extend_from_slice(&e);
            }
        }
        for entry in entries {
            match &entry.children {
                None => {
                    let first = self.store(&entry.data);
                    out.extend(dir_entries(&entry.name, 0x20, first, entry.data.len() as u32, &mut taken));
                }
                Some(children) => {
                    // The folder's own cluster number is needed for its
                    // `.` entry before its contents are known: reserve
                    // the room first and fill it afterwards.
                    let room = children.iter().map(|c| 32 * (2 + c.name.encode_utf16().count() / 13)).sum::<usize>() + 64;
                    let first = self.store(&vec![0; room]);
                    let own = dots.map(|d| d.0).unwrap_or(0);
                    let inner = self.directory(children, Some((first, own)));
                    assert!(inner.len() <= room);
                    let at = (first as usize - 2) * self.cluster_bytes;
                    self.data[at..at + inner.len()].copy_from_slice(&inner);
                    out.extend(dir_entries(&entry.name, 0x10, first, 0, &mut taken));
                }
            }
        }
        out
    }
}

fn total_bytes(entries: &[Entry]) -> u64 {
    entries.iter().map(|e| match &e.children {
        Some(c) => 65536 + total_bytes(c),
        None => e.data.len() as u64 + 65536,
    }).sum()
}

/// A card image: a FAT16 partition named `label` holding `entries`
/// and, if given, a second partition of type `other.0` holding
/// `other.1`.  The image is a multiple of 512 KiB long.
pub fn card(label: &str, entries: &[Entry], other: Option<(u8, &[u8])>) -> Result<Vec<u8>, String> {
    // Size of the FAT volume: what the files need and some room, at
    // least 16 MiB (below that it would not be FAT16).
    let megabytes = (total_bytes(entries) * 5 / 4).div_ceil(1 << 20).max(16);
    if megabytes > 2000 {
        return Err("more than a FAT16 volume holds (2 GB)".into());
    }
    let per_cluster: usize = match megabytes {
        0..=127 => 4,
        128..=255 => 8,
        256..=511 => 16,
        512..=1023 => 32,
        _ => 64,
    };
    let sectors = megabytes as usize * 2048;
    let root_sectors = ROOT_ENTRIES * 32 / SECTOR;
    let clusters = (sectors - 1 - root_sectors) / per_cluster;
    let fat_sectors = ((clusters + 2) * 2).div_ceil(SECTOR);
    let clusters = (sectors - 1 - root_sectors - 2 * fat_sectors) / per_cluster;

    let mut v = Volume { cluster_bytes: per_cluster * SECTOR, fat: vec![0xfff8, 0xffff], data: Vec::new() };
    let mut root = Vec::new();
    if !label.is_empty() {
        let mut e = [0u8; 32];
        e[..11].fill(b' ');
        let l: Vec<u8> = label.bytes().take(11).map(|b| b.to_ascii_uppercase()).collect();
        e[..l.len()].copy_from_slice(&l);
        e[11] = 0x08;
        root.extend_from_slice(&e);
    }
    root.extend(v.directory(entries, None));
    if root.len() > ROOT_ENTRIES * 32 {
        return Err("too many entries for the card's root directory; use folders".into());
    }
    if v.fat.len() - 2 > clusters {
        return Err("the files do not fit".into());
    }

    let mut boot = [0u8; SECTOR];
    boot[..3].copy_from_slice(&[0xeb, 0x3c, 0x90]);
    boot[3..11].copy_from_slice(b"MSDOS5.0");
    boot[11..13].copy_from_slice(&(SECTOR as u16).to_le_bytes());
    boot[13] = per_cluster as u8;
    boot[14] = 1; // reserved sectors
    boot[16] = 2; // FATs
    boot[17..19].copy_from_slice(&(ROOT_ENTRIES as u16).to_le_bytes());
    if sectors < 0x10000 {
        boot[19..21].copy_from_slice(&(sectors as u16).to_le_bytes());
    } else {
        boot[32..36].copy_from_slice(&(sectors as u32).to_le_bytes());
    }
    boot[21] = 0xf8;
    boot[22..24].copy_from_slice(&(fat_sectors as u16).to_le_bytes());
    boot[24] = 63;
    boot[26] = 255;
    boot[28..32].copy_from_slice(&FAT_START.to_le_bytes());
    boot[36] = 0x80;
    boot[38] = 0x29;
    boot[39..43].copy_from_slice(b"RNSE");
    boot[43..54].fill(b' ');
    let l: Vec<u8> = label.bytes().take(11).map(|b| b.to_ascii_uppercase()).collect();
    boot[43..43 + l.len()].copy_from_slice(&l);
    boot[54..62].copy_from_slice(b"FAT16   ");
    boot[510] = 0x55;
    boot[511] = 0xaa;

    let other_start = FAT_START as usize + sectors;
    let other_sectors = other.map(|(_, d)| d.len().div_ceil(SECTOR)).unwrap_or(0);
    let total = (other_start + other_sectors).div_ceil(1024) * 1024;
    let mut image = vec![0u8; total * SECTOR];

    let entry = |image: &mut [u8], n: usize, kind: u8, start: usize, len: usize| {
        let e = &mut image[446 + 16 * n..462 + 16 * n];
        e[1..4].copy_from_slice(&[0xfe, 0xff, 0xff]);
        e[4] = kind;
        e[5..8].copy_from_slice(&[0xfe, 0xff, 0xff]);
        e[8..12].copy_from_slice(&(start as u32).to_le_bytes());
        e[12..16].copy_from_slice(&(len as u32).to_le_bytes());
    };
    entry(&mut image, 0, if sectors < 0x10000 { 0x04 } else { 0x06 }, FAT_START as usize, sectors);
    if let Some((kind, data)) = other {
        entry(&mut image, 1, kind, other_start, other_sectors);
        image[other_start * SECTOR..other_start * SECTOR + data.len()].copy_from_slice(data);
    }
    image[510] = 0x55;
    image[511] = 0xaa;

    let base = FAT_START as usize * SECTOR;
    image[base..base + SECTOR].copy_from_slice(&boot);
    let fat: Vec<u8> = v.fat.iter().flat_map(|c| c.to_le_bytes()).collect();
    for copy in 0..2 {
        let at = base + (1 + copy * fat_sectors) * SECTOR;
        image[at..at + fat.len()].copy_from_slice(&fat);
    }
    let at = base + (1 + 2 * fat_sectors) * SECTOR;
    image[at..at + root.len()].copy_from_slice(&root);
    let at = at + root_sectors * SECTOR;
    image[at..at + v.data.len()].copy_from_slice(&v.data);
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read a file back the way a FAT16 reader would: the partition
    /// table, the boot sector's numbers, the directories with their
    /// long names, the chains.
    fn find(image: &[u8], path: &[&str]) -> Option<Vec<u8>> {
        let start = u32::from_le_bytes(image[454..458].try_into().unwrap()) as usize * SECTOR;
        let b = &image[start..];
        let per_cluster = b[13] as usize * SECTOR;
        let fat_at = start + u16::from_le_bytes([b[14], b[15]]) as usize * SECTOR;
        let fat_bytes = u16::from_le_bytes([b[22], b[23]]) as usize * SECTOR;
        let root_at = fat_at + b[16] as usize * fat_bytes;
        let root_len = u16::from_le_bytes([b[17], b[18]]) as usize * 32;
        let data_at = root_at + root_len;
        let chain = |first: usize, size: Option<usize>| -> Vec<u8> {
            let (mut c, mut out) = (first, Vec::new());
            while (2..0xfff8).contains(&c) {
                out.extend_from_slice(&image[data_at + (c - 2) * per_cluster..data_at + (c - 1) * per_cluster]);
                c = u16::from_le_bytes([image[fat_at + 2 * c], image[fat_at + 2 * c + 1]]) as usize;
            }
            if let Some(n) = size { out.truncate(n); }
            out
        };
        let mut dir = image[root_at..root_at + root_len].to_vec();
        for (depth, want) in path.iter().enumerate() {
            let mut long: Vec<u16> = Vec::new();
            let mut found = None;
            for e in dir.chunks(32) {
                if e[0] == 0 { break; }
                if e[11] == 0x0f {
                    let mut part = Vec::new();
                    for at in [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30] {
                        part.push(u16::from_le_bytes([e[at], e[at + 1]]));
                    }
                    part.extend(long.drain(..));
                    long = part;
                    continue;
                }
                let name = if long.is_empty() {
                    let stem = String::from_utf8_lossy(&e[..8]).trim_end().to_string();
                    let ext = String::from_utf8_lossy(&e[8..11]).trim_end().to_string();
                    if ext.is_empty() { stem } else { format!("{}.{}", stem, ext) }
                } else {
                    let end = long.iter().position(|&u| u == 0).unwrap_or(long.len());
                    String::from_utf16_lossy(&long[..end])
                };
                long.clear();
                if name == *want {
                    found = Some((e[11], u16::from_le_bytes([e[26], e[27]]) as usize,
                        u32::from_le_bytes(e[28..32].try_into().unwrap()) as usize));
                    break;
                }
            }
            let (attr, first, size) = found?;
            if depth + 1 == path.len() {
                return Some(chain(first, if attr & 0x10 != 0 { None } else { Some(size) }));
            }
            dir = chain(first, None);
        }
        None
    }

    #[test]
    fn files_and_folders_read_back() {
        let song: Vec<u8> = (0..70_000u32).map(|i| (i * 7) as u8).collect();
        let entries = vec![
            Entry::file("TRACK01.MP3", song.clone()),
            Entry::file("A long name with spaces.mp3", b"second".to_vec()),
            Entry::folder("Album", vec![
                Entry::file("01 - First.mp3", b"first".to_vec()),
                Entry::file("EMPTY.TXT", Vec::new()),
            ]),
        ];
        let volume = vec![0x5au8; 5000];
        let image = card("MUSIC", &entries, Some((0x96, &volume))).unwrap();
        assert_eq!(image.len() % (512 * 1024), 0);
        assert_eq!(&image[510..512], &[0x55, 0xaa]);
        assert_eq!(find(&image, &["TRACK01.MP3"]).unwrap(), song);
        assert_eq!(find(&image, &["A long name with spaces.mp3"]).unwrap(), b"second");
        assert_eq!(find(&image, &["Album", "01 - First.mp3"]).unwrap(), b"first");
        assert_eq!(find(&image, &["Album", "EMPTY.TXT"]).unwrap(), b"");
        assert!(find(&image, &["Album", "missing"]).is_none());
        // The second partition holds what it was given.
        assert_eq!(image[446 + 16 + 4], 0x96);
        let at = u32::from_le_bytes(image[446 + 16 + 8..446 + 16 + 12].try_into().unwrap()) as usize * 512;
        assert_eq!(&image[at..at + 5000], &volume[..]);
    }
}
