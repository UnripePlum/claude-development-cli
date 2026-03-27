use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{Arc, Mutex};

#[derive(Debug)]
pub enum RecorderError {
    NoInputDevice,
    StreamConfig(String),
    StreamError(String),
}

impl std::fmt::Display for RecorderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoInputDevice => write!(f, "No audio input device found. Grant microphone access to your terminal in System Settings > Privacy > Microphone"),
            Self::StreamConfig(e) => write!(f, "Audio config error: {}", e),
            Self::StreamError(e) => write!(f, "Audio stream error: {}", e),
        }
    }
}

pub struct AudioRecorder {
    stream: Option<cpal::Stream>,
    buffer: Arc<Mutex<Vec<f32>>>,
    device_sample_rate: u32,
}

impl AudioRecorder {
    pub fn new() -> Self {
        Self {
            stream: None,
            buffer: Arc::new(Mutex::new(Vec::new())),
            device_sample_rate: 16000,
        }
    }

    pub fn start(&mut self) -> Result<(), RecorderError> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or(RecorderError::NoInputDevice)?;

        let config = device
            .default_input_config()
            .map_err(|e| RecorderError::StreamConfig(e.to_string()))?;

        self.device_sample_rate = config.sample_rate();
        let channels = config.channels() as usize;

        // Clear buffer
        self.buffer = Arc::new(Mutex::new(Vec::new()));
        let buf = Arc::clone(&self.buffer);

        let stream_config: cpal::StreamConfig = config.into();

        let stream = device
            .build_input_stream(
                &stream_config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    // Convert to mono by averaging channels, then push
                    let mut lock = buf.lock().unwrap();
                    if channels == 1 {
                        lock.extend_from_slice(data);
                    } else {
                        for chunk in data.chunks(channels) {
                            let sum: f32 = chunk.iter().sum();
                            lock.push(sum / channels as f32);
                        }
                    }
                },
                move |err| {
                    eprintln!("Audio input error: {}", err);
                },
                None, // timeout
            )
            .map_err(|e| RecorderError::StreamError(e.to_string()))?;

        stream
            .play()
            .map_err(|e| RecorderError::StreamError(e.to_string()))?;

        self.stream = Some(stream);
        Ok(())
    }

    /// Stop recording and return mono f32 samples resampled to 16kHz.
    pub fn stop(&mut self) -> Result<Vec<f32>, RecorderError> {
        // Pause stream first to prevent race with callback
        if let Some(ref stream) = self.stream {
            let _ = stream.pause();
        }
        self.stream = None;

        let samples = {
            let lock = self.buffer.lock().unwrap();
            lock.clone()
        };

        // Resample to 16kHz if needed
        if self.device_sample_rate != 16000 && !samples.is_empty() {
            Ok(resample(&samples, self.device_sample_rate, 16000))
        } else {
            Ok(samples)
        }
    }
}

/// Simple linear interpolation resampler (sufficient for speech).
fn resample(input: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || input.is_empty() {
        return input.to_vec();
    }
    let ratio = from_rate as f64 / to_rate as f64;
    let out_len = (input.len() as f64 / ratio) as usize;
    let mut output = Vec::with_capacity(out_len);

    for i in 0..out_len {
        let src_idx = i as f64 * ratio;
        let idx = src_idx as usize;
        let frac = src_idx - idx as f64;

        let sample = if idx + 1 < input.len() {
            input[idx] as f64 * (1.0 - frac) + input[idx + 1] as f64 * frac
        } else {
            input[idx.min(input.len() - 1)] as f64
        };
        output.push(sample as f32);
    }
    output
}
