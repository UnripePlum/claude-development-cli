use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug)]
pub enum TranscriberError {
    Transcription(String),
    WavWrite(String),
}

impl std::fmt::Display for TranscriberError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transcription(e) => write!(f, "Transcription failed: {}", e),
            Self::WavWrite(e) => write!(f, "WAV write failed: {}", e),
        }
    }
}

pub struct Transcriber;

impl Transcriber {
    /// Create a new Transcriber. No model loading needed — Python handles it.
    pub fn new<F>(_progress_fn: F) -> Result<Self, TranscriberError>
    where
        F: Fn(u64, u64) + Send + 'static,
    {
        // Verify Python and faster-whisper are available
        let check = Command::new("python3")
            .args(["-c", "import faster_whisper"])
            .output();
        match check {
            Ok(output) if output.status.success() => Ok(Self),
            Ok(output) => {
                let err = String::from_utf8_lossy(&output.stderr);
                Err(TranscriberError::Transcription(format!(
                    "faster-whisper not installed. Run: pip install faster-whisper\n{}",
                    err.trim()
                )))
            }
            Err(e) => Err(TranscriberError::Transcription(format!(
                "python3 not found: {}",
                e
            ))),
        }
    }

    /// Transcribe f32 mono 16kHz samples by writing WAV and calling Python script.
    pub fn transcribe(&self, samples: &[f32]) -> Result<String, TranscriberError> {
        // Write samples to a temp WAV file
        let wav_path = std::env::temp_dir().join("cdc_voice_input.wav");
        write_wav(&wav_path, samples, 16000)?;

        // Find the stt.py script
        let script_path = find_stt_script();

        // Call Python
        let output = Command::new("python3")
            .arg(&script_path)
            .arg(&wav_path)
            .output()
            .map_err(|e| TranscriberError::Transcription(format!("Failed to run stt.py: {}", e)))?;

        // Clean up temp file
        let _ = std::fs::remove_file(&wav_path);

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(TranscriberError::Transcription(format!(
                "stt.py failed: {}",
                stderr.trim()
            )));
        }

        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();

        // Debug log if CDC_VOICE_LOG is set
        if let Ok(path) = std::env::var("CDC_VOICE_LOG") {
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .and_then(|mut f| {
                    use std::io::Write;
                    writeln!(f, "[STT] result={:?}", text)
                });
        }

        Ok(text)
    }
}

/// Find stt.py relative to the executable or in common locations.
fn find_stt_script() -> PathBuf {
    // Check next to executable
    if let Ok(exe) = std::env::current_exe() {
        let dir = exe.parent().unwrap_or(std::path::Path::new("."));
        let candidate = dir.join("scripts").join("stt.py");
        if candidate.exists() {
            return candidate;
        }
        // Two levels up (target/release/../scripts)
        let candidate = dir.parent()
            .and_then(|p| p.parent())
            .map(|p| p.join("scripts").join("stt.py"));
        if let Some(c) = candidate {
            if c.exists() {
                return c;
            }
        }
    }
    // Check cwd
    let cwd_candidate = PathBuf::from("scripts/stt.py");
    if cwd_candidate.exists() {
        return cwd_candidate;
    }
    // Fallback: assume it's in PATH-accessible location
    PathBuf::from("scripts/stt.py")
}

/// Write f32 mono samples as a 16-bit PCM WAV file.
fn write_wav(path: &PathBuf, samples: &[f32], sample_rate: u32) -> Result<(), TranscriberError> {
    let mut file = std::fs::File::create(path)
        .map_err(|e| TranscriberError::WavWrite(e.to_string()))?;

    let num_samples = samples.len() as u32;
    let bytes_per_sample = 2u16; // 16-bit PCM
    let data_size = num_samples * bytes_per_sample as u32;
    let file_size = 36 + data_size;

    // WAV header
    file.write_all(b"RIFF").map_err(|e| TranscriberError::WavWrite(e.to_string()))?;
    file.write_all(&file_size.to_le_bytes()).map_err(|e| TranscriberError::WavWrite(e.to_string()))?;
    file.write_all(b"WAVE").map_err(|e| TranscriberError::WavWrite(e.to_string()))?;

    // fmt chunk
    file.write_all(b"fmt ").map_err(|e| TranscriberError::WavWrite(e.to_string()))?;
    file.write_all(&16u32.to_le_bytes()).map_err(|e| TranscriberError::WavWrite(e.to_string()))?;
    file.write_all(&1u16.to_le_bytes()).map_err(|e| TranscriberError::WavWrite(e.to_string()))?; // PCM
    file.write_all(&1u16.to_le_bytes()).map_err(|e| TranscriberError::WavWrite(e.to_string()))?; // mono
    file.write_all(&sample_rate.to_le_bytes()).map_err(|e| TranscriberError::WavWrite(e.to_string()))?;
    let byte_rate = sample_rate * bytes_per_sample as u32;
    file.write_all(&byte_rate.to_le_bytes()).map_err(|e| TranscriberError::WavWrite(e.to_string()))?;
    file.write_all(&bytes_per_sample.to_le_bytes()).map_err(|e| TranscriberError::WavWrite(e.to_string()))?;
    file.write_all(&16u16.to_le_bytes()).map_err(|e| TranscriberError::WavWrite(e.to_string()))?; // bits per sample

    // data chunk
    file.write_all(b"data").map_err(|e| TranscriberError::WavWrite(e.to_string()))?;
    file.write_all(&data_size.to_le_bytes()).map_err(|e| TranscriberError::WavWrite(e.to_string()))?;

    // Convert f32 to i16 PCM
    for &sample in samples {
        let clamped = sample.max(-1.0).min(1.0);
        let pcm = (clamped * 32767.0) as i16;
        file.write_all(&pcm.to_le_bytes()).map_err(|e| TranscriberError::WavWrite(e.to_string()))?;
    }

    file.flush().map_err(|e| TranscriberError::WavWrite(e.to_string()))?;
    Ok(())
}
