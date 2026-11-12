//! Rate-limited logging of device accesses, used while reverse engineering
//! which peripherals the firmware talks to.

use std::collections::HashMap;

#[derive(Default)]
pub struct IoLog {
    pub enabled: bool,
    /// Print at most this many accesses per (address, direction).
    pub limit: u32,
    counts: HashMap<(u32, bool), u32>,
    /// Return address at the time of the access (set by the owner).
    pub pr: u32,
}

impl IoLog {
    pub fn new(enabled: bool, limit: u32) -> Self {
        IoLog { enabled, limit, counts: HashMap::new(), pr: 0 }
    }

    #[inline]
    pub fn log(&mut self, what: &str, write: bool, addr: u32, size: u32, val: u32, pc: u32) {
        if !self.enabled {
            return;
        }
        self.log_slow(what, write, addr, size, val, pc, self.pr);
    }

    #[cold]
    fn log_slow(&mut self, what: &str, write: bool, addr: u32, size: u32, val: u32, pc: u32, pr: u32) {
        let c = self.counts.entry((addr, write)).or_insert(0);
        *c += 1;
        if *c <= self.limit {
            let w = (size * 2) as usize;
            eprintln!(
                "[io] {:<6} {} {:08x}.{} {} {:0w$x}  pc={:08x} pr={:08x}{}",
                what,
                if write { "W" } else { "R" },
                addr,
                size,
                if write { "<=" } else { "=>" },
                val,
                pc,
                pr,
                if *c == self.limit { "  (further accesses not shown)" } else { "" },
                w = w
            );
        }
    }

    /// Summary of every address seen: (addr, is_write, count), sorted.
    pub fn summary(&self) -> Vec<(u32, bool, u32)> {
        let mut v: Vec<_> = self.counts.iter().map(|(&(a, w), &c)| (a, w, c)).collect();
        v.sort();
        v
    }
}
