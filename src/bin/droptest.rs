//! Diagnostic drop target used to verify that FPilot's outgoing drag data
//! object can be consumed by another process.
//!
//! Run it, then start a synthetic drag from FPilot (`drag-host` debug command).
//! The probe creates a small top-most window under the cursor, registers
//! itself as an OLE drop target, logs every format query and writes the result
//! to `droptest.log`.

use std::{path::PathBuf, sync::Mutex};

use windows::{
    core::{implement, Result as WinResult},
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, POINT, POINTL, WPARAM},
        System::{
            Com::{DVASPECT_CONTENT, FORMATETC, IDataObject, STGMEDIUM, TYMED_HGLOBAL},
            Ole::{
                CF_HDROP, DROPEFFECT, IDropTarget, IDropTarget_Impl, OleInitialize,
                RegisterDragDrop, ReleaseStgMedium, RevokeDragDrop,
            },
            SystemServices::MODIFIERKEYS_FLAGS,
        },
        UI::{
            Shell::{DragQueryFileW, HDROP},
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetCursorPos,
                GetMessageW, MSG, PostQuitMessage, RegisterClassW, SetTimer, ShowWindow,
                TranslateMessage, WM_DESTROY, WM_TIMER, WNDCLASSW, WS_EX_TOPMOST,
                WS_OVERLAPPED, SW_SHOW,
            },
        },
    },
};

static LOG: Mutex<String> = Mutex::new(String::new());

fn log_line(text: impl AsRef<str>) {
    println!("{}", text.as_ref());
    if let Ok(mut log) = LOG.lock() {
        log.push_str(text.as_ref());
        log.push('\n');
    }
}

#[implement(IDropTarget)]
struct ProbeTarget;

impl ProbeTarget {
    fn inspect(&self, label: &str, data: Option<&IDataObject>, effect: &mut u32) -> u32 {
        let Some(data) = data else {
            log_line(format!("{label}: no data object"));
            *effect = 0;
            return 0;
        };
        let format = FORMATETC {
            cfFormat: CF_HDROP.0,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        };
        let query = unsafe { data.QueryGetData(&format) };
        log_line(format!("{label}: QueryGetData(CF_HDROP) = 0x{:08X}", query.0));
        if query.is_err() {
            *effect = 0;
            return 0;
        }
        let medium: STGMEDIUM = match unsafe { data.GetData(&format) } {
            Ok(medium) => medium,
            Err(error) => {
                log_line(format!("{label}: GetData failed: {error:?}"));
                *effect = 0;
                return 0;
            }
        };
        let mut count = 0u32;
        unsafe {
            let hdrop = HDROP(medium.u.hGlobal.0);
            let files = DragQueryFileW(hdrop, u32::MAX, None);
            log_line(format!("{label}: DragQueryFile count = {files}"));
            for index in 0..files {
                let length = DragQueryFileW(hdrop, index, None) as usize;
                let mut buffer = vec![0u16; length + 1];
                let written = DragQueryFileW(hdrop, index, Some(&mut buffer));
                let text = String::from_utf16_lossy(&buffer[..written as usize]);
                log_line(format!("{label}: file[{index}] = {text}"));
                count += 1;
            }
            ReleaseStgMedium(&medium as *const _ as *mut _);
        }
        *effect = 1; // DROPEFFECT_COPY
        count
    }
}

#[allow(non_snake_case)]
impl IDropTarget_Impl for ProbeTarget_Impl {
    fn DragEnter(
        &self,
        pdataobj: windows::core::Ref<IDataObject>,
        _grfkeystate: MODIFIERKEYS_FLAGS,
        _pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> WinResult<()> {
        let mut effect = 0u32;
        let data = pdataobj.ok().ok();
        self.inspect("DragEnter", data, &mut effect);
        unsafe { *pdweffect = DROPEFFECT(effect) };
        Ok(())
    }

    fn DragOver(
        &self,
        _grfkeystate: MODIFIERKEYS_FLAGS,
        _pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> WinResult<()> {
        unsafe { *pdweffect = DROPEFFECT(1) };
        Ok(())
    }

    fn DragLeave(&self) -> WinResult<()> {
        log_line("DragLeave");
        Ok(())
    }

    fn Drop(
        &self,
        pdataobj: windows::core::Ref<IDataObject>,
        _grfkeystate: MODIFIERKEYS_FLAGS,
        _pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> WinResult<()> {
        let mut effect = 0u32;
        let data = pdataobj.ok().ok();
        self.inspect("Drop", data, &mut effect);
        unsafe { *pdweffect = DROPEFFECT(effect) };
        Ok(())
    }
}

unsafe extern "system" fn probe_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_TIMER => {
            log_line("probe: timeout, exiting");
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn main() {
    let seconds: u64 = std::env::args()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(25);
    unsafe {
        let _ = OleInitialize(None);
    }

    let class_name = windows::core::w!("FpilotDropProbe");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(probe_wndproc),
        lpszClassName: class_name,
        ..Default::default()
    };
    unsafe {
        RegisterClassW(&wc);
    }

    let mut cursor = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut cursor);
    }
    let x = (cursor.x - 200).max(0);
    let y = (cursor.y - 150).max(0);
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST,
            class_name,
            windows::core::w!("FPilot drop probe"),
            WS_OVERLAPPED,
            x,
            y,
            400,
            300,
            None,
            None,
            None,
            None,
        )
    }
    .expect("create probe window");
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
    }

    let target: IDropTarget = ProbeTarget.into();
    unsafe {
        RegisterDragDrop(hwnd, &target).expect("register drop target");
        SetTimer(Some(hwnd), 1, (seconds * 1000) as u32, None);
    }
    log_line(format!(
        "probe: window at {x},{y} (400x300), timeout {seconds}s, cursor {},{}, hwnd {hwnd:?}",
        cursor.x, cursor.y
    ));

    let mut message = MSG::default();
    while unsafe { GetMessageW(&mut message, None, 0, 0) }.as_bool() {
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    unsafe {
        let _ = RevokeDragDrop(hwnd);
    }
    let log = LOG.lock().map(|log| log.clone()).unwrap_or_default();
    let path = PathBuf::from("droptest.log");
    std::fs::write(&path, log).ok();
    println!("probe: log written to {}", path.display());
}
