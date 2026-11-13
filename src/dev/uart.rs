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
//! A byte written while the transmitter is off waits in the FIFO until
//! it is switched on, and what arrives while the receiver is off is
//! lost, as on any UART.

use std::collections::VecDeque;

const ST_TX_READY: u16 = 1 << 8;
const ST_RX_READY: u16 = 1 << 6;
const ST_TX_END: u16 = 1 << 2;

/// What a device at the far end of a serial line needs from a port.
pub trait SerialPort {
    /// Bytes the firmware has sent since the last call.
    fn take_tx(&mut self) -> Vec<u8>;
    /// Queue bytes for the firmware to receive.
    fn feed(&mut self, data: &[u8]);
    /// Receiver switched on and nothing waiting to be read.
    fn rx_idle(&self) -> bool;
}

impl SerialPort for Uart {
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

#[derive(Default)]
pub struct Uart {
    mode: u16,
    baud: u16,
    pub control: u16,
    fifo_control: u16,
    other: [u16; 8],
    /// Bytes the firmware sent, not yet collected by the host.
    pub tx: Vec<u8>,
    /// Written with the transmitter off: not on the line yet.
    held: Vec<u8>,
    pub rx: VecDeque<u8>,
    pub tx_total: u64,
    pub rx_total: u64,
    /// Bytes that arrived while the receiver was off.
    pub rx_lost: u64,
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
        if self.control & 0x10 == 0 {
            self.rx_lost += data.len() as u64;
            return;
        }
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
            0x04 => {
                self.control = v;
                if v & 0x20 != 0 && !self.held.is_empty() {
                    self.tx_total += self.held.len() as u64;
                    self.tx.append(&mut self.held);
                }
            }
            0x06 => {
                if self.control & 0x20 != 0 {
                    self.tx.push(v as u8);
                    self.tx_total += 1;
                } else if self.held.len() < 16 {
                    self.held.push(v as u8);
                }
            }
            0x08 => {} // error flags: nothing to clear
            0x0c => {
                if v & 0x8000 != 0 {
                    self.rx.clear();
                }
                if v & 0x0080 != 0 {
                    self.held.clear();
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
        assert!(u.tx.is_empty(), "transmitter off");
        u.receive(&[9]);
        assert_eq!((u.rx.len(), u.rx_lost), (0, 1), "receiver off");
        u.write(4, 0x0030);
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
