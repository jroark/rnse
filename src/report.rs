//! What `sdk/examples/report` sends from a unit over the CAN bus, put
//! back together from a log of the frames: the registers it read, the
//! state it was started in, and the boot ROM.
//!
//! The frames (identifier, data):
//!
//! | id  | data |
//! |-----|------|
//! | 7F1 | `'R'`, unit (0x92 or 0x93), version, number of registers, length of the ROM (3 bytes), pass |
//! | 7F2 | index, size, address (4 bytes) |
//! | 7F3 | index, size, value (4 bytes) |
//! | 7F4 | 0, 0, 0, 0, CRC-32 of the ROM (4 bytes) |
//! | 7F0 | offset (3 bytes), 5 bytes of the ROM |
//!
//! After the registers come values under made-up addresses: 1 to 7 the
//! state the program was started in, and from version 2 on, on the 192,
//! 0x10-0x16 and 0x20-0x26 what its two card processors answered
//! (`sdk/examples/report/dsp.h`).
//!
//! The round is sent again and again, so a log may start anywhere and
//! may have lost frames; what one round lacks another supplies.

use crate::dev::hcan::Frame;
use std::collections::BTreeMap;

/// CRC-32 as zlib has it.
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut c = 0xffff_ffffu32;
    for b in bytes {
        c ^= u32::from(*b);
        for _ in 0..8 {
            c = (c >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(c & 1));
        }
    }
    !c
}

/// The frames in a log, whatever wrote it: `candump` in either of its
/// formats (`can0 7F0 [8] 00 11 ...` and `can0 7F0#0011...`), the
/// emulator's `--trace-can` (`[can] tx 7f0 [8] 00 11 ...`), or a table
/// with commas that has the identifier in the second column, the length
/// in the sixth and the bytes after it.  Lines that are none of these
/// are passed over.
pub fn frames(log: &str) -> Vec<Frame> {
    let hex = |t: &str, max: usize| (!t.is_empty() && t.len() <= max).then(|| u32::from_str_radix(t, 16).ok()).flatten();
    let mut out = Vec::new();
    for line in log.lines() {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        let mut found = None;
        // ID#DATA
        for t in &tokens {
            if let Some((id, data)) = t.split_once('#') {
                if let Some(id) = hex(id, 8) {
                    let bytes: Option<Vec<u8>> = (0..data.len() / 2).map(|i| u8::from_str_radix(data.get(2 * i..2 * i + 2)?, 16).ok()).collect();
                    if let (Some(bytes), true) = (bytes, data.len() % 2 == 0 && data.len() <= 16) {
                        found = Some((id, bytes));
                    }
                }
            }
        }
        // ID [n] bytes
        if found.is_none() {
            for i in 1..tokens.len() {
                let t = tokens[i];
                let n = t.strip_prefix('[').and_then(|t| t.strip_suffix(']')).and_then(|t| t.parse::<usize>().ok());
                if let (Some(n), Some(id)) = (n, hex(tokens[i - 1], 8)) {
                    let bytes: Option<Vec<u8>> = tokens.get(i + 1..i + 1 + n).and_then(|b| b.iter().map(|b| u8::from_str_radix(b, 16).ok().filter(|_| b.len() <= 2)).collect());
                    if let (Some(bytes), true) = (bytes, n <= 8) {
                        found = Some((id, bytes));
                        break;
                    }
                }
            }
        }
        // time,ID,extended,direction,bus,length,bytes...
        if found.is_none() {
            let cells: Vec<&str> = line.split(',').map(|c| c.trim()).collect();
            if cells.len() >= 6 {
                let id = hex(cells[1].trim_start_matches("0x"), 8);
                let n = cells[5].parse::<usize>().ok().filter(|n| *n <= 8);
                if let (Some(id), Some(n)) = (id, n) {
                    let bytes: Option<Vec<u8>> = cells.get(6..6 + n).and_then(|b| b.iter().map(|b| u8::from_str_radix(b.trim_start_matches("0x"), 16).ok()).collect());
                    if let Some(bytes) = bytes {
                        found = Some((id, bytes));
                    }
                }
            }
        }
        if let Some((id, bytes)) = found {
            if id < 0x800 {
                out.push(Frame::new(id as u16, &bytes));
            }
        }
    }
    out
}

/// A register as the program read it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Register {
    pub addr: Option<u32>,
    pub size: u8,
    pub value: Option<u32>,
}

#[derive(Default, Debug)]
pub struct Report {
    /// 0x92 or 0x93; 0 when no header frame was in the log.
    pub unit: u8,
    /// The program's version: 2 has the card processors of the 192.
    pub version: u8,
    /// How many registers the program says it read (not counting the
    /// seven values of the start state that follow them).
    pub count: usize,
    pub registers: BTreeMap<u8, Register>,
    pub rom_len: usize,
    /// The ROM's bytes as far as they arrived.
    pub rom: BTreeMap<usize, u8>,
    pub crc: Option<u32>,
    /// Bytes that arrived twice and were not the same both times.
    pub conflicts: usize,
    pub frames: usize,
}

impl Report {
    pub fn from_frames(frames: &[Frame]) -> Report {
        let mut r = Report::default();
        for f in frames {
            let d = &f.data;
            let word = |at: usize| u32::from_be_bytes([d[at], d[at + 1], d[at + 2], d[at + 3]]);
            match (f.id, f.len) {
                (0x7f1, 8) if d[0] == b'R' => {
                    r.unit = d[1];
                    r.version = d[2];
                    r.count = d[3] as usize;
                    r.rom_len = (d[4] as usize) << 16 | (d[5] as usize) << 8 | d[6] as usize;
                }
                (0x7f2, 8) | (0x7f3, 8) => {
                    let e = r.registers.entry(d[0]).or_insert(Register { addr: None, size: d[1], value: None });
                    e.size = d[1];
                    if f.id == 0x7f2 { e.addr = Some(word(2)) } else { e.value = Some(word(2)) }
                }
                (0x7f4, 8) => r.crc = Some(word(4)),
                (0x7f0, 8) => {
                    let at = (d[0] as usize) << 16 | (d[1] as usize) << 8 | d[2] as usize;
                    for i in 0..5 {
                        if let Some(old) = r.rom.insert(at + i, d[3 + i]) {
                            r.conflicts += usize::from(old != d[3 + i]);
                        }
                    }
                }
                _ => continue,
            }
            r.frames += 1;
        }
        r
    }

    /// The ROM, if all of it is there: otherwise the ranges missing.
    pub fn rom_bytes(&self) -> Result<Vec<u8>, Vec<(usize, usize)>> {
        let mut missing: Vec<(usize, usize)> = Vec::new();
        let mut out = Vec::with_capacity(self.rom_len);
        for at in 0..self.rom_len {
            match self.rom.get(&at) {
                Some(b) => out.push(*b),
                None => match missing.last_mut() {
                    Some(last) if last.1 == at => last.1 = at + 1,
                    _ => missing.push((at, at + 1)),
                },
            }
        }
        if missing.is_empty() && self.rom_len != 0 { Ok(out) } else { Err(missing) }
    }

    /// How many values follow the registers.
    pub fn extras(&self) -> usize {
        if self.version >= 2 && self.unit == 0x92 { 21 } else { 7 }
    }

    /// Everything arrived and the ROM has the check sum the unit
    /// computed.
    pub fn complete(&self) -> bool {
        let registers = (0..self.count + self.extras()).all(|i| self.registers.get(&(i as u8)).is_some_and(|r| r.addr.is_some() && r.value.is_some()));
        registers && self.unit != 0 && self.rom_bytes().is_ok_and(|rom| Some(crc32(&rom)) == self.crc)
    }

    /// A value by the register's address.
    pub fn value(&self, addr: u32) -> Option<u32> {
        self.registers.values().find(|r| r.addr == Some(addr)).and_then(|r| r.value)
    }

    /// The report as text.
    pub fn describe(&self) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let unit = match self.unit { 0x92 => "192", 0x93 => "193", _ => "unknown unit" };
        let _ = writeln!(s, "report from a {}: {} frames, {} of {} registers", unit, self.frames,
            self.registers.values().filter(|r| r.value.is_some()).count(), self.count + self.extras());
        for (i, r) in &self.registers {
            let (Some(addr), Some(value)) = (r.addr, r.value) else {
                let _ = writeln!(s, "  #{:<3} incomplete", i);
                continue;
            };
            let name = name(self.unit, addr);
            let digits = 2 * r.size.clamp(1, 4) as usize;
            let _ = writeln!(s, "  {:08x} {:>8}  {}", addr, format!("{:0digits$x}", value), name);
        }
        // What the clock register says, on the 192.
        if let (0x92, Some(frqcr)) = (self.unit, self.value(0xffc0_0000)) {
            let div = |code: u32, table: &[u32]| table.get(code as usize).copied();
            let cpu = div(frqcr >> 6 & 7, &[1, 2, 3, 4, 6, 8]);
            let bus = div(frqcr >> 3 & 7, &[1, 2, 3, 4, 6, 8]);
            let per = div(frqcr & 7, &[2, 3, 4, 6, 8]);
            if let (Some(cpu), Some(bus), Some(per)) = (cpu, bus, per) {
                let _ = writeln!(s, "clocks (FRQCR): CPU 1/{}, bus 1/{}, peripherals 1/{} of the clock generator's output{}", cpu, bus, per,
                    if frqcr & 0x400 != 0 { "" } else { " (its first multiplier is off)" });
                let _ = writeln!(s, "  with 235.2 MHz from the 19.6 MHz crystal that is {:.1} MHz for the peripherals, a tick of {:.2} ms from the timer value 14699",
                    235.2 / per as f64, 14700.0 * 4.0 * per as f64 / 235_200.0);
            }
        }
        // What the 192's card processors answered.
        for (n, base) in [(0, 0x10u32), (1, 0x20)] {
            let Some(result) = self.value(base) else { continue };
            let _ = writeln!(s, "card processor {}:", n);
            if result != 1 {
                let _ = writeln!(s, "  did not get through: stopped at step {:#x} ({})", result, match result {
                    0xe1 => "its boot ROM took no word: is the processor there, and its reset released?",
                    0xe2 => "its boot ROM stopped taking the program",
                    0xe3 => "the program sent nothing: it did not start, or not as written",
                    0xe4 | 0xe6 => "the program did not take the count, or its loop did not end in 20 s",
                    0xe5 => "the program did not report the clock switched",
                    _ => "?",
                });
            }
            if let (Some(pins), true) = (self.value(base + 1), result == 1 || result > 0xe3) {
                let m = match pins >> 4 & 15 { 0 => 16, m => m };
                let _ = writeln!(s, "  port pins after reset {:02x}: clock multiplier {} (pins 7-4), boot mode pins {}{} ({}), pin 3 {}, pin 2 {}",
                    pins & 0xff, m, pins >> 1 & 1, pins & 1,
                    match pins & 3 { 3 => "host word boot", 1 => "host byte boot", 0 => "no boot", _ => "not allowed" },
                    pins >> 3 & 1, pins >> 2 & 1);
                let _ = writeln!(s, "  with the board's 22.5792 MHz crystal that is {:.4} MHz", 22.5792 * m as f64);
                let (clock, host) = (self.value(base + 2).unwrap_or(0), self.value(base + 3).unwrap_or(0));
                let _ = writeln!(s, "  clock control {:04x}, host status {:04x}", clock, host);
            }
            if let (1, Some(t1), Some(t2), Some(loops)) = (result, self.value(base + 4), self.value(base + 5), self.value(base + 6)) {
                let clocks = loops as f64 * 32768.0;
                // Timer 0 counts a quarter of the main processor's
                // peripheral clock.
                let per = t1 as f64 * 4.0 / (clocks / 22.5792);
                let _ = writeln!(s, "  {} clocks of the crystal took {} counts of the main processor's timer 0: its peripheral clock is {:.3} MHz{}",
                    clocks, t1, per, if (per - 58.8).abs() < 0.3 { ", the 58.8 MHz the emulator has" } else { ", NOT the 58.8 MHz the emulator has" });
                if t2 != 0 {
                    let _ = writeln!(s, "  the same on the multiplied clock took {} counts: {:.3} times as fast", t2, t1 as f64 / t2 as f64);
                }
            }
        }
        match self.rom_bytes() {
            Ok(rom) => {
                let crc = crc32(&rom);
                let _ = writeln!(s, "boot ROM: {} bytes, CRC-32 {:08x}, {}", rom.len(), crc, match self.crc {
                    Some(c) if c == crc => "as the unit computed it".to_string(),
                    Some(c) => format!("BUT THE UNIT COMPUTED {:08x}", c),
                    None => "the unit's own check sum did not arrive".to_string(),
                });
            }
            Err(missing) if self.rom_len == 0 => {
                let _ = writeln!(s, "boot ROM: no header frame, length unknown ({} bytes arrived){}", self.rom.len(), if missing.is_empty() { "" } else { "" });
            }
            Err(missing) => {
                let n: usize = missing.iter().map(|m| m.1 - m.0).sum();
                let _ = writeln!(s, "boot ROM: {} of {} bytes missing, in {} places, first at {:#x}", n, self.rom_len, missing.len(), missing[0].0);
            }
        }
        if self.conflicts != 0 {
            let _ = writeln!(s, "{} bytes of the ROM arrived twice with different values", self.conflicts);
        }
        s
    }
}

/// What a register is called.  The start state travels under made-up
/// addresses 1 to 7.
pub fn name(unit: u8, addr: u32) -> &'static str {
    match addr {
        1 => return "SR at the start",
        2 => return "VBR at the start",
        3 => return "PR at the start (where the caller was)",
        4 => return "stack pointer at the start",
        5 => return "R4 (the language)",
        6 => return "R5",
        7 => return "R6",
        0x10 | 0x20 => return "card processor: how far the questions got (1 = all the way)",
        0x11 | 0x21 => return "card processor: port pins after reset",
        0x12 | 0x22 => return "card processor: clock control after reset",
        0x13 | 0x23 => return "card processor: host status",
        0x14 | 0x24 => return "card processor: timer counts for the loops, clock input",
        0x15 | 0x25 => return "card processor: timer counts for the loops, multiplied clock",
        0x16 | 0x26 => return "card processor: loops of 32768 clocks",
        0xff00_0030 => return "PVR, processor version",
        0xff00_0040 => return "CVR, cache version",
        0xff00_0044 => return "PRR, product revision",
        0xff00_001c => return "CCR, cache control",
        0xff00_0010 => return "MMUCR",
        _ => {}
    }
    if unit == 0x93 {
        return match addr {
            0xffd8_0004 => "TSTR, timers started",
            0xffd8_0008 => "TCOR0, the tick's reload",
            0xffd8_0010 => "TCR0",
            0xffd8_0020 => "TCOR2",
            0xffd8_0028 => "TCR2",
            0xffd8_1004 => "CAN timer started",
            0xffd8_1008 => "CAN timer reload",
            0xffd8_1010 => "CAN timer control",
            0xff80_2000 => "interrupt pins",
            0xff80_4000..=0xff80_402f => "interrupt priorities",
            0xff80_4038 | 0xff80_4108 => "interrupt mask",
            0xffe4_0000 => "debug port, mode",
            0xffe4_0030 => "debug port, divisor",
            0xffe4_0034 => "debug port, clock source",
            0xff92_6000 => "faceplate port, mode",
            0xff92_6004 => "faceplate port, bit rate",
            0xffdf_0004 => "CAN bit timing 1",
            0xffdf_0006 => "CAN bit timing 0",
            0xff90_9000 | 0xff90_9004 => "port with the faceplate's reset",
            0xff93_f004..=0xff93_f00c => "port with the display motor",
            0xffc4_4008 => "port with the watchdog strobe",
            0xff97_2000 => "audio clock",
            0xff98_4000 => "sound interface, control",
            _ => "",
        };
    }
    match addr {
        0xffc0_0000 => "FRQCR, clock ratios",
        0xffc0_0004 => "STBCR",
        0xffc0_000c => "WTCSR, watchdog timer",
        0xffc0_0010 => "STBCR2",
        0xff80_0000 => "BCR1, bus",
        0xff80_0004 => "BCR2, bus widths",
        0xff80_0008 => "WCR1, wait states",
        0xff80_000c => "WCR2",
        0xff80_0010 => "WCR3",
        0xff80_0014 => "MCR, memory",
        0xff80_0018 => "PCR",
        0xff80_001c => "RTCSR, refresh timer",
        0xff80_0024 => "RTCOR",
        0xff80_002c => "PCTRA, port directions",
        0xff80_0030 => "PDTRA, port data",
        0xff80_0040 => "PCTRB",
        0xff80_0044 => "PDTRB",
        0xff80_0048 => "GPIOIC",
        0xff80_0050 => "BCR3",
        0xfe0a_00f0 => "BCR4",
        0xffd8_0000 => "TOCR",
        0xffd8_0004 => "TSTR, timers started",
        0xffd8_0008 => "TCOR0, the tick's reload",
        0xffd8_0010 => "TCR0",
        0xffd8_0014 => "TCOR1",
        0xffd8_001c => "TCR1",
        0xffd8_0020 => "TCOR2, the CAN timer's reload",
        0xffd8_0028 => "TCR2",
        0xfe10_0004 => "TSTR2",
        0xfe10_0008 => "TCOR3",
        0xfe10_0010 => "TCR3",
        0xfe10_0014 => "TCOR4",
        0xfe10_001c => "TCR4",
        0xffd0_0000 => "ICR, interrupt control",
        0xffd0_0004 => "IPRA",
        0xffd0_0008 => "IPRB",
        0xffd0_000c => "IPRC",
        0xffd0_0010 => "IPRD",
        0xfe08_0000 => "INTPRI00",
        0xfe08_0040 => "INTMSK00",
        0xffa0_0040 => "DMAOR",
        0xffa0_000c | 0xffa0_001c | 0xffa0_002c | 0xffa0_003c => "CHCR0-3, DMA channel control",
        0xffa0_005c | 0xffa0_006c | 0xffa0_007c | 0xffa0_008c => "CHCR4-7, DMA channel control",
        0xffe0_0000 => "SCSMR1, serial port mode",
        0xffe0_0004 => "SCBRR1, serial port bit rate",
        0xffe0_0008 => "SCSCR1",
        0xffe8_0000 => "SCSMR2, serial port with FIFO, mode",
        0xffe8_0004 => "SCBRR2, its bit rate",
        0xffe8_0008 => "SCSCR2",
        0xffe8_0018 => "SCFCR2",
        0xffc8_003c => "RCR1, clock chip",
        0xffc8_0040 => "RCR2",
        0xb230_0000..=0xb2a0_0001 => if addr & 1 == 0 { "port direction" } else { "port data" },
        0xa7b0_0000..=0xa7b0_00ff => "companion interrupt controller",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round(rom: &[u8], lose: usize) -> Vec<Frame> {
        let mut f = vec![Frame::new(0x7f1, &[b'R', 0x92, 1, 1, 0, (rom.len() >> 8) as u8, rom.len() as u8, 0])];
        for n in 0..8u8 {
            let addr: u32 = if n == 0 { 0xffc0_0000 } else { n as u32 };
            let mut a = vec![n, if n == 0 { 2 } else { 4 }];
            a.extend(addr.to_be_bytes());
            a.extend([0, 0]);
            f.push(Frame::new(0x7f2, &a));
            let mut v = vec![n, if n == 0 { 2 } else { 4 }];
            v.extend((if n == 0 { 0x0e0au32 } else { 0x1000 + n as u32 }).to_be_bytes());
            v.extend([0, 0]);
            f.push(Frame::new(0x7f3, &v));
        }
        let mut c = vec![0, 0, 0, 0];
        c.extend(crc32(rom).to_be_bytes());
        f.push(Frame::new(0x7f4, &c));
        for (i, chunk) in rom.chunks(5).enumerate() {
            let at = i * 5;
            let mut d = vec![(at >> 16) as u8, (at >> 8) as u8, at as u8];
            d.extend(chunk);
            d.resize(8, 0xff);
            f.push(Frame::new(0x7f0, &d));
        }
        f.remove(lose);
        f
    }

    #[test]
    fn a_log_with_gaps_is_filled_from_the_next_round() {
        let rom: Vec<u8> = (0..1000u32).map(|i| (i * 31 >> 3) as u8).collect();
        // Two rounds, each with a different frame lost.
        let mut frames = round(&rom, 40);
        let one = Report::from_frames(&frames);
        assert!(!one.complete());
        assert_eq!(one.rom_bytes().unwrap_err(), vec![(110, 115)]);
        frames.extend(round(&rom, 3));
        // As text in three formats, mixed.
        let mut log = String::from("something else\n");
        for (i, f) in frames.iter().enumerate() {
            let bytes: Vec<String> = f.data[..f.len as usize].iter().map(|b| format!("{:02X}", b)).collect();
            log += &match i % 3 {
                0 => format!("  can0  {:03X}   [{}]  {}\n", f.id, f.len, bytes.join(" ")),
                1 => format!("(1700000000.123456) can0 {:03X}#{}\n", f.id, bytes.concat()),
                _ => format!("[can] tx {}\n", f),
            };
        }
        let parsed = super::frames(&log);
        assert_eq!(parsed, frames);
        let r = Report::from_frames(&parsed);
        assert!(r.complete(), "{}", r.describe());
        assert_eq!(r.rom_bytes().unwrap(), rom);
        assert_eq!((r.unit, r.count, r.conflicts), (0x92, 1, 0));
        assert_eq!(r.value(0xffc0_0000), Some(0x0e0a));
        assert_eq!(r.value(3), Some(0x1003));
        let text = r.describe();
        assert!(text.contains("peripherals 1/4") && text.contains("58.8 MHz") && text.contains("as the unit computed it"), "{}", text);
        // A table with commas.
        let csv = "Time Stamp,ID,Extended,Dir,Bus,LEN,D1,D2,D3,D4,D5,D6,D7,D8\n1234,000007F4,false,Rx,0,8,00,00,00,00,DE,AD,BE,EF,\n";
        assert_eq!(super::frames(csv), vec![Frame::new(0x7f4, &[0, 0, 0, 0, 0xde, 0xad, 0xbe, 0xef])]);
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }
}
