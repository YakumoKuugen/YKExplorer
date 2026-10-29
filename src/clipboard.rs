use std::{
    iter::once,
    os::windows::ffi::OsStrExt,
    path::PathBuf,
    ptr,
    thread::sleep,
    time::Duration,
};

use windows::{
    Win32::{
        Foundation::{GlobalFree, HGLOBAL, POINT},
        System::{
            DataExchange::{
                CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard,
                RegisterClipboardFormatW, SetClipboardData,
            },
            Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock},
            Ole::{CF_HDROP, CF_UNICODETEXT, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_MOVE},
        },
        UI::Shell::{CFSTR_PREFERREDDROPEFFECT, DROPFILES, DragQueryFileW, HDROP},
    },
    core::BOOL,
};

#[derive(Debug, Clone)]
pub struct ClipboardFiles {
    pub paths: Vec<PathBuf>,
    pub cut: bool,
}

struct ClipboardGuard;

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

fn open_clipboard() -> Result<ClipboardGuard, String> {
    for attempt in 0..8 {
        match unsafe { OpenClipboard(None) } {
            Ok(()) => return Ok(ClipboardGuard),
            Err(err) => {
                if attempt == 7 {
                    return Err(format!("剪贴板正被其他程序使用：{err}"));
                }
                sleep(Duration::from_millis(15));
            }
        }
    }
    Err("无法打开剪贴板".into())
}

fn preferred_drop_effect_format() -> u32 {
    unsafe { RegisterClipboardFormatW(CFSTR_PREFERREDDROPEFFECT) }
}

fn alloc_hglobal(bytes: &[u8]) -> Result<HGLOBAL, String> {
    unsafe {
        let handle = GlobalAlloc(GMEM_MOVEABLE, bytes.len())
            .map_err(|e| format!("无法分配剪贴板内存：{e}"))?;
        let target = GlobalLock(handle);
        if target.is_null() {
            let _ = GlobalFree(Some(handle));
            return Err("无法锁定剪贴板内存".into());
        }
        ptr::copy_nonoverlapping(bytes.as_ptr(), target as *mut u8, bytes.len());
        let _ = GlobalUnlock(handle);
        Ok(handle)
    }
}

fn put_hglobal(format: u32, handle: HGLOBAL) -> Result<(), String> {
    match unsafe { SetClipboardData(format, Some(windows::Win32::Foundation::HANDLE(handle.0))) } {
        Ok(_) => Ok(()),
        Err(err) => {
            unsafe {
                let _ = GlobalFree(Some(handle));
            }
            Err(format!("写入剪贴板失败：{err}"))
        }
    }
}

pub fn write_files(paths: &[PathBuf], cut: bool) -> Result<(), String> {
    if paths.is_empty() {
        return Err("没有可复制的项目".into());
    }

    let mut names = Vec::<u16>::new();
    for path in paths {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(once(0)).collect();
        names.extend(wide);
    }
    names.push(0);

    let header_size = std::mem::size_of::<DROPFILES>();
    let byte_size = header_size + names.len() * 2;
    let mut payload = vec![0u8; byte_size];
    unsafe {
        let header = payload.as_mut_ptr() as *mut DROPFILES;
        ptr::write(
            header,
            DROPFILES {
                pFiles: header_size as u32,
                pt: POINT { x: 0, y: 0 },
                fNC: BOOL(0),
                fWide: BOOL(1),
            },
        );
        ptr::copy_nonoverlapping(
            names.as_ptr() as *const u8,
            payload.as_mut_ptr().add(header_size),
            names.len() * 2,
        );
    }

    let effect = if cut { DROPEFFECT_MOVE } else { DROPEFFECT_COPY };
    let effect_bytes = effect.0.to_ne_bytes();

    let _guard = open_clipboard()?;
    unsafe {
        EmptyClipboard().map_err(|e| format!("清空剪贴板失败：{e}"))?;
    }
    let hdrop = alloc_hglobal(&payload)?;
    put_hglobal(CF_HDROP.0 as u32, hdrop)?;

    let effect_handle = alloc_hglobal(&effect_bytes)?;
    put_hglobal(preferred_drop_effect_format(), effect_handle)?;
    Ok(())
}

pub fn write_text(text: &str) -> Result<(), String> {
    let wide: Vec<u16> = text.encode_utf16().chain(once(0)).collect();
    let bytes = unsafe {
        std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * std::mem::size_of::<u16>())
    };
    let handle = alloc_hglobal(bytes)?;
    let _guard = open_clipboard()?;
    unsafe {
        EmptyClipboard().map_err(|e| format!("清空剪贴板失败：{e}"))?;
    }
    put_hglobal(CF_UNICODETEXT.0 as u32, handle)?;
    Ok(())
}

pub fn read_files() -> Option<ClipboardFiles> {
    let _guard = open_clipboard().ok()?;
    let handle = unsafe { GetClipboardData(CF_HDROP.0 as u32) }.ok()?;
    let hdrop = HDROP(handle.0);
    let count = unsafe { DragQueryFileW(hdrop, u32::MAX, None) };
    if count == 0 {
        return None;
    }
    let mut paths = Vec::with_capacity(count as usize);
    for index in 0..count {
        let len = unsafe { DragQueryFileW(hdrop, index, None) };
        if len == 0 {
            continue;
        }
        let mut buffer = vec![0u16; len as usize + 1];
        let written = unsafe { DragQueryFileW(hdrop, index, Some(&mut buffer)) };
        if written == 0 {
            continue;
        }
        let len = buffer.iter().position(|&c| c == 0).unwrap_or(written as usize);
        let path = String::from_utf16_lossy(&buffer[..len]);
        paths.push(PathBuf::from(path));
    }
    if paths.is_empty() {
        return None;
    }
    Some(ClipboardFiles {
        paths,
        cut: read_preferred_is_move(),
    })
}

fn read_preferred_is_move() -> bool {
    unsafe {
        let format = preferred_drop_effect_format();
        if format == 0 {
            return false;
        }
        let Ok(handle) = GetClipboardData(format) else {
            return false;
        };
        let ptr = GlobalLock(HGLOBAL(handle.0));
        if ptr.is_null() {
            return false;
        }
        let effect = (ptr as *const DROPEFFECT).read_unaligned();
        let _ = GlobalUnlock(HGLOBAL(handle.0));
        (effect.0 & DROPEFFECT_MOVE.0) != 0
    }
}

pub fn clear_file_clipboard_if_matches(expected: &[PathBuf]) -> bool {
    let Some(current) = read_files() else {
        return false;
    };
    if current.paths != expected {
        return false;
    }
    let Ok(_guard) = open_clipboard() else {
        return false;
    };
    unsafe {
        let _ = EmptyClipboard();
    }
    true
}

#[allow(dead_code)]
pub fn text_of(effect: DROPEFFECT) -> &'static str {
    if effect == DROPEFFECT_MOVE {
        "移动"
    } else {
        "复制"
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scratch_dir() -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-scratch")
            .join(format!(
                "clip-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    #[ignore = "requires an interactive Windows desktop/clipboard"]
    fn file_list_round_trips_through_clipboard() {
        let dir = scratch_dir();
        let a = dir.join("中文 A.txt");
        let b = dir.join("second.txt");
        std::fs::write(&a, b"a").unwrap();
        std::fs::write(&b, b"b").unwrap();

        write_files(&[a.clone(), b.clone()], true).expect("write clipboard files");
        let read = read_files().expect("read clipboard files");
        assert_eq!(read.paths, vec![a, b]);
        assert!(read.cut);

        write_files(&[dir.join("中文 A.txt")], false).expect("write clipboard copy");
        let read = read_files().expect("read clipboard copy");
        assert_eq!(read.paths.len(), 1);
        assert!(!read.cut);

        std::fs::remove_dir_all(dir).ok();
    }
}
