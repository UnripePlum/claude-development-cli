pub mod grid;

pub use grid::TerminalGrid;

pub struct Pane {
    pub id: u32,
    pub name: String,
    pub grid: TerminalGrid,
    pub parser: vte::Parser,
    pub status: PaneStatus,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PaneStatus {
    Running,
    Exited(i32),
}

impl Pane {
    pub fn new(id: u32, name: String, cols: u16, rows: u16) -> Self {
        Self {
            id,
            name,
            grid: TerminalGrid::new(cols, rows),
            parser: vte::Parser::new(),
            status: PaneStatus::Running,
        }
    }

    pub fn process_bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.parser.advance(&mut self.grid, *byte);
        }
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.grid.resize(cols, rows);
    }
}
