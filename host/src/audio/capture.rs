//! PCM sources for the PC speaker stream: WASAPI loopback of an output device (a monitor input elsewhere),
//! or a generated signal.

use std::time::Duration;

use tokio::sync::mpsc;

/// Interleaved f32 blocks from a running capture. Dropping it stops the capture.
pub struct CaptureStream {
    pub sample_rate: u32,
    pub channels: usize,
    pub rx: mpsc::Receiver<Vec<f32>>,
    _stop: Box<dyn Send>,
}

pub trait CaptureSource: Send + Sync {
    fn open(&self) -> Result<CaptureStream, String>;
}

const BLOCK_QUEUE: usize = 64;

/// Records what an output device plays (WASAPI loopback on Windows), without playing anything itself.
pub struct LoopbackSource {
    /// Output device name; None for the default output device.
    pub device: Option<String>,
}

impl CaptureSource for LoopbackSource {
    fn open(&self) -> Result<CaptureStream, String> {
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
        let (tx, rx) = mpsc::channel(BLOCK_QUEUE);
        let wanted = self.device.clone();
        // cpal streams are not Send on every platform, so the stream lives and dies on its own thread.
        std::thread::Builder::new()
            .name("speaker-capture".into())
            .spawn(move || match loopback::start(wanted.as_deref(), tx) {
                Ok((stream, rate, channels)) => {
                    let _ = ready_tx.send(Ok((rate, channels)));
                    let _ = stop_rx.recv();
                    drop(stream);
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            })
            .map_err(|e| format!("cannot start capture thread: {e}"))?;
        let (sample_rate, channels) =
            ready_rx.recv_timeout(Duration::from_secs(5)).map_err(|_| "capture did not start".to_string())??;
        Ok(CaptureStream { sample_rate, channels, rx, _stop: Box::new(stop_tx) })
    }
}

mod loopback {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{SampleFormat, Stream};
    use tokio::sync::mpsc;
    use tracing::warn;

    pub fn start(wanted: Option<&str>, tx: mpsc::Sender<Vec<f32>>) -> Result<(Stream, u32, usize), String> {
        let (device, supported) = open_device(wanted)?;
        let config = supported.config();
        let (rate, channels) = (config.sample_rate, usize::from(config.channels));
        let err = |e| warn!("speaker capture error: {e}");
        let stream = match supported.sample_format() {
            SampleFormat::F32 => device.build_input_stream(
                config,
                move |data: &[f32], _: &_| {
                    let _ = tx.try_send(data.to_vec());
                },
                err,
                None,
            ),
            SampleFormat::I16 => device.build_input_stream(
                config,
                move |data: &[i16], _: &_| {
                    let _ = tx.try_send(data.iter().map(|&s| f32::from(s) / 32768.0).collect());
                },
                err,
                None,
            ),
            other => return Err(format!("unsupported capture sample format {other:?}")),
        }
        .map_err(|e| format!("cannot open loopback capture: {e}"))?;
        stream.play().map_err(|e| e.to_string())?;
        Ok((stream, rate, channels))
    }

    /// WASAPI captures an output device in loopback mode when an input stream is built on it.
    #[cfg(windows)]
    fn open_device(wanted: Option<&str>) -> Result<(cpal::Device, cpal::SupportedStreamConfig), String> {
        let host = cpal::default_host();
        let device = match wanted {
            None => host.default_output_device().ok_or("no default output device")?,
            Some(name) => host
                .output_devices()
                .map_err(|e| e.to_string())?
                .find(|d| d.description().is_ok_and(|desc| desc.name() == name))
                .ok_or_else(|| format!("output device {name:?} not found"))?,
        };
        let supported = device.default_output_config().map_err(|e| e.to_string())?;
        Ok((device, supported))
    }

    /// Elsewhere there is no loopback mode: the user names an input device that carries the output mix
    /// (on PipeWire/PulseAudio, `pulse` with `PULSE_SOURCE=@DEFAULT_MONITOR@`). Never falls back to the
    /// default input, which is usually the microphone.
    #[cfg(not(windows))]
    fn open_device(wanted: Option<&str>) -> Result<(cpal::Device, cpal::SupportedStreamConfig), String> {
        let name = wanted.ok_or(
            "set [audio] speaker_device to a monitor input (for example \"pulse\" with PULSE_SOURCE=@DEFAULT_MONITOR@)",
        )?;
        let device = cpal::default_host()
            .input_devices()
            .map_err(|e| e.to_string())?
            .find(|d| d.description().is_ok_and(|desc| desc.name() == name))
            .ok_or_else(|| format!("input device {name:?} not found"))?;
        let supported = device.default_input_config().map_err(|e| e.to_string())?;
        Ok((device, supported))
    }
}

/// A scripted signal delivered in real time: tones and silences, for tests and diagnostics.
pub struct GeneratedSource {
    pub sample_rate: u32,
    pub channels: usize,
    /// (frequency in Hz, or 0 for silence; seconds)
    pub script: Vec<(f32, f32)>,
    /// Deliver faster than real time (tests).
    pub speedup: u32,
}

impl CaptureSource for GeneratedSource {
    fn open(&self) -> Result<CaptureStream, String> {
        let (tx, rx) = mpsc::channel(BLOCK_QUEUE);
        let (rate, ch, script, speedup) = (self.sample_rate, self.channels, self.script.clone(), self.speedup.max(1));
        let task = tokio::spawn(async move {
            let block = (rate / 100) as usize; // 10 ms
            let mut tick = tokio::time::interval(Duration::from_millis(10) / speedup);
            let mut n = 0u64;
            for (freq, secs) in script {
                let blocks = (secs * 100.0).round() as usize;
                for _ in 0..blocks {
                    tick.tick().await;
                    let samples: Vec<f32> = (0..block)
                        .flat_map(|i| {
                            let t = (n + i as u64) as f32 / rate as f32;
                            let v = if freq > 0.0 { (t * freq * std::f32::consts::TAU).sin() * 0.5 } else { 0.0 };
                            vec![v; ch]
                        })
                        .collect();
                    n += block as u64;
                    if tx.send(samples).await.is_err() {
                        return;
                    }
                }
            }
        });
        Ok(CaptureStream { sample_rate: rate, channels: ch, rx, _stop: Box::new(AbortOnDrop(task)) })
    }
}

struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}
