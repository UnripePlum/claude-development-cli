use crate::event::{encode_key, encode_mouse};
use crate::pane::{Pane, PaneStatus};
use crate::pty::{PtyEvent, PtyManager};
use crate::ui::{self, ActivePane};

use crossbeam_channel::Receiver;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers, MouseButton,
    MouseEvent, MouseEventKind,
};
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;
use std::time::Duration;

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
            pty,
            pty_rx: rx,
        })
    }

    fn drain_output(&mut self) {
        while let Ok(event) = self.pty_rx.try_recv() {
            match event {
                PtyEvent::Output(data) => self.pane.process_bytes(&data),
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
}

fn term_rect(terminal: &Terminal<CrosstermBackend<std::io::Stdout>>) -> Rect {
    let s = terminal.size().unwrap_or_default();
    Rect::new(0, 0, s.width, s.height)
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
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
    let mut pane_rects: Vec<(ActivePane, Rect)> = Vec::new();
    let mut fullscreen: Option<ActivePane> = None;
    let mut cwd_input: Option<String> = None; // Some = entering cwd for new worker
    let mut cwd_suggestions: Vec<String> = Vec::new();

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

        // 3. Draw
        {
            let worker_panes: Vec<&Pane> = workers.iter().map(|w| &w.pane).collect();
            let active_copy = active;
            let rects_out = &mut pane_rects;
            let cwd_ref = &cwd_input;
            let sugg_ref = &cwd_suggestions;
            terminal.draw(|frame| {
                *rects_out = ui::render(frame, &orchestrator.pane, &worker_panes, &active_copy, fullscreen);
                if let Some(input) = cwd_ref {
                    ui::render_cwd_input(frame, input, sugg_ref);
                }
            })?;
        }

        // 4. Poll crossterm events (16ms timeout ≈ 60fps)
        //    Drain ALL pending events per frame to avoid IME input lag
        //    (e.g. Korean composition + space arrive as two rapid events)
        let mut should_quit = false;
        if crossterm::event::poll(Duration::from_millis(16))? {
            loop {
                let event = crossterm::event::read()?;
                match event {
                    Event::Key(key) => {
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
                                }
                                KeyCode::Tab => {
                                    let matches = smart_complete(input);
                                    if matches.len() == 1 {
                                        *input = matches[0].clone();
                                        cwd_suggestions.clear();
                                    } else if matches.len() > 1 {
                                        let common = common_prefix(&matches);
                                        if common.len() > input.len() {
                                            *input = common;
                                        }
                                        cwd_suggestions = matches;
                                    } else {
                                        cwd_suggestions.clear();
                                    }
                                }
                                KeyCode::Backspace => {
                                    input.pop();
                                    cwd_suggestions = smart_complete(input);
                                }
                                KeyCode::Char(c) => {
                                    input.push(c);
                                    cwd_suggestions = smart_complete(input);
                                }
                                _ => {}
                            }
                        } else {

                        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

                        if ctrl {
                            match key.code {
                                KeyCode::Char('q') => {
                                    should_quit = true;
                                    break;
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
                                KeyCode::Char('w') => {
                                    // Close focused worker
                                    if let ActivePane::Worker(idx) = active {
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
                                            fullscreen = None; // exit fullscreen on layout change
                                            resize_all_panes(
                                                &mut orchestrator,
                                                &mut workers,
                                                term_rect(&terminal),
                                                None,
                                            );
                                        }
                                    }
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
                        // Click → switch focus
                        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                            for (ap, rect) in &pane_rects {
                                if mouse.column >= rect.x
                                    && mouse.column < rect.x + rect.width
                                    && mouse.row >= rect.y
                                    && mouse.row < rect.y + rect.height
                                {
                                    if *ap != active {
                                        switch_focus(active, *ap, &mut orchestrator, &mut workers);
                                        active = *ap;
                                    }
                                    break;
                                }
                            }
                        }
                        // Forward mouse with coordinates relative to inner area
                        if let Some((_, rect)) =
                            pane_rects.iter().find(|(ap, _)| *ap == active)
                        {
                            let inner = ui::inner_rect(*rect);
                            if mouse.column >= inner.x
                                && mouse.column < inner.x + inner.width
                                && mouse.row >= inner.y
                                && mouse.row < inner.y + inner.height
                            {
                                let adjusted = MouseEvent {
                                    kind: mouse.kind,
                                    column: mouse.column - inner.x,
                                    row: mouse.row - inner.y,
                                    modifiers: mouse.modifiers,
                                };
                                let mp = active_pane_mut(
                                    &mut orchestrator,
                                    &mut workers,
                                    &active,
                                );
                                if mp.pane.status == PaneStatus::Running {
                                    let bytes = encode_mouse(adjusted);
                                    if !bytes.is_empty() {
                                        let _ = mp.pty.write(&bytes);
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
