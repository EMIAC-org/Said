//! Local desktop state machine.
//!
//! Owns the AudioRecorder and tracks whether we are idle/recording/processing.
//! All API calls (gateway, backend) have been moved to `api.rs`; this module
//! is deliberately thin so the Mutex lock time stays near-zero.

use std::time::Instant;

use said_core::{AppSnapshot, ProcessSummary, all_modes};
use said_paster::is_accessibility_granted;
use said_recorder::{AudioRecorder, ChunkReceiver, LevelReceiver, MIN_DURATION_S, StopReceiver};

#[cfg(target_os = "macos")]
use said_hotkey::is_input_monitoring_granted;

use crate::permissions;

// ── State machine ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
pub enum AppState {
    Idle,
    Recording,
    Processing,
}

impl AppState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Recording => "recording",
            Self::Processing => "processing",
        }
    }
}

pub const RECORDING_TOO_SHORT_ERROR: &str = "recording too short";

// ── DesktopApp ────────────────────────────────────────────────────────────────

pub struct DesktopApp {
    pub state: AppState,
    pub recorder: AudioRecorder,
    pub last_result: Option<ProcessSummary>,
    pub last_error: Option<String>,
    pub recording_started: Option<Instant>,
}

impl DesktopApp {
    pub fn new() -> Self {
        Self {
            state: AppState::Idle,
            recorder: AudioRecorder::new(),
            last_result: None,
            last_error: None,
            recording_started: None,
        }
    }

    /// Build a snapshot for the frontend.
    /// History, total_words, daily_streak come from backend SQLite.
    /// Mode is always "mini" — model switching has been removed.
    pub fn snapshot(&self) -> AppSnapshot {
        AppSnapshot {
            state: self.state.as_str().to_string(),
            platform: std::env::consts::OS.to_string(),
            current_mode: "mini",
            current_mode_label: "Fast (gpt-5.4-mini)",
            current_model: "gpt-5.4-mini",
            auto_paste_supported: cfg!(target_os = "macos"),
            accessibility_granted: is_accessibility_granted(),
            microphone_granted: permissions::microphone_granted(),
            #[cfg(target_os = "macos")]
            input_monitoring_granted: is_input_monitoring_granted(),
            #[cfg(not(target_os = "macos"))]
            input_monitoring_granted: false,
            modes: all_modes().to_vec(),
            last_result: self.last_result.clone(),
            last_error: self.last_error.clone(),
            history: vec![],
            total_words: 0,
            daily_streak: 0,
            avg_wpm: 0,
        }
    }

    /// Take the live audio chunk receiver for Deepgram WS streaming (P5).
    /// Returns `None` if the recorder hasn't started yet or the receiver was already taken.
    pub fn take_chunk_receiver(&mut self) -> Option<ChunkReceiver> {
        self.recorder.take_chunk_receiver()
    }

    pub fn take_level_receiver(&mut self) -> Option<LevelReceiver> {
        self.recorder.take_level_receiver()
    }

    /// Begin recording. Returns the snapshot for the UI.
    pub fn start_recording(&mut self) -> Result<AppSnapshot, String> {
        if self.state == AppState::Processing {
            return Err("still processing previous recording".into());
        }
        self.recorder.start()?;
        self.state = AppState::Recording;
        self.last_error = None;
        self.recording_started = Some(Instant::now());
        Ok(self.snapshot())
    }

    /// Stop the recorder and return the raw WAV bytes.
    /// Sets state → Processing so the UI shows a spinner.
    /// The caller is responsible for the async API call and then
    /// calling `finish_ok` or `finish_err`.
    pub fn stop_and_extract(&mut self) -> Result<Vec<u8>, String> {
        if self.state != AppState::Recording {
            return Err("not recording".into());
        }
        let was_too_short = self
            .recording_started
            .map(|started| started.elapsed().as_secs_f32() < MIN_DURATION_S)
            .unwrap_or(false);
        self.state = AppState::Processing;
        match self.recorder.stop() {
            Some(wav) => Ok(wav),
            None if was_too_short => Err(RECORDING_TOO_SHORT_ERROR.to_string()),
            None => Err("no audio captured".to_string()),
        }
    }

    /// Initiate recorder stop without waiting for the CoreAudio thread to return
    /// samples or for WAV encoding to finish. The caller must collect the WAV
    /// outside the shared app mutex.
    pub fn begin_stop(&mut self) -> Result<(StopReceiver, bool), String> {
        if self.state != AppState::Recording {
            return Err("not recording".into());
        }
        let was_too_short = self
            .recording_started
            .map(|started| started.elapsed().as_secs_f32() < MIN_DURATION_S)
            .unwrap_or(false);
        match self.recorder.initiate_stop() {
            Some(rx) => {
                self.state = AppState::Processing;
                Ok((rx, was_too_short))
            }
            None if was_too_short => Err(RECORDING_TOO_SHORT_ERROR.to_string()),
            None => Err("no audio captured".to_string()),
        }
    }

    pub fn finish_stop(stop_rx: StopReceiver, was_too_short: bool) -> Result<Vec<u8>, String> {
        match AudioRecorder::collect_wav(stop_rx) {
            Some(wav) => Ok(wav),
            None if was_too_short => Err(RECORDING_TOO_SHORT_ERROR.to_string()),
            None => Err("no audio captured".to_string()),
        }
    }

    pub fn finish_cancelled(&mut self) -> AppSnapshot {
        self.state = AppState::Idle;
        self.last_error = None;
        self.recording_started = None;
        self.snapshot()
    }

    pub fn finish_ok(&mut self, result: ProcessSummary) -> AppSnapshot {
        self.state = AppState::Idle;
        self.last_result = Some(result);
        self.last_error = None;
        self.recording_started = None;
        self.snapshot()
    }

    pub fn finish_err(&mut self, err: String) -> AppSnapshot {
        self.state = AppState::Idle;
        self.last_error = Some(err);
        self.recording_started = None;
        self.snapshot()
    }
}
