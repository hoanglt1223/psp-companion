//! IMA ADPCM as sent in `VoiceChunk` (codec 1): each chunk carries its own decoder state, so chunks
//! decode independently. Layout: `predictor: i16`, `step_index: u8`, `reserved: u8`, then 4-bit codes,
//! low nibble first.

use thiserror::Error;

pub const HEADER_LEN: usize = 4;

const STEPS: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66, 73, 80, 88, 97, 107,
    118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449, 494, 544, 598, 658, 724, 796, 876, 963,
    1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272, 2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894,
    6484, 7132, 7845, 8630, 9493, 10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794,
    32767,
];
const INDEX_ADJUST: [i32; 8] = [-1, -1, -1, -1, 2, 4, 6, 8];

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AdpcmError {
    #[error("ADPCM chunk shorter than its header")]
    Short,
    #[error("ADPCM step index {0} out of range")]
    BadIndex(u8),
}

#[derive(Clone, Copy)]
struct State {
    predictor: i32,
    index: i32,
}

impl State {
    fn decode(&mut self, code: u8) -> i16 {
        let step = STEPS[self.index as usize];
        let mut diff = step >> 3;
        if code & 4 != 0 {
            diff += step;
        }
        if code & 2 != 0 {
            diff += step >> 1;
        }
        if code & 1 != 0 {
            diff += step >> 2;
        }
        self.predictor = if code & 8 != 0 { self.predictor - diff } else { self.predictor + diff }.clamp(-32768, 32767);
        self.index = (self.index + INDEX_ADJUST[usize::from(code & 7)]).clamp(0, 88);
        self.predictor as i16
    }

    fn encode(&mut self, sample: i16) -> u8 {
        let step = STEPS[self.index as usize];
        let mut diff = i32::from(sample) - self.predictor;
        let mut code = 0u8;
        if diff < 0 {
            code = 8;
            diff = -diff;
        }
        let mut s = step;
        for bit in [4u8, 2, 1] {
            if diff >= s {
                code |= bit;
                diff -= s;
            }
            s >>= 1;
        }
        // Track exactly what the decoder will reconstruct.
        self.decode(code);
        code
    }
}

pub fn decode_chunk(chunk: &[u8]) -> Result<Vec<i16>, AdpcmError> {
    if chunk.len() < HEADER_LEN {
        return Err(AdpcmError::Short);
    }
    if chunk[2] > 88 {
        return Err(AdpcmError::BadIndex(chunk[2]));
    }
    let mut st = State { predictor: i32::from(i16::from_le_bytes([chunk[0], chunk[1]])), index: i32::from(chunk[2]) };
    let mut out = Vec::with_capacity((chunk.len() - HEADER_LEN) * 2);
    for &b in &chunk[HEADER_LEN..] {
        out.push(st.decode(b & 0x0F));
        out.push(st.decode(b >> 4));
    }
    Ok(out)
}

/// The reference encoder for tests and diagnostics; the PSP has its own in `psp/adpcm.c`.
/// An odd sample count is padded with a repeat of the last sample.
pub fn encode_chunk(samples: &[i16], predictor: i16, index: u8) -> Vec<u8> {
    let mut st = State { predictor: i32::from(predictor), index: i32::from(index.min(88)) };
    let mut out = Vec::with_capacity(HEADER_LEN + samples.len().div_ceil(2));
    out.extend_from_slice(&predictor.to_le_bytes());
    out.push(index.min(88));
    out.push(0);
    for pair in samples.chunks(2) {
        let lo = st.encode(pair[0]);
        let hi = st.encode(*pair.get(1).unwrap_or(&pair[0]));
        out.push(lo | (hi << 4));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_tracks_a_tone() {
        let pcm: Vec<i16> = (0..11_025).map(|i| ((i as f32 * 0.1).sin() * 12_000.0) as i16).collect();
        let enc = encode_chunk(&pcm, 0, 0);
        assert_eq!(enc.len(), HEADER_LEN + pcm.len() / 2 + pcm.len() % 2);
        let dec = decode_chunk(&enc).unwrap();
        let err: f64 = pcm.iter().zip(&dec).skip(200).map(|(a, b)| f64::from(a.abs_diff(*b))).sum::<f64>()
            / (pcm.len() - 200) as f64;
        assert!(err < 400.0, "mean error {err}");
    }

    #[test]
    fn rejects_bad_chunks() {
        assert_eq!(decode_chunk(&[0, 0, 0]), Err(AdpcmError::Short));
        assert_eq!(decode_chunk(&[0, 0, 89, 0]), Err(AdpcmError::BadIndex(89)));
        assert_eq!(decode_chunk(&[0x10, 0x00, 0, 0]).unwrap(), Vec::<i16>::new());
    }

    #[test]
    fn known_vector() {
        // Step 7: code 7 adds 0+7+3+1 = 11 and moves to index 8 (step 16); code 0xF subtracts 2+16+8+4 = 30.
        assert_eq!(decode_chunk(&[0, 0, 0, 0, 0xF7]).unwrap(), vec![11, -19]);
    }
}
