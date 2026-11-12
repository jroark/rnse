//! Serial channels of the companion chip (0x07aN_0000).
//!
//! Sixteen-bit registers, worked out from the firmware's drivers:
//!
//! | offset | register                                                    |
//! |--------|-------------------------------------------------------------|
//! | +0x00  | mode                                                        |
//! | +0x02  | baud rate divisor                                           |
//! | +0x04  | control: 7 = TX interrupt enable, 6 = RX interrupt enable,  |
//! |        | 5 = transmitter on, 4 = receiver on, 2 = TX-end int. enable |
//! | +0x06  | transmit data                                               |
//! | +0x08  | status: 9 = TX FIFO full, 8 = TX ready, 6 = RX data ready,  |
//! |        | 5..3 = receive errors (write 0 to clear), 2 = TX finished   |
//! | +0x0a  | receive data                                                |
//! | +0x0c  | FIFO control: 15 = reset RX FIFO, 7 = reset TX FIFO         |
//!
//! Transmission is instantaneous; received bytes are queued by the host.

use std::collections::VecDeque;

const ST_TX_READY: u16 = 1 << 8;
const ST_RX_READY: u16 = 1 << 6;
const ST_TX_END: u16 = 1 << 2;

#[derive(Default)]
pub struct Uart {
    mode: u16,
    baud: u16,
    pub control: u16,
    fifo_control: u16,
    other: [u16; 8],
    /// Bytes the firmware sent, not yet collected by the host.
    pub tx: Vec<u8>,
    pub rx: VecDeque<u8>,
    pub tx_total: u64,
    pub rx_total: u64,
}

impl Uart {
    fn status(&self) -> u16 {
        ST_TX_READY | ST_TX_END | if self.rx.is_empty() { 0 } else { ST_RX_READY }
    }

    /// Interrupt request line.
    pub fn irq(&self) -> bool {
        let c = self.control;
        (c & 0x80 != 0 && c & 0x20 != 0) || (c & 0x04 != 0 && c & 0x20 != 0) || (c & 0x40 != 0 && !self.rx.is_empty())
    }

    /// Queue bytes for the firmware to receive.
    pub fn receive(&mut self, data: &[u8]) {
        self.rx.extend(data.iter().copied());
        self.rx_total += data.len() as u64;
    }

    pub fn read(&mut self, off: u32) -> u16 {
        match off & 0x1e {
            0x00 => self.mode,
            0x02 => self.baud,
            0x04 => self.control,
            0x08 => self.status(),
            0x0a => self.rx.pop_front().unwrap_or(0) as u16,
            0x0c => self.fifo_control,
            o => self.other[(o as usize >> 1) & 7],
        }
    }

    pub fn write(&mut self, off: u32, v: u16) {
        match off & 0x1e {
            0x00 => self.mode = v,
            0x02 => self.baud = v,
            0x04 => self.control = v,
            0x06 => {
                self.tx.push(v as u8);
                self.tx_total += 1;
            }
            0x08 => {} // error flags: nothing to clear
            0x0c => {
                if v & 0x8000 != 0 {
                    self.rx.clear();
                }
                self.fifo_control = v & !0x8080;
            }
            o => self.other[(o as usize >> 1) & 7] = v,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transmit_and_receive() {
        let mut u = Uart::default();
        assert_eq!(u.read(8) & 0x0300, 0x0100, "ready, not full");
        u.write(6, 0x41);
        assert_eq!(u.tx, b"A");
        assert!(!u.irq());
        u.write(4, 0x00f8);
        assert!(u.irq(), "TX interrupt while enabled");
        u.write(4, 0x0058);
        assert!(!u.irq());
        u.receive(&[1, 2]);
        assert!(u.irq(), "RX interrupt");
        assert_eq!(u.read(8) & 0x40, 0x40);
        assert_eq!((u.read(0xa), u.read(0xa)), (1, 2));
        assert_eq!(u.read(8) & 0x40, 0);
        assert!(!u.irq());
    }
}
