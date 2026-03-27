use crate::app::TextSelection;
use crate::pane::grid::Cell;
use crate::pane::TerminalGrid;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::Modifier;
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthChar;

pub struct PaneWidget<'a> {
    grid: &'a TerminalGrid,
    selection: Option<&'a TextSelection>,
}

impl<'a> PaneWidget<'a> {
    pub fn new(grid: &'a TerminalGrid, _focused: bool, selection: Option<&'a TextSelection>) -> Self {
        Self { grid, selection }
    }

    fn is_selected(&self, row: u16, col: u16) -> bool {
        let Some(sel) = self.selection else { return false };
        if !sel.active { return false; }
        let (sc, sr, ec, er) = sel.normalized();
        if row < sr || row > er { return false; }
        if sr == er {
            // Single line selection
            col >= sc && col <= ec
        } else if row == sr {
            col >= sc
        } else if row == er {
            col <= ec
        } else {
            true // Middle rows fully selected
        }
    }
}

const DEFAULT_CELL: Cell = Cell {
    ch: ' ',
    fg: ratatui::style::Color::Reset,
    bg: ratatui::style::Color::Reset,
    bold: false,
    italic: false,
    underline: false,
    dim: false,
    strikethrough: false,
    reverse: false,
};

impl<'a> Widget for PaneWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let grid = self.grid;

        for row in 0..area.height {
            let mut col = 0u16;
            while col < area.width {
                let pos = Position::new(area.x + col, area.y + row);

                if let Some(buf_cell) = buf.cell_mut(pos) {
                    if let Some(view) = grid.view_row(row) {
                        let cell = view
                            .get(col as usize)
                            .unwrap_or(&DEFAULT_CELL);
                        let char_width = cell.ch.width();

                        if char_width == Some(0) {
                            col += 1;
                            continue;
                        }

                        buf_cell.set_char(cell.ch);
                        if self.is_selected(row, col) {
                            // Invert colors for selection highlight
                            buf_cell.bg = if cell.fg == ratatui::style::Color::Reset {
                                ratatui::style::Color::White
                            } else {
                                cell.fg
                            };
                            buf_cell.fg = if cell.bg == ratatui::style::Color::Reset {
                                ratatui::style::Color::Black
                            } else {
                                cell.bg
                            };
                        } else {
                            buf_cell.fg = cell.fg;
                            buf_cell.bg = cell.bg;
                        }
                        let mut m = Modifier::empty();
                        if cell.bold { m |= Modifier::BOLD; }
                        if cell.italic { m |= Modifier::ITALIC; }
                        if cell.underline { m |= Modifier::UNDERLINED; }
                        if cell.dim { m |= Modifier::DIM; }
                        if cell.strikethrough { m |= Modifier::CROSSED_OUT; }
                        if cell.reverse { m |= Modifier::REVERSED; }
                        buf_cell.modifier = m;

                        let w = char_width.unwrap_or(1) as u16;

                        // Mark continuation cell for wide characters
                        if w == 2 {
                            let next = Position::new(area.x + col + 1, area.y + row);
                            if let Some(nc) = buf.cell_mut(next) {
                                nc.reset();
                            }
                        }

                        col += w.max(1);
                    } else {
                        buf_cell.set_char(' ');
                        buf_cell.fg = ratatui::style::Color::Reset;
                        buf_cell.bg = ratatui::style::Color::Reset;
                        buf_cell.modifier = Modifier::empty();
                        col += 1;
                    }
                } else {
                    col += 1;
                }
            }
        }

        // No custom cursor — focus is indicated by the pane border color.
        // Claude Code manages its own cursor display via ANSI styling.
    }
}
