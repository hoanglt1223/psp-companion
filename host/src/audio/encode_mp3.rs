//! MP3 encoding for the PSP's Media Engine: MPEG-1 Layer III at 44.1 kHz, constant bitrate, no bit
//! reservoir (every frame decodes on its own, so a lost packet costs exactly one frame).

use mp3lame_encoder::{Bitrate, Builder, DualPcm, Encoder, FlushNoGap, MonoPcm, Quality};
use thiserror::Error;

pub const SAMPLE_RATE: u32 = 44_100;
pub const FRAME_SAMPLES: usize = 1152;
/// Duration of one frame in microseconds (1152 / 44100 s).
pub const FRAME_US: u64 = FRAME_SAMPLES as u64 * 1_000_000 / SAMPLE_RATE as u64;

#[derive(Debug, Error)]
pub enum EncodeError {
    #[error("unsupported encoder setting: {0}")]
    Config(String),
    #[error("MP3 encoding failed: {0}")]
    Encode(String),
}

/// Length in bytes of the MPEG-1 Layer III frame starting at `h`, or None if it is not one.
pub fn frame_len(h: &[u8]) -> Option<usize> {
    const KBPS: [u32; 16] = [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0];
    const RATES: [u32; 4] = [44_100, 48_000, 32_000, 0];
    if h.len() < 4 || h[0] != 0xFF || h[1] & 0xFE != 0xFA {
        return None; // sync, MPEG-1, Layer III (CRC bit either way)
    }
    let kbps = KBPS[usize::from(h[2] >> 4)];
    let rate = RATES[usize::from((h[2] >> 2) & 3)];
    if kbps == 0 || rate == 0 {
        return None;
    }
    Some((144 * kbps * 1000 / rate) as usize + usize::from((h[2] >> 1) & 1))
}

/// Splits a byte stream into whole frames, keeping a partial tail for the next call.
#[derive(Default)]
pub struct FrameSplitter {
    buf: Vec<u8>,
}

impl FrameSplitter {
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        self.buf.extend_from_slice(bytes);
        let mut frames = Vec::new();
        let mut pos = 0;
        while self.buf.len() - pos >= 4 {
            match frame_len(&self.buf[pos..]) {
                Some(n) if self.buf.len() - pos >= n => {
                    frames.push(self.buf[pos..pos + n].to_vec());
                    pos += n;
                }
                Some(_) => break,
                None => pos += 1, // resync; LAME never emits junk, but stay robust
            }
        }
        self.buf.drain(..pos);
        frames
    }
}

fn bitrate(kbps: u16) -> Result<Bitrate, EncodeError> {
    Ok(match kbps {
        32 => Bitrate::Kbps32,
        40 => Bitrate::Kbps40,
        48 => Bitrate::Kbps48,
        64 => Bitrate::Kbps64,
        80 => Bitrate::Kbps80,
        96 => Bitrate::Kbps96,
        112 => Bitrate::Kbps112,
        128 => Bitrate::Kbps128,
        other => return Err(EncodeError::Config(format!("bitrate {other} kbps"))),
    })
}

pub struct Mp3Encoder {
    enc: Encoder,
    channels: u8,
    splitter: FrameSplitter,
    out: Vec<u8>,
}

impl Mp3Encoder {
    /// `channels` is 1 or 2; input is 44.1 kHz interleaved 16-bit PCM.
    pub fn new(channels: u8, kbps: u16) -> Result<Self, EncodeError> {
        let cfg = |e: mp3lame_encoder::BuildError| EncodeError::Config(e.to_string());
        if !(1..=2).contains(&channels) {
            return Err(EncodeError::Config(format!("{channels} channels")));
        }
        let mut b = Builder::new().ok_or_else(|| EncodeError::Config("LAME init failed".into()))?;
        b.set_num_channels(channels).map_err(cfg)?;
        b.set_sample_rate(SAMPLE_RATE).map_err(cfg)?;
        // LAME would otherwise pick a lower output rate for low bitrates; the PSP path assumes 44.1 kHz.
        b.set_output_sample_rate(std::num::NonZeroU32::new(SAMPLE_RATE)).map_err(cfg)?;
        b.set_brate(bitrate(kbps)?).map_err(cfg)?;
        b.set_quality(Quality::Good).map_err(cfg)?;
        b.set_to_write_vbr_tag(false).map_err(cfg)?;
        if channels == 1 {
            b.set_mode(mp3lame_encoder::Mode::Mono).map_err(cfg)?;
        }
        // SAFETY: the builder owns a valid LAME handle until build(); this only sets a flag on it.
        let rc = unsafe { mp3lame_encoder::ffi::lame_set_disable_reservoir(b.as_ptr(), 1) };
        if rc != 0 {
            return Err(EncodeError::Config("cannot disable the bit reservoir".into()));
        }
        let enc = b.build().map_err(cfg)?;
        Ok(Self { enc, channels, splitter: FrameSplitter::default(), out: Vec::new() })
    }

    pub fn channels(&self) -> u8 {
        self.channels
    }

    /// Encodes interleaved samples; returns the frames completed so far.
    pub fn encode(&mut self, pcm: &[i16]) -> Result<Vec<Vec<u8>>, EncodeError> {
        let per_ch = pcm.len() / usize::from(self.channels);
        self.out.clear();
        self.out.reserve(mp3lame_encoder::max_required_buffer_size(per_ch));
        let n = if self.channels == 1 {
            self.enc.encode(MonoPcm(pcm), self.out.spare_capacity_mut())
        } else {
            let (l, r): (Vec<i16>, Vec<i16>) = pcm.as_chunks::<2>().0.iter().map(|c| (c[0], c[1])).unzip();
            self.enc.encode(DualPcm { left: &l, right: &r }, self.out.spare_capacity_mut())
        }
        .map_err(|e| EncodeError::Encode(e.to_string()))?;
        // SAFETY: the encoder initialised the first `n` bytes of the spare capacity.
        unsafe { self.out.set_len(n) };
        Ok(self.splitter.push(&self.out))
    }

    /// Pads the last partial frame with silence and returns the remaining frames.
    pub fn flush(&mut self) -> Result<Vec<Vec<u8>>, EncodeError> {
        self.out.clear();
        self.out.reserve(7200);
        let n = self
            .enc
            .flush::<FlushNoGap>(self.out.spare_capacity_mut())
            .map_err(|e| EncodeError::Encode(e.to_string()))?;
        // SAFETY: as in encode().
        unsafe { self.out.set_len(n) };
        Ok(self.splitter.push(&self.out))
    }
}

/// Linear-interpolation resampler for interleaved f32 audio, streaming across calls.
pub struct Resampler {
    channels: usize,
    step: f64,
    pos: f64,
    prev: Vec<f32>,
}

impl Resampler {
    pub fn new(from_rate: u32, to_rate: u32, channels: usize) -> Self {
        Self { channels, step: f64::from(from_rate) / f64::from(to_rate), pos: 0.0, prev: vec![0.0; channels] }
    }

    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        let ch = self.channels;
        let frames = input.len() / ch;
        let mut out = Vec::with_capacity((frames as f64 / self.step) as usize * ch + ch);
        let at = |i: isize, c: usize| if i < 0 { self.prev[c] } else { input[i as usize * ch + c] };
        // `pos` is measured from the previous block's last frame (index -1).
        while self.pos < frames as f64 {
            let i = self.pos.floor() as isize - 1;
            let t = (self.pos - self.pos.floor()) as f32;
            for c in 0..ch {
                let a = at(i, c);
                let b = at(i + 1, c);
                out.push(a + (b - a) * t);
            }
            self.pos += self.step;
        }
        self.pos -= frames as f64;
        if frames > 0 {
            self.prev.copy_from_slice(&input[(frames - 1) * ch..frames * ch]);
        }
        out
    }
}

pub fn to_i16(samples: &[f32]) -> Vec<i16> {
    samples.iter().map(|s| (s.clamp(-1.0, 1.0) * 32767.0).round() as i16).collect()
}

/// Mixes interleaved audio with any channel count down or up to `to` channels.
pub fn remix(input: &[f32], from: usize, to: usize) -> Vec<f32> {
    if from == to {
        return input.to_vec();
    }
    input
        .chunks_exact(from)
        .flat_map(|f| {
            let (l, r) = if from == 1 { (f[0], f[0]) } else { (f[0], f[1]) };
            if to == 1 { vec![(l + r) * 0.5] } else { vec![l, r] }
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use symphonia::core::codecs::audio::AudioDecoderOptions;
    use symphonia::core::formats::probe::Hint;
    use symphonia::core::formats::{FormatOptions, TrackType};
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;

    pub fn sine(freq: f32, rate: u32, channels: usize, secs: f32) -> Vec<f32> {
        let n = (rate as f32 * secs) as usize;
        (0..n)
            .flat_map(|i| vec![(i as f32 * freq * std::f32::consts::TAU / rate as f32).sin() * 0.5; channels])
            .collect()
    }

    /// Decodes an MP3 byte stream with an independent decoder; returns interleaved samples and channels.
    pub fn decode(mp3: &[u8]) -> (Vec<f32>, usize) {
        let mss = MediaSourceStream::new(Box::new(std::io::Cursor::new(mp3.to_vec())), Default::default());
        let mut hint = Hint::new();
        hint.with_extension("mp3");
        let mut format = symphonia::default::get_probe()
            .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
            .unwrap();
        let track = format.default_track(TrackType::Audio).unwrap().clone();
        let params = track.codec_params.as_ref().unwrap().audio().unwrap().clone();
        let mut dec =
            symphonia::default::get_codecs().make_audio_decoder(&params, &AudioDecoderOptions::default()).unwrap();
        let mut out = Vec::new();
        let mut channels = 0;
        while let Ok(Some(packet)) = format.next_packet() {
            let buf = dec.decode(&packet).unwrap();
            channels = buf.spec().channels().count();
            let mut v = vec![0f32; buf.samples_interleaved()];
            buf.copy_to_slice_interleaved(&mut v);
            out.extend(v);
        }
        (out, channels)
    }

    /// Strength of `freq` in the signal (Goertzel), normalised by length.
    pub fn tone_power(mono: &[f32], freq: f32, rate: f32) -> f32 {
        let w = std::f32::consts::TAU * freq / rate;
        let (mut s1, mut s2) = (0f32, 0f32);
        for &x in mono {
            let s = x + 2.0 * w.cos() * s1 - s2;
            s2 = s1;
            s1 = s;
        }
        (s1 * s1 + s2 * s2 - 2.0 * w.cos() * s1 * s2).sqrt() / mono.len() as f32
    }

    #[test]
    fn frame_lengths() {
        assert_eq!(frame_len(&[0xFF, 0xFB, 0x70, 0x00]), Some(313)); // 96 kbps, 44.1 kHz
        assert_eq!(frame_len(&[0xFF, 0xFB, 0x72, 0x00]), Some(314)); // with padding
        assert_eq!(frame_len(&[0xFF, 0xFB, 0x30, 0xC0]), Some(156)); // 48 kbps
        assert_eq!(frame_len(&[0xFF, 0xF3, 0x70, 0x00]), None, "MPEG-2");
        assert_eq!(frame_len(&[0xFF, 0xFB, 0xF0, 0x00]), None, "bad bitrate");
    }

    #[test]
    fn stereo_96k_round_trips_through_a_reference_decoder() {
        let pcm = to_i16(&sine(1000.0, SAMPLE_RATE, 2, 1.0));
        let mut enc = Mp3Encoder::new(2, 96).unwrap();
        let mut frames = Vec::new();
        for chunk in pcm.chunks(441 * 2) {
            frames.extend(enc.encode(chunk).unwrap());
        }
        frames.extend(enc.flush().unwrap());
        assert!(frames.iter().all(|f| frame_len(f) == Some(f.len()) && f.len() <= 314));
        let bytes: Vec<u8> = frames.concat();
        let kbps = bytes.len() as f64 * 8.0 / 1000.0 / (frames.len() as f64 * FRAME_US as f64 / 1e6);
        assert!((95.0..97.0).contains(&kbps), "{kbps} kbps");

        let (out, channels) = decode(&bytes);
        assert_eq!(channels, 2);
        let left: Vec<f32> = out.as_chunks::<2>().0.iter().map(|c| c[0]).collect();
        let at = tone_power(&left[4410..], 1000.0, 44100.0);
        let off = tone_power(&left[4410..], 3000.0, 44100.0);
        assert!(at > 0.2 && off < at / 50.0, "1 kHz {at}, 3 kHz {off}");
    }

    #[test]
    fn frames_decode_independently_without_the_reservoir() {
        let pcm = to_i16(&sine(440.0, SAMPLE_RATE, 1, 1.0));
        let mut enc = Mp3Encoder::new(1, 48).unwrap();
        let mut frames = enc.encode(&pcm).unwrap();
        frames.extend(enc.flush().unwrap());
        // main_data_begin (the first 9 bits after the 4-byte header) must be 0 in every frame.
        for f in &frames {
            let begin = (u16::from(f[4]) << 1) | u16::from(f[5] >> 7);
            assert_eq!(begin, 0);
        }
        // Dropping every third frame still decodes the rest.
        let lossy: Vec<u8> =
            frames.iter().enumerate().filter(|(i, _)| i % 3 != 2).flat_map(|(_, f)| f.clone()).collect();
        let (out, channels) = decode(&lossy);
        assert_eq!(channels, 1);
        // Gaps break phase continuity, so measure the tone within each frame-sized window.
        let windows: Vec<f32> =
            out[2304..].as_chunks::<1152>().0.iter().map(|w| tone_power(w, 440.0, 44100.0)).collect();
        let p = windows.iter().sum::<f32>() / windows.len() as f32;
        assert!(p > 0.1);
    }

    #[test]
    fn resampler_keeps_pitch_and_length() {
        let input = sine(1000.0, 48_000, 2, 1.0);
        let mut r = Resampler::new(48_000, SAMPLE_RATE, 2);
        let out: Vec<f32> = input.chunks(480 * 2).flat_map(|c| r.process(c)).collect();
        let frames = out.len() / 2;
        assert!((44_090..=44_110).contains(&frames), "{frames}");
        let left: Vec<f32> = out.as_chunks::<2>().0.iter().map(|c| c[0]).collect();
        assert!(tone_power(&left, 1000.0, 44100.0) > 0.2);
        assert_eq!(remix(&[0.2, 0.4], 2, 1), vec![0.3f32 * 1.0]);
        assert_eq!(remix(&[0.5], 1, 2), vec![0.5, 0.5]);
        assert_eq!(to_i16(&[2.0, -1.0]), vec![32767, -32767]);
    }
}
