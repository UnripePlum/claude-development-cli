use crate::event::{encode_key, encode_mouse};
use crate::pane::{Pane, PaneStatus};
use crate::pty::{PtyEvent, PtyManager};
use crate::ui::{self, ActivePane};
use crate::voice::{VoiceEvent, VoiceManager, VoiceState};

use crossbeam_channel::Receiver;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers, MouseButton,
    MouseEvent, MouseEventKind,
};
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;
use std::time::{Duration, Instant};

/// Text selection state for mouse drag.
#[derive(Clone)]
pub struct TextSelection {
    pub pane: ActivePane,
    pub start_col: u16,
    pub start_row: u16,
    pub end_col: u16,
    pub end_row: u16,
    pub active: bool,
}

impl TextSelection {
    fn none() -> Self {
        Self { pane: ActivePane::Orchestrator, start_col: 0, start_row: 0, end_col: 0, end_row: 0, active: false }
    }

    /// Get normalized (top-left, bottom-right) range.
    pub fn normalized(&self) -> (u16, u16, u16, u16) {
        if self.start_row < self.end_row || (self.start_row == self.end_row && self.start_col <= self.end_col) {
            (self.start_col, self.start_row, self.end_col, self.end_row)
        } else {
            (self.end_col, self.end_row, self.start_col, self.start_row)
        }
    }
}

/// Voice correction state machine.
enum CorrectionState {
    Idle,
    WaitingForResponse {
        raw_text: String,
        hard_ceiling: Instant,
    },
}

fn correction_timeout_ms() -> u64 {
    std::env::var("CDC_CORRECTION_TIMEOUT_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5000)
}

fn correction_quiescence_ms() -> u64 {
    std::env::var("CDC_CORRECTION_QUIESCENCE_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(500)
}

fn format_correction_prompt(raw: &str) -> String {
    format!(
        "다음 음성 명령의 오타를 보정해서 [CDC_CORRECT] 태그 안에 결과만 출력해: {}\n예시: [CDC_CORRECT]보정된 텍스트[/CDC_CORRECT]",
        raw
    )
}

/// Active confirmation dialog.
#[derive(Clone, PartialEq)]
pub enum Dialog {
    None,
    ConfirmQuit,
    ConfirmCloseWorker(usize),
    SaveSession(String), // input buffer for session name
}

/// RAII guard that restores the terminal on any exit path (panic, error, normal).
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(
            std::io::stdout(),
            DisableMouseCapture,
            LeaveAlternateScreen
        );
    }
}

/// A pane bundled with its PTY and event receiver.
pub struct ManagedPane {
    pub pane: Pane,
    pub cwd: Option<String>,
    pub last_output_time: Option<Instant>,
    pty: PtyManager,
    pty_rx: Receiver<PtyEvent>,
}

impl ManagedPane {
    fn spawn(
        id: u32,
        name: String,
        cmd: &str,
        cols: u16,
        rows: u16,
        cwd: Option<&str>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let (tx, rx) = crossbeam_channel::bounded(4096);
        let pane = Pane::new(id, name, cols, rows);
        let pty = PtyManager::spawn_with_cwd(cmd, &[], cols, rows, tx, cwd)?;
        Ok(Self {
            pane,
            cwd: cwd.map(|s| s.to_string()),
            last_output_time: None,
            pty,
            pty_rx: rx,
        })
    }

    fn drain_output(&mut self) {
        while let Ok(event) = self.pty_rx.try_recv() {
            match event {
                PtyEvent::Output(data) => {
                    self.pane.process_bytes(&data);
                    self.last_output_time = Some(Instant::now());
                }
                PtyEvent::Exited => {
                    let code = self.pty.try_wait_exit_code().unwrap_or(0);
                    self.pane.status = PaneStatus::Exited(code);
                }
            }
        }
    }

    fn flush_responses(&mut self) {
        if !self.pane.grid.response_buf.is_empty() {
            let responses: Vec<u8> = self.pane.grid.response_buf.drain(..).collect();
            let _ = self.pty.write(&responses);
        }
    }

    pub fn write_to_pty(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.pty.write(data)
    }
}

fn term_rect(terminal: &Terminal<CrosstermBackend<std::io::Stdout>>) -> Rect {
    let s = terminal.size().unwrap_or_default();
    Rect::new(0, 0, s.width, s.height)
}

pub fn run(restore_session: Option<crate::session::Session>) -> Result<(), Box<dyn std::error::Error>> {
    // Install panic hook BEFORE entering raw mode
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(
            std::io::stdout(),
            DisableMouseCapture,
            LeaveAlternateScreen
        );
        original_hook(info);
    }));

    // Setup terminal
    crossterm::terminal::enable_raw_mode()?;
    let _guard = TerminalGuard;

    let mut stdout = std::io::stdout();
    crossterm::execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    // Initial state
    let cmd = std::env::var("CDC_CMD").unwrap_or_else(|_| "claude".to_string());

    // Orchestrator starts full-screen (0 workers → orch_rect = full area)
    let initial_layout = ui::compute_layout(term_rect(&terminal), 0);
    let inner = ui::inner_rect(initial_layout.orch_rect);
    let mut orchestrator =
        ManagedPane::spawn(0, "orchestrator".into(), &cmd, inner.width, inner.height, None)?;

    let mut workers: Vec<ManagedPane> = Vec::new();
    let mut active = ActivePane::Orchestrator;
    let mut next_id = 1u32;

    // Restore session: spawn workers from saved session
    if let Some(ref session) = restore_session {
        for wi in &session.workers {
            let new_count = workers.len() + 1;
            let layout = ui::compute_layout(term_rect(&terminal), new_count);
            if let Some(rect) = layout.worker_rects.last() {
                let inner = ui::inner_rect(*rect);
                if let Ok(mp) = ManagedPane::spawn(
                    next_id,
                    wi.name.clone(),
                    &cmd,
                    inner.width,
                    inner.height,
                    wi.cwd.as_deref(),
                ) {
                    workers.push(mp);
                    next_id += 1;
                }
            }
        }
        if !workers.is_empty() {
            resize_all_panes(&mut orchestrator, &mut workers, term_rect(&terminal), None);
        }
    }
    let mut pane_rects: Vec<(ActivePane, Rect)> = Vec::new();
    let mut fullscreen: Option<ActivePane> = None;
    let mut cwd_input: Option<String> = None; // Some = entering cwd for new worker
    let mut cwd_suggestions: Vec<String> = Vec::new();
    let mut cwd_suggestion_idx: usize = 0;
    let mut selection = TextSelection::none();
    let mut dialog = Dialog::None;
    let mut frame_count: u64 = 0;
    let mut _loop_start = Instant::now();

    // Voice input
    let (mut voice_mgr, voice_rx) = VoiceManager::new();
    let mut voice_state = VoiceState::Idle;
    let mut correction_state = CorrectionState::Idle;

    // Main event loop
    loop {
        // 1. Drain all PTY outputs
        orchestrator.drain_output();
        for w in &mut workers {
            w.drain_output();
        }

        // 2. Flush terminal query responses back to PTYs
        orchestrator.flush_responses();
        for w in &mut workers {
            w.flush_responses();
        }

        // 2b. Drain voice events
        while let Ok(event) = voice_rx.try_recv() {
            match event {
                VoiceEvent::Transcribed(text) => {
                    // Cancel any pending correction (fallback raw)
                    if let CorrectionState::WaitingForResponse { raw_text, .. } = &correction_state {
                        route_text(raw_text, &mut orchestrator, &mut workers);
                    }
                    // Start correction via orchestrator Claude
                    // Skip if orchestrator is blocked on permission prompt
                    let orch_blocked = ui::is_pane_blocked(&orchestrator.pane);
                    if orchestrator.pane.status == PaneStatus::Running && !orch_blocked {
                        let prompt = format_correction_prompt(&text);
                        let _ = orchestrator.write_to_pty(format!("{}\n", prompt).as_bytes());
                        correction_state = CorrectionState::WaitingForResponse {
                            raw_text: text,
                            hard_ceiling: Instant::now() + Duration::from_millis(correction_timeout_ms()),
                        };
                    } else {
                        // Orchestrator not running — skip correction, route raw
                        route_text(&text, &mut orchestrator, &mut workers);
                    }
                    voice_state = VoiceState::Idle;
                }
                VoiceEvent::StateChanged(state) => {
                    voice_state = state;
                }
                VoiceEvent::DownloadProgress(dl, total) => {
                    voice_state = VoiceState::Downloading(dl, total);
                }
                VoiceEvent::Error(msg) => {
                    voice_state = VoiceState::Error(msg);
                }
            }
        }

        // 2c. Check voice recording timeout
        voice_mgr.check_timeout();

        // 2d. Check correction state (quiescence or hard ceiling)
        if let CorrectionState::WaitingForResponse { ref raw_text, hard_ceiling } = correction_state {
            let quiescent = orchestrator.last_output_time
                .map_or(false, |t| t.elapsed() > Duration::from_millis(correction_quiescence_ms()));
            let ceiling_hit = Instant::now() >= hard_ceiling;

            if quiescent || ceiling_hit {
                // Extract corrected text: sentinels → heuristic → raw fallback
                let corrected = orchestrator.pane.grid
                    .extract_between_markers("[CDC_CORRECT]", "[/CDC_CORRECT]")
                    .or_else(|| {
                        // Heuristic: try to find a line in grid that differs from the prompt
                        let all = orchestrator.pane.grid.extract_all_text();
                        heuristic_extract_correction(&all, raw_text)
                    })
                    .unwrap_or_else(|| raw_text.clone());

                route_text(&corrected, &mut orchestrator, &mut workers);
                correction_state = CorrectionState::Idle;
            }
        }

        // 3. Draw
        {
            let worker_panes: Vec<&Pane> = workers.iter().map(|w| &w.pane).collect();
            let active_copy = active;
            let rects_out = &mut pane_rects;
            let cwd_ref = &cwd_input;
            let sugg_ref = &cwd_suggestions;
            let dialog_ref = &dialog;
            let fc = frame_count;
            let vs = &voice_state;
            let correcting = matches!(correction_state, CorrectionState::WaitingForResponse { .. });
            let sel_ref = &selection;
            terminal.draw(|frame| {
                *rects_out = ui::render(frame, &orchestrator.pane, &worker_panes, &active_copy, fullscreen, fc, vs, correcting, sel_ref);
                if let Some(input) = cwd_ref {
                    ui::render_cwd_input(frame, input, sugg_ref);
                }
                if *dialog_ref != Dialog::None {
                    ui::render_dialog(frame, dialog_ref);
                }
            })?;
        }
        frame_count = frame_count.wrapping_add(1);

        // 4. Poll crossterm events (16ms timeout ≈ 60fps)
        //    Drain ALL pending events per frame to avoid IME input lag
        //    (e.g. Korean composition + space arrive as two rapid events)
        let mut should_quit = false;
        if crossterm::event::poll(Duration::from_millis(16))? {
            loop {
                let event = crossterm::event::read()?;
                match event {
                    Event::Key(key) => {
                        // Dialog mode: intercept Y/N/Esc
                        if dialog != Dialog::None {
                            match &dialog {
                                Dialog::ConfirmQuit => match key.code {
                                    KeyCode::Char('y') | KeyCode::Char('Y') => {
                                        should_quit = true;
                                        break;
                                    }
                                    KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                                        dialog = Dialog::None;
                                    }
                                    _ => {}
                                },
                                Dialog::ConfirmCloseWorker(idx) => {
                                    let idx = *idx;
                                    match key.code {
                                        KeyCode::Char('y') | KeyCode::Char('Y') => {
                                            if idx < workers.len() {
                                                let mut removed = workers.remove(idx);
                                                let _ = removed.pty.kill();
                                                let new = if workers.is_empty() {
                                                    ActivePane::Orchestrator
                                                } else {
                                                    ActivePane::Worker(idx.min(workers.len() - 1))
                                                };
                                                switch_focus(active, new, &mut orchestrator, &mut workers);
                                                active = new;
                                                fullscreen = None;
                                                resize_all_panes(
                                                    &mut orchestrator,
                                                    &mut workers,
                                                    term_rect(&terminal),
                                                    None,
                                                );
                                            }
                                            dialog = Dialog::None;
                                        }
                                        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                                            dialog = Dialog::None;
                                        }
                                        _ => {}
                                    }
                                }
                                Dialog::SaveSession(_input) => {
                                    // Handled below in session section
                                    match key.code {
                                        KeyCode::Enter => {
                                            if let Dialog::SaveSession(ref name) = dialog {
                                                let session_name = if name.is_empty() {
                                                    chrono::Local::now().format("session-%Y%m%d-%H%M%S").to_string()
                                                } else {
                                                    name.clone()
                                                };
                                                let worker_infos: Vec<_> = workers.iter().map(|w| {
                                                    crate::session::WorkerInfo {
                                                        name: w.pane.name.clone(),
                                                        cwd: w.cwd.clone(),
                                                    }
                                                }).collect();
                                                let session = crate::session::Session {
                                                    name: session_name,
                                                    workers: worker_infos,
                                                    created_at: chrono::Local::now().to_rfc3339(),
                                                };
                                                let _ = crate::session::save_session(&session);
                                            }
                                            dialog = Dialog::None;
                                        }
                                        KeyCode::Esc => {
                                            dialog = Dialog::None;
                                        }
                                        KeyCode::Backspace => {
                                            if let Dialog::SaveSession(ref mut name) = dialog {
                                                name.pop();
                                            }
                                        }
                                        KeyCode::Char(c) => {
                                            if let Dialog::SaveSession(ref mut name) = dialog {
                                                name.push(c);
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                                Dialog::None => unreachable!(),
                            }
                        } else
                        // CWD input mode: capture keys for directory path
                        if let Some(ref mut input) = cwd_input {
                            match key.code {
                                KeyCode::Enter => {
                                    let cwd_str = input.clone();
                                    let cwd = if cwd_str.is_empty() {
                                        None
                                    } else {
                                        // Resolve ~ and relative paths
                                        let expanded = if cwd_str.starts_with('~') {
                                            if let Some(home) = std::env::var("HOME").ok() {
                                                cwd_str.replacen('~', &home, 1)
                                            } else {
                                                cwd_str.clone()
                                            }
                                        } else {
                                            cwd_str.clone()
                                        };
                                        let path = std::path::Path::new(&expanded);
                                        let resolved = if path.is_relative() {
                                            std::env::current_dir()
                                                .map(|cwd| cwd.join(path))
                                                .unwrap_or_else(|_| path.to_path_buf())
                                        } else {
                                            path.to_path_buf()
                                        };
                                        Some(resolved.to_string_lossy().to_string())
                                    };
                                    cwd_input = None;
                                    cwd_suggestions.clear();
                                    cwd_suggestion_idx = 0;
                                    // Create worker with the entered cwd
                                    let new_count = workers.len() + 1;
                                    let layout =
                                        ui::compute_layout(term_rect(&terminal), new_count);
                                    if let Some(rect) = layout.worker_rects.last() {
                                        let wi = ui::inner_rect(*rect);
                                        if let Ok(mp) = ManagedPane::spawn(
                                            next_id,
                                            format!("worker-{}", next_id),
                                            &cmd,
                                            wi.width,
                                            wi.height,
                                            cwd.as_deref(),
                                        ) {
                                            workers.push(mp);
                                            let new_ap =
                                                ActivePane::Worker(workers.len() - 1);
                                            switch_focus(
                                                active, new_ap, &mut orchestrator,
                                                &mut workers,
                                            );
                                            active = new_ap;
                                            next_id += 1;
                                            fullscreen = None;
                                            resize_all_panes(
                                                &mut orchestrator,
                                                &mut workers,
                                                term_rect(&terminal),
                                                None,
                                            );
                                        }
                                    }
                                }
                                KeyCode::Esc => {
                                    cwd_input = None;
                                    cwd_suggestions.clear();
                                    cwd_suggestion_idx = 0;
                                }
                                KeyCode::Tab => {
                                    if !cwd_suggestions.is_empty() {
                                        // Cycle to next suggestion
                                        cwd_suggestion_idx = (cwd_suggestion_idx + 1) % cwd_suggestions.len();
                                        *input = cwd_suggestions[cwd_suggestion_idx].clone();
                                    } else {
                                        // First Tab: generate suggestions
                                        let matches = smart_complete(input);
                                        if matches.len() == 1 {
                                            *input = matches[0].clone();
                                        } else if !matches.is_empty() {
                                            cwd_suggestion_idx = 0;
                                            *input = matches[0].clone();
                                            cwd_suggestions = matches;
                                        }
                                    }
                                }
                                KeyCode::BackTab => {
                                    // Shift+Tab: cycle backwards
                                    if !cwd_suggestions.is_empty() {
                                        cwd_suggestion_idx = if cwd_suggestion_idx == 0 {
                                            cwd_suggestions.len() - 1
                                        } else {
                                            cwd_suggestion_idx - 1
                                        };
                                        *input = cwd_suggestions[cwd_suggestion_idx].clone();
                                    }
                                }
                                KeyCode::Backspace => {
                                    input.pop();
                                    cwd_suggestions.clear();
                                    cwd_suggestion_idx = 0;
                                }
                                KeyCode::Char(c) => {
                                    input.push(c);
                                    cwd_suggestions.clear();
                                    cwd_suggestion_idx = 0;
                                }
                                _ => {}
                            }
                        } else {

                        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

                        if ctrl {
                            match key.code {
                                KeyCode::Char('q') => {
                                    dialog = Dialog::ConfirmQuit;
                                }
                                KeyCode::Char('z') => {
                                    // Toggle fullscreen
                                    if fullscreen.is_some() {
                                        fullscreen = None;
                                    } else {
                                        fullscreen = Some(active);
                                    }
                                    // Resize the pane to match new area
                                    resize_all_panes(
                                        &mut orchestrator,
                                        &mut workers,
                                        term_rect(&terminal),
                                        fullscreen,
                                    );
                                }
                                KeyCode::Char('o') => {
                                    let new = ActivePane::Orchestrator;
                                    switch_focus(active, new, &mut orchestrator, &mut workers);
                                    active = new;
                                }
                                KeyCode::Char('n') => {
                                    // Enter cwd input mode for new worker
                                    cwd_input = Some(String::new());
                                }
                                KeyCode::Char('d') => {
                                    // Debug: dump grid state to file
                                    dump_grid_debug(&orchestrator, &workers, &active);
                                }
                                KeyCode::Char('w') => {
                                    // Close focused worker (with confirmation)
                                    if let ActivePane::Worker(idx) = active {
                                        if idx < workers.len() {
                                            dialog = Dialog::ConfirmCloseWorker(idx);
                                        }
                                    }
                                }
                                KeyCode::Char('r') => {
                                    // Voice input: toggle recording
                                    // Note: Ctrl+R no longer forwards to child PTY (was reverse-search in bash)
                                    voice_mgr.toggle();
                                }
                                KeyCode::Char('s') => {
                                    // Save session
                                    dialog = Dialog::SaveSession(String::new());
                                }
                                KeyCode::Char(c @ '1'..='9') => {
                                    let idx = (c as usize) - ('1' as usize);
                                    if idx < workers.len() {
                                        let new = ActivePane::Worker(idx);
                                        switch_focus(active, new, &mut orchestrator, &mut workers);
                                        active = new;
                                    }
                                }
                                _ => {
                                    // Forward unhandled Ctrl+key to active pane
                                    let mp = active_pane_mut(
                                        &mut orchestrator,
                                        &mut workers,
                                        &active,
                                    );
                                    if mp.pane.status == PaneStatus::Running {
                                        let bytes = encode_key(key);
                                        if !bytes.is_empty() {
                                            let _ = mp.pty.write(&bytes);
                                        }
                                    }
                                }
                            }
                        } else if key.modifiers.contains(KeyModifiers::SHIFT) {
                            match key.code {
                                KeyCode::PageUp => {
                                    let mp = active_pane_mut(
                                        &mut orchestrator,
                                        &mut workers,
                                        &active,
                                    );
                                    let half = (mp.pane.grid.rows / 2).max(1) as usize;
                                    mp.pane.grid.scroll_view_up(half);
                                }
                                KeyCode::PageDown => {
                                    let mp = active_pane_mut(
                                        &mut orchestrator,
                                        &mut workers,
                                        &active,
                                    );
                                    let half = (mp.pane.grid.rows / 2).max(1) as usize;
                                    mp.pane.grid.scroll_view_down(half);
                                }
                                _ => {
                                    // Forward other Shift+key to active pane
                                    let mp = active_pane_mut(
                                        &mut orchestrator,
                                        &mut workers,
                                        &active,
                                    );
                                    if mp.pane.status == PaneStatus::Running {
                                        let bytes = encode_key(key);
                                        if !bytes.is_empty() {
                                            let _ = mp.pty.write(&bytes);
                                        }
                                    }
                                }
                            }
                        } else {
                            // Forward key to the active pane
                            let mp =
                                active_pane_mut(&mut orchestrator, &mut workers, &active);
                            if mp.pane.status == PaneStatus::Running {
                                let bytes = encode_key(key);
                                if !bytes.is_empty() {
                                    let _ = mp.pty.write(&bytes);
                                }
                            }
                        }
                    } // else (not cwd_input)
                    }
                    Event::Mouse(mouse) => {
                        match mouse.kind {
                            MouseEventKind::Down(MouseButton::Left) => {
                                // Record click start — find which pane
                                selection = TextSelection::none();
                                for (ap, rect) in &pane_rects {
                                    let inner = ui::inner_rect(*rect);
                                    if mouse.column >= inner.x && mouse.column < inner.x + inner.width
                                        && mouse.row >= inner.y && mouse.row < inner.y + inner.height
                                    {
                                        let col = mouse.column - inner.x;
                                        let row = mouse.row - inner.y;
                                        selection = TextSelection {
                                            pane: *ap,
                                            start_col: col, start_row: row,
                                            end_col: col, end_row: row,
                                            active: false, // not yet a drag
                                        };
                                        // Switch focus
                                        if *ap != active {
                                            switch_focus(active, *ap, &mut orchestrator, &mut workers);
                                            active = *ap;
                                        }
                                        break;
                                    }
                                }
                            }
                            MouseEventKind::Drag(MouseButton::Left) => {
                                // Extend selection, clamped to starting pane
                                if let Some((_, rect)) = pane_rects.iter().find(|(ap, _)| *ap == selection.pane) {
                                    let inner = ui::inner_rect(*rect);
                                    selection.end_col = mouse.column.saturating_sub(inner.x).min(inner.width.saturating_sub(1));
                                    selection.end_row = mouse.row.saturating_sub(inner.y).min(inner.height.saturating_sub(1));
                                    selection.active = true; // now it's a real selection
                                }
                            }
                            MouseEventKind::Up(MouseButton::Left) => {
                                if selection.active {
                                    // Finalize selection → copy to clipboard
                                    let mp = match selection.pane {
                                        ActivePane::Orchestrator => &orchestrator,
                                        ActivePane::Worker(idx) => workers.get(idx).unwrap_or(&orchestrator),
                                    };
                                    let text = extract_selection(&mp.pane.grid, &selection);
                                    if !text.is_empty() {
                                        copy_to_clipboard(&text);
                                    }
                                    // Keep selection visible until next click
                                } else {
                                    // Was a click (no drag) — forward to PTY
                                    if let Some((_, rect)) = pane_rects.iter().find(|(ap, _)| *ap == active) {
                                        let inner = ui::inner_rect(*rect);
                                        if mouse.column >= inner.x && mouse.column < inner.x + inner.width
                                            && mouse.row >= inner.y && mouse.row < inner.y + inner.height
                                        {
                                            // Forward both down and up events for click
                                            let col = mouse.column - inner.x;
                                            let row = mouse.row - inner.y;
                                            let mp = active_pane_mut(&mut orchestrator, &mut workers, &active);
                                            if mp.pane.status == PaneStatus::Running {
                                                let down = MouseEvent {
                                                    kind: MouseEventKind::Down(MouseButton::Left),
                                                    column: col, row, modifiers: mouse.modifiers,
                                                };
                                                let up = MouseEvent {
                                                    kind: MouseEventKind::Up(MouseButton::Left),
                                                    column: col, row, modifiers: mouse.modifiers,
                                                };
                                                let bytes = encode_mouse(down);
                                                if !bytes.is_empty() { let _ = mp.pty.write(&bytes); }
                                                let bytes = encode_mouse(up);
                                                if !bytes.is_empty() { let _ = mp.pty.write(&bytes); }
                                            }
                                        }
                                    }
                                }
                            }
                            _ => {
                                // Only forward scroll events to PTY — ignore moved/other
                                if matches!(mouse.kind, MouseEventKind::ScrollUp | MouseEventKind::ScrollDown) {
                                    if let Some((_, rect)) = pane_rects.iter().find(|(ap, _)| *ap == active) {
                                        let inner = ui::inner_rect(*rect);
                                        if mouse.column >= inner.x && mouse.column < inner.x + inner.width
                                            && mouse.row >= inner.y && mouse.row < inner.y + inner.height
                                        {
                                            let adjusted = MouseEvent {
                                                kind: mouse.kind,
                                                column: mouse.column - inner.x,
                                                row: mouse.row - inner.y,
                                                modifiers: mouse.modifiers,
                                            };
                                            let mp = active_pane_mut(&mut orchestrator, &mut workers, &active);
                                            if mp.pane.status == PaneStatus::Running {
                                                let bytes = encode_mouse(adjusted);
                                                if !bytes.is_empty() { let _ = mp.pty.write(&bytes); }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Event::Resize(w, h) => {
                        resize_all_panes(
                            &mut orchestrator,
                            &mut workers,
                            Rect::new(0, 0, w, h),
                            fullscreen,
                        );
                        let _ = terminal.clear();
                    }
                    Event::FocusGained => {
                        // Forward focus-in to active pane if it enabled tracking
                        let mp =
                            active_pane_mut(&mut orchestrator, &mut workers, &active);
                        if mp.pane.grid.focus_tracking {
                            let _ = mp.pty.write(b"\x1b[I");
                        }
                    }
                    Event::FocusLost => {
                        // Forward focus-out to active pane if it enabled tracking
                        let mp =
                            active_pane_mut(&mut orchestrator, &mut workers, &active);
                        if mp.pane.grid.focus_tracking {
                            let _ = mp.pty.write(b"\x1b[O");
                        }
                    }
                    _ => {}
                }

                // Drain remaining events without waiting
                if !crossterm::event::poll(Duration::from_millis(0))? {
                    break;
                }
            }
        }
        if should_quit {
            break;
        }
    }

    // Cleanup
    orchestrator.pty.kill()?;
    for w in &mut workers {
        let _ = w.pty.kill();
    }
    Ok(())
}

/// Send focus-in/out events (\x1b[I / \x1b[O) to PTYs when active pane changes.
/// Claude Code enables focus event tracking (\x1b[?1004h) and may ignore input
/// until it receives a focus-in event.
fn switch_focus(
    old: ActivePane,
    new: ActivePane,
    orchestrator: &mut ManagedPane,
    workers: &mut [ManagedPane],
) {
    if old == new {
        return;
    }
    // Send focus-out to old pane (only if it enabled focus tracking)
    let old_mp = match old {
        ActivePane::Orchestrator => Some(&mut *orchestrator),
        ActivePane::Worker(idx) => workers.get_mut(idx),
    };
    if let Some(mp) = old_mp {
        if mp.pane.grid.focus_tracking {
            let _ = mp.pty.write(b"\x1b[O");
        }
    }
    // Send focus-in to new pane (only if it enabled focus tracking)
    let new_mp = match new {
        ActivePane::Orchestrator => Some(&mut *orchestrator),
        ActivePane::Worker(idx) => workers.get_mut(idx),
    };
    if let Some(mp) = new_mp {
        if mp.pane.grid.focus_tracking {
            let _ = mp.pty.write(b"\x1b[I");
        }
    }
}

fn active_pane_mut<'a>(
    orchestrator: &'a mut ManagedPane,
    workers: &'a mut [ManagedPane],
    active: &ActivePane,
) -> &'a mut ManagedPane {
    match active {
        ActivePane::Orchestrator => orchestrator,
        ActivePane::Worker(idx) => &mut workers[*idx],
    }
}

fn resize_all_panes(
    orchestrator: &mut ManagedPane,
    workers: &mut [ManagedPane],
    total: Rect,
    fullscreen: Option<ActivePane>,
) {
    // In fullscreen mode, give the fullscreen pane the entire area
    if let Some(fs) = fullscreen {
        let fi = ui::inner_rect(total);
        match fs {
            ActivePane::Orchestrator => {
                orchestrator.pane.resize(fi.width, fi.height);
                let _ = orchestrator.pty.resize(fi.width, fi.height);
            }
            ActivePane::Worker(idx) => {
                if let Some(w) = workers.get_mut(idx) {
                    w.pane.resize(fi.width, fi.height);
                    let _ = w.pty.resize(fi.width, fi.height);
                }
            }
        }
        return;
    }

    let layout = ui::compute_layout(total, workers.len());

    // Resize orchestrator
    let oi = ui::inner_rect(layout.orch_rect);
    orchestrator.pane.resize(oi.width, oi.height);
    let _ = orchestrator.pty.resize(oi.width, oi.height);

    // Resize each worker
    for (i, w) in workers.iter_mut().enumerate() {
        if let Some(rect) = layout.worker_rects.get(i) {
            let wi = ui::inner_rect(*rect);
            w.pane.resize(wi.width, wi.height);
            let _ = w.pty.resize(wi.width, wi.height);
        }
    }
}

/// Tab-complete a directory path. Returns matching directory names.
fn complete_path(input: &str) -> Vec<String> {
    use std::path::Path;

    let home = std::env::var("HOME").unwrap_or_default();
    let expanded = if input.starts_with('~') {
        input.replacen('~', &home, 1)
    } else if input.is_empty() {
        ".".to_string()
    } else {
        input.to_string()
    };

    let path = Path::new(&expanded);

    let (dir, prefix) = if (input.ends_with('/') || input.is_empty()) && path.is_dir() {
        (path.to_path_buf(), String::new())
    } else {
        let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let prefix = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        (dir, prefix)
    };

    // Build the base path in expanded form (without the prefix part)
    let expanded_base = if expanded.ends_with('/') || input.is_empty() {
        expanded.clone()
    } else {
        let dir_str = dir.to_string_lossy();
        if dir_str == "." {
            String::new()
        } else {
            format!("{}/", dir_str)
        }
    };

    let mut matches = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            if let Ok(ft) = entry.file_type() {
                if ft.is_dir() {
                    if let Some(name) = entry.file_name().to_str().map(|s| s.to_string()) {
                        // Skip hidden dirs unless user typed a dot
                        if name.starts_with('.') && !prefix.starts_with('.') {
                            continue;
                        }
                        if prefix.is_empty() || name.starts_with(&prefix) {
                            let mut completed = format!("{}{}/", expanded_base, name);
                            // Convert back to ~ form if applicable
                            if !home.is_empty() && completed.starts_with(&home) {
                                completed = completed.replacen(&home, "~", 1);
                            }
                            matches.push(completed);
                        }
                    }
                }
            }
        }
    }
    matches.sort();
    matches
}

/// Smart complete: try path completion first, then fuzzy search as fallback.
fn smart_complete(input: &str) -> Vec<String> {
    if input.is_empty() {
        return Vec::new();
    }
    // If it looks like an explicit path, try path completion first
    if input.starts_with('~') || input.starts_with('/') || input.starts_with('.') {
        let results: Vec<String> = complete_path(input).into_iter().take(8).collect();
        if !results.is_empty() {
            return results;
        }
    }
    // Fuzzy search: each segment separated by / must appear in the path (in order)
    let segments: Vec<String> = input
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_lowercase())
        .collect();
    if segments.is_empty() {
        return Vec::new();
    }
    let home = std::env::var("HOME").unwrap_or_default();
    if home.is_empty() {
        return Vec::new();
    }
    let mut results = Vec::new();
    search_dirs_segments(&home, &segments, 3, &home, &mut results);
    // Also search current directory
    if let Ok(cwd) = std::env::current_dir() {
        let cwd_str = cwd.to_string_lossy().to_string();
        if !cwd_str.starts_with(&home) || cwd_str == home {
            search_dirs_segments(&cwd_str, &segments, 2, &home, &mut results);
        }
    }
    results.sort();
    results.dedup();
    results.truncate(8);
    results
}

/// Search directories where the relative path matches all segments in order.
/// e.g. segments=["projects","hi"] matches "~/projects/hihangul/"
fn search_dirs_segments(
    dir: &str,
    segments: &[String],
    depth: usize,
    home: &str,
    results: &mut Vec<String>,
) {
    if depth == 0 || results.len() >= 8 || segments.is_empty() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if results.len() >= 8 {
            break;
        }
        let Ok(ft) = entry.file_type() else { continue };
        if !ft.is_dir() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(|s| s.to_string()) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        let name_lower = name.to_lowercase();
        let path = entry.path();
        let path_str = path.to_string_lossy().to_string();

        // Check if this directory name matches the first segment
        if name_lower.contains(&segments[0]) {
            if segments.len() == 1 {
                // All segments matched — add this directory
                let mut display = path_str.clone();
                if !home.is_empty() && display.starts_with(home) {
                    display = display.replacen(home, "~", 1);
                }
                results.push(format!("{}/", display));
            } else {
                // Match remaining segments deeper
                search_dirs_segments(
                    &path_str,
                    &segments[1..],
                    depth - 1,
                    home,
                    results,
                );
            }
        }
        // Also recurse with all segments (directory name didn't match first segment)
        search_dirs_segments(&path_str, segments, depth - 1, home, results);
    }
}

/// Dump grid contents to /tmp/cdc_grid_debug.txt for debugging.
fn dump_grid_debug(orchestrator: &ManagedPane, workers: &[ManagedPane], active: &ui::ActivePane) {
    use std::io::Write;
    let Ok(mut f) = std::fs::File::create("/tmp/cdc_grid_debug.txt") else { return };
    let mp = match active {
        ui::ActivePane::Orchestrator => orchestrator,
        ui::ActivePane::Worker(idx) => {
            if let Some(w) = workers.get(*idx) { w } else { return }
        }
    };
    let grid = &mp.pane.grid;
    let _ = writeln!(f, "Grid: {}x{} cursor=({},{}) scroll_offset={} scroll_top={} scroll_bottom={}",
        grid.cols, grid.rows, grid.cursor.row, grid.cursor.col,
        grid.scroll_offset, grid.scroll_top(), grid.scroll_bottom());
    let _ = writeln!(f, "Scrollback lines: {}", grid.scrollback.len());
    let _ = writeln!(f, "\n=== Grid contents (row: text) ===");
    for r in 0..grid.rows {
        let row = &grid.cells[r as usize];
        let text: String = row.iter().map(|c| c.ch).collect();
        let trimmed = text.trim_end();
        if !trimmed.is_empty() {
            let _ = writeln!(f, "[{:03}] {}", r, trimmed);
        }
    }
    let _ = writeln!(f, "\n=== Last 20 scrollback lines ===");
    let sb_len = grid.scrollback.len();
    let start = sb_len.saturating_sub(20);
    for (i, row) in grid.scrollback.iter().skip(start).enumerate() {
        let text: String = row.iter().map(|c| c.ch).collect();
        let trimmed = text.trim_end();
        if !trimmed.is_empty() {
            let _ = writeln!(f, "[sb {:03}] {}", start + i, trimmed);
        }
    }
    let _ = writeln!(f, "\nDump complete.");
}

/// Extract selected text from grid.
fn extract_selection(grid: &crate::pane::TerminalGrid, sel: &TextSelection) -> String {
    let (sc, sr, ec, er) = sel.normalized();
    let mut lines = Vec::new();
    for r in sr..=er {
        if let Some(row) = grid.view_row(r) {
            let col_start = if r == sr { sc as usize } else { 0 };
            let col_end = if r == er { (ec as usize) + 1 } else { row.len() };
            let line: String = row[col_start..col_end.min(row.len())]
                .iter()
                .filter(|c| c.ch != '\0')
                .map(|c| c.ch)
                .collect();
            lines.push(line.trim_end().to_string());
        }
    }
    // Remove trailing empty lines
    while lines.last().map_or(false, |l| l.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

/// Copy text to system clipboard (macOS: pbcopy).
fn copy_to_clipboard(text: &str) {
    use std::io::Write;
    if let Ok(mut child) = std::process::Command::new("pbcopy")
        .stdin(std::process::Stdio::piped())
        .spawn()
    {
        if let Some(ref mut stdin) = child.stdin {
            let _ = stdin.write_all(text.as_bytes());
        }
        let _ = child.wait();
    }
}

/// Heuristic extraction: find a line in grid output that looks like a corrected version.
/// Skips lines that match the prompt or are clearly UI chrome.
fn heuristic_extract_correction(grid_text: &str, raw_text: &str) -> Option<String> {
    let prompt_fragment = raw_text.trim();
    for line in grid_text.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() { continue; }
        if trimmed.len() < 3 { continue; }
        // Skip lines that are the raw text itself or the prompt
        if trimmed == prompt_fragment { continue; }
        if trimmed.contains("음성 명령") && trimmed.contains("보정") { continue; }
        if trimmed.contains("[CDC_CORRECT]") { continue; }
        // Skip typical Claude UI chrome
        if trimmed.starts_with('>') || trimmed.starts_with('❯') { continue; }
        if trimmed.starts_with("```") { continue; }
        // This line looks like a correction response
        return Some(trimmed.to_string());
    }
    None
}

/// Route text to worker (if pattern matches) or orchestrator.
fn route_text(text: &str, orchestrator: &mut ManagedPane, workers: &mut [ManagedPane]) {
    if let Some((worker_idx, content)) = parse_worker_route(text) {
        if let Some(w) = workers.get_mut(worker_idx) {
            if w.pane.status == PaneStatus::Running {
                let _ = w.write_to_pty(format!("{}\n", content).as_bytes());
            }
        }
    } else {
        let _ = orchestrator.write_to_pty(format!("{}\n", text).as_bytes());
    }
}

/// Parse voice command for worker routing.
/// Matches patterns like "워커 1에게 ...", "worker 2에 ...", "워커 1번에 ..."
/// Returns (worker_index_0based, content_to_send) or None.
fn parse_worker_route(text: &str) -> Option<(usize, String)> {
    let t = text.trim();
    let lower = t.to_lowercase();

    // Patterns: "워커 N에게 ...", "워커 N번에 ...", "worker N에 ..."
    let prefixes = ["워커 ", "워커", "worker ", "worker"];
    let mut rest = None;
    for p in &prefixes {
        if let Some(r) = lower.strip_prefix(p) {
            // Use original text positioning
            rest = Some(&t[p.len()..]);
            let _ = r; // use lowercase match but original text
            break;
        }
    }
    let rest = rest?.trim();

    // Extract number: "1에게 ...", "2번에 ...", "1 ..."
    let num_end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
    if num_end == 0 {
        // Try Korean number words
        let korean_nums = [
            ("일", 1), ("이", 2), ("삼", 3), ("사", 4), ("오", 5),
            ("육", 6), ("칠", 7), ("팔", 8), ("구", 9),
            ("하나", 1), ("둘", 2), ("셋", 3), ("넷", 4), ("다섯", 5),
        ];
        for (word, num) in &korean_nums {
            if rest.starts_with(word) {
                let after = rest[word.len()..].trim();
                // Skip particles: 에게, 에, 번에, 번에게, 한테
                let content = strip_particle(after);
                if !content.is_empty() {
                    return Some(((*num - 1) as usize, content.to_string()));
                }
            }
        }
        return None;
    }

    let num: usize = rest[..num_end].parse().ok()?;
    if num == 0 { return None; }
    let after = rest[num_end..].trim();
    let content = strip_particle(after);
    if content.is_empty() { return None; }

    Some((num - 1, content.to_string()))
}

/// Strip Korean particles from the beginning: 에게, 에, 번에게, 번에, 한테, 에다가
fn strip_particle(s: &str) -> &str {
    let particles = ["번에게 ", "번에게", "번에 ", "번에", "에게 ", "에게", "한테 ", "한테", "에다가 ", "에다가", "에 ", "에"];
    for p in &particles {
        if let Some(rest) = s.strip_prefix(p) {
            return rest.trim();
        }
    }
    s.trim()
}

/// Find the longest common prefix among strings.
fn common_prefix(strings: &[String]) -> String {
    if strings.is_empty() {
        return String::new();
    }
    let first = &strings[0];
    let mut len = first.len();
    for s in &strings[1..] {
        len = len.min(s.len());
        for (i, (a, b)) in first.bytes().zip(s.bytes()).enumerate() {
            if a != b {
                len = len.min(i);
                break;
            }
        }
    }
    first[..len].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_correction_prompt() {
        let prompt = format_correction_prompt("워커 일에게 십을 넘겨");
        assert!(prompt.contains("[CDC_CORRECT]"));
        assert!(prompt.contains("워커 일에게 십을 넘겨"));
        assert!(prompt.contains("보정"));
    }

    #[test]
    fn test_parse_worker_route_korean_number() {
        let result = parse_worker_route("워커 1에게 테스트 해");
        assert_eq!(result, Some((0, "테스트 해".to_string())));
    }

    #[test]
    fn test_parse_worker_route_digit() {
        let result = parse_worker_route("워커 2번에 빌드 실행");
        assert_eq!(result, Some((1, "빌드 실행".to_string())));
    }

    #[test]
    fn test_parse_worker_route_no_match() {
        let result = parse_worker_route("빌드해줘");
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_worker_route_korean_word_number() {
        let result = parse_worker_route("워커 일에게 코드 작성해");
        assert_eq!(result, Some((0, "코드 작성해".to_string())));
    }

    #[test]
    fn test_heuristic_extract_correction() {
        let grid = "❯ 다음 음성 명령의 보정\n워커 1에게 10을 넘겨\n❯";
        let result = heuristic_extract_correction(grid, "워커 일에게 십을 넘겨");
        assert_eq!(result, Some("워커 1에게 10을 넘겨".to_string()));
    }

    #[test]
    fn test_heuristic_extract_no_match() {
        let grid = "❯ prompt text\n❯";
        let result = heuristic_extract_correction(grid, "워커 일에게 십을 넘겨");
        assert_eq!(result, None);
    }
}
