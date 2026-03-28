pub mod pane_widget;

pub use pane_widget::PaneWidget;

use crate::app::{Dialog, TextSelection};
use crate::pane::{Pane, PaneStatus};
use crate::voice::VoiceState;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

/// Which pane is currently focused.
#[derive(Clone, Copy, PartialEq)]
pub enum ActivePane {
    Orchestrator,
    Worker(usize),
}

/// Computed layout rectangles.
pub struct Layout {
    pub worker_rects: Vec<Rect>,
    pub orch_rect: Rect,
}

/// Compute layout given the total area and number of workers.
///
/// - 0 workers: orchestrator fills the entire area.
/// - 1+ workers: workers tile horizontally in the top ~80%, orchestrator occupies the bottom ~20%.
pub fn compute_layout(area: Rect, num_workers: usize) -> Layout {
    let orch_h = if num_workers == 0 {
        // No workers: orchestrator takes bottom ~40%, top area is empty
        (area.height * 40 / 100).max(8).min(area.height)
    } else {
        (area.height * 20 / 100).max(6).min(area.height)
    };

    if num_workers == 0 {
        let orch_rect = Rect::new(area.x, area.y + area.height.saturating_sub(orch_h), area.width, orch_h);
        return Layout {
            worker_rects: vec![],
            orch_rect,
        };
    }
    let worker_h = area.height.saturating_sub(orch_h);
    let orch_rect = Rect::new(area.x, area.y + worker_h, area.width, orch_h);

    let base_w = area.width / num_workers as u16;
    let worker_rects = (0..num_workers)
        .map(|i| {
            let x = area.x + (i as u16) * base_w;
            let width = if i == num_workers - 1 {
                area.width.saturating_sub((i as u16) * base_w)
            } else {
                base_w
            };
            Rect::new(x, area.y, width, worker_h)
        })
        .collect();

    Layout {
        worker_rects,
        orch_rect,
    }
}

/// Return the inner area of a bordered rect (1-cell border on each side).
pub fn inner_rect(rect: Rect) -> Rect {
    Rect::new(
        rect.x + 1,
        rect.y + 1,
        rect.width.saturating_sub(2),
        rect.height.saturating_sub(2),
    )
}

/// Render all panes and return (ActivePane, Rect) pairs for mouse-click detection.
/// If `fullscreen` is Some, render only that pane at the full area.
pub fn render(
    frame: &mut Frame,
    orchestrator: &Pane,
    worker_panes: &[&Pane],
    active: &ActivePane,
    fullscreen: Option<ActivePane>,
    frame_count: u64,
    voice_state: &VoiceState,
    correcting: bool,
    selection: &TextSelection,
) -> Vec<(ActivePane, Rect)> {
    // Fullscreen mode: render only the focused pane at full area
    if let Some(fs) = fullscreen {
        let area = frame.area();
        match fs {
            ActivePane::Orchestrator => {
                let title = voice_title("Orchestrator [fullscreen]", voice_state, correcting);
                let sel = if selection.active && selection.pane == ActivePane::Orchestrator { Some(selection) } else { None };
                render_pane(frame, orchestrator, area, true, &title, frame_count, sel, false);
                set_cursor_for_ime(frame, orchestrator, area);
            }
            ActivePane::Worker(idx) => {
                if let Some(pane) = worker_panes.get(idx) {
                    let sel = if selection.active && selection.pane == ActivePane::Worker(idx) { Some(selection) } else { None };
                    render_pane(
                        frame,
                        pane,
                        area,
                        true,
                        &format!("Pane {} [fullscreen]", idx + 1),
                        frame_count,
                        sel,
                        true,
                    );
                    set_cursor_for_ime(frame, pane, area);
                }
            }
        }
        return vec![(fs, area)];
    }

    // Normal multi-pane layout
    let layout = compute_layout(frame.area(), worker_panes.len());
    let mut rects = Vec::new();

    // Show hint when no workers exist
    if worker_panes.is_empty() {
        let area = frame.area();
        let hint_y = area.height.saturating_sub(layout.orch_rect.height) / 2;
        let hint = Paragraph::new("Press Ctrl+N to add a new pane")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(ratatui::layout::Alignment::Center);
        let hint_rect = Rect::new(area.x, hint_y, area.width, 1);
        frame.render_widget(hint, hint_rect);
    }

    for (i, pane) in worker_panes.iter().enumerate() {
        let rect = layout.worker_rects[i];
        let focused = *active == ActivePane::Worker(i);
        let sel = if selection.active && selection.pane == ActivePane::Worker(i) { Some(selection) } else { None };
        render_pane(frame, pane, rect, focused, &format!("Pane {}", i + 1), frame_count, sel, true);
        rects.push((ActivePane::Worker(i), rect));
    }

    let orch_focused = *active == ActivePane::Orchestrator;
    let orch_title = voice_title("Orchestrator", voice_state, correcting);
    let orch_sel = if selection.active && selection.pane == ActivePane::Orchestrator { Some(selection) } else { None };
    render_pane(
        frame,
        orchestrator,
        layout.orch_rect,
        orch_focused,
        &orch_title,
        frame_count,
        orch_sel,
        false,
    );
    rects.push((ActivePane::Orchestrator, layout.orch_rect));

    // Set hardware cursor position for IME input on the active pane
    match active {
        ActivePane::Orchestrator => {
            set_cursor_for_ime(frame, orchestrator, layout.orch_rect);
        }
        ActivePane::Worker(idx) => {
            if let Some(pane) = worker_panes.get(*idx) {
                if let Some(rect) = layout.worker_rects.get(*idx) {
                    set_cursor_for_ime(frame, pane, *rect);
                }
            }
        }
    }

    rects
}

/// Render a centered input overlay for cwd entry with optional suggestions.
pub fn render_cwd_input(frame: &mut Frame, input: &str, suggestions: &[String], selected_idx: usize) {
    use ratatui::text::{Line, Span};

    let area = frame.area();
    // Dialog width: ~78 chars (60 * 1.3), capped to terminal
    let width = 78u16.min(area.width.saturating_sub(4));
    let x = (area.width.saturating_sub(width)) / 2;
    let max_visible = 12usize;
    let sugg_lines = suggestions.len().min(max_visible) as u16;
    let height = 3 + sugg_lines;
    let y = (area.height / 3).min(area.height.saturating_sub(height));
    let rect = Rect::new(x, y, width, height);

    let mut lines = vec![
        Line::from(format!(" {}_", input)),
    ];
    for (i, s) in suggestions.iter().enumerate().take(max_visible) {
        let selected = i == selected_idx;
        let prefix = if selected { " > " } else { "   " };
        let style = if selected {
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        lines.push(Line::from(Span::styled(format!("{}{}", prefix, s), style)));
    }

    let title = format!("New Pane — {} match{}", suggestions.len(),
        if suggestions.len() == 1 { "" } else { "es" });
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(title);
    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(Clear, rect);
    frame.render_widget(paragraph, rect);

    // Set hardware cursor at input position
    let cursor_x = rect.x + 1 + 1 + input.len() as u16; // border + space + text
    let cursor_y = rect.y + 1;
    if cursor_x < rect.x + rect.width && cursor_y < rect.y + rect.height {
        frame.set_cursor_position(ratatui::layout::Position::new(cursor_x, cursor_y));
    }
}

/// Render STT confirmation dialog with editable text.
pub fn render_stt_confirm(frame: &mut Frame, text: &str, cursor: usize) {
    let area = frame.area();
    let width = 78u16.min(area.width.saturating_sub(4));
    let height = 4u16;
    let x = (area.width.saturating_sub(width)) / 2;
    let y = (area.height / 3).min(area.height.saturating_sub(height));
    let rect = Rect::new(x, y, width, height);

    // Build text with cursor indicator
    let before: String = text.chars().take(cursor).collect();
    let after: String = text.chars().skip(cursor).collect();
    let display = format!(" {}|{}", before, after);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Green))
        .title("Voice Input — Edit and press Enter to send, Esc to cancel");
    let paragraph = Paragraph::new(display).block(block);
    frame.render_widget(Clear, rect);
    frame.render_widget(paragraph, rect);

    // Set hardware cursor
    let cursor_x = rect.x + 1 + 1 + before.len() as u16;
    let cursor_y = rect.y + 1;
    if cursor_x < rect.x + rect.width && cursor_y < rect.y + rect.height {
        frame.set_cursor_position(ratatui::layout::Position::new(cursor_x, cursor_y));
    }
}

/// Render execution mode selection dialog (Claude vs Terminal).
pub fn render_mode_select(frame: &mut Frame, selected: usize) {
    use ratatui::text::{Line, Span};

    let area = frame.area();
    let width = 50u16.min(area.width.saturating_sub(4));
    let height = 6u16;
    let x = (area.width.saturating_sub(width)) / 2;
    let y = (area.height / 3).min(area.height.saturating_sub(height));
    let rect = Rect::new(x, y, width, height);

    let options = [
        ("Claude Code", "AI-powered coding assistant"),
        ("Terminal", "Plain shell (zsh/bash)"),
    ];

    let mut lines = Vec::new();
    for (i, (label, desc)) in options.iter().enumerate() {
        let prefix = if i == selected { " > " } else { "   " };
        let style = if i == selected {
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        lines.push(Line::from(Span::styled(format!("{}{}", prefix, label), style)));
        lines.push(Line::from(Span::styled(
            format!("     {}", desc),
            Style::default().fg(Color::DarkGray),
        )));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title("Pane Type — ↑↓ select, Enter confirm, Esc cancel");
    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(Clear, rect);
    frame.render_widget(paragraph, rect);
}

/// Render permission mode selection dialog.
pub fn render_perm_select(frame: &mut Frame, selected: usize) {
    use ratatui::text::{Line, Span};

    let area = frame.area();
    let width = 50u16.min(area.width.saturating_sub(4));
    let height = 6u16;
    let x = (area.width.saturating_sub(width)) / 2;
    let y = (area.height / 3).min(area.height.saturating_sub(height));
    let rect = Rect::new(x, y, width, height);

    let options = [
        ("Skip Permissions", "--dangerously-skip-permissions"),
        ("Normal", "Requires permission for each action"),
    ];

    let mut lines = Vec::new();
    for (i, (label, desc)) in options.iter().enumerate() {
        let prefix = if i == selected { " > " } else { "   " };
        let style = if i == selected {
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        lines.push(Line::from(Span::styled(format!("{}{}", prefix, label), style)));
        lines.push(Line::from(Span::styled(
            format!("     {}", desc),
            Style::default().fg(Color::DarkGray),
        )));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title("Permission Mode — ↑↓ select, Enter confirm, Esc cancel");
    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(Clear, rect);
    frame.render_widget(paragraph, rect);
}

/// Set the hardware cursor position for IME composition.
/// Position the cursor so the OS IME overlay (Korean, Japanese, etc.)
/// appears at the right place. We use set_cursor_position which also
/// makes the cursor visible — this doubles as Claude's text cursor.
fn set_cursor_for_ime(frame: &mut Frame, pane: &Pane, outer_rect: Rect) {
    let inner = inner_rect(outer_rect);
    let cursor_col = pane.grid.cursor.col;
    let cursor_row = pane.grid.cursor.row;
    let x = inner.x + cursor_col;
    let y = inner.y + cursor_row;
    // Only set if within bounds
    if x < inner.x + inner.width && y < inner.y + inner.height {
        frame.set_cursor_position(ratatui::layout::Position::new(x, y));
    }
}


/// Build orchestrator title with voice state suffix.
fn voice_title(base: &str, voice_state: &VoiceState, correcting: bool) -> String {
    if correcting {
        return format!("{} [CORRECTING...]", base);
    }
    match voice_state {
        VoiceState::Idle => format!("{} — Ctrl+R: voice", base),
        VoiceState::Recording => format!("{} [REC] Ctrl+R: stop", base),
        VoiceState::Downloading(dl, total) => {
            let pct = if *total > 0 { dl * 100 / total } else { 0 };
            format!("{} [DL: {}%]", base, pct)
        }
        VoiceState::Transcribing => format!("{} [STT...]", base),
        VoiceState::Error(msg) => {
            let short = if msg.len() > 30 { &msg[..30] } else { msg };
            format!("{} [ERR: {}]", base, short)
        }
    }
}

/// Detect if a pane needs permission or is waiting for input by scanning last rows.
fn detect_pane_alert(pane: &Pane) -> PaneAlert {
    if pane.status != PaneStatus::Running {
        return PaneAlert::None;
    }
    // Scan last 5 rows of visible grid for permission/input patterns
    let rows = pane.grid.rows;
    for r in rows.saturating_sub(5)..rows {
        if let Some(row) = pane.grid.view_row(r) {
            let line: String = row.iter().map(|c| c.ch).collect();
            let trimmed = line.trim();
            // Claude Code permission patterns
            if trimmed.contains("Allow") && (trimmed.contains("(y/n)") || trimmed.contains("Yes") || trimmed.contains("allow this")) {
                return PaneAlert::NeedsPermission;
            }
            if trimmed.contains("Do you want to proceed") || trimmed.contains("approve") {
                return PaneAlert::NeedsPermission;
            }
        }
    }
    PaneAlert::None
}

#[derive(PartialEq)]
enum PaneAlert {
    None,
    NeedsPermission,
    ReceivingPrompt, // for SendToWorker yellow blink
}

fn render_pane(frame: &mut Frame, pane: &Pane, area: Rect, focused: bool, title: &str, frame_count: u64, selection: Option<&TextSelection>, is_worker: bool) {
    let alert = detect_pane_alert(pane);
    let blink_on = (frame_count / 15) % 2 == 0; // ~0.5s blink at 60fps

    let border_color = if alert == PaneAlert::NeedsPermission && blink_on {
        Color::Red
    } else if alert == PaneAlert::ReceivingPrompt && blink_on {
        Color::Yellow
    } else if focused {
        Color::Cyan
    } else if pane.status != PaneStatus::Running {
        Color::DarkGray
    } else {
        Color::Gray
    };

    let title_str = if let PaneStatus::Exited(code) = &pane.status {
        format!("{} [exited: {}]", title, code)
    } else if alert == PaneAlert::NeedsPermission {
        format!("{} [NEEDS PERMISSION]", title)
    } else if pane.grid.is_receiving_prompt {
        format!("{} [receiving prompt]", title)
    } else {
        title.to_string()
    };

    let mut border_style = Style::default().fg(border_color);
    if alert == PaneAlert::NeedsPermission && blink_on {
        border_style = border_style.add_modifier(Modifier::BOLD);
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title_str);

    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(PaneWidget::new(&pane.grid, focused, selection), inner);

    // Render [X] close button on top-right for worker panes
    if is_worker && area.width > 10 {
        let x_pos = area.x + area.width - 4;
        let y_pos = area.y;
        let buf = frame.buffer_mut();
        let close_style = Style::default().fg(Color::Red).add_modifier(Modifier::BOLD);
        if let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(x_pos, y_pos)) {
            cell.set_char('['); cell.set_style(close_style);
        }
        if let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(x_pos + 1, y_pos)) {
            cell.set_char('X'); cell.set_style(close_style);
        }
        if let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(x_pos + 2, y_pos)) {
            cell.set_char(']'); cell.set_style(close_style);
        }
    }
}

/// Render a confirmation/input dialog overlay.
pub fn render_dialog(frame: &mut Frame, dialog: &Dialog, selected: usize) {
    use ratatui::text::{Line, Span};

    let area = frame.area();
    let width = 50u16.min(area.width.saturating_sub(4));
    let x = (area.width.saturating_sub(width)) / 2;

    match dialog {
        Dialog::ConfirmQuit => {
            let height = 6u16;
            let y = (area.height.saturating_sub(height)) / 2;
            let rect = Rect::new(x, y, width, height);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Red))
                .title("Quit CDC?");
            let yes_style = if selected == 0 {
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            let no_style = if selected == 1 {
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            let lines = vec![
                Line::from("All workers will be terminated."),
                Line::from(""),
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled(if selected == 0 { "> [Quit]" } else { "  [Quit]" }, yes_style),
                    Span::raw("   "),
                    Span::styled(if selected == 1 { "> [Cancel]" } else { "  [Cancel]" }, no_style),
                ]),
            ];
            let p = Paragraph::new(lines).block(block);
            frame.render_widget(Clear, rect);
            frame.render_widget(p, rect);
        }
        Dialog::ConfirmCloseWorker(idx) => {
            let height = 6u16;
            let y = (area.height.saturating_sub(height)) / 2;
            let rect = Rect::new(x, y, width, height);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow))
                .title(format!("Close Pane {}?", idx + 1));
            let yes_style = if selected == 0 {
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            let no_style = if selected == 1 {
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            let lines = vec![
                Line::from("Pane process will be killed."),
                Line::from(""),
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled(if selected == 0 { "> [Close]" } else { "  [Close]" }, yes_style),
                    Span::raw("   "),
                    Span::styled(if selected == 1 { "> [Cancel]" } else { "  [Cancel]" }, no_style),
                ]),
            ];
            let p = Paragraph::new(lines).block(block);
            frame.render_widget(Clear, rect);
            frame.render_widget(p, rect);
        }
        Dialog::SaveSession(input) => {
            let height = 4u16;
            let y = (area.height.saturating_sub(height)) / 2;
            let rect = Rect::new(x, y, width, height);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Green))
                .title("Save Session");
            let text = format!("Name (Enter=auto): {}_", input);
            let p = Paragraph::new(text).block(block);
            frame.render_widget(Clear, rect);
            frame.render_widget(p, rect);
        }
        Dialog::None => {}
    }
}
