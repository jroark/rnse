//! A GDB remote stub: `target remote :PORT` from an SH-4 GDB
//! (`gdb-multiarch`, with `set architecture sh4` and `set endian big`,
//! or simply the program's ELF file loaded) gets registers, memory,
//! breakpoints, single steps and Ctrl-C.
//!
//! Only what GDB needs for that is implemented, on one connection at a
//! time and without threads.  The machine runs only while GDB has told
//! it to continue or step.
//!
//! Registers in GDB's order for the SH-4: r0-r15, pc, pr, gbr, vbr,
//! mach, macl, sr, fpul, fpscr, fr0-fr15, ssr, spc, r0-r7 of bank 0,
//! r0-r7 of bank 1.
//!
//! Memory is RAM and flash as the CPU's P1/P2 areas show them (the top
//! three address bits are ignored); device registers are not readable
//! this way, because reading them has effects.  Writes go to RAM only.
//!
//! `monitor` commands: `time` (emulated milliseconds since the start),
//! `screenshot FILE` (the picture as a PNG), `console` (what the
//! software has sent to its debug port since the last time).

use crate::machine::Machine;
use crate::sh4::cpu::Stop;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};

const SIGINT: u8 = 2;
const SIGILL: u8 = 4;
const SIGTRAP: u8 = 5;

/// How the session ended.
#[derive(Debug, PartialEq, Eq)]
pub enum End {
    /// GDB detached or went away: the machine may go on by itself.
    Detached,
    /// GDB asked for the program to be killed.
    Killed,
}

struct Link {
    stream: TcpStream,
    no_ack: bool,
}

impl Link {
    /// The next packet's payload, or `None` when the connection closed.
    /// A lone 0x03 (Ctrl-C) arrives as the one-byte packet `\x03`.
    fn packet(&mut self) -> io::Result<Option<Vec<u8>>> {
        let mut byte = [0u8];
        loop {
            // Up to the '$'.
            loop {
                if self.stream.read(&mut byte)? == 0 {
                    return Ok(None);
                }
                match byte[0] {
                    b'$' => break,
                    3 => return Ok(Some(vec![3])),
                    _ => {} // acknowledgements and noise
                }
            }
            let mut data = Vec::new();
            loop {
                if self.stream.read(&mut byte)? == 0 {
                    return Ok(None);
                }
                if byte[0] == b'#' {
                    break;
                }
                data.push(byte[0]);
            }
            let mut sum = [0u8; 2];
            self.stream.read_exact(&mut sum)?;
            let want = u8::from_str_radix(std::str::from_utf8(&sum).unwrap_or("zz"), 16).ok();
            let have = data.iter().fold(0u8, |a, b| a.wrapping_add(*b));
            if self.no_ack {
                return Ok(Some(data));
            }
            if want == Some(have) {
                self.stream.write_all(b"+")?;
                return Ok(Some(data));
            }
            self.stream.write_all(b"-")?;
        }
    }

    fn send(&mut self, payload: &[u8]) -> io::Result<()> {
        let sum = payload.iter().fold(0u8, |a, b| a.wrapping_add(*b));
        let mut out = Vec::with_capacity(payload.len() + 4);
        out.push(b'$');
        out.extend_from_slice(payload);
        out.extend_from_slice(format!("#{:02x}", sum).as_bytes());
        self.stream.write_all(&out)
    }

    /// Has GDB sent Ctrl-C?  Does not wait.
    fn interrupted(&mut self) -> bool {
        let mut byte = [0u8];
        if self.stream.set_nonblocking(true).is_err() {
            return false;
        }
        let got = matches!(self.stream.read(&mut byte), Ok(1) if byte[0] == 3);
        let _ = self.stream.set_nonblocking(false);
        got
    }
}

fn hex(bytes: &[u8]) -> Vec<u8> {
    bytes.iter().flat_map(|b| format!("{:02x}", b).into_bytes()).collect()
}

fn unhex(text: &[u8]) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 {
        return None;
    }
    text.chunks(2).map(|c| u8::from_str_radix(std::str::from_utf8(c).ok()?, 16).ok()).collect()
}

fn number(text: &[u8]) -> Option<u32> {
    u32::from_str_radix(std::str::from_utf8(text).ok()?, 16).ok()
}

/// Number of registers in a `g` packet.
const REGS: usize = 59;

fn register(m: &Machine, n: usize) -> u32 {
    let c = &m.cpu;
    // Which bank R0-R7 are: bank 1 in privileged mode with SR.RB set.
    let bank1 = c.sr & 0x6000_0000 == 0x6000_0000;
    match n {
        0..=15 => c.r[n],
        16 => c.pc,
        17 => c.pr,
        18 => c.gbr,
        19 => c.vbr,
        20 => c.mach,
        21 => c.macl,
        22 => c.sr,
        23 => c.fpul,
        24 => c.fpscr,
        25..=40 => c.fr[n - 25],
        41 => c.ssr,
        42 => c.spc,
        43..=50 => if bank1 { c.r_bank[n - 43] } else { c.r[n - 43] },
        51..=58 => if bank1 { c.r[n - 51] } else { c.r_bank[n - 51] },
        _ => 0,
    }
}

fn set_register(m: &mut Machine, n: usize, v: u32) {
    let bank1 = m.cpu.sr & 0x6000_0000 == 0x6000_0000;
    let c = &mut m.cpu;
    match n {
        0..=15 => c.r[n] = v,
        16 => c.pc = v,
        17 => c.pr = v,
        18 => c.gbr = v,
        19 => c.vbr = v,
        20 => c.mach = v,
        21 => c.macl = v,
        22 => c.set_sr(v),
        23 => c.fpul = v,
        24 => c.fpscr = v,
        25..=40 => c.fr[n - 25] = v,
        41 => c.ssr = v,
        42 => c.spc = v,
        43..=50 => if bank1 { c.r_bank[n - 43] = v } else { c.r[n - 43] = v },
        51..=58 => if bank1 { c.r[n - 51] = v } else { c.r_bank[n - 51] = v },
        _ => {}
    }
}

/// One instruction, whatever breakpoints there are.
fn step(m: &mut Machine) -> Stop {
    let saved = std::mem::take(&mut m.cpu.breakpoints);
    let stop = m.run(1);
    m.cpu.breakpoints = saved;
    stop
}

/// Run until something stops the machine; returns the signal to report.
fn resume(m: &mut Machine, link: &mut Link, single: bool) -> u8 {
    if m.cpu.fault.is_some() {
        return SIGILL;
    }
    // Off the breakpoint we may be standing on.
    let mut stop = step(m);
    if !single {
        let slice = m.cycles_per_ms() * 4;
        while stop == Stop::Budget {
            if link.interrupted() {
                return SIGINT;
            }
            stop = m.run(slice);
        }
    }
    match stop {
        Stop::Fault => SIGILL,
        _ => SIGTRAP,
    }
}

fn monitor(m: &mut Machine, command: &str, console_seen: &mut usize) -> String {
    let mut words = command.split_whitespace();
    match words.next() {
        Some("time") => format!("{} ms\n", m.cpu.cycles / m.cycles_per_ms()),
        Some("screenshot") => match words.next() {
            Some(path) => {
                let (w, h, rgb) = m.render();
                match std::fs::write(path, crate::png::encode_rgb(w, h, &rgb)) {
                    Ok(()) => format!("{}x{} picture written to {}\n", w, h, path),
                    Err(e) => format!("{}: {}\n", path, e),
                }
            }
            None => "screenshot FILE\n".into(),
        },
        Some("console") => {
            let text = String::from_utf8_lossy(&m.console[(*console_seen).min(m.console.len())..]).into_owned();
            *console_seen = m.console.len();
            text
        }
        _ => "commands: time, screenshot FILE, console\n".into(),
    }
}

/// Serve one GDB connection on `listener`.  The machine is stopped when
/// GDB connects and runs only on its say-so.
pub fn serve(m: &mut Machine, listener: &TcpListener) -> io::Result<End> {
    let (stream, _) = listener.accept()?;
    stream.set_nodelay(true)?;
    let mut link = Link { stream, no_ack: false };
    let mut console_seen = 0usize;
    // Breakpoints of this session; the machine's own (from the command
    // line) stay.
    let mut ours: Vec<u32> = Vec::new();
    let end = loop {
        let Some(p) = link.packet()? else { break End::Detached };
        let text = String::from_utf8_lossy(&p).into_owned();
        let reply: Vec<u8> = match p.first().copied() {
            Some(3) => continue, // Ctrl-C while stopped
            Some(b'?') => format!("S{:02x}", SIGTRAP).into_bytes(),
            Some(b'g') => hex(&(0..REGS).flat_map(|n| register(m, n).to_be_bytes()).collect::<Vec<u8>>()),
            Some(b'G') => match unhex(&p[1..]) {
                Some(bytes) => {
                    for (n, v) in bytes.chunks_exact(4).enumerate().take(REGS) {
                        set_register(m, n, u32::from_be_bytes([v[0], v[1], v[2], v[3]]));
                    }
                    b"OK".to_vec()
                }
                None => b"E01".to_vec(),
            },
            Some(b'p') => match number(&p[1..]) {
                Some(n) if (n as usize) < REGS => hex(&register(m, n as usize).to_be_bytes()),
                _ => b"xxxxxxxx".to_vec(),
            },
            Some(b'P') => {
                let mut parts = text[1..].splitn(2, '=');
                let n = parts.next().and_then(|s| u32::from_str_radix(s, 16).ok());
                let v = parts.next().and_then(|s| unhex(s.as_bytes()));
                match (n, v) {
                    (Some(n), Some(v)) if v.len() == 4 && (n as usize) < REGS => {
                        set_register(m, n as usize, u32::from_be_bytes([v[0], v[1], v[2], v[3]]));
                        b"OK".to_vec()
                    }
                    _ => b"E01".to_vec(),
                }
            }
            Some(b'm') => {
                let mut parts = text[1..].splitn(2, ',');
                let addr = parts.next().and_then(|s| u32::from_str_radix(s, 16).ok());
                let len = parts.next().and_then(|s| u32::from_str_radix(s, 16).ok());
                match (addr, len) {
                    (Some(addr), Some(len)) => {
                        // As much as is readable; nothing readable is an error.
                        let bytes: Vec<u8> = (0..len.min(0x4000)).map_while(|i| m.peek8(addr.wrapping_add(i))).collect();
                        if bytes.is_empty() && len != 0 { b"E01".to_vec() } else { hex(&bytes) }
                    }
                    _ => b"E01".to_vec(),
                }
            }
            Some(b'M') | Some(b'X') => {
                let colon = p.iter().position(|b| *b == b':');
                let head = colon.map(|c| String::from_utf8_lossy(&p[1..c]).into_owned()).unwrap_or_default();
                let mut parts = head.splitn(2, ',');
                let addr = parts.next().and_then(|s| u32::from_str_radix(s, 16).ok());
                let len = parts.next().and_then(|s| u32::from_str_radix(s, 16).ok());
                let data = colon.and_then(|c| {
                    if p[0] == b'M' {
                        unhex(&p[c + 1..])
                    } else {
                        // Binary, with 0x7d escaping the next byte.
                        let mut out = Vec::new();
                        let mut bytes = p[c + 1..].iter();
                        while let Some(&b) = bytes.next() {
                            out.push(if b == 0x7d { bytes.next().map_or(0, |n| n ^ 0x20) } else { b });
                        }
                        Some(out)
                    }
                });
                match (addr, len, data) {
                    (Some(addr), Some(len), Some(data)) if data.len() == len as usize => {
                        if data.iter().enumerate().all(|(i, b)| m.poke8(addr.wrapping_add(i as u32), *b)) {
                            b"OK".to_vec()
                        } else {
                            b"E01".to_vec()
                        }
                    }
                    _ => b"E01".to_vec(),
                }
            }
            Some(b'c') | Some(b's') => {
                if let Some(addr) = number(&p[1..]) {
                    m.cpu.pc = addr;
                }
                let signal = resume(m, &mut link, p[0] == b's');
                if signal == SIGILL {
                    if let Some(fault) = &m.cpu.fault {
                        let mut note = b"O".to_vec();
                        note.extend(hex(format!("the machine stopped: {}\n", fault).as_bytes()));
                        link.send(&note)?;
                    }
                }
                format!("S{:02x}", signal).into_bytes()
            }
            Some(b'Z') | Some(b'z') if p.len() > 3 && (p[1] == b'0' || p[1] == b'1') => {
                let addr = text[3..].split(',').next().and_then(|s| u32::from_str_radix(s, 16).ok());
                match addr {
                    Some(addr) => {
                        if p[0] == b'Z' {
                            if !ours.contains(&addr) {
                                ours.push(addr);
                                m.cpu.breakpoints.push(addr);
                            }
                        } else if let Some(i) = ours.iter().position(|a| *a == addr) {
                            ours.remove(i);
                            if let Some(j) = m.cpu.breakpoints.iter().position(|a| *a == addr) {
                                m.cpu.breakpoints.remove(j);
                            }
                        }
                        b"OK".to_vec()
                    }
                    None => b"E01".to_vec(),
                }
            }
            Some(b'H') | Some(b'T') => b"OK".to_vec(),
            Some(b'D') => {
                link.send(b"OK")?;
                break End::Detached;
            }
            Some(b'k') => break End::Killed,
            Some(b'q') | Some(b'Q') | Some(b'v') => {
                if text.starts_with("qSupported") {
                    b"PacketSize=4000;QStartNoAckMode+;swbreak+;hwbreak+".to_vec()
                } else if text == "QStartNoAckMode" {
                    link.send(b"OK")?;
                    link.no_ack = true;
                    continue;
                } else if text == "qAttached" {
                    b"1".to_vec()
                } else if text == "qC" {
                    b"QC1".to_vec()
                } else if text == "qfThreadInfo" {
                    b"m1".to_vec()
                } else if text == "qsThreadInfo" {
                    b"l".to_vec()
                } else if text.starts_with("qSymbol") {
                    b"OK".to_vec()
                } else if let Some(command) = text.strip_prefix("qRcmd,") {
                    let command = unhex(command.as_bytes()).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
                    let answer = monitor(m, &command, &mut console_seen);
                    if !answer.is_empty() {
                        let mut note = b"O".to_vec();
                        note.extend(hex(answer.as_bytes()));
                        link.send(&note)?;
                    }
                    b"OK".to_vec()
                } else {
                    Vec::new() // not supported
                }
            }
            _ => Vec::new(),
        };
        link.send(&reply)?;
    };
    for addr in ours {
        if let Some(j) = m.cpu.breakpoints.iter().position(|a| *a == addr) {
            m.cpu.breakpoints.remove(j);
        }
    }
    Ok(end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sh4::SR_MD;

    fn exchange(s: &mut TcpStream, payload: &str) -> String {
        let sum = payload.bytes().fold(0u8, |a, b| a.wrapping_add(b));
        s.write_all(format!("${}#{:02x}", payload, sum).as_bytes()).unwrap();
        read_reply(s)
    }

    fn read_reply(s: &mut TcpStream) -> String {
        let mut byte = [0u8];
        loop {
            s.read_exact(&mut byte).unwrap();
            if byte[0] == b'$' {
                break;
            }
        }
        let mut out = Vec::new();
        loop {
            s.read_exact(&mut byte).unwrap();
            if byte[0] == b'#' {
                break;
            }
            out.push(byte[0]);
        }
        let mut sum = [0u8; 2];
        s.read_exact(&mut sum).unwrap();
        s.write_all(b"+").unwrap();
        String::from_utf8(out).unwrap()
    }

    /// A session as GDB would hold it, against a three-instruction loop.
    #[test]
    fn session() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let mut m = Machine::new();
            m.echo_console = false;
            // 8c001000: add #1,r1 ; add #2,r2 ; bra 8c001000 ; nop
            let code: [u16; 4] = [0x7101, 0x7202, 0xaffc, 0x0009];
            m.board.load(0x0c00_1000, &code.iter().flat_map(|w| w.to_be_bytes()).collect::<Vec<u8>>());
            m.cpu.reset();
            m.cpu.set_sr(SR_MD);
            m.cpu.pc = 0x8c00_1000;
            let end = serve(&mut m, &listener).unwrap();
            (end, m.cpu.r[1], m.cpu.breakpoints.len())
        });
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        assert!(exchange(&mut s, "qSupported:swbreak+").contains("PacketSize"));
        assert_eq!(exchange(&mut s, "?"), "S05");
        let regs = exchange(&mut s, "g");
        assert_eq!(regs.len(), REGS * 8);
        assert_eq!(&regs[16 * 8..17 * 8], "8c001000", "pc");
        assert_eq!(exchange(&mut s, "m8c001000,4"), "71017202");
        assert_eq!(exchange(&mut s, "s"), "S05");
        assert_eq!(exchange(&mut s, "p10"), "8c001002");
        assert_eq!(exchange(&mut s, "p1"), "00000001");
        // A breakpoint on the second instruction: once round the loop.
        assert_eq!(exchange(&mut s, "Z0,8c001002,2"), "OK");
        assert_eq!(exchange(&mut s, "c"), "S05");
        assert_eq!(exchange(&mut s, "p10"), "8c001002");
        assert_eq!(exchange(&mut s, "p1"), "00000002");
        assert_eq!(exchange(&mut s, "p2"), "00000002");
        // Registers and memory can be changed.
        assert_eq!(exchange(&mut s, "P1=00000010"), "OK");
        assert_eq!(exchange(&mut s, "M8c001100,2:abcd"), "OK");
        assert_eq!(exchange(&mut s, "m8c001100,2"), "abcd");
        assert_eq!(exchange(&mut s, "Ma0000000,1:00"), "E01", "flash, not RAM");
        assert_eq!(exchange(&mut s, "z0,8c001002,2"), "OK");
        // Free running until Ctrl-C.
        let sum = b"c".iter().fold(0u8, |a, b| a.wrapping_add(*b));
        s.write_all(format!("$c#{:02x}", sum).as_bytes()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        s.write_all(&[3]).unwrap();
        assert_eq!(read_reply(&mut s), "S02");
        let monitor = exchange(&mut s, &format!("qRcmd,{}", String::from_utf8(hex(b"time")).unwrap()));
        assert!(monitor.starts_with('O'), "{}", monitor);
        assert_eq!(read_reply(&mut s), "OK");
        assert_eq!(exchange(&mut s, "D"), "OK");
        let (end, r1, breakpoints) = server.join().unwrap();
        assert_eq!(end, End::Detached);
        assert!(r1 > 0x10, "the loop ran on from the value written: {:#x}", r1);
        assert_eq!(breakpoints, 0);
    }
}
