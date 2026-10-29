#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

mod app;
mod assets;
mod clipboard;
#[cfg(debug_assertions)]
mod debug_ipc;
mod drag_host;
mod drag_out;
mod explorer;
mod fs;
mod ops;
mod shell_registry;
mod text_input;
mod theme;

use gpui::{
    App, Application, Bounds, KeyBinding, WindowBounds, WindowOptions, prelude::*, px, size,
};

use app::{
    Back, Cancel, CloseTab, CopyFiles, CopyPath, CutFiles, FILE_MANAGER_CONTEXT, FocusPath,
    FocusSearch, Forward, NewFile, NewFolder, NewTab, NextTab, OpenSelected, OpenSelectedNewTab,
    PasteFiles, PermanentDelete, PilotApp, PrevTab, Refresh, RenameSelection, RevealSelection,
    SelectAll, SelectExtendFirst, SelectExtendLast, SelectExtendNext, SelectExtendPrev,
    SelectFirst, SelectLast, SelectNext, SelectPrev, ToggleHidden, ToggleSidebar, TrashSelection,
    Undo, Up, ViewDetails, ViewGrid, ViewSplit,
};
use text_input::{
    EditorBackspace, EditorCancel, EditorCopy, EditorCut, EditorDelete, EditorEnd, EditorHome,
    EditorLeft, EditorPaste, EditorRight, EditorSelectAll, EditorSelectLeft, EditorSelectRight,
    EditorSubmit,
};

fn write_crash_log(message: &str) {
    let entry = format!(
        "\n==== FPilot crash {} ====\n{message}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f")
    );
    for path in crash_log_paths() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            use std::io::Write;
            let _ = file.write_all(entry.as_bytes());
            let _ = file.flush();
            break;
        }
    }
}

fn crash_log_paths() -> Vec<std::path::PathBuf> {
    let mut paths = Vec::new();
    if let Some(local) = dirs::data_local_dir() {
        paths.push(local.join("FPilot").join("crash.log"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            paths.push(parent.join("fpilot-crash.log"));
        }
    }
    paths
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn unhandled_exception_filter(
    info: *const windows::Win32::System::Diagnostics::Debug::EXCEPTION_POINTERS,
) -> i32 {
    use windows::Win32::System::Diagnostics::Debug::EXCEPTION_CONTINUE_SEARCH;
    if !info.is_null() {
        let pointers = unsafe { &*info };
        if !pointers.ExceptionRecord.is_null() {
            let record = unsafe { &*pointers.ExceptionRecord };
            let address = record.ExceptionAddress as usize;
            let symbolic = std::backtrace::Backtrace::force_capture();
            write_crash_log(&format!(
                "unhandled exception code=0x{:08X} address=0x{:X} flags=0x{:X}\nmodule: {}\nsymbolic:\n{symbolic}\nstack:\n{}",
                record.ExceptionCode.0,
                address,
                record.ExceptionFlags,
                describe_module(address),
                capture_stack()
            ));
        }
    }
    EXCEPTION_CONTINUE_SEARCH
}


fn describe_module(address: usize) -> String {
    use windows::Win32::{
        Foundation::HMODULE,
        System::LibraryLoader::{
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            GetModuleFileNameW, GetModuleHandleExW,
        },
    };

    let mut module = HMODULE(std::ptr::null_mut());
    let ok = unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            windows::core::PCWSTR(address as *const u16),
            &mut module,
        )
        .is_ok()
    };
    if !ok || module.0.is_null() {
        return "unknown module".to_string();
    }
    let mut buffer = [0u16; 260];
    let length = unsafe { GetModuleFileNameW(Some(module), &mut buffer) } as usize;
    let name = String::from_utf16_lossy(&buffer[..length.min(buffer.len())]);
    format!("{name}")
}

fn capture_stack() -> String {
    use windows::Win32::System::Diagnostics::Debug::RtlCaptureStackBackTrace;

    const MAX_FRAMES: usize = 24;
    let mut frames: [*mut core::ffi::c_void; MAX_FRAMES] = [std::ptr::null_mut(); MAX_FRAMES];
    let captured = unsafe { RtlCaptureStackBackTrace(0, &mut frames, None) } as usize;
    let mut text = String::new();
    for frame in frames.iter().take(captured) {
        text.push_str(&format!(
            "  0x{:016X}  {}\n",
            *frame as usize,
            describe_module(*frame as usize)
        ));
    }
    if text.is_empty() {
        text.push_str("  (no frames)\n");
    }
    text
}

fn install_crash_logger() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        write_crash_log(&format!("panic: {info}\n{backtrace}"));
        default_hook(info);
    }));

    #[cfg(target_os = "windows")]
    unsafe {
        use windows::Win32::System::Diagnostics::Debug::SetUnhandledExceptionFilter;
        let _ = SetUnhandledExceptionFilter(Some(unhandled_exception_filter));
    }
}

fn main() {
    env_logger::init();
    install_crash_logger();
    Application::new()
        .with_assets(assets::Assets)
        .run(|cx: &mut App| {
            bind_keys(cx);

            cx.on_window_closed(|cx| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            let bounds = Bounds::centered(None, size(px(1440.), px(900.)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: None,
                window_decorations: Some(gpui::WindowDecorations::Client),
                window_min_size: Some(size(px(1120.), px(680.))),
                focus: true,
                show: true,
                is_resizable: true,
                is_movable: true,
                ..Default::default()
            };

            cx.open_window(options, |window, cx| {
                window.set_window_title("FPilot");
                {
                    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
                    if let Ok(handle) = <gpui::Window as HasWindowHandle>::window_handle(window) {
                        if let RawWindowHandle::Win32(raw) = handle.as_raw() {
                            drag_host::install(windows::Win32::Foundation::HWND(
                                raw.hwnd.get() as *mut _,
                            ));
                        }
                    }
                }
                let app = cx.new(|cx| PilotApp::new(cx));
                if let Some(path) = explorer::launch_path() {
                    app.update(cx, |app, cx| app.open_start_path(path, cx));
                }
                #[cfg(debug_assertions)]
                if let Some(port) = debug_port() {
                    let requests = debug_ipc::start(port);
                    app.update(cx, |app, cx| app.start_debug_worker(requests, cx));
                }
                app
            })
            .ok();
            cx.activate(true);
        });
}

#[cfg(debug_assertions)]
fn debug_port() -> Option<u16> {
    std::env::var("FPILOT_DEBUG_PORT")
        .ok()
        .and_then(|port| port.parse::<u16>().ok())
        .or_else(|| {
            std::env::args().find_map(|argument| {
                argument
                    .strip_prefix("--debug-port=")
                    .and_then(|port| port.parse::<u16>().ok())
            })
        })
}

fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("alt-left", Back, Some(FILE_MANAGER_CONTEXT)),
        KeyBinding::new("alt-right", Forward, Some(FILE_MANAGER_CONTEXT)),
        KeyBinding::new("alt-up", Up, Some(FILE_MANAGER_CONTEXT)),
        KeyBinding::new("backspace", Up, Some(FILE_MANAGER_CONTEXT)),
        KeyBinding::new("f5", Refresh, Some(FILE_MANAGER_CONTEXT)),
        KeyBinding::new("ctrl-r", Refresh, Some(FILE_MANAGER_CONTEXT)),
        KeyBinding::new("ctrl-l", FocusPath, Some(FILE_MANAGER_CONTEXT)),
        KeyBinding::new("ctrl-f", FocusSearch, Some(FILE_MANAGER_CONTEXT)),
        KeyBinding::new("ctrl-t", NewTab, Some(FILE_MANAGER_CONTEXT)),
        KeyBinding::new("ctrl-w", CloseTab, Some(FILE_MANAGER_CONTEXT)),
        KeyBinding::new("ctrl-tab", NextTab, Some(FILE_MANAGER_CONTEXT)),
        KeyBinding::new("ctrl-shift-tab", PrevTab, Some(FILE_MANAGER_CONTEXT)),
        KeyBinding::new("enter", OpenSelected, Some(FILE_MANAGER_CONTEXT)),
        KeyBinding::new("ctrl-enter", OpenSelectedNewTab, Some(FILE_MANAGER_CONTEXT)),
        KeyBinding::new("ctrl-c", CopyFiles, Some(FILE_MANAGER_CONTEXT)),
        KeyBinding::new("ctrl-x", CutFiles, Some(FILE_MANAGER_CONTEXT)),
        KeyBinding::new("ctrl-v", PasteFiles, Some(FILE_MANAGER_CONTEXT)),
        KeyBinding::new("ctrl-z", Undo, Some(FILE_MANAGER_CONTEXT)),
        KeyBinding::new("delete", TrashSelection, Some(FILE_MANAGER_CONTEXT)),
        KeyBinding::new(
            "shift-delete",
            PermanentDelete,
            Some(FILE_MANAGER_CONTEXT),
        ),
        KeyBinding::new("f2", RenameSelection, Some(FILE_MANAGER_CONTEXT)),
        KeyBinding::new(
            "ctrl-shift-n",
            NewFolder,
            Some(FILE_MANAGER_CONTEXT),
        ),
        KeyBinding::new("ctrl-alt-n", NewFile, Some(FILE_MANAGER_CONTEXT)),
        KeyBinding::new("ctrl-a", SelectAll, Some(FILE_MANAGER_CONTEXT)),
        KeyBinding::new("up", SelectPrev, Some(FILE_MANAGER_CONTEXT)),
        KeyBinding::new("down", SelectNext, Some(FILE_MANAGER_CONTEXT)),
        KeyBinding::new(
            "shift-up",
            SelectExtendPrev,
            Some(FILE_MANAGER_CONTEXT),
        ),
        KeyBinding::new(
            "shift-down",
            SelectExtendNext,
            Some(FILE_MANAGER_CONTEXT),
        ),
        KeyBinding::new("home", SelectFirst, Some(FILE_MANAGER_CONTEXT)),
        KeyBinding::new("end", SelectLast, Some(FILE_MANAGER_CONTEXT)),
        KeyBinding::new(
            "shift-home",
            SelectExtendFirst,
            Some(FILE_MANAGER_CONTEXT),
        ),
        KeyBinding::new("shift-end", SelectExtendLast, Some(FILE_MANAGER_CONTEXT)),
        KeyBinding::new("ctrl-1", ViewDetails, Some(FILE_MANAGER_CONTEXT)),
        KeyBinding::new("ctrl-2", ViewGrid, Some(FILE_MANAGER_CONTEXT)),
        KeyBinding::new("ctrl-3", ViewSplit, Some(FILE_MANAGER_CONTEXT)),
        KeyBinding::new("ctrl-b", ToggleSidebar, Some(FILE_MANAGER_CONTEXT)),
        KeyBinding::new("ctrl-shift-h", ToggleHidden, Some(FILE_MANAGER_CONTEXT)),
        KeyBinding::new("ctrl-shift-c", CopyPath, Some(FILE_MANAGER_CONTEXT)),
        KeyBinding::new(
            "ctrl-shift-e",
            RevealSelection,
            Some(FILE_MANAGER_CONTEXT),
        ),
        KeyBinding::new("escape", Cancel, Some(FILE_MANAGER_CONTEXT)),
    ]);

    // Text-editor bindings are context-scoped, so Backspace / Enter / Escape
    // do not leak into the file list while an editor is focused.
    let editor = Some("TextInput");
    cx.bind_keys([
        KeyBinding::new("backspace", EditorBackspace, editor),
        KeyBinding::new("delete", EditorDelete, editor),
        KeyBinding::new("left", EditorLeft, editor),
        KeyBinding::new("right", EditorRight, editor),
        KeyBinding::new("shift-left", EditorSelectLeft, editor),
        KeyBinding::new("shift-right", EditorSelectRight, editor),
        KeyBinding::new("ctrl-a", EditorSelectAll, editor),
        KeyBinding::new("home", EditorHome, editor),
        KeyBinding::new("end", EditorEnd, editor),
        KeyBinding::new("ctrl-c", EditorCopy, editor),
        KeyBinding::new("ctrl-x", EditorCut, editor),
        KeyBinding::new("ctrl-v", EditorPaste, editor),
        KeyBinding::new("enter", EditorSubmit, editor),
        KeyBinding::new("escape", EditorCancel, editor),
    ]);
}
