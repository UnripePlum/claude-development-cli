pub mod pane_widget;

pub use pane_widget::PaneWidget;

use crate::pane::{Pane, PaneStatus};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders};
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
    if num_workers == 0 {
        return Layout {
            worker_rects: vec![],
            orch_rect: area,
        };
    }

    let orch_h = (area.height * 20 / 100).max(6).min(area.height);
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
) -> Vec<(ActivePane, Rect)> {
    // Fullscreen mode: render only the focused pane at full area
    if let Some(fs) = fullscreen {
        let area = frame.area();
        match fs {
            ActivePane::Orchestrator => {
                render_pane(frame, orchestrator, area, true, "Orchestrator [fullscreen]");
            }
            ActivePane::Worker(idx) => {
                if let Some(pane) = worker_panes.get(idx) {
                    render_pane(
                        frame,
                        pane,
                        area,
                        true,
                        &format!("Worker {} [fullscreen]", idx + 1),
                    );
                }
            }
        }
        return vec![(fs, area)];
    }

    // Normal multi-pane layout
    let layout = compute_layout(frame.area(), worker_panes.len());
    let mut rects = Vec::new();

    for (i, pane) in worker_panes.iter().enumerate() {
        let rect = layout.worker_rects[i];
        let focused = *active == ActivePane::Worker(i);
        render_pane(frame, pane, rect, focused, &format!("Worker {}", i + 1));
        rects.push((ActivePane::Worker(i), rect));
    }

    let orch_focused = *active == ActivePane::Orchestrator;
    render_pane(
        frame,
        orchestrator,
        layout.orch_rect,
        orch_focused,
        "Orchestrator",
    );
    rects.push((ActivePane::Orchestrator, layout.orch_rect));

    rects
}

/// Render a centered input overlay for cwd entry with optional suggestions.
pub fn render_cwd_input(frame: &mut Frame, input: &str, suggestions: &[String]) {
    use ratatui::widgets::Paragraph;

    let area = frame.area();
    let width = 60u16.min(area.width.saturating_sub(4));
    let x = (area.width.saturating_sub(width)) / 2;
    let y = area.height / 2;
    let sugg_lines = suggestions.len() as u16;
    let height = 3 + sugg_lines; // input box + suggestion lines
    let rect = Rect::new(x, y, width, height);

    // Build display text
    let mut text = format!("cwd (Tab=complete, empty=inherit): {}_", input);
    for s in suggestions {
        text.push_str(&format!("\n  {}", s));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .title("New Worker");
    let paragraph = Paragraph::new(text).block(block);
    frame.render_widget(ratatui::widgets::Clear, rect);
    frame.render_widget(paragraph, rect);
}

fn render_pane(frame: &mut Frame, pane: &Pane, area: Rect, focused: bool, title: &str) {
    let border_color = if focused {
        Color::Cyan
    } else if pane.status != PaneStatus::Running {
        Color::DarkGray
    } else {
        Color::Gray
    };

    let title_str = if let PaneStatus::Exited(code) = &pane.status {
        format!("{} [exited: {}]", title, code)
    } else {
        title.to_string()
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(title_str);

    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(PaneWidget::new(&pane.grid, focused), inner);
}
