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

pub struct Atapi {
    pub image: Option<Vec<u8>>,
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
    pub log: IoLog,
    pub trace: bool,
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
            log: IoLog::new(false, 4),
            trace: false,
            sectors_read: 0,
        };
        a.signature();
        a
    }

    /// Change the disc while running (or remove it with `None`); the
    /// next command reports a unit attention, as after a real swap.
    pub fn insert(&mut self, image: Option<Vec<u8>>) {
        self.image = image;
        self.unit_attention = true;
    }

    /// Put a disc in the drive before power-on.
    pub fn load(&mut self, image: Vec<u8>) {
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
        self.image.as_ref().map(|i| (i.len() / 2048) as u32).unwrap_or(0)
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
        let img = self.image.as_ref().unwrap();
        let s = lba as usize * 2048;
        self.buf = img[s..s + n as usize * 2048].to_vec();
        self.pos = 0;
        self.sectors_read += n as u64;
        self.next_chunk();
    }

    fn run_packet(&mut self) {
        let p = self.packet;
        if self.trace {
            eprintln!("[atapi] packet {:02x?}", p);
        }
        let be16 = |i: usize| ((p[i] as usize) << 8) | p[i + 1] as usize;
        let be32 = |i: usize| ((p[i] as u32) << 24) | ((p[i + 1] as u32) << 16) | ((p[i + 2] as u32) << 8) | p[i + 3] as u32;
        let op = p[0];
        // REQUEST SENSE and INQUIRY work without a medium and do not
        // consume a pending unit attention.
        if op != 0x03 && op != 0x12 {
            if self.unit_attention {
                self.unit_attention = false;
                if self.image.is_some() {
                    self.fail(6, 0x28, 0); // medium may have changed
                    return;
                }
            }
            let needs_medium = matches!(op, 0x00 | 0x25 | 0x28 | 0xa8 | 0x2b | 0x43 | 0x42 | 0x51 | 0xbe | 0xb9 | 0xad);
            if needs_medium && self.image.is_none() {
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
            0x1b | 0x1e | 0x2b | 0xbb | 0x35 => self.complete_ok(), // START STOP, PREVENT, SEEK, SET SPEED, SYNC
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
                let mut d = vec![0u8, 0, 1, 1];
                if format == 1 {
                    d.extend_from_slice(&[0, 0x14, 1, 0]);
                    d.extend_from_slice(&addr(0));
                } else {
                    if p[6] <= 1 {
                        d.extend_from_slice(&[0, 0x14, 1, 0]);
                        d.extend_from_slice(&addr(0));
                    }
                    d.extend_from_slice(&[0, 0x14, 0xaa, 0]);
                    d.extend_from_slice(&addr(self.sectors()));
                }
                let len = (d.len() - 2) as u16;
                d[0..2].copy_from_slice(&len.to_be_bytes());
                self.data_in(d, be16(7));
            }
            0x42 => {
                // READ SUB-CHANNEL: no audio playing
                let mut d = vec![0u8; 16];
                d[1] = 0x15;
                d[3] = 12;
                d[4] = 1;
                d[5] = 0x14;
                d[6] = 1;
                d[7] = 1;
                self.data_in(d, be16(7));
            }
            0x46 => {
                // GET CONFIGURATION: feature header only, current profile
                let mut d = vec![0u8; 8];
                d[3] = 4;
                d[7] = if self.image.is_some() { 0x08 } else { 0 }; // CD-ROM
                self.data_in(d, be16(7));
            }
            0x4a => {
                // GET EVENT STATUS NOTIFICATION (media class)
                let present = self.image.is_some();
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
