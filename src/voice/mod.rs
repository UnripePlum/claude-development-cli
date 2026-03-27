pub mod recorder;
pub mod transcriber;

use crossbeam_channel::{Receiver, Sender};
use recorder::AudioRecorder;
use transcriber::Transcriber;
use std::sync::Arc;
use std::time::Instant;

#[derive(Clone, Debug)]
pub enum VoiceState {
    Idle,
    Recording,
    Downloading(u64, u64),
    Transcribing,
    Error(String),
}

pub enum VoiceEvent {
    StateChanged(VoiceState),
    Transcribed(String),
    DownloadProgress(u64, u64),
    Error(String),
}

pub struct VoiceManager {
    state: VoiceState,
    recorder: Option<AudioRecorder>,
    transcriber: Option<Arc<Transcriber>>,
    tx: Sender<VoiceEvent>,
    recording_start: Option<Instant>,
}

const MAX_RECORDING_SECS: u64 = 300; // 5 minutes

impl VoiceManager {
    pub fn new() -> (Self, Receiver<VoiceEvent>) {
        let (tx, rx) = crossbeam_channel::bounded(256);
        let mgr = Self {
            state: VoiceState::Idle,
            recorder: None,
            transcriber: None,
            tx,
            recording_start: None,
        };
        (mgr, rx)
    }

    pub fn state(&self) -> &VoiceState {
        &self.state
    }

    pub fn toggle(&mut self) {
        match &self.state {
            VoiceState::Idle => self.start_recording(),
            VoiceState::Recording => self.stop_and_transcribe(),
            VoiceState::Downloading(_, _) | VoiceState::Transcribing => {
                // Ignore toggle during download/transcription
            }
            VoiceState::Error(_) => {
                // Clear error and try again
                self.start_recording();
            }
        }
    }

    /// Check if recording has exceeded max duration; auto-stop if so.
    pub fn check_timeout(&mut self) {
        if let VoiceState::Recording = &self.state {
            if let Some(start) = self.recording_start {
                if start.elapsed().as_secs() >= MAX_RECORDING_SECS {
                    self.stop_and_transcribe();
                }
            }
        }
    }

    fn start_recording(&mut self) {
        let mut rec = AudioRecorder::new();
        match rec.start() {
            Ok(()) => {
                self.recorder = Some(rec);
                self.state = VoiceState::Recording;
                self.recording_start = Some(Instant::now());
                let _ = self.tx.send(VoiceEvent::StateChanged(VoiceState::Recording));
            }
            Err(e) => {
                let msg = format!("{}", e);
                self.state = VoiceState::Error(msg.clone());
                let _ = self.tx.send(VoiceEvent::Error(msg));
            }
        }
    }

    fn stop_and_transcribe(&mut self) {
        self.recording_start = None;
        let samples = if let Some(mut rec) = self.recorder.take() {
            match rec.stop() {
                Ok(s) => s,
                Err(e) => {
                    let msg = format!("{}", e);
                    self.state = VoiceState::Error(msg.clone());
                    let _ = self.tx.send(VoiceEvent::Error(msg));
                    return;
                }
            }
        } else {
            return;
        };

        if samples.is_empty() {
            self.state = VoiceState::Idle;
            let _ = self.tx.send(VoiceEvent::StateChanged(VoiceState::Idle));
            return;
        }

        // Clone or initialize transcriber
        let tx = self.tx.clone();
        let transcriber = if let Some(ref t) = self.transcriber {
            Arc::clone(t)
        } else {
            // Need to initialize — this happens on the background thread
            self.state = VoiceState::Downloading(0, 0);
            let _ = self.tx.send(VoiceEvent::StateChanged(VoiceState::Downloading(0, 0)));

            let tx_init = self.tx.clone();

            // Spawn thread for model loading (potentially with download)
            let (init_tx, init_rx) = crossbeam_channel::bounded(1);
            std::thread::spawn(move || {
                let progress = move |dl: u64, total: u64| {
                    let _ = tx_init.send(VoiceEvent::DownloadProgress(dl, total));
                };
                match Transcriber::new(progress) {
                    Ok(t) => { let _ = init_tx.send(Ok(Arc::new(t))); }
                    Err(e) => { let _ = init_tx.send(Err(e)); }
                }
            });

            // We can't block here — schedule transcription after init completes
            let tx2 = tx.clone();
            std::thread::spawn(move || {
                match init_rx.recv() {
                    Ok(Ok(arc)) => {
                        let _ = tx2.send(VoiceEvent::StateChanged(VoiceState::Transcribing));
                        match arc.transcribe(&samples) {
                            Ok(text) => {
                                let _ = tx2.send(VoiceEvent::Transcribed(text));
                            }
                            Err(e) => {
                                let _ = tx2.send(VoiceEvent::Error(format!("{}", e)));
                            }
                        }
                    }
                    Ok(Err(e)) => {
                        let _ = tx2.send(VoiceEvent::Error(format!("{}", e)));
                    }
                    Err(_) => {
                        let _ = tx2.send(VoiceEvent::Error("Transcriber init failed".into()));
                    }
                }
            });

            self.state = VoiceState::Transcribing;
            return;
        };

        // Transcriber already loaded — spawn transcription thread directly
        self.state = VoiceState::Transcribing;
        let _ = self.tx.send(VoiceEvent::StateChanged(VoiceState::Transcribing));

        std::thread::spawn(move || {
            match transcriber.transcribe(&samples) {
                Ok(text) => {
                    let _ = tx.send(VoiceEvent::Transcribed(text));
                }
                Err(e) => {
                    let _ = tx.send(VoiceEvent::Error(format!("{}", e)));
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_state_is_idle() {
        let (mgr, _rx) = VoiceManager::new();
        assert!(matches!(mgr.state(), VoiceState::Idle));
    }

    #[test]
    fn test_toggle_while_transcribing_is_ignored() {
        let (mut mgr, _rx) = VoiceManager::new();
        mgr.state = VoiceState::Transcribing;
        mgr.toggle(); // should not panic
        assert!(matches!(mgr.state(), VoiceState::Transcribing));
    }

    #[test]
    fn test_toggle_while_downloading_is_ignored() {
        let (mut mgr, _rx) = VoiceManager::new();
        mgr.state = VoiceState::Downloading(50, 100);
        mgr.toggle(); // should not panic
        assert!(matches!(mgr.state(), VoiceState::Downloading(_, _)));
    }
}
