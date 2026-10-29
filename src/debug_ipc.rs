//! Local-only, debug-build-only command channel.
//!
//! Enable with `YKEXPLORER_DEBUG_PORT=47654` when launching a debug build. The
//! channel is bound to 127.0.0.1 and is never compiled into release builds.
//! It exists so UI/file behavior can be inspected without moving the user's
//! physical mouse or stealing focus.

use std::{
    io::{BufRead, BufReader, Write},
    net::{TcpListener, TcpStream},
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::Duration,
};

pub struct DebugRequest {
    pub command: String,
    pub response: Sender<String>,
}

pub fn start(port: u16) -> Receiver<DebugRequest> {
    let (tx, rx) = mpsc::channel::<DebugRequest>();
    thread::spawn(move || {
        let listener = match TcpListener::bind(("127.0.0.1", port)) {
            Ok(listener) => listener,
            Err(error) => {
                log::error!("debug IPC cannot bind 127.0.0.1:{port}: {error}");
                return;
            }
        };
        log::info!("debug IPC listening on 127.0.0.1:{port}");
        for stream in listener.incoming() {
            let Ok(stream) = stream else {
                continue;
            };
            let tx = tx.clone();
            thread::spawn(move || handle_client(stream, tx));
        }
    });
    rx
}

fn handle_client(stream: TcpStream, tx: Sender<DebugRequest>) {
    let Ok(reader_stream) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(reader_stream);
    let mut writer = stream;
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let command = line.trim().to_string();
        if command.is_empty() {
            continue;
        }
        let (response_tx, response_rx) = mpsc::channel();
        if tx
            .send(DebugRequest {
                command,
                response: response_tx,
            })
            .is_err()
        {
            return;
        }
        let response = response_rx
            .recv_timeout(Duration::from_secs(20))
            .unwrap_or_else(|_| "error timeout".to_string());
        if writer
            .write_all(format!("{response}\n").as_bytes())
            .is_err()
        {
            return;
        }
    }
}
