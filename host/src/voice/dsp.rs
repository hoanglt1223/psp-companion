//! Cleanup of the PSP microphone signal before speech-to-text, and WAV files.

use std::io::Write;
use std::path::Path;

/// High-pass (removes DC and rumble below about 80 Hz), a noise gate at twice the estimated noise floor,
/// and peak normalisation (gain capped at 20 dB) — the PSP-3000 mic is quiet and hissy.
pub fn clean_speech(samples: &[i16], rate: u32) -> Vec<f32> {
    if samples.is_empty() {
        return Vec::new();
    }
    let rc = 1.0 / (std::f32::consts::TAU * 80.0);
    let dt = 1.0 / rate as f32;
    let alpha = rc / (rc + dt);
    let mut out = Vec::with_capacity(samples.len());
    let (mut prev_x, mut prev_y) = (f32::from(samples[0]) / 32768.0, 0.0f32);
    for &s in samples {
        let x = f32::from(s) / 32768.0;
        let y = alpha * (prev_y + x - prev_x);
        out.push(y);
        prev_x = x;
        prev_y = y;
    }

    let win = (rate as usize / 50).max(1); // 20 ms
    let mut rms: Vec<f32> =
        out.chunks(win).map(|w| (w.iter().map(|v| v * v).sum::<f32>() / w.len() as f32).sqrt()).collect();
    let mut sorted = rms.clone();
    sorted.sort_by(f32::total_cmp);
    let floor = sorted[sorted.len() / 10];
    let threshold = floor * 2.0;
    for (w, level) in out.chunks_mut(win).zip(rms.iter_mut()) {
        if *level < threshold {
            w.iter_mut().for_each(|v| *v *= 0.1);
        }
    }

    let peak = out.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    if peak > 0.0 {
        let gain = (0.7 / peak).min(10.0);
        out.iter_mut().for_each(|v| *v *= gain);
    }
    out
}

/// Seconds of signal that rise clearly above the quietest parts.
pub fn voiced_seconds(samples: &[f32], rate: u32) -> f32 {
    let win = (rate as usize / 50).max(1);
    let rms: Vec<f32> =
        samples.chunks(win).map(|w| (w.iter().map(|v| v * v).sum::<f32>() / w.len() as f32).sqrt()).collect();
    let peak = rms.iter().fold(0.0f32, |m, v| m.max(*v));
    let voiced = rms.iter().filter(|r| **r > peak * 0.15 && **r > 0.01).count();
    voiced as f32 * win as f32 / rate as f32
}

pub fn wav_bytes(samples: &[f32], rate: u32) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
    }
    out
}

pub fn write_wav(path: &Path, samples: &[f32], rate: u32) -> std::io::Result<()> {
    let mut f = std::fs::File::create(path)?;
    f.write_all(&wav_bytes(samples, rate))
}

/// Mono 16-bit PCM from a WAV file (any rate); used for TTS responses that come back as WAV.
pub fn parse_wav(bytes: &[u8]) -> Option<(u32, Vec<f32>)> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let (mut pos, mut fmt) = (12usize, None);
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let len = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().ok()?) as usize;
        let body = bytes.get(pos + 8..pos + 8 + len.min(bytes.len() - pos - 8))?;
        if id == b"fmt " && body.len() >= 16 {
            let format = u16::from_le_bytes([body[0], body[1]]);
            let channels = u16::from_le_bytes([body[2], body[3]]);
            let rate = u32::from_le_bytes(body[4..8].try_into().ok()?);
            let bits = u16::from_le_bytes([body[14], body[15]]);
            if format != 1 || bits != 16 || channels == 0 {
                return None;
            }
            fmt = Some((rate, usize::from(channels)));
        } else if id == b"data" {
            let (rate, ch) = fmt?;
            let samples =
                body.chunks_exact(2 * ch).map(|f| f32::from(i16::from_le_bytes([f[0], f[1]])) / 32768.0).collect();
            return Some((rate, samples));
        }
        pos += 8 + len + (len & 1);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn speech_like(rate: u32) -> Vec<i16> {
        // 0.5 s of hiss, 1 s of a 300 Hz "voice", 0.5 s of hiss, plus a DC offset.
        let mut seed = 1u32;
        let mut noise = move || {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            ((seed >> 16) as f32 / 32768.0 - 1.0) * 300.0
        };
        (0..rate * 2)
            .map(|i| {
                let t = i as f32 / rate as f32;
                let voice =
                    if (0.5..1.5).contains(&t) { (t * 300.0 * std::f32::consts::TAU).sin() * 3000.0 } else { 0.0 };
                (voice + noise() + 2000.0) as i16
            })
            .collect()
    }

    #[test]
    fn cleans_and_normalises_speech() {
        let out = clean_speech(&speech_like(11_025), 11_025);
        let mean = out.iter().sum::<f32>() / out.len() as f32;
        assert!(mean.abs() < 0.01, "DC removed: {mean}");
        let peak = out.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((0.6..=0.71).contains(&peak), "normalised: {peak}");
        let rms = |s: &[f32]| (s.iter().map(|v| v * v).sum::<f32>() / s.len() as f32).sqrt();
        let noise = rms(&out[1000..5000]);
        let voice = rms(&out[6000..15000]);
        assert!(voice / noise > 30.0, "noise gated: voice {voice} noise {noise}");
        let v = voiced_seconds(&out, 11_025);
        assert!((0.8..1.2).contains(&v), "voiced {v} s");
        assert!(voiced_seconds(&clean_speech(&[0; 11_025], 11_025), 11_025) < 0.05);
    }

    #[test]
    fn wav_round_trip() {
        let s = vec![0.0, 0.5, -0.5, 0.25];
        let bytes = wav_bytes(&s, 16_000);
        assert_eq!(&bytes[..4], b"RIFF");
        let (rate, back) = parse_wav(&bytes).unwrap();
        assert_eq!(rate, 16_000);
        assert!(back.iter().zip(&s).all(|(a, b)| (a - b).abs() < 1e-3));
        assert!(parse_wav(b"RIFF....WAVEjunk").is_none());
    }
}
