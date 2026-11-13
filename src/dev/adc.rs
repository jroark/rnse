//! A/D converter of the companion chip (0x07d0_0000).
//!
//! Seven analogue inputs, ten bits.  The register layout follows the
//! Hitachi converters of the time, widened to 16 bits:
//!
//! | offset | register                                                    |
//! |--------|-------------------------------------------------------------|
//! | +0, +2, +4, +6 | results A-D, left-justified (value << 6)            |
//! | +8     | control/status: 15 = conversion done (write 0 to clear),    |
//! |        | 14 = interrupt enable, 13 = start, 2..0 = channel           |
//!
//! The firmware always converts a group: channel 3 means inputs 0-3 into
//! A-D, channel 6 inputs 4-6 into A-C.  Starting a conversion finishes it
//! at once here; the start bit clears itself.
//!
//! Inputs identified so far: 3 = position of the fold-down display
//! (see [`crate::board::Board::step_display`]).
//!
//! The 193's system-on-chip has the same converter with eight result
//! registers (+0x00..+0x0e) and the control register at +0x10; channel n
//! converts inputs 0..=n in one go.  There the display position is
//! input 4.  [`Adc::read_wide`] and [`Adc::write_wide`] serve that
//! layout.

const DONE: u16 = 1 << 15;
const INT_ENABLE: u16 = 1 << 14;
const START: u16 = 1 << 13;

#[derive(Default)]
pub struct Adc {
    control: u16,
    result: [u16; 8],
    /// Level on each input, 0..=1023.
    pub input: [u16; 8],
    pub conversions: u64,
}

impl Adc {
    pub fn with_inputs(input: [u16; 8]) -> Adc {
        Adc { input, ..Adc::default() }
    }

    pub fn irq(&self) -> bool {
        self.control & (DONE | INT_ENABLE) == (DONE | INT_ENABLE)
    }

    pub fn read(&mut self, off: u32) -> u16 {
        match off & 0xe {
            8 => self.control,
            o if o < 8 => self.result[(o >> 1) as usize],
            _ => 0,
        }
    }

    pub fn write(&mut self, off: u32, v: u16) {
        if off & 0xe != 8 {
            return;
        }
        // The done flag can only be cleared from outside.
        let done = self.control & v & DONE;
        self.control = (v & !(DONE | START)) | done;
        if v & START != 0 {
            let ch = (v & 7) as usize;
            let first = ch & 4;
            for k in 0..=(ch & 3) {
                self.result[k] = (self.input[first + k] & 0x3ff) << 6;
            }
            self.control |= DONE;
            self.conversions += 1;
        }
    }
}

impl Adc {
    /// Register read in the eight-result layout.
    pub fn read_wide(&mut self, off: u32) -> u16 {
        match off & 0x1e {
            0x10 => self.control,
            o if o < 0x10 => self.result[(o >> 1) as usize],
            _ => 0,
        }
    }

    /// Register write in the eight-result layout.
    pub fn write_wide(&mut self, off: u32, v: u16) {
        if off & 0x1e != 0x10 {
            return;
        }
        let done = self.control & v & DONE;
        self.control = (v & !(DONE | START)) | done;
        if v & START != 0 {
            for k in 0..=(v & 7) as usize {
                self.result[k] = (self.input[k] & 0x3ff) << 6;
            }
            self.control |= DONE;
            self.conversions += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_a_group() {
        let mut a = Adc::default();
        a.input = [1, 2, 3, 1023, 5, 6, 7, 0];
        a.write(8, 0x40e0);
        assert!(!a.irq());
        a.write(8, 0x40e3 | START);
        assert!(a.irq());
        assert_eq!(a.read(8) & START, 0, "start clears itself");
        assert_eq!([a.read(0), a.read(2), a.read(4), a.read(6)], [1 << 6, 2 << 6, 3 << 6, 1023 << 6]);
        // The handler clears the flag by writing it as 0 ...
        let c = a.read(8) & 0x7fff;
        a.write(8, c);
        assert!(!a.irq());
        // ... and writing it as 1 does not set it.
        a.write(8, c | DONE);
        assert!(!a.irq());
        a.write(8, 0x40e6 | START);
        assert_eq!([a.read(0), a.read(2), a.read(4)], [5 << 6, 6 << 6, 7 << 6]);
    }

    #[test]
    fn eight_result_layout() {
        let mut a = Adc::with_inputs([0, 10, 20, 30, 1023, 50, 60, 70]);
        a.write_wide(0x10, 0x40e0);
        a.write_wide(0x10, 0x40e7 | START);
        assert!(a.irq());
        assert_eq!(a.read_wide(0x08), 1023 << 6);
        assert_eq!(a.read_wide(0x0e), 70 << 6);
        let c = a.read_wide(0x10) & 0x7fff;
        a.write_wide(0x10, c);
        assert!(!a.irq());
    }
}
