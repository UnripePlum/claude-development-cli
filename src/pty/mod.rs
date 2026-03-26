pub mod reader;

pub use reader::PtyEvent;

use crossbeam_channel::Sender;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use std::io::Write;
use std::thread::JoinHandle;

pub struct PtyManager {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send>,
    writer: Box<dyn Write + Send>,
    _reader_handle: JoinHandle<()>,
}

impl PtyManager {
    pub fn spawn(
        cmd: &str,
        args: &[&str],
        cols: u16,
        rows: u16,
        tx: Sender<PtyEvent>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::spawn_with_cwd(cmd, args, cols, rows, tx, None)
    }

    pub fn spawn_with_cwd(
        cmd: &str,
        args: &[&str],
        cols: u16,
        rows: u16,
        tx: Sender<PtyEvent>,
        cwd: Option<&str>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut cmd_builder = CommandBuilder::new(cmd);
        cmd_builder.args(args);
        cmd_builder.env("TERM", "xterm-256color");
        if let Some(dir) = cwd {
            cmd_builder.cwd(dir);
        }

        let child = pair.slave.spawn_command(cmd_builder)?;
        drop(pair.slave);

        let reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let reader_handle = reader::spawn_reader(reader, tx);

        Ok(Self {
            master: pair.master,
            child,
            writer,
            _reader_handle: reader_handle,
        })
    }

    pub fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.writer.write_all(data)?;
        self.writer.flush()
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), Box<dyn std::error::Error>> {
        self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        Ok(())
    }

    pub fn try_wait_exit_code(&mut self) -> Option<i32> {
        match self.child.try_wait() {
            Ok(Some(status)) => Some(status.exit_code() as i32),
            _ => None,
        }
    }

    pub fn kill(&mut self) -> std::io::Result<()> {
        let _ = self.child.kill();
        let _ = self.child.wait(); // reap zombie process
        Ok(())
    }
}
