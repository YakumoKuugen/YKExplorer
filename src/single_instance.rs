//! Single-instance hand-off for shell launches.
//!
//! Clicking the taskbar/desktop icon while YKExplorer is already running should
//! feel like Explorer: bring up another window instead of starting a second
//! process. A per-user, per-session named pipe carries one line per launch
//! (the requested folder, or an empty line for "just open a window").
//!
//! If the pipe is unavailable for any reason, the process still starts
//! normally instead of failing; the worst case is the old behaviour of two
//! independent processes.

use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Mutex, OnceLock},
    time::Duration,
};

use windows::{
    Win32::{
        Foundation::{CloseHandle, GENERIC_WRITE, HANDLE},
        Storage::FileSystem::{
            CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_SHARE_NONE,
            OPEN_EXISTING, PIPE_ACCESS_INBOUND, ReadFile, WriteFile,
        },
        System::Pipes::{
            ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
            PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
        },
    },
    core::HSTRING,
};

const MAX_QUEUED_LAUNCHES: usize = 16;
const PIPE_BUFFER: u32 = 32 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Startup {
    /// The request was handed to the already-running instance; this process
    /// should exit immediately without creating a window.
    Forwarded,
    /// No other instance answered; this process owns the pipe.
    Primary,
}

fn launch_queue() -> &'static Mutex<VecDeque<String>> {
    static QUEUE: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();
    QUEUE.get_or_init(|| Mutex::new(VecDeque::new()))
}

/// Pop one pending launch request. Empty strings mean "open a window without a
/// specific folder" (the icon was clicked without arguments).
pub fn take_launch_request() -> Option<String> {
    launch_queue()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .pop_front()
}

fn enqueue_launch(path: String) {
    let mut queue = launch_queue()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if queue.len() >= MAX_QUEUED_LAUNCHES {
        queue.pop_front();
    }
    queue.push_back(path);
}

fn sanitize_part(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .take(48)
        .collect();
    if cleaned.is_empty() {
        "user".to_string()
    } else {
        cleaned
    }
}

/// A named pipe shared by every YKExplorer process of the same user/session.
fn pipe_name() -> HSTRING {
    let user = std::env::var("USERNAME").unwrap_or_default();
    let session = std::env::var("SESSIONNAME").unwrap_or_default();
    HSTRING::from(format!(
        r"\\.\pipe\YKExplorer-{}-{}",
        sanitize_part(&user),
        sanitize_part(&session)
    ))
}

fn try_forward(name: &HSTRING, payload: &str, attempts: usize, delay: Duration) -> bool {
    for attempt in 0..attempts {
        let opened = unsafe {
            CreateFileW(
                name,
                GENERIC_WRITE.0,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        };
        if let Ok(handle) = opened {
            let bytes = payload.as_bytes();
            let mut written = 0u32;
            let _ = unsafe { WriteFile(handle, Some(bytes), Some(&mut written), None) };
            let _ = unsafe { CloseHandle(handle) };
            return true;
        }
        if attempt + 1 < attempts {
            std::thread::sleep(delay);
        }
    }
    false
}

fn create_pipe(name: &HSTRING, first_instance: bool) -> Option<HANDLE> {
    let mut mode = PIPE_ACCESS_INBOUND;
    if first_instance {
        mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
    }
    let handle = unsafe {
        CreateNamedPipeW(
            name,
            mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
            PIPE_UNLIMITED_INSTANCES,
            0,
            PIPE_BUFFER,
            0,
            None,
        )
    };
    (!handle.is_invalid()).then_some(handle)
}

fn spawn_server(handle: HANDLE, name: HSTRING) {
    // Raw pointers are not `Send`. Convert to a plain integer for the thread
    // and rebuild the handle there; the pipe thread is the only user.
    let raw_handle = handle.0 as usize;
    let name = name.to_string();
    let spawned = std::thread::Builder::new()
        .name("ykexplorer-launch-pipe".into())
        .spawn(move || {
            let name = HSTRING::from(name);
            let mut handle = HANDLE(raw_handle as *mut _);
            loop {
                // `ConnectNamedPipe` also returns an error when a client
                // connected between `CreateNamedPipeW` and this call;
                // `ReadFile` is the real test of whether a connection exists.
                let _ = unsafe { ConnectNamedPipe(handle, None) };

                let mut buffer = vec![0u8; PIPE_BUFFER as usize];
                let mut read = 0u32;
                let read_ok =
                    unsafe { ReadFile(handle, Some(&mut buffer), Some(&mut read), None) }.is_ok();
                if read_ok {
                    let text = String::from_utf8_lossy(&buffer[..read as usize])
                        .trim_end_matches(|character: char| {
                            matches!(character, '\r' | '\n' | '\0')
                        })
                        .to_string();
                    log::info!("single-instance launch request: {text:?}");
                    enqueue_launch(text);
                }

                unsafe {
                    let _ = DisconnectNamedPipe(handle);
                    let _ = CloseHandle(handle);
                }

                // The previous instance is single-use; create the next one
                // before blocking in `ConnectNamedPipe` again.
                loop {
                    if let Some(next) = create_pipe(&name, false) {
                        handle = next;
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
        });

    if let Err(error) = spawned {
        log::warn!("cannot start single-instance pipe thread: {error}");
    }
}

/// Decide whether this process is the primary instance. A forwarded launch
/// returns `Startup::Forwarded` and the caller must exit without creating UI.
pub fn start(launch_path: Option<PathBuf>) -> Startup {
    let name = pipe_name();
    let payload = launch_path
        .as_ref()
        .map(|path| path.to_string_lossy().into_owned())
        // A bare newline is the "open a window, no specific folder" request;
        // sending a zero-byte message would look like a broken pipe.
        .unwrap_or_else(|| "\n".to_string());

    // Fast path: an instance is already listening.
    if try_forward(&name, &payload, 1, Duration::ZERO) {
        return Startup::Forwarded;
    }

    // Be the first instance. The FIRST_PIPE_INSTANCE flag makes simultaneous
    // starts deterministic: exactly one process wins, the other retries the
    // client path and forwards its launch.
    if let Some(handle) = create_pipe(&name, true) {
        log::info!("single-instance pipe ready");
        spawn_server(handle, name);
        return Startup::Primary;
    }

    if try_forward(&name, &payload, 24, Duration::from_millis(60)) {
        return Startup::Forwarded;
    }

    // Pipe is busy/unavailable but nobody answered: keep working as a normal
    // standalone process rather than refusing to start.
    log::warn!("single-instance pipe unavailable; starting a separate process");
    Startup::Primary
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipe_name_is_sanitized() {
        let name = pipe_name().to_string();
        assert!(name.starts_with(r"\\.\pipe\YKExplorer-"));
        let suffix = name.trim_start_matches(r"\\.\pipe\YKExplorer-");
        assert!(!suffix.contains(' '));
        assert!(!suffix.contains('\\'));
    }

    #[test]
    fn launch_queue_is_bounded() {
        let mut queue = launch_queue()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        queue.clear();
        drop(queue);

        for index in 0..(MAX_QUEUED_LAUNCHES + 4) {
            enqueue_launch(format!("C:\\path-{index}"));
        }
        let mut seen = 0;
        while take_launch_request().is_some() {
            seen += 1;
        }
        assert_eq!(seen, MAX_QUEUED_LAUNCHES);
    }
}
