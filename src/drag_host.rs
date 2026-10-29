//! Host for the native OLE drag operation.
//!
//! `DoDragDrop` must run on the UI thread that owns the source window: OLE
//! pumps messages for that thread and the drag cursor / drop target
//! interaction depends on it. Running it on a worker thread freezes the source
//! window and makes every target reject the drop.
//!
//! GPUI 0.2.2 owns the window procedure, so we subclass the window and handle
//! a private `WM_APP` message there. The message is posted from the drag
//! watcher, which runs on GPUI's foreground executor (the UI thread), so the
//! whole drag happens on the UI thread with GPUI's borrow released.

use std::{
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    UI::WindowsAndMessaging::{
        DefWindowProcW, GWLP_WNDPROC, PostMessageW, SetWindowLongPtrW, WM_APP,
    },
};

use crate::drag_out::{self, DragOutcome};

type WindowProc = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT;

/// Private message used to run the drag on the UI thread.
const WM_FPILOT_BEGIN_DRAG: u32 = WM_APP + 0x51;
static ORIGINAL_PROC: Mutex<Option<WindowProc>> = Mutex::new(None);
static INSTALLED: AtomicBool = AtomicBool::new(false);

struct PendingDrag {
    paths: Vec<PathBuf>,
    shift: bool,
}

static PENDING: Mutex<Option<PendingDrag>> = Mutex::new(None);
static RESULT: Mutex<Option<Result<DragOutcome, String>>> = Mutex::new(None);

unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_FPILOT_BEGIN_DRAG {
        let pending = PENDING.lock().ok().and_then(|mut slot| slot.take());
        if let Some(pending) = pending {
            // Do NOT release the capture here: GPUI captured the mouse on
            // button down and OLE relies on that capture to track the drag and
            // find drop targets. Releasing it makes targets report NONE.
            log::info!(
                "starting DoDragDrop on UI thread: {} items (shift={})",
                pending.paths.len(),
                pending.shift
            );
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                drag_out::start_file_drag(&pending.paths, pending.shift, hwnd.0 as isize)
            }))
            .unwrap_or_else(|_| Err("拖放异常终止".to_string()));
            log::info!("DoDragDrop returned: {outcome:?}");
            if let Ok(mut slot) = RESULT.lock() {
                *slot = Some(outcome);
            }
        }
        return LRESULT(0);
    }

    let original = ORIGINAL_PROC.lock().ok().and_then(|slot| *slot);
    match original {
        Some(proc) => unsafe { proc(hwnd, msg, wparam, lparam) },
        None => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Install the window subclass. Safe to call more than once.
pub fn install(hwnd: HWND) {
    if hwnd.is_invalid() {
        return;
    }
    if INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }
    unsafe {
        let previous = SetWindowLongPtrW(
            hwnd,
            GWLP_WNDPROC,
            subclass_proc as unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT
                as isize,
        );
        if previous == 0 {
            INSTALLED.store(false, Ordering::SeqCst);
            log::error!("failed to subclass window for native drag");
            return;
        }
        let original: WindowProc = std::mem::transmute(previous);
        if let Ok(mut slot) = ORIGINAL_PROC.lock() {
            *slot = Some(original);
        }
    }
    log::info!("window subclass installed for native drag");
}

/// Ask the UI thread to start a native drag. Returns false if the request
/// could not be queued.
pub fn request_drag(hwnd: HWND, paths: Vec<PathBuf>, shift: bool) -> bool {
    if hwnd.is_invalid() {
        return false;
    }
    {
        let Ok(mut slot) = PENDING.lock() else {
            return false;
        };
        if slot.is_some() {
            return false;
        }
        *slot = Some(PendingDrag { paths, shift });
    }
    unsafe { PostMessageW(Some(hwnd), WM_FPILOT_BEGIN_DRAG, WPARAM(0), LPARAM(0)).is_ok() }
}

/// Take the outcome of a finished drag, if any.
pub fn take_result() -> Option<Result<DragOutcome, String>> {
    RESULT.lock().ok().and_then(|mut slot| slot.take())
}

