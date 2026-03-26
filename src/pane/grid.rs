use ratatui::style::Color;
use std::collections::VecDeque;
use unicode_width::UnicodeWidthChar;

#[derive(Clone, Copy)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub dim: bool,
    pub strikethrough: bool,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            fg: Color::Reset,
            bg: Color::Reset,
            bold: false,
            italic: false,
            underline: false,
            dim: false,
            strikethrough: false,
        }
    }
}

#[derive(Clone, Copy, Default)]
struct CellAttr {
    fg: Color,
    bg: Color,
    bold: bool,
    italic: bool,
    underline: bool,
    dim: bool,
    strikethrough: bool,
}

pub struct CursorPos {
    pub row: u16,
    pub col: u16,
}

pub struct TerminalGrid {
    pub cells: Vec<Vec<Cell>>,
    pub cursor: CursorPos,
    pub cols: u16,
    pub rows: u16,
    pub application_cursor_keys: bool,
    pub cursor_visible: bool,
    pub focus_tracking: bool,
    pub response_buf: Vec<u8>,
    pub scrollback: VecDeque<Vec<Cell>>,
    pub scroll_offset: usize,
    current_attr: CellAttr,
    scroll_top: u16,
    scroll_bottom: u16,
    saved_cursor: Option<(u16, u16)>,
    last_char: Option<char>,
    max_scrollback: usize,
}

impl TerminalGrid {
    pub fn new(cols: u16, rows: u16) -> Self {
        let cells = vec![vec![Cell::default(); cols as usize]; rows as usize];
        Self {
            cells,
            cursor: CursorPos { row: 0, col: 0 },
            cols,
            rows,
            application_cursor_keys: false,
            cursor_visible: true,
            focus_tracking: false,
            response_buf: Vec::new(),
            scrollback: VecDeque::new(),
            scroll_offset: 0,
            max_scrollback: 10000,
            current_attr: CellAttr {
                fg: Color::Reset,
                bg: Color::Reset,
                bold: false,
                italic: false,
                underline: false,
                dim: false,
                strikethrough: false,
            },
            scroll_top: 0,
            scroll_bottom: rows.saturating_sub(1),
            saved_cursor: None,
            last_char: None,
        }
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        let mut new_cells = vec![vec![Cell::default(); cols as usize]; rows as usize];
        let copy_rows = self.rows.min(rows) as usize;
        let copy_cols = self.cols.min(cols) as usize;
        for (r, new_row) in new_cells.iter_mut().enumerate().take(copy_rows) {
            new_row[..copy_cols].copy_from_slice(&self.cells[r][..copy_cols]);
        }
        self.cells = new_cells;
        self.cols = cols;
        self.rows = rows;
        self.scroll_bottom = rows.saturating_sub(1);
        self.cursor.row = self.cursor.row.min(rows.saturating_sub(1));
        self.cursor.col = self.cursor.col.min(cols.saturating_sub(1));
    }

    pub fn cell_at(&self, row: u16, col: u16) -> &Cell {
        &self.cells[row as usize][col as usize]
    }

    /// Get a row for rendering, accounting for scroll_offset.
    /// When scroll_offset > 0, earlier rows come from scrollback.
    pub fn view_row(&self, view_row: u16) -> Option<&[Cell]> {
        if self.scroll_offset == 0 {
            return self.cells.get(view_row as usize).map(|r| r.as_slice());
        }
        let sb_len = self.scrollback.len();
        let start = sb_len.saturating_sub(self.scroll_offset);
        let abs = start + view_row as usize;
        if abs < sb_len {
            Some(&self.scrollback[abs])
        } else {
            let r = abs - sb_len;
            self.cells.get(r).map(|row| row.as_slice())
        }
    }

    pub fn scroll_view_up(&mut self, lines: usize) {
        let max = self.scrollback.len();
        self.scroll_offset = (self.scroll_offset + lines).min(max);
    }

    pub fn scroll_view_down(&mut self, lines: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(lines);
    }

    fn write_char(&mut self, c: char) {
        self.scroll_offset = 0;
        let width = c.width().unwrap_or(1) as u16;

        // If the character doesn't fit on this line, wrap
        if self.cursor.col + width > self.cols {
            self.cursor.col = 0;
            self.scroll_up_if_needed();
        }

        let r = self.cursor.row as usize;
        let c_idx = self.cursor.col as usize;
        if r < self.cells.len() && c_idx < self.cells[r].len() {
            self.cells[r][c_idx] = Cell {
                ch: c,
                fg: self.current_attr.fg,
                bg: self.current_attr.bg,
                bold: self.current_attr.bold,
                italic: self.current_attr.italic,
                underline: self.current_attr.underline,
                dim: self.current_attr.dim,
                strikethrough: self.current_attr.strikethrough,
            };
            // Fill the second cell of a wide character with a placeholder space
            if width == 2 {
                let next = c_idx + 1;
                if next < self.cells[r].len() {
                    self.cells[r][next] = Cell {
                        ch: ' ',
                        fg: self.current_attr.fg,
                        bg: self.current_attr.bg,
                        ..Cell::default()
                    };
                }
            }
        }
        self.cursor.col += width;
        self.last_char = Some(c);
    }

    fn scroll_up_if_needed(&mut self) {
        if self.cursor.row == self.scroll_bottom {
            // Cursor is at the bottom of the scroll region → scroll up
            self.scroll_up();
        } else if self.cursor.row >= self.rows.saturating_sub(1) {
            // Cursor is at the very bottom of the screen → do nothing
        } else {
            self.cursor.row += 1;
        }
    }

    fn scroll_up(&mut self) {
        let top = self.scroll_top as usize;
        let bottom = self.scroll_bottom as usize;
        if top < self.cells.len() && bottom < self.cells.len() && top < bottom {
            // Save the top line to scrollback (only when scrolling the full screen region)
            if top == 0 {
                self.scrollback.push_back(self.cells[0].clone());
                if self.scrollback.len() > self.max_scrollback {
                    self.scrollback.pop_front();
                }
            }
            for r in top..bottom {
                self.cells[r] = self.cells[r + 1].clone();
            }
            self.cells[bottom] = vec![Cell::default(); self.cols as usize];
        }
        self.scroll_offset = 0;
    }

    fn scroll_down(&mut self) {
        let top = self.scroll_top as usize;
        let bottom = self.scroll_bottom as usize;
        if top < self.cells.len() && bottom < self.cells.len() && top < bottom {
            for r in (top + 1..=bottom).rev() {
                self.cells[r] = self.cells[r - 1].clone();
            }
            self.cells[top] = vec![Cell::default(); self.cols as usize];
        }
    }

    fn erase_in_display(&mut self, mode: u16) {
        match mode {
            0 => {
                // Erase below (from cursor to end)
                let r = self.cursor.row as usize;
                let c = self.cursor.col as usize;
                if r < self.cells.len() {
                    for i in c..self.cols as usize {
                        if i < self.cells[r].len() {
                            self.cells[r][i] = Cell::default();
                        }
                    }
                    for row in (r + 1)..self.rows as usize {
                        if row < self.cells.len() {
                            self.cells[row] = vec![Cell::default(); self.cols as usize];
                        }
                    }
                }
            }
            1 => {
                // Erase above (from start to cursor)
                let r = self.cursor.row as usize;
                let c = self.cursor.col as usize;
                for row in 0..r {
                    if row < self.cells.len() {
                        self.cells[row] = vec![Cell::default(); self.cols as usize];
                    }
                }
                if r < self.cells.len() {
                    for i in 0..=c.min((self.cols as usize).saturating_sub(1)) {
                        self.cells[r][i] = Cell::default();
                    }
                }
            }
            2 | 3 => {
                // Erase all
                self.cells = vec![vec![Cell::default(); self.cols as usize]; self.rows as usize];
            }
            _ => {}
        }
    }

    fn erase_in_line(&mut self, mode: u16) {
        let r = self.cursor.row as usize;
        if r >= self.cells.len() {
            return;
        }
        match mode {
            0 => {
                for i in self.cursor.col as usize..self.cols as usize {
                    if i < self.cells[r].len() {
                        self.cells[r][i] = Cell::default();
                    }
                }
            }
            1 => {
                for i in 0..=self.cursor.col as usize {
                    if i < self.cells[r].len() {
                        self.cells[r][i] = Cell::default();
                    }
                }
            }
            2 => {
                self.cells[r] = vec![Cell::default(); self.cols as usize];
            }
            _ => {}
        }
    }

    fn parse_sgr(&mut self, params: &vte::Params) {
        let mut iter = params.iter();
        while let Some(slice) = iter.next() {
            let param = if slice.is_empty() {
                0 // Empty params treated as SGR 0 (reset) per ECMA-48
            } else {
                slice[0]
            };
            match param {
                0 => {
                    self.current_attr = CellAttr {
                        fg: Color::Reset,
                        bg: Color::Reset,
                        bold: false,
                        italic: false,
                        underline: false,
                        dim: false,
                        strikethrough: false,
                    };
                }
                1 => self.current_attr.bold = true,
                2 => self.current_attr.dim = true,
                3 => self.current_attr.italic = true,
                4 => self.current_attr.underline = true,
                9 => self.current_attr.strikethrough = true,
                22 => {
                    self.current_attr.bold = false;
                    self.current_attr.dim = false;
                }
                23 => self.current_attr.italic = false,
                24 => self.current_attr.underline = false,
                25 => self.current_attr.dim = false,
                29 => self.current_attr.strikethrough = false,
                30..=37 => self.current_attr.fg = ansi_to_color(param - 30),
                38 => {
                    // Check for sub-params within the same slice first (e.g. [38, 2, R, G, B])
                    if slice.len() >= 3 && slice[1] == 2 {
                        // RGB in same slice: [38, 2, R, G, B]
                        let (r, g, b) = (slice[2] as u8, slice.get(3).copied().unwrap_or(0) as u8, slice.get(4).copied().unwrap_or(0) as u8);
                        self.current_attr.fg = Color::Rgb(r, g, b);
                    } else if slice.len() >= 3 && slice[1] == 5 {
                        // 256-color in same slice: [38, 5, idx]
                        self.current_attr.fg = Color::Indexed(slice[2] as u8);
                    } else if let Some(color) = parse_extended_color(&mut iter) {
                        self.current_attr.fg = color;
                    }
                }
                39 => self.current_attr.fg = Color::Reset,
                40..=47 => self.current_attr.bg = ansi_to_color(param - 40),
                48 => {
                    // Check for sub-params within the same slice first (e.g. [48, 2, R, G, B])
                    if slice.len() >= 3 && slice[1] == 2 {
                        let (r, g, b) = (slice[2] as u8, slice.get(3).copied().unwrap_or(0) as u8, slice.get(4).copied().unwrap_or(0) as u8);
                        self.current_attr.bg = Color::Rgb(r, g, b);
                    } else if slice.len() >= 3 && slice[1] == 5 {
                        self.current_attr.bg = Color::Indexed(slice[2] as u8);
                    } else if let Some(color) = parse_extended_color(&mut iter) {
                        self.current_attr.bg = color;
                    }
                }
                49 => self.current_attr.bg = Color::Reset,
                90..=97 => self.current_attr.fg = Color::Indexed((param - 90 + 8) as u8),
                100..=107 => self.current_attr.bg = Color::Indexed((param - 100 + 8) as u8),
                _ => {}
            }
        }
    }
}

fn ansi_to_color(idx: u16) -> Color {
    match idx {
        0 => Color::Black,
        1 => Color::Red,
        2 => Color::Green,
        3 => Color::Yellow,
        4 => Color::Blue,
        5 => Color::Magenta,
        6 => Color::Cyan,
        7 => Color::White,
        _ => Color::Reset,
    }
}

fn parse_extended_color<'a>(iter: &mut impl Iterator<Item = &'a [u16]>) -> Option<Color> {
    let mode = iter.next()?.first().copied()?;
    match mode {
        5 => {
            let idx = iter.next()?.first().copied()?;
            Some(Color::Indexed(idx as u8))
        }
        2 => {
            let r = iter.next()?.first().copied()? as u8;
            let g = iter.next()?.first().copied()? as u8;
            let b = iter.next()?.first().copied()? as u8;
            Some(Color::Rgb(r, g, b))
        }
        _ => None,
    }
}

fn param_or(params: &vte::Params, idx: usize, default: u16) -> u16 {
    params
        .iter()
        .nth(idx)
        .and_then(|s| s.first().copied())
        .map(|v| if v == 0 { default } else { v })
        .unwrap_or(default)
}

impl vte::Perform for TerminalGrid {
    fn print(&mut self, c: char) {
        self.write_char(c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            0x0A => {
                // LF
                self.scroll_up_if_needed();
            }
            0x0D => {
                // CR
                self.cursor.col = 0;
            }
            0x08 => {
                // BS
                self.cursor.col = self.cursor.col.saturating_sub(1);
            }
            0x09 => {
                // TAB
                let next_tab = (self.cursor.col / 8 + 1) * 8;
                self.cursor.col = next_tab.min(self.cols.saturating_sub(1));
            }
            0x07 => {} // BEL - ignore
            _ => {}
        }
    }

    fn csi_dispatch(
        &mut self,
        params: &vte::Params,
        intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        match action {
            'm' if intermediates.is_empty() => self.parse_sgr(params),
            'H' | 'f' => {
                // CUP - Cursor Position
                let row = param_or(params, 0, 1).saturating_sub(1);
                let col = param_or(params, 1, 1).saturating_sub(1);
                self.cursor.row = row.min(self.rows.saturating_sub(1));
                self.cursor.col = col.min(self.cols.saturating_sub(1));
            }
            'A' => {
                // CUU - Cursor Up
                let n = param_or(params, 0, 1);
                self.cursor.row = self.cursor.row.saturating_sub(n);
            }
            'B' => {
                // CUD - Cursor Down
                let n = param_or(params, 0, 1);
                self.cursor.row = (self.cursor.row + n).min(self.rows.saturating_sub(1));
            }
            'C' => {
                // CUF - Cursor Forward
                let n = param_or(params, 0, 1);
                self.cursor.col = (self.cursor.col + n).min(self.cols.saturating_sub(1));
            }
            'D' => {
                // CUB - Cursor Back
                let n = param_or(params, 0, 1);
                self.cursor.col = self.cursor.col.saturating_sub(n);
            }
            'J' => {
                // ED - Erase in Display
                let mode = param_or(params, 0, 0);
                self.erase_in_display(mode);
            }
            'K' => {
                // EL - Erase in Line
                let mode = param_or(params, 0, 0);
                self.erase_in_line(mode);
            }
            'L' => {
                // IL - Insert Lines
                let n = param_or(params, 0, 1) as usize;
                let row = self.cursor.row as usize;
                let bottom = self.scroll_bottom as usize;
                for _ in 0..n {
                    if row <= bottom && bottom < self.cells.len() {
                        self.cells.remove(bottom);
                        self.cells
                            .insert(row, vec![Cell::default(); self.cols as usize]);
                    }
                }
            }
            'M' => {
                // DL - Delete Lines
                let n = param_or(params, 0, 1) as usize;
                let row = self.cursor.row as usize;
                let bottom = self.scroll_bottom as usize;
                for _ in 0..n {
                    if row <= bottom && row < self.cells.len() {
                        self.cells.remove(row);
                        self.cells
                            .insert(bottom, vec![Cell::default(); self.cols as usize]);
                    }
                }
            }
            'r' => {
                // DECSTBM - Set Scroll Region
                let top = param_or(params, 0, 1).saturating_sub(1);
                let bottom = param_or(params, 1, self.rows).saturating_sub(1);
                self.scroll_top = top.min(self.rows.saturating_sub(1));
                self.scroll_bottom = bottom.min(self.rows.saturating_sub(1));
                self.cursor.row = 0;
                self.cursor.col = 0;
            }
            'h' | 'l' => {
                let enable = action == 'h';
                if intermediates == b"?" {
                    let mode = param_or(params, 0, 0);
                    match mode {
                        1 => {
                            // DECCKM: Application cursor key mode
                            self.application_cursor_keys = enable;
                        }
                        7 => {} // DECAWM: Auto-wrap mode — ignore (we always wrap)
                        12 => {} // Blinking cursor — ignore
                        25 => self.cursor_visible = enable,
                        1000 | 1002 | 1003 | 1006 => {} // Mouse tracking modes — ignore
                        1004 => self.focus_tracking = enable,
                        2004 => {} // Bracketed paste mode — ignore
                        1049 | 47 | 1047 => {
                            // Alternate screen buffer enter/leave: clear grid and
                            // home cursor so there is no stale content underneath
                            self.cells = vec![
                                vec![Cell::default(); self.cols as usize];
                                self.rows as usize
                            ];
                            self.cursor.row = 0;
                            self.cursor.col = 0;
                            self.scroll_top = 0;
                            self.scroll_bottom = self.rows.saturating_sub(1);
                        }
                        _ => {}
                    }
                }
            }
            'd' => {
                // VPA - Vertical Position Absolute
                let row = param_or(params, 0, 1).saturating_sub(1);
                self.cursor.row = row.min(self.rows.saturating_sub(1));
            }
            'G' => {
                // CHA - Cursor Horizontal Absolute
                let col = param_or(params, 0, 1).saturating_sub(1);
                self.cursor.col = col.min(self.cols.saturating_sub(1));
            }
            'X' => {
                // ECH - Erase Characters
                let n = param_or(params, 0, 1) as usize;
                let r = self.cursor.row as usize;
                let c = self.cursor.col as usize;
                if r < self.cells.len() {
                    for i in c..(c + n).min(self.cols as usize) {
                        if i < self.cells[r].len() {
                            self.cells[r][i] = Cell::default();
                        }
                    }
                }
            }
            'S' => {
                // SU - Scroll Up N lines within scroll region
                let n = param_or(params, 0, 1);
                for _ in 0..n {
                    self.scroll_up();
                }
            }
            'T' => {
                // SD - Scroll Down N lines within scroll region
                let n = param_or(params, 0, 1);
                for _ in 0..n {
                    self.scroll_down();
                }
            }
            '@' => {
                // ICH - Insert N blank characters at cursor position
                let n = param_or(params, 0, 1) as usize;
                let r = self.cursor.row as usize;
                let c = self.cursor.col as usize;
                if r < self.cells.len() {
                    let row = &mut self.cells[r];
                    let len = row.len();
                    // Shift characters right by n, dropping characters that fall off the end
                    let end = len.saturating_sub(n);
                    for i in (c..end).rev() {
                        row[i + n] = row[i];
                    }
                    // Fill inserted positions with blanks
                    for cell in row.iter_mut().take((c + n).min(len)).skip(c) {
                        *cell = Cell::default();
                    }
                }
            }
            'P' => {
                // DCH - Delete N characters at cursor, shift remaining left
                let n = param_or(params, 0, 1) as usize;
                let r = self.cursor.row as usize;
                let c = self.cursor.col as usize;
                if r < self.cells.len() {
                    let row = &mut self.cells[r];
                    let len = row.len();
                    // Shift characters left by n
                    for i in c..len {
                        let src = i + n;
                        row[i] = if src < len { row[src] } else { Cell::default() };
                    }
                    // Clear the tail
                    let tail_start = len - n.min(len - c);
                    for cell in row.iter_mut().take(len).skip(tail_start) {
                        *cell = Cell::default();
                    }
                }
            }
            'b' => {
                // REP - Repeat preceding graphic character N times
                let n = param_or(params, 0, 1);
                if let Some(ch) = self.last_char {
                    for _ in 0..n {
                        self.write_char(ch);
                    }
                }
            }
            'n' if intermediates.is_empty() => {
                // DSR - Device Status Report
                let mode = param_or(params, 0, 0);
                match mode {
                    6 => {
                        // CPR - Cursor Position Report: respond with ESC [ row ; col R
                        let r = self.cursor.row + 1;
                        let c = self.cursor.col + 1;
                        self.response_buf
                            .extend_from_slice(format!("\x1b[{};{}R", r, c).as_bytes());
                    }
                    5 => {
                        // Status report: respond "OK"
                        self.response_buf.extend_from_slice(b"\x1b[0n");
                    }
                    _ => {}
                }
            }
            'c' if intermediates.is_empty() => {
                // DA1 - Device Attributes: report as VT220 with ANSI color
                self.response_buf.extend_from_slice(b"\x1b[?62;22c");
            }
            _ => {}
        }
    }

    fn osc_dispatch(&mut self, _params: &[&[u8]], _bell_terminated: bool) {
        // Ignore OSC sequences (window title, etc.)
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        match (byte, intermediates) {
            (b'M', _) => {
                // RI - Reverse Index
                if self.cursor.row == self.scroll_top {
                    self.scroll_down();
                } else {
                    self.cursor.row = self.cursor.row.saturating_sub(1);
                }
            }
            (b'7', _) => {
                // DECSC - Save Cursor
                self.saved_cursor = Some((self.cursor.row, self.cursor.col));
            }
            (b'8', _) => {
                // DECRC - Restore Cursor
                if let Some((row, col)) = self.saved_cursor {
                    self.cursor.row = row;
                    self.cursor.col = col;
                }
            }
            _ => {}
        }
    }

    fn hook(&mut self, _params: &vte::Params, _intermediates: &[u8], _ignore: bool, _action: char) {
    }

    fn unhook(&mut self) {}

    fn put(&mut self, _byte: u8) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_grid() {
        let grid = TerminalGrid::new(80, 24);
        assert_eq!(grid.cols, 80);
        assert_eq!(grid.rows, 24);
        assert_eq!(grid.cells.len(), 24);
        assert_eq!(grid.cells[0].len(), 80);
        assert_eq!(grid.cell_at(0, 0).ch, ' ');
    }

    #[test]
    fn test_vte_print_and_color() {
        let mut grid = TerminalGrid::new(80, 24);
        let mut parser = vte::Parser::new();
        let input = b"Hello\x1b[31m World";
        for byte in input {
            parser.advance(&mut grid, *byte);
        }
        assert_eq!(grid.cell_at(0, 0).ch, 'H');
        assert_eq!(grid.cell_at(0, 0).fg, Color::Reset);
        assert_eq!(grid.cell_at(0, 6).ch, 'W');
        assert_eq!(grid.cell_at(0, 6).fg, Color::Red);
    }

    #[test]
    fn test_erase_display() {
        let mut grid = TerminalGrid::new(80, 24);
        let mut parser = vte::Parser::new();
        for byte in b"Hello" {
            parser.advance(&mut grid, *byte);
        }
        for byte in b"\x1b[2J" {
            parser.advance(&mut grid, *byte);
        }
        assert_eq!(grid.cell_at(0, 0).ch, ' ');
    }

    #[test]
    fn test_scroll_region() {
        let mut grid = TerminalGrid::new(80, 24);
        let mut parser = vte::Parser::new();
        for byte in b"\x1b[5;20r" {
            parser.advance(&mut grid, *byte);
        }
        assert_eq!(grid.scroll_top, 4);
        assert_eq!(grid.scroll_bottom, 19);
    }

    #[test]
    fn test_wide_char_korean() {
        let mut grid = TerminalGrid::new(80, 24);
        let mut parser = vte::Parser::new();
        // "한글" = two Korean chars, each 2 cells wide
        let input = "한글".as_bytes();
        for byte in input {
            parser.advance(&mut grid, *byte);
        }
        assert_eq!(grid.cell_at(0, 0).ch, '한');
        assert_eq!(grid.cell_at(0, 1).ch, ' '); // placeholder for wide char
        assert_eq!(grid.cell_at(0, 2).ch, '글');
        assert_eq!(grid.cell_at(0, 3).ch, ' '); // placeholder
        assert_eq!(grid.cursor.col, 4); // cursor advanced by 4 (2 chars × 2 width)
    }

    #[test]
    fn test_alternate_screen() {
        let mut grid = TerminalGrid::new(80, 24);
        let mut parser = vte::Parser::new();
        // Write some content
        for byte in b"Hello" {
            parser.advance(&mut grid, *byte);
        }
        assert_eq!(grid.cell_at(0, 0).ch, 'H');
        // Enter alternate screen - should clear
        for byte in b"\x1b[?1049h" {
            parser.advance(&mut grid, *byte);
        }
        assert_eq!(grid.cell_at(0, 0).ch, ' ');
        assert_eq!(grid.cursor.row, 0);
        assert_eq!(grid.cursor.col, 0);
    }

    #[test]
    fn test_sgr_bare_reset() {
        let mut grid = TerminalGrid::new(80, 24);
        let mut parser = vte::Parser::new();
        // Set red, then bare ESC[m should reset
        for byte in b"\x1b[31mA\x1b[mB" {
            parser.advance(&mut grid, *byte);
        }
        assert_eq!(grid.cell_at(0, 0).fg, Color::Red);
        assert_eq!(grid.cell_at(0, 1).fg, Color::Reset);
    }

    #[test]
    fn test_wide_char_wrap_at_line_end() {
        // Grid is 5 cols wide; place a wide char at col 4 — it must wrap to next line
        let mut grid = TerminalGrid::new(5, 5);
        let mut parser = vte::Parser::new();
        // Write 4 ASCII chars then a wide Korean char that doesn't fit
        let input = "ABCD한".as_bytes();
        for byte in input {
            parser.advance(&mut grid, *byte);
        }
        // ABCD fit on row 0, cols 0-3
        assert_eq!(grid.cell_at(0, 0).ch, 'A');
        assert_eq!(grid.cell_at(0, 3).ch, 'D');
        // Wide char wrapped to row 1
        assert_eq!(grid.cell_at(1, 0).ch, '한');
        assert_eq!(grid.cell_at(1, 1).ch, ' '); // wide char placeholder
        assert_eq!(grid.cursor.row, 1);
        assert_eq!(grid.cursor.col, 2);
    }

    #[test]
    fn test_sgr_256_color() {
        let mut grid = TerminalGrid::new(80, 24);
        let mut parser = vte::Parser::new();
        // ESC[38;5;200m sets fg to indexed color 200
        for byte in b"\x1b[38;5;200mA" {
            parser.advance(&mut grid, *byte);
        }
        assert_eq!(grid.cell_at(0, 0).fg, Color::Indexed(200));
        // ESC[48;5;100m sets bg to indexed color 100
        for byte in b"\x1b[48;5;100mB" {
            parser.advance(&mut grid, *byte);
        }
        assert_eq!(grid.cell_at(0, 1).bg, Color::Indexed(100));
    }

    #[test]
    fn test_sgr_rgb_color() {
        let mut grid = TerminalGrid::new(80, 24);
        let mut parser = vte::Parser::new();
        // ESC[38;2;10;20;30m sets fg to RGB(10,20,30)
        for byte in b"\x1b[38;2;10;20;30mA" {
            parser.advance(&mut grid, *byte);
        }
        assert_eq!(grid.cell_at(0, 0).fg, Color::Rgb(10, 20, 30));
        // ESC[48;2;50;60;70m sets bg to RGB(50,60,70)
        for byte in b"\x1b[48;2;50;60;70mB" {
            parser.advance(&mut grid, *byte);
        }
        assert_eq!(grid.cell_at(0, 1).bg, Color::Rgb(50, 60, 70));
    }

    #[test]
    fn test_cursor_cup() {
        let mut grid = TerminalGrid::new(80, 24);
        let mut parser = vte::Parser::new();
        // CUP row=5 col=10 (1-based)
        for byte in b"\x1b[5;10H" {
            parser.advance(&mut grid, *byte);
        }
        assert_eq!(grid.cursor.row, 4);
        assert_eq!(grid.cursor.col, 9);
    }

    #[test]
    fn test_cursor_movement() {
        let mut grid = TerminalGrid::new(80, 24);
        let mut parser = vte::Parser::new();
        // Start at (0,0), move down 3
        for byte in b"\x1b[3B" {
            parser.advance(&mut grid, *byte);
        }
        assert_eq!(grid.cursor.row, 3);
        assert_eq!(grid.cursor.col, 0);
        // Move right 5
        for byte in b"\x1b[5C" {
            parser.advance(&mut grid, *byte);
        }
        assert_eq!(grid.cursor.col, 5);
        // Move up 2
        for byte in b"\x1b[2A" {
            parser.advance(&mut grid, *byte);
        }
        assert_eq!(grid.cursor.row, 1);
        // Move left 3
        for byte in b"\x1b[3D" {
            parser.advance(&mut grid, *byte);
        }
        assert_eq!(grid.cursor.col, 2);
    }

    #[test]
    fn test_insert_lines() {
        let mut grid = TerminalGrid::new(10, 5);
        let mut parser = vte::Parser::new();
        // Write lines: A on row 0, B on row 1
        for byte in b"A\r\nB" {
            parser.advance(&mut grid, *byte);
        }
        assert_eq!(grid.cell_at(0, 0).ch, 'A');
        assert_eq!(grid.cell_at(1, 0).ch, 'B');
        // Move cursor to row 0 and insert 1 line (IL)
        for byte in b"\x1b[1;1H\x1b[1L" {
            parser.advance(&mut grid, *byte);
        }
        // Row 0 should now be blank, A pushed to row 1, B to row 2
        assert_eq!(grid.cell_at(0, 0).ch, ' ');
        assert_eq!(grid.cell_at(1, 0).ch, 'A');
        assert_eq!(grid.cell_at(2, 0).ch, 'B');
    }

    #[test]
    fn test_delete_lines() {
        let mut grid = TerminalGrid::new(10, 5);
        let mut parser = vte::Parser::new();
        // Write A on row 0, B on row 1, C on row 2
        for byte in b"A\r\nB\r\nC" {
            parser.advance(&mut grid, *byte);
        }
        // Move cursor to row 0 and delete 1 line (DL)
        for byte in b"\x1b[1;1H\x1b[1M" {
            parser.advance(&mut grid, *byte);
        }
        // B should now be on row 0, C on row 1
        assert_eq!(grid.cell_at(0, 0).ch, 'B');
        assert_eq!(grid.cell_at(1, 0).ch, 'C');
        assert_eq!(grid.cell_at(2, 0).ch, ' ');
    }

    #[test]
    fn test_sgr_dim() {
        let mut grid = TerminalGrid::new(80, 24);
        let mut parser = vte::Parser::new();
        // SGR 2 = dim
        for byte in b"\x1b[2mA" {
            parser.advance(&mut grid, *byte);
        }
        assert!(grid.cell_at(0, 0).dim);
        assert!(!grid.cell_at(0, 0).bold);
        // SGR 0 resets dim
        for byte in b"\x1b[0mB" {
            parser.advance(&mut grid, *byte);
        }
        assert!(!grid.cell_at(0, 1).dim);
    }

    #[test]
    fn test_sgr_strikethrough() {
        let mut grid = TerminalGrid::new(80, 24);
        let mut parser = vte::Parser::new();
        // SGR 9 = strikethrough
        for byte in b"\x1b[9mA" {
            parser.advance(&mut grid, *byte);
        }
        assert!(grid.cell_at(0, 0).strikethrough);
        // SGR 29 = turn off strikethrough
        for byte in b"\x1b[29mB" {
            parser.advance(&mut grid, *byte);
        }
        assert!(!grid.cell_at(0, 1).strikethrough);
        // SGR 0 also resets strikethrough
        for byte in b"\x1b[9mC\x1b[0mD" {
            parser.advance(&mut grid, *byte);
        }
        assert!(grid.cell_at(0, 2).strikethrough);
        assert!(!grid.cell_at(0, 3).strikethrough);
    }

    #[test]
    fn test_cursor_save_restore() {
        let mut grid = TerminalGrid::new(80, 24);
        let mut parser = vte::Parser::new();
        // Move to row=5, col=10, save cursor
        for byte in b"\x1b[6;11H\x1b7" {
            parser.advance(&mut grid, *byte);
        }
        assert_eq!(grid.cursor.row, 5);
        assert_eq!(grid.cursor.col, 10);
        // Move elsewhere
        for byte in b"\x1b[1;1H" {
            parser.advance(&mut grid, *byte);
        }
        assert_eq!(grid.cursor.row, 0);
        assert_eq!(grid.cursor.col, 0);
        // Restore cursor
        for byte in b"\x1b8" {
            parser.advance(&mut grid, *byte);
        }
        assert_eq!(grid.cursor.row, 5);
        assert_eq!(grid.cursor.col, 10);
    }

    #[test]
    fn test_cursor_save_restore_no_save() {
        // Restoring without a prior save should be a no-op
        let mut grid = TerminalGrid::new(80, 24);
        let mut parser = vte::Parser::new();
        for byte in b"\x1b[3;4H" {
            parser.advance(&mut grid, *byte);
        }
        for byte in b"\x1b8" {
            parser.advance(&mut grid, *byte);
        }
        // Should remain at last explicit position since no save happened
        assert_eq!(grid.cursor.row, 2);
        assert_eq!(grid.cursor.col, 3);
    }
}
