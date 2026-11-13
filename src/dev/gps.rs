//! A GPS fix for the 193's navigation software.
//!
//! The unit has no self-contained receiver: a block of the
//! system-on-chip at 0xff93_8000 hands raw data to a GPS library inside
//! the OS through a pair of message buffers, and the library works out
//! the position (see docs/hardware-193.md).  Emulating that would mean
//! simulating the satellites.  Instead the emulator answers one level
//! up: the navigation application asks the OS for the library's latest
//! result once a second or so (service 0x10a of trap 33), and gets a
//! record of 47 bytes.  [`Gps::record`] builds that record.
//!
//! What is known of the record, from the routine that fills it
//! (0x805436c4 in V57510AA) and the application's that reads it
//! (0x01c291a4):
//!
//! | bytes  | contents                                                  |
//! |--------|-----------------------------------------------------------|
//! | 0      | status: low nibble 3 or 4 with a fix (taken to be two and |
//! |        | three dimensions), 0 without                              |
//! | 1-4    | latitude, 1/256 of an arc second (36 degrees north until  |
//! |        | there is a fix)                                           |
//! | 5-8    | longitude, same unit (136 degrees east until then).  West |
//! |        | and south are taken to be negative                        |
//! | 9-10   | a word of the library's, taken to be the altitude         |
//! | 11, 12 | a byte of the library's, twice; the fix is thrown away if |
//! |        | it is 250 or more (taken to be a dilution of precision)   |
//! | 13-14  | course over ground, 1024ths of a circle                   |
//! | 16-17  | speed over ground in km/h.  Without it the software does |
//! |        | not take the course from the fix; with a figure ten times |
//! |        | too large it overshoots, so the unit is the kilometre per |
//! |        | hour or close to it                                       |
//! | 20     | a byte, taken to be the number of satellites used         |
//! | 22-27  | date and time in BCD: year, month, day, hour, minute,     |
//! |        | second, in Japanese time (the application takes 9 hours   |
//! |        | off; years above 90 are 19xx)                             |
//! | 28-36  | further values of the library's, not understood           |
//! | 37     | a flag from the library's status                          |

/// Where the emulated unit is, and since when.
#[derive(Clone, Debug, PartialEq)]
pub struct Gps {
    /// Degrees, north positive.
    pub lat: f64,
    /// Degrees, east positive.
    pub lon: f64,
    /// Metres.
    pub alt: i16,
    /// Seconds since 1970 (UTC) at the moment the machine started.
    pub epoch: u64,
    /// Course over ground in degrees, clockwise from north.
    pub heading: f64,
    /// Speed over ground in kilometres per hour.
    pub speed: f64,
}

fn bcd(v: u64) -> u8 {
    ((v / 10 % 10) << 4 | v % 10) as u8
}

/// Year, month and day of a count of days since 1970-01-01.
fn civil(days: u64) -> (u64, u64, u64) {
    // Howard Hinnant's algorithm, for dates from 1970 on.
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (d, m) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    (yoe + era * 400 + u64::from(m <= 2), m, d)
}

impl Gps {
    /// A fix at a place, with a default date (noon UTC, 1 June 2018).
    pub fn at(lat: f64, lon: f64) -> Gps {
        Gps { lat, lon, alt: 250, epoch: 1_527_854_400, heading: 0.0, speed: 0.0 }
    }

    /// `"LAT,LON[,COURSE]"` in degrees, as the command line gives it.
    pub fn parse(s: &str) -> Result<Gps, String> {
        let (lat, lon) = s.split_once(',').ok_or("a position is LAT,LON[,COURSE] in degrees, e.g. 42.6464,-83.2139")?;
        let (lon, course) = lon.split_once(',').unwrap_or((lon, "0"));
        let course: f64 = course.trim().parse().map_err(|_| format!("{} is not a course", course))?;
        let lat: f64 = lat.trim().parse().map_err(|_| format!("{} is not a latitude", lat))?;
        let lon: f64 = lon.trim().parse().map_err(|_| format!("{} is not a longitude", lon))?;
        if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
            return Err("latitude is -90 to 90, longitude -180 to 180".into());
        }
        Ok(Gps { heading: course, ..Gps::at(lat, lon) })
    }

    /// The record the application is given, `ms` milliseconds after the
    /// machine started.
    pub fn record(&self, ms: u64) -> [u8; 47] {
        let mut r = [0u8; 47];
        self.patch(&mut r, ms);
        r
    }

    /// Move `metres` along the course.
    pub fn advance(&mut self, metres: f64) {
        let h = self.heading.to_radians();
        self.lat += metres * h.cos() / 111_320.0;
        self.lon += metres * h.sin() / (111_320.0 * self.lat.to_radians().cos().max(0.01));
    }

    /// Put the fix into a record the library made, leaving what is not
    /// understood as it is.
    pub fn patch(&self, r: &mut [u8], ms: u64) {
        let unit = 256.0 * 3600.0;
        let lat = (self.lat * unit).round() as i32;
        let lon = (self.lon * unit).round() as i32;
        r[0] = 4;
        r[1..5].copy_from_slice(&lat.to_be_bytes());
        r[5..9].copy_from_slice(&lon.to_be_bytes());
        r[9..11].copy_from_slice(&self.alt.to_be_bytes());
        r[11] = 10;
        r[12] = 10;
        // The course in 1024ths of a circle.
        let course = (self.heading.rem_euclid(360.0) / 360.0 * 1024.0) as u16 & 0x3ff;
        r[13..15].copy_from_slice(&course.to_be_bytes());
        r[16..18].copy_from_slice(&(self.speed.round() as u16).to_be_bytes());
        r[20] = 8;
        // Japanese time.
        let t = self.epoch + ms / 1000 + 9 * 3600;
        let (y, m, d) = civil(t / 86_400);
        let s = t % 86_400;
        r[22..28].copy_from_slice(&[bcd(y % 100), bcd(m), bcd(d), bcd(s / 3600), bcd(s / 60 % 60), bcd(s % 60)]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_of_a_fix() {
        let g = Gps::parse("42.5, -83.25").unwrap();
        let r = g.record(61_000);
        assert_eq!(r[0] & 0xf, 4);
        assert_eq!(i32::from_be_bytes([r[1], r[2], r[3], r[4]]), (42 * 3600 + 1800) * 256);
        assert_eq!(i32::from_be_bytes([r[5], r[6], r[7], r[8]]), -(83 * 3600 + 900) * 256);
        // 12:01:01 UTC on 1 June 2018 is 21:01:01 in Japan.
        assert_eq!(r[22..28], [0x18, 0x06, 0x01, 0x21, 0x01, 0x01]);
        // What the library itself reports before it has a fix.
        let r = Gps::at(36.0, 136.0).record(0);
        assert_eq!(r[1..9], [0x01, 0xfa, 0x40, 0x00, 0x07, 0x78, 0x80, 0x00]);
        assert!(Gps::parse("91,0").is_err() && Gps::parse("here").is_err());
    }

    #[test]
    fn moving() {
        let mut g = Gps::at(42.0, -83.0);
        g.heading = 90.0;
        g.advance(1000.0);
        assert!((g.lat - 42.0).abs() < 1e-9);
        assert!((g.lon - (-83.0 + 1000.0 / (111_320.0 * 42f64.to_radians().cos()))).abs() < 1e-9);
        assert_eq!(g.record(0)[13..15], [1, 0], "east is a quarter of the circle");
        g.speed = 50.4;
        assert_eq!(g.record(0)[16..18], [0, 50]);
    }

    #[test]
    fn dates() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(17_683), (2018, 6, 1));
        assert_eq!(civil(19_782), (2024, 2, 29));
    }
}
