//! Microphone capture and conversion to Whisper's input format (16 kHz, mono, f32).

use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, Stream, StreamConfig};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Whisper models expect 16 kHz mono audio.
pub const WHISPER_RATE: u32 = 16_000;

/// A finished recording, already converted to 16 kHz mono.
pub struct Clip {
    pub samples: Vec<f32>,
}

impl Clip {
    pub fn duration_secs(&self) -> f32 {
        self.samples.len() as f32 / WHISPER_RATE as f32
    }

    /// Root-mean-square loudness. Near zero means the mic picked up silence.
    pub fn rms(&self) -> f32 {
        if self.samples.is_empty() {
            return 0.0;
        }
        let sum: f32 = self.samples.iter().map(|s| s * s).sum();
        (sum / self.samples.len() as f32).sqrt()
    }

    pub fn save_wav(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: WHISPER_RATE,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(path, spec)?;
        for s in &self.samples {
            w.write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)?;
        }
        w.finalize()?;
        Ok(())
    }

    /// Load any PCM WAV file and convert it to 16 kHz mono.
    pub fn from_wav(path: &Path) -> Result<Self> {
        let mut r = hound::WavReader::open(path)
            .with_context(|| format!("Could not open {}", path.display()))?;
        let spec = r.spec();
        let interleaved: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => r.samples::<f32>().collect::<Result<_, _>>()?,
            hound::SampleFormat::Int => {
                let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
                r.samples::<i32>()
                    .map(|s| s.map(|v| v as f32 / max))
                    .collect::<Result<_, _>>()?
            }
        };
        let mono = downmix(&interleaved, spec.channels as usize);
        Ok(Self { samples: resample(&mono, spec.sample_rate, WHISPER_RATE) })
    }
}

/// Records from the default microphone between `start()` and `stop()`.
///
/// The mic is only opened while recording, so Windows' "microphone in use"
/// indicator is off the rest of the time.
pub struct Recorder {
    device: cpal::Device,
    config: StreamConfig,
    format: SampleFormat,
    buffer: Arc<Mutex<Vec<f32>>>, // mono, at the device's native rate
    stream: Option<Stream>,
}

impl Recorder {
    pub fn new() -> Result<Self> {
        let device = cpal::default_host()
            .default_input_device()
            .ok_or_else(|| anyhow!("No microphone found. Check Windows sound settings."))?;
        let supported = device
            .default_input_config()
            .context("Could not read the microphone's default format")?;
        Ok(Self {
            format: supported.sample_format(),
            config: supported.config(),
            device,
            buffer: Arc::new(Mutex::new(Vec::new())),
            stream: None,
        })
    }

    pub fn device_name(&self) -> String {
        self.device
            .description()
            .map(|d| d.name().to_string())
            .unwrap_or_else(|_| "unknown microphone".into())
    }

    pub fn is_recording(&self) -> bool {
        self.stream.is_some()
    }

    pub fn start(&mut self) -> Result<()> {
        if self.stream.is_some() {
            return Ok(());
        }
        self.buffer.lock().unwrap().clear();
        let stream = match self.format {
            SampleFormat::F32 => self.build::<f32>()?,
            SampleFormat::I16 => self.build::<i16>()?,
            SampleFormat::U16 => self.build::<u16>()?,
            SampleFormat::I32 => self.build::<i32>()?,
            other => return Err(anyhow!("Unsupported microphone sample format: {other:?}")),
        };
        stream.play().context("Could not start the microphone")?;
        self.stream = Some(stream);
        Ok(())
    }

    /// Stop recording and return the clip at 16 kHz mono.
    pub fn stop(&mut self) -> Clip {
        // Dropping the stream closes the microphone.
        self.stream.take();
        let raw = std::mem::take(&mut *self.buffer.lock().unwrap());
        Clip { samples: resample(&raw, self.config.sample_rate, WHISPER_RATE) }
    }

    fn build<T>(&self) -> Result<Stream>
    where
        T: SizedSample,
        f32: FromSample<T>,
    {
        let buffer = self.buffer.clone();
        let channels = self.config.channels as usize;
        let stream = self.device.build_input_stream::<T, _, _>(
            self.config.clone(),
            move |data: &[T], _| {
                let mut buf = buffer.lock().unwrap();
                for frame in data.chunks(channels) {
                    let sum: f32 = frame.iter().map(|s| f32::from_sample_(*s)).sum();
                    buf.push(sum / frame.len() as f32);
                }
            },
            |err| eprintln!("  [audio error] {err}"),
            None,
        )?;
        Ok(stream)
    }
}

fn downmix(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    interleaved
        .chunks(channels)
        .map(|f| f.iter().sum::<f32>() / f.len() as f32)
        .collect()
}

/// Simple sample-rate conversion. When downsampling (the usual case: 48 kHz mic
/// to 16 kHz) each output sample averages the input samples it covers, which
/// filters out most aliasing. Good enough for speech recognition.
pub fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || input.is_empty() {
        return input.to_vec();
    }
    let ratio = from as f64 / to as f64;
    let out_len = (input.len() as f64 / ratio).floor() as usize;
    let mut out = Vec::with_capacity(out_len);
    if ratio > 1.0 {
        for i in 0..out_len {
            let start = (i as f64 * ratio) as usize;
            let end = (((i + 1) as f64 * ratio) as usize)
                .min(input.len())
                .max(start + 1);
            let window = &input[start..end];
            out.push(window.iter().sum::<f32>() / window.len() as f32);
        }
    } else {
        for i in 0..out_len {
            let pos = i as f64 * ratio;
            let idx = pos as usize;
            let frac = (pos - idx as f64) as f32;
            let a = input[idx];
            let b = *input.get(idx + 1).unwrap_or(&a);
            out.push(a + (b - a) * frac);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_48k_to_16k_keeps_duration() {
        let one_second = vec![0.5f32; 48_000];
        let out = resample(&one_second, 48_000, 16_000);
        assert_eq!(out.len(), 16_000);
        assert!(out.iter().all(|s| (s - 0.5).abs() < 1e-6));
    }

    #[test]
    fn resample_44100_to_16k() {
        let out = resample(&vec![0.0f32; 44_100], 44_100, 16_000);
        assert_eq!(out.len(), 16_000);
    }

    #[test]
    fn downmix_stereo() {
        assert_eq!(downmix(&[1.0, 0.0, 0.5, 0.5], 2), vec![0.5, 0.5]);
    }
}
