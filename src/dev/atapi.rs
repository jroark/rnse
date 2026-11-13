//! ATAPI CD/DVD-ROM drive.
//!
//! Implements the ATA task file, the PACKET command protocol (PIO), and
//! the subset of MMC commands a data disc needs.  The medium is a plain
//! image of 2048-byte sectors (an .iso file).

use crate::iolog::IoLog;

const ST_ERR: u8 = 0x01;
const ST_DRQ: u8 = 0x08;
const ST_DSC: u8 = 0x10;
const ST_DRDY: u8 = 0x40;
const ST_BSY: u8 = 0x80;

// Interrupt reason (sector count register in ATAPI)
const IR_COD: u8 = 0x01;
const IR_IO: u8 = 0x02;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    Idle,
    /// Waiting for the 12 command packet bytes.
    Packet,
    /// Device to host data.
    DataIn,
    /// Host to device data (MODE SELECT and friends).
    DataOut,
    /// Device to host data through the DMA port (DMARQ asserted).
    DataInDma,
}

/// An audio disc: where each track starts (in frames of 1/75 s from
/// the start of the programme area) and where the last one ends.
#[derive(Clone, PartialEq)]
pub struct AudioDisc {
    pub tracks: Vec<u32>,
    pub lead_out: u32,
    /// What is on the disc: 44.1 kHz stereo samples, left first, from
    /// the start of the programme area.  Without it every track is a
    /// tone of its own pitch (see [`AudioDisc::samples`]).
    pub sound: Option<std::sync::Arc<Vec<i16>>>,
}

impl std::fmt::Debug for AudioDisc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AudioDisc {{ tracks: {:?}, lead_out: {}, sound: {} }}", self.tracks, self.lead_out, self.sound.is_some())
    }
}

/// Stereo samples to a frame of 1/75 s.
pub const SAMPLES_PER_FRAME: u64 = 588;

impl AudioDisc {
    /// A disc of `count` tracks of `seconds` each.
    pub fn uniform(count: u32, seconds: u32) -> AudioDisc {
        AudioDisc { tracks: (0..count).map(|i| i * seconds * 75).collect(), lead_out: count * seconds * 75, sound: None }
    }

    /// A disc made of recordings, one per track: 44.1 kHz stereo
    /// samples each.  Tracks are padded to whole frames.
    pub fn from_tracks(recordings: &[Vec<i16>]) -> AudioDisc {
        let (mut tracks, mut sound) = (Vec::new(), Vec::new());
        for r in recordings {
            tracks.push((sound.len() as u64 / (2 * SAMPLES_PER_FRAME)) as u32);
            sound.extend_from_slice(r);
            let whole = (sound.len() as u64).div_ceil(2 * SAMPLES_PER_FRAME) * 2 * SAMPLES_PER_FRAME;
            sound.resize(whole as usize, 0);
        }
        let lead_out = (sound.len() as u64 / (2 * SAMPLES_PER_FRAME)) as u32;
        AudioDisc { tracks, lead_out, sound: Some(std::sync::Arc::new(sound)) }
    }

    /// The pitch of track `n` (from 1) of a disc without recordings:
    /// 220 Hz, 330 Hz, 440 Hz and so on, so that tracks can be told
    /// apart by ear and by a test.
    pub fn tone_hz(n: usize) -> u32 {
        110 * (n as u32 + 1)
    }

    /// `out.len() / 2` stereo samples from sample `start` of the disc.
    pub fn samples(&self, start: u64, out: &mut [i16]) {
        match &self.sound {
            Some(sound) => {
                for (i, o) in out.iter_mut().enumerate() {
                    *o = sound.get(start as usize * 2 + i).copied().unwrap_or(0);
                }
            }
            None => {
                for (i, pair) in out.chunks_mut(2).enumerate() {
                    let n = start + i as u64;
                    let track = self.track_at((n / SAMPLES_PER_FRAME) as u32);
                    let begin = self.tracks.get(track - 1).copied().unwrap_or(0) as u64 * SAMPLES_PER_FRAME;
                    let t = (n - begin.min(n)) as f64 / 44_100.0;
                    let v = ((t * Self::tone_hz(track) as f64 * std::f64::consts::TAU).sin() * 8000.0) as i16;
                    pair.fill(v);
                }
            }
        }
    }

    /// Track number (from 1) playing at `frame`.
    fn track_at(&self, frame: u32) -> usize {
        self.tracks.iter().rposition(|&t| t <= frame).map_or(1, |i| i + 1)
    }
}

/// What the drive's audio player is doing, as READ SUB-CHANNEL reports
/// it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AudioStatus {
    /// Nothing to report (0x15).
    None,
    Playing,
    Paused,
    /// Reached the end of what it was asked to play (0x13).
    Completed,
}

/// A data disc: an image in memory, or a (possibly very large) file read
/// on demand, such as a navigation DVD.
#[derive(Clone)]
pub enum Disc {
    Mem(std::sync::Arc<Vec<u8>>),
    File { file: std::sync::Arc<std::sync::Mutex<std::fs::File>>, offset: u64, sectors: u32 },
    /// An image somewhere else, read through a function: bytes from an
    /// offset into a buffer, returning how many it got.  (In a browser:
    /// a file the user picked, which never leaves their machine.)
    Source { read: Reader, offset: u64, sectors: u32 },
}

/// Reads bytes of an image from an offset; see [`Disc::Source`].
pub type Reader = std::sync::Arc<dyn Fn(u64, &mut [u8]) -> usize + Send + Sync>;

/// Where in an image of `len` bytes the disc's volume is, and how many
/// sectors it has: the whole image, or the partition of type 0x96 of an
/// SD card prepared for the 193, cut to the size the volume descriptor
/// gives.
fn locate(len: u64, read: &mut dyn FnMut(u64, &mut [u8]) -> bool) -> (u64, u32) {
    let mut mbr = [0u8; 512];
    let (mut offset, mut bytes) = (0u64, len);
    if len >= 512 && read(0, &mut mbr) && mbr[510] == 0x55 && mbr[511] == 0xaa {
        for e in mbr[446..510].chunks(16) {
            let start = u32::from_le_bytes([e[8], e[9], e[10], e[11]]) as u64 * 512;
            let size = u32::from_le_bytes([e[12], e[13], e[14], e[15]]) as u64 * 512;
            if e[4] == 0x96 && start < len {
                offset = start;
                bytes = size.min(len - start);
                break;
            }
        }
    }
    // The volume descriptor knows the real size.
    let mut pvd = [0u8; 2048];
    if read(offset + 16 * 2048, &mut pvd) && &pvd[1..6] == b"CD001" {
        let n = u32::from_le_bytes([pvd[80], pvd[81], pvd[82], pvd[83]]) as u64 * 2048;
        if n != 0 && n <= bytes {
            bytes = n;
        }
    }
    (offset, (bytes / 2048) as u32)
}

impl Disc {
    /// Open an image file.  Besides a plain ISO image this accepts the
    /// image of an SD card prepared for the 193 unit, which carries the
    /// navigation DVD's volume in a partition of type 0x96; the disc is
    /// then that partition.
    pub fn open(path: &str) -> std::io::Result<Disc> {
        use std::io::{Read, Seek, SeekFrom};
        let mut file = std::fs::File::open(path)?;
        let len = file.seek(SeekFrom::End(0))?;
        let (offset, sectors) = locate(len, &mut |at, buf| file.seek(SeekFrom::Start(at)).is_ok() && file.read_exact(buf).is_ok());
        Ok(Disc::File { file: std::sync::Arc::new(std::sync::Mutex::new(file)), offset, sectors })
    }

    /// An image of `len` bytes read through `read`, taken the way
    /// [`open`](Self::open) takes a file.
    pub fn from_reader(len: u64, read: Reader) -> Disc {
        let (offset, sectors) = locate(len, &mut |at, buf| read(at, buf) == buf.len());
        Disc::Source { read, offset, sectors }
    }

    pub fn sectors(&self) -> u32 {
        match self {
            Disc::Mem(v) => (v.len() / 2048) as u32,
            Disc::File { sectors, .. } => *sectors,
            Disc::Source { sectors, .. } => *sectors,
        }
    }

    /// `n` sectors from `lba` (zeros where the image ends early).
    pub fn read(&self, lba: u32, n: u32) -> Vec<u8> {
        let mut out = vec![0u8; n as usize * 2048];
        match self {
            Disc::Mem(v) => {
                let s = (lba as usize * 2048).min(v.len());
                let e = (s + out.len()).min(v.len());
                out[..e - s].copy_from_slice(&v[s..e]);
            }
            Disc::Source { read, offset, .. } => {
                read(offset + lba as u64 * 2048, &mut out);
            }
            Disc::File { file, offset, .. } => {
                use std::io::{Read, Seek, SeekFrom};
                let mut f = file.lock().unwrap();
                if f.seek(SeekFrom::Start(offset + lba as u64 * 2048)).is_ok() {
                    let mut got = 0;
                    while got < out.len() {
                        match f.read(&mut out[got..]) {
                            Ok(0) | Err(_) => break,
                            Ok(k) => got += k,
                        }
                    }
                }
            }
        }
        out
    }
}

impl From<Vec<u8>> for Disc {
    fn from(v: Vec<u8>) -> Disc {
        Disc::Mem(std::sync::Arc::new(v))
    }
}

pub struct Atapi {
    pub image: Option<Disc>,
    /// An audio disc in the drive instead of a data disc.
    pub audio: Option<AudioDisc>,
    pub audio_status: AudioStatus,
    /// Where the pickup is on an audio disc, in thousandths of a frame,
    /// and where playing stops.
    /// Where the pickup is, in stereo samples from the start of the
    /// programme area, and the fraction of a sample time has got ahead.
    audio_sample: u64,
    audio_micros: u64,
    audio_end: u32,
    error: u8,
    features: u8,
    /// Sector count / interrupt reason.
    count: u8,
    lba_low: u8,
    cyl_lo: u8,
    cyl_hi: u8,
    drive: u8,
    status: u8,
    control: u8,
    irq_line: bool,
    phase: Phase,
    packet: [u8; 12],
    pkt_pos: usize,
    /// Data for the current command and the read position in it.
    buf: Vec<u8>,
    pos: usize,
    /// End of the chunk currently offered with DRQ.
    chunk_end: usize,
    /// Byte count limit the host asked for.
    limit: usize,
    /// The current packet command asked for DMA data transfer.
    dma: bool,
    sense: (u8, u8, u8),
    unit_attention: bool,
    /// The drive raises its interrupt when it is ready to take the
    /// twelve bytes of a packet command (the older of the two behaviours
    /// ATAPI allows; the 193's software waits for it).
    pub packet_irq: bool,
    pub log: IoLog,
    pub trace: bool,
    /// Set when a traced packet command starts, for the machine to add
    /// where the firmware was (see `Machine::run`).
    pub trace_mark: bool,
    /// Count of sectors read, for statistics.
    pub sectors_read: u64,
}

impl Default for Atapi {
    fn default() -> Self {
        Self::new()
    }
}

impl Atapi {
    pub fn new() -> Self {
        let mut a = Atapi {
            image: None,
            error: 0,
            features: 0,
            count: 0,
            lba_low: 0,
            cyl_lo: 0,
            cyl_hi: 0,
            drive: 0,
            status: 0,
            control: 0,
            irq_line: false,
            phase: Phase::Idle,
            packet: [0; 12],
            pkt_pos: 0,
            buf: Vec::new(),
            pos: 0,
            chunk_end: 0,
            limit: 0,
            dma: false,
            sense: (0, 0, 0),
            unit_attention: false,
            packet_irq: false,
            audio: None,
            audio_status: AudioStatus::None,
            audio_sample: 0,
            audio_micros: 0,
            audio_end: 0,
            log: IoLog::new(false, 4),
            trace: false,
            trace_mark: false,
            sectors_read: 0,
        };
        a.signature();
        a
    }

    /// Change the disc while running (or remove it with `None`); the
    /// next command reports a unit attention, as after a real swap.
    pub fn insert(&mut self, image: Option<Vec<u8>>) {
        self.insert_disc(image.map(Disc::from));
    }

    /// Like [`insert`](Self::insert), for any kind of data disc.
    pub fn insert_disc(&mut self, image: Option<Disc>) {
        self.image = image;
        self.audio = None;
        self.audio_status = AudioStatus::None;
        self.unit_attention = true;
    }

    /// Put an audio disc in the drive (while running: with a unit
    /// attention, as after a real swap).
    pub fn insert_audio(&mut self, disc: AudioDisc, running: bool) {
        self.image = None;
        self.audio = Some(disc);
        self.audio_status = AudioStatus::None;
        self.audio_sample = 0;
        self.unit_attention = running;
    }

    /// The data disc is a DVD: it holds more than fits on a CD.
    fn is_dvd(&self) -> bool {
        self.image.as_ref().is_some_and(|d| d.sectors() > 450_000)
    }

    fn has_disc(&self) -> bool {
        self.image.is_some() || self.audio.is_some()
    }

    /// Frame of an audio disc the pickup is at.
    pub fn audio_frame(&self) -> u32 {
        (self.audio_sample / SAMPLES_PER_FRAME) as u32
    }

    /// Let `ms` milliseconds pass for the audio player.
    pub fn tick(&mut self, ms: u64) {
        self.play_out(ms * 1000, None);
    }

    /// Let `micros` microseconds pass for the audio player.  What comes
    /// out of the drive's audio output in that time, 44.1 kHz stereo, is
    /// added to `out`: the disc while it plays, silence otherwise.
    pub fn play_out(&mut self, micros: u64, mut out: Option<&mut Vec<i16>>) {
        // Whole samples in this much time, the remainder carried over.
        self.audio_micros += micros * 441;
        let mut n = self.audio_micros / 10_000;
        self.audio_micros %= 10_000;
        if self.audio_status == AudioStatus::Playing {
            let end = self.audio_end as u64 * SAMPLES_PER_FRAME;
            let playing = n.min(end.saturating_sub(self.audio_sample));
            if let (Some(out), Some(disc)) = (out.as_deref_mut(), self.audio.as_ref()) {
                let at = out.len();
                out.resize(at + playing as usize * 2, 0);
                disc.samples(self.audio_sample, &mut out[at..]);
            }
            self.audio_sample += playing;
            n -= playing;
            if self.audio_sample >= end {
                self.audio_status = AudioStatus::Completed;
            }
        }
        if let Some(out) = out {
            out.resize(out.len() + n as usize * 2, 0);
        }
    }

    fn play(&mut self, from: u32, to: u32) {
        let Some(disc) = self.audio.as_ref() else {
            self.fail(5, 0x64, 0); // illegal mode for this track
            return;
        };
        let to = to.min(disc.lead_out);
        if from >= disc.lead_out || from > to {
            self.fail(5, 0x21, 0);
            return;
        }
        self.audio_sample = from as u64 * SAMPLES_PER_FRAME;
        self.audio_end = to;
        self.audio_status = if from == to { AudioStatus::Completed } else { AudioStatus::Playing };
        self.complete_ok();
    }

    /// Put a disc in the drive before power-on.
    pub fn load(&mut self, image: Vec<u8>) {
        self.load_disc(image.into());
    }

    /// Like [`load`](Self::load), for any kind of data disc.
    pub fn load_disc(&mut self, image: Disc) {
        self.image = Some(image);
        self.unit_attention = false;
    }

    fn signature(&mut self) {
        self.count = 1;
        self.lba_low = 1;
        self.cyl_lo = 0x14;
        self.cyl_hi = 0xeb;
        self.drive &= 0x10;
        self.error = 1;
        self.status = ST_DRDY | ST_DSC;
        self.phase = Phase::Idle;
    }

    /// State of the INTRQ line (masked by nIEN).
    pub fn irq(&self) -> bool {
        self.irq_line && self.control & 2 == 0
    }

    fn sectors(&self) -> u32 {
        match self.audio.as_ref() {
            Some(disc) => disc.lead_out,
            None => self.image.as_ref().map(|i| i.sectors()).unwrap_or(0),
        }
    }

    // ---- register access --------------------------------------------------

    /// Read a command block register (0 = data, 1..7) or the alternate
    /// status (reg 8).
    pub fn read(&mut self, reg: u32, size: u32, pc: u32) -> u32 {
        let v = match reg {
            0 => self.read_data(size),
            1 => self.error as u32,
            2 => self.count as u32,
            3 => self.lba_low as u32,
            4 => self.cyl_lo as u32,
            5 => self.cyl_hi as u32,
            6 => self.drive as u32,
            7 => {
                self.irq_line = false;
                self.status as u32
            }
            _ => self.status as u32,
        };
        if reg != 0 {
            self.log.log("atapi", false, reg, 1, v, pc);
        }
        v
    }

    pub fn write(&mut self, reg: u32, size: u32, v: u32, pc: u32) {
        if reg != 0 {
            self.log.log("atapi", true, reg, 1, v, pc);
        }
        match reg {
            0 => self.write_data(size, v),
            1 => self.features = v as u8,
            2 => self.count = v as u8,
            3 => self.lba_low = v as u8,
            4 => self.cyl_lo = v as u8,
            5 => self.cyl_hi = v as u8,
            6 => self.drive = v as u8,
            7 => self.command(v as u8),
            _ => {
                let old = self.control;
                self.control = v as u8;
                if old & 4 != 0 && v & 4 == 0 {
                    // SRST released
                    self.signature();
                    self.irq_line = false;
                }
            }
        }
    }

    fn read_data(&mut self, size: u32) -> u32 {
        if self.phase != Phase::DataIn {
            return 0;
        }
        let mut v = 0u32;
        // The board wires the drive so that the first byte of each pair
        // is the high half of the 16-bit word, i.e. data arrives in
        // memory order on this big-endian CPU.
        for _ in 0..size.max(1) {
            let b = if self.pos < self.chunk_end { self.buf[self.pos] } else { 0 };
            self.pos += 1;
            v = (v << 8) | b as u32;
        }
        if self.pos >= self.chunk_end {
            self.next_chunk();
        }
        v
    }

    fn write_data(&mut self, size: u32, v: u32) {
        match self.phase {
            Phase::Packet => {
                let n = size.max(1) as usize;
                for i in 0..n {
                    if self.pkt_pos < 12 {
                        self.packet[self.pkt_pos] = (v >> (8 * (n - 1 - i))) as u8;
                        self.pkt_pos += 1;
                    }
                }
                if self.pkt_pos >= 12 {
                    self.run_packet();
                }
            }
            Phase::DataOut => {
                let n = size.max(1) as usize;
                for i in 0..n {
                    if self.pos < self.buf.len() {
                        self.buf[self.pos] = (v >> (8 * (n - 1 - i))) as u8;
                    }
                    self.pos += 1;
                }
                if self.pos >= self.chunk_end {
                    self.complete_ok();
                }
            }
            _ => {}
        }
    }

    /// DMARQ: the drive has data for the host's DMA controller.
    pub fn dma_request(&self) -> bool {
        self.phase == Phase::DataInDma
    }

    /// One DMA data cycle (DMACK asserted), `size` bytes wide.
    pub fn dma_read(&mut self, size: u32) -> u32 {
        if self.phase != Phase::DataInDma {
            return 0;
        }
        let mut v = 0u32;
        for _ in 0..size.max(1) {
            let b = if self.pos < self.buf.len() { self.buf[self.pos] } else { 0 };
            self.pos += 1;
            v = (v << 8) | b as u32;
        }
        if self.pos >= self.buf.len() {
            self.complete_ok();
        }
        v
    }

    // ---- ATA commands -------------------------------------------------------

    fn command(&mut self, cmd: u8) {
        self.irq_line = false;
        if self.drive & 0x10 != 0 {
            // No slave device.
            return;
        }
        if self.trace {
            eprintln!("[atapi] ATA command {:02x} features={:02x} count={:02x}", cmd, self.features, self.count);
        }
        match cmd {
            0xa0 => {
                // PACKET
                self.limit = (self.cyl_lo as usize) | ((self.cyl_hi as usize) << 8);
                if self.limit == 0 || self.limit == 0xffff {
                    self.limit = 0xfffe;
                }
                self.dma = self.features & 1 != 0;
                self.phase = Phase::Packet;
                self.pkt_pos = 0;
                self.count = IR_COD;
                self.status = ST_DRDY | ST_DSC | ST_DRQ;
                self.irq_line = self.packet_irq;
            }
            0xa1 => {
                // IDENTIFY PACKET DEVICE
                let mut id = [0u16; 256];
                id[0] = 0x85c0; // ATAPI, CD-ROM, removable, 12 byte packets
                put_str(&mut id[10..20], "RNSE0000000000000000");
                put_str(&mut id[23..27], "1.00    ");
                put_str(&mut id[27..47], "RNSE EMULATED DVD-ROM                   ");
                id[49] = 0x0b00; // LBA, DMA, IORDY
                id[51] = 0x0200;
                id[53] = 0x0006;
                id[63] = 0x0007;
                id[64] = 0x0003;
                id[80] = 0x001e;
                id[88] = 0x0007;
                self.buf = id.iter().flat_map(|w| w.to_le_bytes()).collect();
                self.pos = 0;
                self.chunk_end = 512;
                self.phase = Phase::DataIn;
                self.status = ST_DRDY | ST_DSC | ST_DRQ;
                self.irq_line = true;
            }
            0xef => {
                // SET FEATURES: accept everything.
                self.status = ST_DRDY | ST_DSC;
                self.error = 0;
                self.irq_line = true;
            }
            0x08 => {
                // DEVICE RESET
                self.signature();
            }
            0x00 | 0xe0 | 0xe1 | 0xe2 | 0xe3 | 0xe5 | 0xe6 => {
                // NOP / power management
                self.count = 0xff;
                self.status = ST_DRDY | ST_DSC;
                self.error = 0;
                self.irq_line = true;
            }
            _ => {
                // Includes IDENTIFY DEVICE (0xec), which a packet device
                // aborts after placing its signature.
                self.signature();
                self.error = 0x04;
                self.status = ST_DRDY | ST_DSC | ST_ERR;
                self.irq_line = true;
            }
        }
    }

    // ---- packet commands ----------------------------------------------------

    fn complete_ok(&mut self) {
        self.phase = Phase::Idle;
        self.count = IR_COD | IR_IO;
        self.error = 0;
        self.status = ST_DRDY | ST_DSC;
        self.irq_line = true;
    }

    fn fail(&mut self, key: u8, asc: u8, ascq: u8) {
        self.sense = (key, asc, ascq);
        self.phase = Phase::Idle;
        self.count = IR_COD | IR_IO;
        self.error = key << 4;
        self.status = ST_DRDY | ST_DSC | ST_ERR;
        self.irq_line = true;
    }

    /// Offer the next piece of `buf` to the host, or finish the command.
    fn next_chunk(&mut self) {
        if self.pos >= self.buf.len() {
            self.complete_ok();
            return;
        }
        if self.dma {
            // The whole transfer goes through the DMA port; the host is
            // interrupted only when it is complete.
            self.chunk_end = self.buf.len();
            self.status = ST_DRDY | ST_DSC;
            self.phase = Phase::DataInDma;
            return;
        }
        let mut n = (self.buf.len() - self.pos).min(self.limit);
        if n > 1 && n % 2 == 1 && self.pos + n < self.buf.len() {
            n -= 1;
        }
        self.chunk_end = self.pos + n;
        self.cyl_lo = n as u8;
        self.cyl_hi = (n >> 8) as u8;
        self.count = IR_IO;
        self.status = ST_DRDY | ST_DSC | ST_DRQ;
        self.phase = Phase::DataIn;
        self.irq_line = true;
    }

    fn data_in(&mut self, mut data: Vec<u8>, alloc: usize) {
        data.truncate(alloc);
        self.buf = data;
        self.pos = 0;
        self.next_chunk();
    }

    fn read_sectors(&mut self, lba: u32, n: u32) {
        let total = self.sectors();
        if n == 0 {
            self.complete_ok();
            return;
        }
        if lba >= total || lba + n > total {
            self.fail(5, 0x21, 0); // LBA out of range
            return;
        }
        let Some(img) = self.image.as_ref() else {
            self.fail(5, 0x64, 0); // an audio track cannot be read as data
            return;
        };
        self.buf = img.read(lba, n);
        self.pos = 0;
        self.sectors_read += n as u64;
        self.next_chunk();
    }

    fn run_packet(&mut self) {
        let p = self.packet;
        if self.trace {
            eprintln!("[atapi] packet {:02x?}", p);
            self.trace_mark = !matches!(p[0], 0x00 | 0x42);
        }
        let be16 = |i: usize| ((p[i] as usize) << 8) | p[i + 1] as usize;
        let be32 = |i: usize| ((p[i] as u32) << 24) | ((p[i + 1] as u32) << 16) | ((p[i + 2] as u32) << 8) | p[i + 3] as u32;
        let op = p[0];
        // REQUEST SENSE and INQUIRY work without a medium and do not
        // consume a pending unit attention.
        if op != 0x03 && op != 0x12 {
            if self.unit_attention {
                self.unit_attention = false;
                if self.has_disc() {
                    self.fail(6, 0x28, 0); // medium may have changed
                    return;
                }
            }
            let needs_medium = matches!(op, 0x00 | 0x25 | 0x28 | 0xa8 | 0x2b | 0x43 | 0x42 | 0x51 | 0xbe | 0xb9 | 0xad);
            let needs_medium = needs_medium || matches!(op, 0x45 | 0x47 | 0x48 | 0x4b | 0x4e | 0xa5 | 0xbc);
            if needs_medium && !self.has_disc() {
                self.fail(2, 0x3a, 0); // medium not present
                return;
            }
        }
        match op {
            0x00 => self.complete_ok(), // TEST UNIT READY
            0x03 => {
                // REQUEST SENSE
                let mut d = vec![0u8; 18];
                d[0] = 0x70;
                d[2] = self.sense.0;
                d[7] = 10;
                d[12] = self.sense.1;
                d[13] = self.sense.2;
                self.sense = (0, 0, 0);
                self.data_in(d, p[4] as usize);
            }
            0x12 => {
                // INQUIRY
                let mut d = vec![0u8; 36];
                d[0] = 0x05;
                d[1] = 0x80;
                d[2] = 0x00;
                d[3] = 0x21;
                d[4] = 31;
                d[8..16].copy_from_slice(b"RNSE    ");
                d[16..32].copy_from_slice(b"EMULATED DVD-ROM");
                d[32..36].copy_from_slice(b"1.00");
                self.data_in(d, p[4] as usize);
            }
            0x1b | 0x2b => {
                // START STOP UNIT, SEEK: playing ends
                if self.audio.is_some() {
                    if op == 0x2b {
                        self.audio_sample = be32(2) as u64 * SAMPLES_PER_FRAME;
                    }
                    self.audio_status = AudioStatus::None;
                }
                self.complete_ok();
            }
            0x1e | 0xbb | 0x35 => self.complete_ok(), // PREVENT, SET SPEED, SYNC
            0x25 => {
                // READ CAPACITY
                let last = self.sectors().saturating_sub(1);
                let mut d = Vec::new();
                d.extend_from_slice(&last.to_be_bytes());
                d.extend_from_slice(&2048u32.to_be_bytes());
                self.data_in(d, 8);
            }
            0x28 => self.read_sectors(be32(2), be16(7) as u32),
            0xa8 => self.read_sectors(be32(2), be32(6)),
            0xbe => {
                // READ CD: user data only
                let n = ((p[6] as u32) << 16) | ((p[7] as u32) << 8) | p[8] as u32;
                self.read_sectors(be32(2), n);
            }
            0x43 => {
                // READ TOC: one data track, formats 0 (TOC) and 1 (session)
                let msf = p[1] & 2 != 0;
                let format = if p[2] & 0xf != 0 { p[2] & 0xf } else { p[9] >> 6 };
                let addr = |lba: u32| -> [u8; 4] {
                    if msf {
                        let f = lba + 150;
                        [0, (f / 75 / 60) as u8, (f / 75 % 60) as u8, (f % 75) as u8]
                    } else {
                        lba.to_be_bytes()
                    }
                };
                // Tracks as (control and ADR, start); a data disc has one.
                let tracks: Vec<(u8, u32)> = match self.audio.as_ref() {
                    Some(disc) => disc.tracks.iter().map(|&t| (0x10, t)).collect(),
                    None => vec![(0x14, 0)],
                };
                let mut d = vec![0u8, 0, 1, tracks.len() as u8];
                if format == 2 {
                    // Full TOC, the lead-in's Q channel: points A0 (first
                    // track), A1 (last track), A2 (lead-out), then the
                    // tracks; always in minutes, seconds and frames.
                    d[3] = 1; // last session
                    let msf = |frame: u32| {
                        let f = frame + 150;
                        [(f / 75 / 60) as u8, (f / 75 % 60) as u8, (f % 75) as u8]
                    };
                    let control = tracks[0].0;
                    let mut point = |control: u8, point: u8, at: [u8; 3]| {
                        d.extend_from_slice(&[1, control, 0, point, 0, 0, 0, 0, at[0], at[1], at[2]]);
                    };
                    point(control, 0xa0, [1, 0, 0]);
                    point(control, 0xa1, [tracks.len() as u8, 0, 0]);
                    point(control, 0xa2, msf(self.sectors()));
                    for (i, (control, start)) in tracks.iter().enumerate() {
                        point(*control, i as u8 + 1, msf(*start));
                    }
                } else if format == 1 {
                    d.extend_from_slice(&[0, tracks[0].0, 1, 0]);
                    d.extend_from_slice(&addr(tracks[0].1));
                } else {
                    for (i, (control, start)) in tracks.iter().enumerate() {
                        if i + 1 >= p[6] as usize {
                            d.extend_from_slice(&[0, *control, i as u8 + 1, 0]);
                            d.extend_from_slice(&addr(*start));
                        }
                    }
                    d.extend_from_slice(&[0, tracks[tracks.len() - 1].0, 0xaa, 0]);
                    d.extend_from_slice(&addr(self.sectors()));
                }
                let len = (d.len() - 2) as u16;
                d[0..2].copy_from_slice(&len.to_be_bytes());
                self.data_in(d, be16(7));
            }
            // READ SUB-CHANNEL is a CD command: a DVD has no sub-channel.
            // The 193's software tells a DVD from a CD by this refusal.
            0x42 if self.is_dvd() => self.fail(5, 0x64, 0),
            0xad => {
                // READ DVD STRUCTURE, format 0: physical format of layer 0
                if !self.is_dvd() || p[7] != 0 {
                    self.fail(5, 0x24, 0);
                    return;
                }
                let mut d = vec![0u8; 21];
                d[1] = 19;
                d[4] = 0x01; // DVD-ROM, version 1
                d[5] = 0x02; // 120 mm, 10.08 Mbit/s
                d[6] = 0x01; // one layer, read-only
                let first = 0x30000u32;
                let last = first + self.sectors() - 1;
                d[8..12].copy_from_slice(&first.to_be_bytes());
                d[12..16].copy_from_slice(&last.to_be_bytes());
                self.data_in(d, be16(8));
            }
            0x42 => {
                // READ SUB-CHANNEL, current position
                let msf = p[1] & 2 != 0;
                let addr = |frame: u32, lead_in: u32| -> [u8; 4] {
                    if msf {
                        let f = frame + lead_in;
                        [0, (f / 75 / 60) as u8, (f / 75 % 60) as u8, (f % 75) as u8]
                    } else {
                        frame.to_be_bytes()
                    }
                };
                let mut d = vec![0u8; 16];
                d[1] = match self.audio_status {
                    AudioStatus::None => 0x15,
                    AudioStatus::Playing => 0x11,
                    AudioStatus::Paused => 0x12,
                    AudioStatus::Completed => 0x13,
                };
                d[3] = 12;
                d[4] = 1;
                match self.audio.as_ref() {
                    Some(disc) => {
                        let frame = self.audio_frame();
                        let track = disc.track_at(frame);
                        d[5] = 0x10;
                        d[6] = track as u8;
                        d[7] = 1;
                        d[8..12].copy_from_slice(&addr(frame, 150));
                        d[12..16].copy_from_slice(&addr(frame - disc.tracks[track - 1], 0));
                    }
                    None => {
                        d[5] = 0x14;
                        d[6] = 1;
                        d[7] = 1;
                    }
                }
                // A finished play is reported once.
                if self.audio_status == AudioStatus::Completed {
                    self.audio_status = AudioStatus::None;
                }
                self.data_in(d, be16(7));
            }
            // PLAY AUDIO (10) and (12): start frame and length
            0x45 | 0xa5 => {
                let len = if op == 0x45 { be16(7) as u32 } else { be32(6) };
                self.play(be32(2), be32(2).saturating_add(len));
            }
            // PLAY CD: like PLAY AUDIO, with the addresses as frames or
            // (bit 1 of byte 1) as minutes, seconds and frames
            0xbc => {
                if p[1] & 2 != 0 {
                    let frame = |i: usize| (p[i] as u32 * 60 + p[i + 1] as u32) * 75 + p[i + 2] as u32;
                    self.play(frame(3).saturating_sub(150), frame(6).saturating_sub(150));
                } else {
                    self.play(be32(2), be32(2).saturating_add(be32(6)));
                }
            }
            // PLAY AUDIO MSF
            0x47 => {
                let frame = |i: usize| (p[i] as u32 * 60 + p[i + 1] as u32) * 75 + p[i + 2] as u32;
                self.play(frame(3).saturating_sub(150), frame(6).saturating_sub(150));
            }
            // PLAY AUDIO TRACK INDEX
            0x48 => match self.audio.clone() {
                Some(disc) => {
                    let start = |t: u8| disc.tracks.get((t.max(1) - 1) as usize).copied();
                    match start(p[4]) {
                        Some(from) => self.play(from, start(p[7].saturating_add(1)).unwrap_or(disc.lead_out)),
                        None => self.fail(5, 0x21, 0),
                    }
                }
                None => self.fail(5, 0x64, 0),
            },
            // PAUSE / RESUME
            0x4b => {
                match (self.audio_status, p[8] & 1 != 0) {
                    (AudioStatus::Playing, false) => self.audio_status = AudioStatus::Paused,
                    (AudioStatus::Paused, true) => self.audio_status = AudioStatus::Playing,
                    _ => {}
                }
                self.complete_ok();
            }
            // STOP PLAY / SCAN
            0x4e => {
                self.audio_status = AudioStatus::None;
                self.complete_ok();
            }
            0x46 => {
                // GET CONFIGURATION: feature header only, current profile
                let mut d = vec![0u8; 8];
                d[3] = 4;
                d[7] = if self.is_dvd() {
                    0x10 // DVD-ROM
                } else if self.has_disc() {
                    0x08 // CD-ROM
                } else {
                    0
                };
                self.data_in(d, be16(7));
            }
            0x4a => {
                // GET EVENT STATUS NOTIFICATION (media class)
                let present = self.has_disc();
                let d = vec![0, 6, 4, 0x10, 0, if present { 2 } else { 0 }, 0, 0];
                self.data_in(d, be16(7));
            }
            0x51 => {
                // READ DISC INFORMATION
                let mut d = vec![0u8; 34];
                d[1] = 32;
                d[2] = 0x0e; // complete session, finalized
                d[3] = 1;
                d[4] = 1;
                d[5] = 1;
                d[6] = 1;
                self.data_in(d, be16(7));
            }
            0x5a | 0x1a => {
                // MODE SENSE: empty parameter list / CD capabilities page
                let page = p[2] & 0x3f;
                let mut d = vec![0u8; 8];
                if page == 0x2a || page == 0x3f {
                    let mut pg = vec![0u8; 22];
                    pg[0] = 0x2a;
                    pg[1] = 20;
                    pg[2] = 0x3b; // reads CD-R/RW, DVD-ROM
                    pg[4] = 0x71;
                    pg[6] = 0x29;
                    pg[8] = 0x1b;
                    pg[9] = 0x90;
                    pg[14] = 0x1b;
                    pg[15] = 0x90;
                    d.extend_from_slice(&pg);
                }
                if page == 0x01 || page == 0x3f {
                    d.extend_from_slice(&[0x01, 0x0a, 0, 3, 0, 0, 0, 0, 0, 0, 0, 0]);
                }
                let len = (d.len() - 2) as u16;
                d[0..2].copy_from_slice(&len.to_be_bytes());
                let alloc = if op == 0x5a { be16(7) } else { p[4] as usize };
                self.data_in(d, alloc);
            }
            0x55 | 0x15 => {
                // MODE SELECT: swallow the parameter list
                let n = if op == 0x55 { be16(7) } else { p[4] as usize };
                if n == 0 {
                    self.complete_ok();
                } else {
                    self.buf = vec![0; n];
                    self.pos = 0;
                    self.chunk_end = n;
                    self.cyl_lo = n as u8;
                    self.cyl_hi = (n >> 8) as u8;
                    self.count = 0;
                    self.status = ST_DRDY | ST_DSC | ST_DRQ;
                    self.phase = Phase::DataOut;
                    self.irq_line = true;
                }
            }
            0xbd => {
                // MECHANISM STATUS
                self.data_in(vec![0u8; 8], be16(8));
            }
            _ => {
                if self.trace {
                    eprintln!("[atapi] unsupported packet command {:02x}", op);
                }
                self.fail(5, 0x20, 0); // invalid command operation code
            }
        }
    }
}

fn put_str(words: &mut [u16], s: &str) {
    let b = s.as_bytes();
    for (i, w) in words.iter_mut().enumerate() {
        let hi = b.get(i * 2).copied().unwrap_or(b' ');
        let lo = b.get(i * 2 + 1).copied().unwrap_or(b' ');
        *w = ((hi as u16) << 8) | lo as u16;
    }
}

#[allow(dead_code)]
const _: u8 = ST_BSY;

#[cfg(test)]
mod tests {
    use super::*;

    /// Send a packet command and collect what the drive returns.
    fn packet(a: &mut Atapi, p: [u8; 12]) -> Vec<u8> {
        a.write(4, 1, 0xfe, 0);
        a.write(5, 1, 0xff, 0);
        a.write(7, 1, 0xa0, 0);
        for pair in p.chunks(2) {
            a.write(0, 2, (pair[0] as u32) << 8 | pair[1] as u32, 0);
        }
        let mut out = Vec::new();
        while a.read(7, 1, 0) & 0x08 != 0 {
            let w = a.read(0, 2, 0);
            out.extend_from_slice(&[(w >> 8) as u8, w as u8]);
        }
        out
    }

    /// What the drive plays comes out as sound: the track's tone while
    /// it plays, silence when it does not, a sample for every moment.
    #[test]
    fn audio_comes_out() {
        let mut a = Atapi::new();
        a.insert_audio(AudioDisc::uniform(3, 10), false);
        let mut out = Vec::new();
        a.play_out(100_000, Some(&mut out));
        assert_eq!(out.len(), 4410 * 2);
        assert!(out.iter().all(|s| *s == 0), "nothing plays yet");
        // PLAY AUDIO MSF from the start of track 2 (00:12:00) to the end.
        packet(&mut a, [0x47, 0, 0, 0, 12, 0, 0, 32, 0, 0, 0, 0]);
        out.clear();
        a.play_out(1_000_000, Some(&mut out));
        assert_eq!(out.len(), 44_100 * 2);
        assert_eq!(a.audio_frame(), 10 * 75 + 75);
        // Track 2's tone: 330 Hz, so 660 changes of sign in a second.
        let left: Vec<i16> = out.chunks(2).map(|p| p[0]).collect();
        let crossings = left.windows(2).filter(|w| (w[0] < 0) != (w[1] < 0)).count();
        assert!((655..=665).contains(&crossings), "{} zero crossings", crossings);
        assert!(out.chunks(2).all(|p| p[0] == p[1]));
        // Paused: silence, and the position stays.
        packet(&mut a, [0x4b, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        out.clear();
        a.play_out(500_000, Some(&mut out));
        assert!(out.len() == 22_050 * 2 && out.iter().all(|s| *s == 0));
        assert_eq!(a.audio_frame(), 10 * 75 + 75);

        // A disc of recordings plays the recordings.
        let ramp: Vec<i16> = (0..2 * 588 * 3).map(|i| i as i16).collect();
        let mut a = Atapi::new();
        a.insert_audio(AudioDisc::from_tracks(&[vec![7; 100], ramp.clone()]), false);
        // Track 2 starts at frame 1 (the first track is padded to one).
        packet(&mut a, [0x45, 0, 0, 0, 0, 1, 0, 0, 3, 0, 0, 0]);
        out.clear();
        a.play_out(40_000, Some(&mut out));
        assert_eq!(out.len(), 1764 * 2);
        assert_eq!(out[..], ramp[..1764 * 2]);
    }

    #[test]
    fn audio_disc_plays() {
        let mut a = Atapi::new();
        a.insert_audio(AudioDisc::uniform(3, 60), false);
        // Table of contents: three audio tracks and the lead-out.
        let toc = packet(&mut a, [0x43, 2, 0, 0, 0, 0, 0, 0, 64, 0, 0, 0]);
        assert_eq!(toc[2..4], [1, 3]);
        assert_eq!(toc[4..12], [0, 0x10, 1, 0, 0, 0, 2, 0], "track 1 at 00:02:00");
        assert_eq!(toc[12..20], [0, 0x10, 2, 0, 0, 1, 2, 0]);
        assert_eq!(toc[28..36], [0, 0x10, 0xaa, 0, 0, 3, 2, 0]);
        // Data cannot be read from it.
        packet(&mut a, [0x28, 0, 0, 0, 0, 16, 0, 0, 1, 0, 0, 0]);
        assert_ne!(a.read(7, 1, 0) & 1, 0);

        // Play track 2 to the end of the disc, look after ten seconds.
        packet(&mut a, [0x48, 0, 0, 0, 2, 1, 0, 3, 1, 0, 0, 0]);
        assert_eq!(a.audio_status, AudioStatus::Playing);
        a.tick(10_000);
        let sub = packet(&mut a, [0x42, 2, 0x40, 1, 0, 0, 0, 0, 16, 0, 0, 0]);
        assert_eq!(sub[1], 0x11);
        assert_eq!(sub[5..8], [0x10, 2, 1]);
        assert_eq!(sub[8..12], [0, 1, 12, 0], "absolute 01:12:00");
        assert_eq!(sub[12..16], [0, 0, 10, 0], "ten seconds into the track");

        // Pause holds the position, resume goes on, the end is reported once.
        packet(&mut a, [0x4b, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        a.tick(5_000);
        assert_eq!(a.audio_frame(), 70 * 75);
        packet(&mut a, [0x4b, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0]);
        a.tick(200_000);
        assert_eq!(packet(&mut a, [0x42, 2, 0x40, 1, 0, 0, 0, 0, 16, 0, 0, 0])[1], 0x13);
        assert_eq!(packet(&mut a, [0x42, 2, 0x40, 1, 0, 0, 0, 0, 16, 0, 0, 0])[1], 0x15);
    }

    /// The image of an SD card with the navigation DVD's volume in a
    /// partition of type 0x96: the disc is that partition, it is a DVD
    /// by its size, and the drive says so the way the 193 asks.
    #[test]
    fn dvd_in_a_partition() {
        use std::io::{Seek, SeekFrom, Write};
        let path = std::env::temp_dir().join(format!("rnse-dvd-test-{}.img", std::process::id()));
        let (start, sectors) = (2048u64 * 512, 500_000u32);
        {
            let mut f = std::fs::File::create(&path).unwrap();
            let mut mbr = [0u8; 512];
            mbr[446 + 4] = 0x0c; // a FAT partition first, as on the cards
            mbr[446 + 8..446 + 12].copy_from_slice(&63u32.to_le_bytes());
            mbr[446 + 12..446 + 16].copy_from_slice(&1000u32.to_le_bytes());
            mbr[462 + 4] = 0x96;
            mbr[462 + 8..462 + 12].copy_from_slice(&2048u32.to_le_bytes());
            mbr[462 + 12..462 + 16].copy_from_slice(&(sectors * 4 + 800).to_le_bytes());
            mbr[510..].copy_from_slice(&[0x55, 0xaa]);
            f.write_all(&mbr).unwrap();
            // Primary volume descriptor, and a sector to recognise.
            let mut pvd = [0u8; 2048];
            pvd[0] = 1;
            pvd[1..6].copy_from_slice(b"CD001");
            pvd[80..84].copy_from_slice(&sectors.to_le_bytes());
            f.seek(SeekFrom::Start(start + 16 * 2048)).unwrap();
            f.write_all(&pvd).unwrap();
            f.seek(SeekFrom::Start(start + 499_999 * 2048)).unwrap();
            f.write_all(b"last sector").unwrap();
            // Sparse: a gigabyte that takes no room.
            f.set_len(start + (sectors as u64 * 4 + 800) * 512).unwrap();
        }
        let disc = Disc::open(path.to_str().unwrap()).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(disc.sectors(), sectors, "the volume's own size, not the partition's");
        assert_eq!(&disc.read(16, 1)[1..6], b"CD001");

        let mut a = Atapi::new();
        a.load_disc(disc);
        let last = packet(&mut a, [0x28, 0, 0, 0x07, 0xa1, 0x1f, 0, 0, 1, 0, 0, 0]);
        assert_eq!(&last[..11], b"last sector");
        assert_eq!(packet(&mut a, [0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])[..4], (sectors - 1).to_be_bytes());
        // No sub-channel on a DVD: illegal mode for this track.
        packet(&mut a, [0x42, 2, 0x40, 1, 0, 0, 0, 0, 16, 0, 0, 0]);
        assert_ne!(a.read(7, 1, 0) & 1, 0);
        let sense = packet(&mut a, [0x03, 0, 0, 0, 18, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!((sense[2], sense[12]), (5, 0x64));
        // Physical format: a DVD-ROM with one read-only layer.
        let pfi = packet(&mut a, [0xad, 0, 0, 0, 0, 0, 0, 0, 0, 0x14, 0, 0]);
        assert_eq!((pfi[4] & 0xf0, pfi[6] & 0x0f), (0, 1));

        // A CD-sized data disc is still a CD.
        let mut a = Atapi::new();
        a.load(vec![0; 64 * 2048]);
        assert_eq!(packet(&mut a, [0x42, 2, 0x40, 1, 0, 0, 0, 0, 16, 0, 0, 0])[1], 0x15);
        packet(&mut a, [0xad, 0, 0, 0, 0, 0, 0, 0, 0, 0x14, 0, 0]);
        assert_ne!(a.read(7, 1, 0) & 1, 0);
    }
}
