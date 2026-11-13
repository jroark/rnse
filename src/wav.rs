//! WAV files, as much as the emulator needs: 16-bit PCM written, and
//! read back when it is 44.1 kHz stereo (an audio CD track).

/// A WAV file of 16-bit samples, channels interleaved.
pub fn encode(rate: u32, channels: u16, samples: &[i16]) -> Vec<u8> {
    let data = samples.len() as u32 * 2;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend(b"RIFF");
    out.extend((36 + data).to_le_bytes());
    out.extend(b"WAVEfmt ");
    out.extend(16u32.to_le_bytes());
    out.extend(1u16.to_le_bytes()); // PCM
    out.extend(channels.to_le_bytes());
    out.extend(rate.to_le_bytes());
    out.extend((rate * channels as u32 * 2).to_le_bytes());
    out.extend((channels * 2).to_le_bytes());
    out.extend(16u16.to_le_bytes());
    out.extend(b"data");
    out.extend(data.to_le_bytes());
    for s in samples {
        out.extend(s.to_le_bytes());
    }
    out
}

/// The samples of a WAV file that is what an audio CD holds: 16-bit
/// PCM, 44.1 kHz, stereo.
pub fn decode_cd(file: &[u8]) -> Result<Vec<i16>, String> {
    if file.len() < 12 || &file[..4] != b"RIFF" || &file[8..12] != b"WAVE" {
        return Err("not a WAV file".into());
    }
    let le16 = |o: usize| u16::from_le_bytes([file[o], file[o + 1]]);
    let le32 = |o: usize| u32::from_le_bytes([file[o], file[o + 1], file[o + 2], file[o + 3]]);
    let (mut at, mut format_ok) = (12, false);
    while at + 8 <= file.len() {
        let (id, len) = (&file[at..at + 4], le32(at + 4) as usize);
        let body = at + 8;
        if id == b"fmt " && body + 16 <= file.len() {
            if (le16(body), le16(body + 2), le32(body + 4), le16(body + 14)) != (1, 2, 44_100, 16) {
                return Err("an audio CD track is 16-bit PCM, 44.1 kHz, stereo; this WAV file is something else".into());
            }
            format_ok = true;
        } else if id == b"data" {
            if !format_ok {
                return Err("WAV file without a format before its data".into());
            }
            let end = (body + len).min(file.len());
            return Ok(file[body..end].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect());
        }
        at = body + len + (len & 1);
    }
    Err("WAV file without data".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let samples: Vec<i16> = (0..2000).map(|i| (i * 37 % 2001 - 1000) as i16).collect();
        let file = encode(44_100, 2, &samples);
        assert_eq!(file.len(), 44 + 4000);
        assert_eq!(decode_cd(&file).unwrap(), samples);
        assert!(decode_cd(&encode(48_000, 2, &samples)).is_err());
        assert!(decode_cd(b"RIFFxxxxWAVE").is_err());
    }
}
