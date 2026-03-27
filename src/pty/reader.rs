use crossbeam_channel::Sender;
use std::io::Read;
use std::thread::JoinHandle;

pub enum PtyEvent {
    Output(Vec<u8>),
    Exited,
}

pub fn spawn_reader(mut reader: Box<dyn Read + Send>, tx: Sender<PtyEvent>) -> JoinHandle<()> {
    // Optional raw PTY byte logging: set CDC_PTY_LOG=/path/to/file
    let log_file = std::env::var("CDC_PTY_LOG").ok().and_then(|path| {
        std::fs::File::create(&path)
            .ok()
            .map(|f| std::io::BufWriter::new(f))
    });

    std::thread::spawn(move || {
        let mut log = log_file;
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    let _ = tx.send(PtyEvent::Exited);
                    break;
                }
                Ok(n) => {
                    if let Some(ref mut f) = log {
                        use std::io::Write;
                        let _ = f.write_all(&buf[..n]);
                        let _ = f.flush();
                    }
                    if tx.send(PtyEvent::Output(buf[..n].to_vec())).is_err() {
                        break;
                    }
                }
                Err(_) => {
                    let _ = tx.send(PtyEvent::Exited);
                    break;
                }
            }
        }
    })
}
