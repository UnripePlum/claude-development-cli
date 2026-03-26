use crossbeam_channel::Sender;
use std::io::Read;
use std::thread::JoinHandle;

pub enum PtyEvent {
    Output(Vec<u8>),
    Exited,
}

pub fn spawn_reader(mut reader: Box<dyn Read + Send>, tx: Sender<PtyEvent>) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    let _ = tx.send(PtyEvent::Exited);
                    break;
                }
                Ok(n) => {
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
