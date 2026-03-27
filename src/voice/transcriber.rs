use std::io::Write;
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

#[derive(Debug)]
pub enum TranscriberError {
    ModelDownload(String),
    ModelLoad(String),
    Transcription(String),
}

impl std::fmt::Display for TranscriberError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ModelDownload(e) => write!(f, "Model download failed: {}", e),
            Self::ModelLoad(e) => write!(f, "Model load failed: {}", e),
            Self::Transcription(e) => write!(f, "Transcription failed: {}", e),
        }
    }
}

const MODEL_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3.bin";
const EXPECTED_MODEL_SIZE: u64 = 3_094_623_691; // ~2.9GB ggml-large-v3.bin

pub struct Transcriber {
    ctx: WhisperContext,
}

impl Transcriber {
    /// Create a new Transcriber, downloading the model if necessary.
    /// `progress_fn` is called with (downloaded_bytes, total_bytes) during download.
    pub fn new<F>(progress_fn: F) -> Result<Self, TranscriberError>
    where
        F: Fn(u64, u64) + Send + 'static,
    {
        let model_path = model_path();
        if !model_path.exists() {
            download_model(&model_path, &progress_fn)?;
        }

        // Verify file size
        if let Ok(meta) = std::fs::metadata(&model_path) {
            let size = meta.len();
            // Allow 5% tolerance for different model versions
            if size < EXPECTED_MODEL_SIZE * 95 / 100 {
                let _ = std::fs::remove_file(&model_path);
                return Err(TranscriberError::ModelDownload(format!(
                    "Model file corrupt ({}B, expected ~{}B). Deleted. Retry Ctrl+R.",
                    size, EXPECTED_MODEL_SIZE
                )));
            }
        }

        let params = WhisperContextParameters::default();
        // Suppress whisper.cpp C library stderr output (model loading logs)
        // which would pollute the TUI
        let ctx = {
            let _guard = SuppressStderr::new();
            WhisperContext::new_with_params(
                model_path.to_str().unwrap_or(""),
                params,
            )
            .map_err(|e| TranscriberError::ModelLoad(format!("{}", e)))?
        };

        Ok(Self { ctx })
    }

    /// Transcribe f32 mono 16kHz samples to Korean text.
    pub fn transcribe(&self, samples: &[f32]) -> Result<String, TranscriberError> {
        let mut params = FullParams::new(SamplingStrategy::BeamSearch { beam_size: 5, patience: 1.0 });
        params.set_language(Some("ko"));
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_suppress_blank(true);
        // Prime the model with domain-specific Korean terms for better accuracy
        params.set_initial_prompt("워커, 오케스트레이터, 프롬프트, 코드, 테스트, 빌드, 커밋, 값, 넘겨, 실행");

        // Suppress whisper.cpp stderr for entire transcription (create_state + full + segment read)
        let _guard = SuppressStderr::new();

        let mut state = self.ctx.create_state()
            .map_err(|e| TranscriberError::Transcription(format!("{}", e)))?;

        state.full(params, samples)
            .map_err(|e| TranscriberError::Transcription(format!("{}", e)))?;

        let num_segments = state.full_n_segments();

        let mut text = String::new();
        for i in 0..num_segments {
            if let Some(segment) = state.get_segment(i) {
                if let Ok(s) = segment.to_str() {
                    let cleaned = sanitize_stt(s);
                    if !cleaned.is_empty() {
                        text.push_str(&cleaned);
                        text.push(' ');
                    }
                }
            }
        }

        let result = text.trim().to_string();

        // Debug log if CDC_VOICE_LOG is set
        if let Ok(path) = std::env::var("CDC_VOICE_LOG") {
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .and_then(|mut f| {
                    use std::io::Write;
                    writeln!(f, "[STT] raw_segments={} result={:?}", num_segments, result)
                });
        }

        Ok(result)
    }
}

fn model_path() -> PathBuf {
    let cache = dirs::cache_dir().unwrap_or_else(|| PathBuf::from("."));
    cache.join("cdc").join("whisper-large-v3.bin")
}

/// RAII guard that redirects stderr to /dev/null and restores on drop.
struct SuppressStderr {
    saved_fd: i32,
}

impl SuppressStderr {
    fn new() -> Self {
        unsafe {
            let saved = libc::dup(2); // save original stderr fd
            if let Ok(devnull) = std::fs::File::open("/dev/null") {
                libc::dup2(devnull.as_raw_fd(), 2);
            }
            Self { saved_fd: saved }
        }
    }
}

impl Drop for SuppressStderr {
    fn drop(&mut self) {
        unsafe {
            libc::dup2(self.saved_fd, 2); // restore stderr
            libc::close(self.saved_fd);
        }
    }
}

/// Remove whisper special tokens, control characters, and repeated artifacts.
fn sanitize_stt(raw: &str) -> String {
    let mut s = raw.trim().to_string();

    // Remove whisper special tokens like <|startoftranscript|>, <|ko|>, [BLANK_AUDIO], etc.
    let patterns = [
        "<|startoftranscript|>", "<|endoftext|>", "<|notimestamps|>",
        "<|ko|>", "<|en|>", "<|transcribe|>", "<|translate|>",
        "[BLANK_AUDIO]", "(blank_audio)", "[MUSIC]", "(music)",
    ];
    for pat in &patterns {
        s = s.replace(pat, "");
    }

    // Remove any remaining <|...|> tokens
    while let Some(start) = s.find("<|") {
        if let Some(end) = s[start..].find("|>") {
            s = format!("{}{}", &s[..start], &s[start + end + 2..]);
        } else {
            break;
        }
    }

    // Remove control characters (except normal whitespace)
    s = s.chars().filter(|c| !c.is_control() || *c == ' ' || *c == '\t').collect();

    // Collapse excessive whitespace
    let mut result = String::new();
    let mut prev_space = false;
    for c in s.chars() {
        if c == ' ' {
            if !prev_space {
                result.push(c);
            }
            prev_space = true;
        } else {
            result.push(c);
            prev_space = false;
        }
    }

    result.trim().to_string()
}

fn download_model<F>(path: &PathBuf, progress_fn: &F) -> Result<(), TranscriberError>
where
    F: Fn(u64, u64),
{
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| TranscriberError::ModelDownload(e.to_string()))?;
    }

    let response = ureq::get(MODEL_URL)
        .call()
        .map_err(|e| TranscriberError::ModelDownload(e.to_string()))?;

    let total = response
        .header("Content-Length")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(EXPECTED_MODEL_SIZE);

    let mut file = std::fs::File::create(path)
        .map_err(|e| TranscriberError::ModelDownload(e.to_string()))?;

    let mut reader = response.into_reader();
    let mut buf = [0u8; 65536];
    let mut downloaded: u64 = 0;

    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| TranscriberError::ModelDownload(e.to_string()))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])
            .map_err(|e| TranscriberError::ModelDownload(e.to_string()))?;
        downloaded += n as u64;
        progress_fn(downloaded, total);
    }

    file.flush()
        .map_err(|e| TranscriberError::ModelDownload(e.to_string()))?;

    Ok(())
}
