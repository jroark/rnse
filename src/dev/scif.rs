//! Serial port with FIFO (SCIF) as found on the SH7770 family, with one
//! 16-bit register per 32-bit word.
//!
//! | offset | register                                                    |
//! |--------|-------------------------------------------------------------|
//! | +0x00  | SCSMR, mode                                                 |
//! | +0x04  | SCBRR, bit rate                                             |
//! | +0x08  | SCSCR, control: 7 = TX interrupt enable, 6 = RX interrupt   |
//! |        | enable, 5 = transmitter on, 4 = receiver on                 |
//! | +0x0c  | SCFTDR, transmit data                                       |
//! | +0x10  | SCFSR, status: 6 = TX finished, 5 = TX FIFO has room,       |
//! |        | 1 = RX FIFO at trigger level, 0 = RX data waiting           |
//! | +0x14  | SCFRDR, receive data                                        |
//! | +0x18  | SCFCR, FIFO control: 2 = reset TX FIFO, 1 = reset RX FIFO   |
//! | +0x1c, +0x20 | data counts of the TX and RX FIFOs                    |
//!
//! Transmission is instantaneous; received bytes are queued by the host.

use std::collections::VecDeque;

#[derive(Default)]
pub struct Scif {
    regs: [u16; 16],
    pub control: u16,
    pub tx: Vec<u8>,
    pub rx: VecDeque<u8>,
    pub tx_total: u64,
    pub rx_total: u64,
}

impl Scif {
    fn status(&self) -> u16 {
        0x0060 | if self.rx.is_empty() { 0 } else { 0x0003 }
    }

    /// Interrupt request line (the port's sources combined).
    pub fn irq(&self) -> bool {
        let c = self.control;
        (c & 0x80 != 0 && c & 0x20 != 0) || (c & 0x40 != 0 && !self.rx.is_empty())
    }

    /// Queue bytes for the firmware to receive.
    pub fn receive(&mut self, data: &[u8]) {
        self.rx.extend(data.iter().copied());
        self.rx_total += data.len() as u64;
    }

    pub fn read(&mut self, off: u32) -> u16 {
        match off & 0x3c {
            0x08 => self.control,
            0x10 => self.status(),
            0x14 => self.rx.pop_front().unwrap_or(0) as u16,
            0x1c => 0,
            0x20 => self.rx.len().min(16) as u16,
            o => self.regs[(o >> 2) as usize],
        }
    }

    pub fn write(&mut self, off: u32, v: u16) {
        match off & 0x3c {
            0x08 => self.control = v,
            0x0c => {
                self.tx.push(v as u8);
                self.tx_total += 1;
            }
            0x10 => {} // flags follow the FIFOs
            0x18 => {
                if v & 2 != 0 {
                    self.rx.clear();
                }
                self.regs[6] = v;
            }
            o => self.regs[(o >> 2) as usize] = v,
        }
    }
}

impl super::uart::SerialPort for Scif {
    fn take_tx(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.tx)
    }
    fn feed(&mut self, data: &[u8]) {
        self.receive(data)
    }
    fn rx_idle(&self) -> bool {
        self.control & 0x10 != 0 && self.rx.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transmit_and_receive() {
        let mut s = Scif::default();
        s.write(0x18, 6);
        s.write(0x18, 0);
        s.write(0x08, 0x30);
        s.write(0x0c, 0x55);
        assert_eq!(s.tx, [0x55]);
        assert!(!s.irq());
        s.write(0x08, 0x70);
        s.receive(&[1, 2]);
        assert!(s.irq());
        assert_eq!(s.read(0x10) & 3, 3);
        assert_eq!((s.read(0x20), s.read(0x14), s.read(0x14)), (2, 1, 2));
        assert!(!s.irq());
        s.write(0x08, 0xf0);
        assert!(s.irq(), "TX interrupt while enabled");
    }
}
