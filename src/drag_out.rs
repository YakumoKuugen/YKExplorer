//! Native Windows OLE drag-out.
//!
//! GPUI 0.2.2 handles file *drops into* a window, but it does not implement an
//! OLE drag source. This module builds a minimal `CF_HDROP` data object and a
//! drop source and calls `DoDragDrop`, which is what allows dragging from
//! FPilot into Explorer, the desktop, chats, browsers and other applications.

use std::{
    ffi::c_void,
    iter::once,
    mem::ManuallyDrop,
    os::windows::ffi::OsStrExt,
    path::PathBuf,
    ptr,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
};

use windows::{
    Win32::{
        Foundation::{
            DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, DV_E_FORMATETC,
            E_INVALIDARG, E_NOTIMPL, GlobalFree, HGLOBAL, OLE_E_ADVISENOTSUPPORTED, S_OK,
        },
        System::{
            Com::{
                DATADIR_GET, DVASPECT_CONTENT, FORMATETC, IAdviseSink, IDataObject,
                IDataObject_Impl, IEnumFORMATETC, STGMEDIUM, STGMEDIUM_0, TYMED_HGLOBAL,
                CoTaskMemFree,
            },
            Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock},
            Ole::{
                CF_HDROP, CF_UNICODETEXT, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_MOVE, DoDragDrop,
                IDropSource, IDropSource_Impl, OleInitialize,
            },
            SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS},
        },
        UI::{
            Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_LBUTTON, VK_SHIFT},
            Shell::{
                BHID_DataObject, CFSTR_PREFERREDDROPEFFECT, DROPFILES, IShellItemArray,
                SHCreateShellItemArrayFromIDLists, SHCreateStdEnumFmtEtc, SHParseDisplayName,
                Common::ITEMIDLIST,
            },
            WindowsAndMessaging::GetCursorPos,
        },
    },
    core::{BOOL, Error, HRESULT, Result as WinResult, implement},
};

const DATA_S_SAMEFORMATETC: HRESULT = HRESULT(0x00040130u32 as i32);

thread_local! {
    static OLE_INITIALIZED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn ensure_ole() {
    OLE_INITIALIZED.with(|initialized| {
        if !initialized.get() {
            unsafe {
                let _ = OleInitialize(None);
            }
            initialized.set(true);
        }
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragDropResult {
    Dropped,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Copy)]
pub struct DragOutcome {
    pub result: DragDropResult,
    pub effect: DROPEFFECT,
}

fn shift_held() -> bool {
    unsafe { (GetAsyncKeyState(VK_SHIFT.0 as i32) as u16 & 0x8000) != 0 }
}

/// Current physical screen cursor position. Safe to call while the UI is idle.
pub fn cursor_position() -> Option<(i32, i32)> {
    unsafe {
        let mut point = windows::Win32::Foundation::POINT::default();
        GetCursorPos(&mut point).ok()?;
        Some((point.x, point.y))
    }
}

/// Whether the physical left mouse button is currently held down.
pub fn left_button_down() -> bool {
    unsafe { (GetAsyncKeyState(VK_LBUTTON.0 as i32) as u16 & 0x8000) != 0 }
}

/// Shift state for the drag effect choice.
pub fn control_down() -> bool {
    unsafe { (GetAsyncKeyState(VK_CONTROL.0 as i32) as u16 & 0x8000) != 0 }
}

#[derive(Debug)]
struct DropFormats {
    preferred_effect: u16,
    unicode_text: u16,
    hdrop: u16,
}

impl DropFormats {
    fn new() -> Self {
        let preferred = unsafe { windows::Win32::System::DataExchange::RegisterClipboardFormatW(CFSTR_PREFERREDDROPEFFECT) };
        Self {
            preferred_effect: preferred as u16,
            unicode_text: CF_UNICODETEXT.0,
            hdrop: CF_HDROP.0,
        }
    }

    fn format_etc(&self, cf: u16) -> FORMATETC {
        FORMATETC {
            cfFormat: cf,
            ptd: ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        }
    }

    fn all(&self) -> Vec<FORMATETC> {
        vec![
            self.format_etc(self.hdrop),
            self.format_etc(self.preferred_effect),
            self.format_etc(self.unicode_text),
        ]
    }
}

#[implement(IDataObject)]
struct FileDataObject {
    paths: Vec<PathBuf>,
    formats: DropFormats,
    preferred_effect: Arc<AtomicU32>,
}

impl FileDataObject {
    fn new(paths: Vec<PathBuf>, preferred_effect: Arc<AtomicU32>) -> Self {
        Self {
            paths,
            formats: DropFormats::new(),
            preferred_effect,
        }
    }

    fn is_supported(&self, format: *const FORMATETC) -> bool {
        if format.is_null() {
            return false;
        }
        unsafe {
            let format = &*format;
            let tymed_ok = format.tymed == 0
                || (format.tymed & TYMED_HGLOBAL.0 as u32) != 0;
            let aspect_ok =
                format.dwAspect == DVASPECT_CONTENT.0 || format.dwAspect == 0;
            aspect_ok
                && tymed_ok
                && (format.cfFormat == self.formats.hdrop
                    || format.cfFormat == self.formats.preferred_effect
                    || format.cfFormat == self.formats.unicode_text)
        }
    }

    fn log_format(prefix: &str, pformatetc: *const FORMATETC) {
        if pformatetc.is_null() {
            log::info!("{prefix}: null FORMATETC");
            return;
        }
        let format = unsafe { &*pformatetc };
        log::info!(
            "{prefix}: cf={} tymed=0x{:X} aspect={} lindex={} ptd={:?}",
            format.cfFormat,
            format.tymed,
            format.dwAspect,
            format.lindex,
            format.ptd
        );
    }

    fn hdrop_medium(&self) -> WinResult<STGMEDIUM> {
        let mut wide_paths = Vec::<u16>::new();
        for path in &self.paths {
            wide_paths.extend(path.as_os_str().encode_wide().chain(once(0)));
        }
        wide_paths.push(0);
        let header_size = std::mem::size_of::<DROPFILES>();
        let total = header_size + wide_paths.len() * 2;
        let hglobal = global_from_bytes(total, |data| unsafe {
            let header = data as *mut DROPFILES;
            header.write(DROPFILES {
                pFiles: header_size as u32,
                pt: windows::Win32::Foundation::POINT { x: 0, y: 0 },
                fNC: BOOL(0),
                fWide: BOOL(1),
            });
            ptr::copy_nonoverlapping(
                wide_paths.as_ptr() as *const u8,
                (data as *mut u8).add(header_size),
                wide_paths.len() * 2,
            );
        })?;
        Ok(hglobal_medium(hglobal))
    }

    fn preferred_medium(&self) -> WinResult<STGMEDIUM> {
        let bytes = self.preferred_effect.load(Ordering::Relaxed).to_le_bytes();
        let hglobal = global_from_bytes(4, |data| unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr(), data as *mut u8, bytes.len());
        })?;
        Ok(hglobal_medium(hglobal))
    }

    fn text_medium(&self) -> WinResult<STGMEDIUM> {
        let text = self
            .paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("\r\n");
        let mut wide: Vec<u16> = text.encode_utf16().collect();
        wide.push(0);
        let bytes = wide.len() * 2;
        let hglobal = global_from_bytes(bytes, |data| unsafe {
            ptr::copy_nonoverlapping(wide.as_ptr() as *const u8, data as *mut u8, bytes);
        })?;
        Ok(hglobal_medium(hglobal))
    }
}

fn hglobal_medium(handle: HGLOBAL) -> STGMEDIUM {
    STGMEDIUM {
        tymed: TYMED_HGLOBAL.0 as u32,
        u: STGMEDIUM_0 { hGlobal: handle },
        pUnkForRelease: ManuallyDrop::new(None),
    }
}

fn global_from_bytes(
    size: usize,
    fill: impl FnOnce(*mut c_void),
) -> WinResult<HGLOBAL> {
    unsafe {
        let handle = GlobalAlloc(GMEM_MOVEABLE, size)?;
        let data = GlobalLock(handle);
        if data.is_null() {
            let _ = GlobalFree(Some(handle));
            return Err(Error::from_win32());
        }
        fill(data);
        let _ = GlobalUnlock(handle);
        Ok(handle)
    }
}

#[allow(non_snake_case)]
impl IDataObject_Impl for FileDataObject_Impl {
    fn GetData(&self, pformatetc: *const FORMATETC) -> WinResult<STGMEDIUM> {
        FileDataObject::log_format("IDataObject::GetData", pformatetc);
        if !self.is_supported(pformatetc) {
            return Err(Error::new(DV_E_FORMATETC, "unsupported format"));
        }
        let format = unsafe { (*pformatetc).cfFormat };
        if format == self.formats.hdrop {
            self.hdrop_medium()
        } else if format == self.formats.preferred_effect {
            self.preferred_medium()
        } else {
            self.text_medium()
        }
    }

    fn GetDataHere(&self, _pformatetc: *const FORMATETC, _pmedium: *mut STGMEDIUM) -> WinResult<()> {
        Err(Error::new(E_NOTIMPL, ""))
    }

    fn QueryGetData(&self, pformatetc: *const FORMATETC) -> HRESULT {
        FileDataObject::log_format("IDataObject::QueryGetData", pformatetc);
        if self.is_supported(pformatetc) {
            S_OK
        } else {
            DV_E_FORMATETC
        }
    }

    fn GetCanonicalFormatEtc(
        &self,
        pformatectin: *const FORMATETC,
        pformatetcout: *mut FORMATETC,
    ) -> HRESULT {
        FileDataObject::log_format("IDataObject::GetCanonicalFormatEtc", pformatectin);
        // Returning E_NOTIMPL here makes some shell drop targets treat the
        // whole data object as unusable. Report the canonical form instead.
        if pformatetcout.is_null() {
            return E_INVALIDARG;
        }
        unsafe {
            (*pformatetcout).ptd = ptr::null_mut();
        }
        DATA_S_SAMEFORMATETC
    }

    fn SetData(
        &self,
        _pformatetc: *const FORMATETC,
        _pmedium: *const STGMEDIUM,
        _frelease: BOOL,
    ) -> WinResult<()> {
        Err(Error::new(E_NOTIMPL, ""))
    }

    fn EnumFormatEtc(&self, dwdirection: u32) -> WinResult<IEnumFORMATETC> {
        log::info!("IDataObject::EnumFormatEtc(direction={dwdirection})");
        if dwdirection != DATADIR_GET.0 as u32 {
            return Err(Error::new(E_NOTIMPL, ""));
        }
        unsafe { SHCreateStdEnumFmtEtc(&self.formats.all()) }
    }

    fn DAdvise(
        &self,
        _pformatetc: *const FORMATETC,
        _advf: u32,
        _padvsink: windows::core::Ref<'_, IAdviseSink>,
    ) -> WinResult<u32> {
        Err(Error::new(
            OLE_E_ADVISENOTSUPPORTED,
            "",
        ))
    }

    fn DUnadvise(&self, _dwconnection: u32) -> WinResult<()> {
        Err(Error::new(
            OLE_E_ADVISENOTSUPPORTED,
            "",
        ))
    }

    fn EnumDAdvise(&self) -> WinResult<windows::Win32::System::Com::IEnumSTATDATA> {
        Err(Error::new(
            OLE_E_ADVISENOTSUPPORTED,
            "",
        ))
    }
}

#[implement(IDropSource)]
struct FileDropSource {
    preferred_effect: Arc<AtomicU32>,
    source_hwnd: isize,
    left_source: std::sync::atomic::AtomicBool,
}

#[allow(non_snake_case)]
impl IDropSource_Impl for FileDropSource_Impl {
    fn QueryContinueDrag(&self, escape_pressed: BOOL, key_state: MODIFIERKEYS_FLAGS) -> HRESULT {
        static LAST_KEY_STATE: AtomicU32 = AtomicU32::new(u32::MAX);
        let previous = LAST_KEY_STATE.swap(key_state.0, Ordering::Relaxed);
        if previous != key_state.0 || escape_pressed.as_bool() {
            log::info!(
                "QueryContinueDrag: escape={} key_state=0x{:X}",
                escape_pressed.as_bool(),
                key_state.0
            );
        }
        // Dropping back onto our own window crashes inside the shell storage
        // component when a shell data object is used. Once the drag has left
        // the source window, cancel instead of re-entering it.
        let over_source = unsafe {
            use windows::Win32::{
                Foundation::POINT,
                UI::WindowsAndMessaging::{
                    GA_ROOT, GetAncestor, GetCursorPos, WindowFromPoint,
                },
            };
            let mut point = POINT::default();
            let _ = GetCursorPos(&mut point);
            let hwnd = WindowFromPoint(point);
            let root = GetAncestor(hwnd, GA_ROOT);
            root.0 as isize == self.source_hwnd
        };
        if over_source {
            if self.left_source.load(Ordering::Relaxed) {
                log::info!("drag re-entered source window; cancelling");
                return DRAGDROP_S_CANCEL;
            }
        } else {
            self.left_source.store(true, Ordering::Relaxed);
        }
        if escape_pressed.as_bool() {
            return DRAGDROP_S_CANCEL;
        }
        // Keep the preferred effect live while the user presses/releases Shift.
        let effect = if shift_held() {
            DROPEFFECT_COPY.0
        } else {
            DROPEFFECT_MOVE.0
        };
        self.preferred_effect.store(effect, Ordering::Relaxed);

        if (key_state & MK_LBUTTON) == MODIFIERKEYS_FLAGS(0) {
            DRAGDROP_S_DROP
        } else {
            S_OK
        }
    }

    fn GiveFeedback(&self, effect: DROPEFFECT) -> HRESULT {
        static LAST_EFFECT: AtomicU32 = AtomicU32::new(u32::MAX);
        let previous = LAST_EFFECT.swap(effect.0, Ordering::Relaxed);
        if previous != effect.0 {
            unsafe {
                use windows::Win32::{
                    Foundation::POINT,
                    UI::WindowsAndMessaging::{
                        GA_ROOT, GetAncestor, GetClassNameW, GetCursorPos, GetWindowTextW,
                        WindowFromPoint,
                    },
                };
                let mut point = POINT::default();
                let _ = GetCursorPos(&mut point);
                let hwnd = WindowFromPoint(point);
                let root = GetAncestor(hwnd, GA_ROOT);
                let mut class = [0u16; 128];
                let mut title = [0u16; 256];
                let mut root_class = [0u16; 128];
                let mut root_title = [0u16; 256];
                let class_len = GetClassNameW(hwnd, &mut class) as usize;
                let title_len = GetWindowTextW(hwnd, &mut title) as usize;
                let root_class_len = GetClassNameW(root, &mut root_class) as usize;
                let root_title_len = GetWindowTextW(root, &mut root_title) as usize;
                log::info!(
                    "GiveFeedback: effect=0x{:X} cursor={},{} target={} [{}] root={} [{}]",
                    effect.0,
                    point.x,
                    point.y,
                    String::from_utf16_lossy(&class[..class_len]),
                    String::from_utf16_lossy(&title[..title_len]),
                    String::from_utf16_lossy(&root_class[..root_class_len]),
                    String::from_utf16_lossy(&root_title[..root_title_len])
                );
            }
        }
        let preferred = if shift_held() {
            DROPEFFECT_COPY.0
        } else {
            DROPEFFECT_MOVE.0
        };
        self.preferred_effect.store(preferred, Ordering::Relaxed);
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

/// Build a standard shell `IDataObject` for the paths. Unlike the small custom
/// object below, this object is designed by Windows for cross-process shell
/// drag/drop and exposes the usual CF_HDROP / Shell IDList formats.
/// Start a native file drag. `copy_if_shift` should be the shift state at the
/// moment the drag threshold was crossed. Holding Shift limits the operation
/// to copy, which is a deliberate FPilot choice (Windows itself uses Shift to
/// force a move in some shell contexts).
struct ShellDragData {
    data: IDataObject,
    _array: IShellItemArray,
    pidls: Vec<*const ITEMIDLIST>,
}

impl Drop for ShellDragData {
    fn drop(&mut self) {
        for pidl in self.pidls.drain(..) {
            unsafe {
                CoTaskMemFree(Some(pidl as *const c_void));
            }
        }
    }
}

fn shell_data_object(paths: &[PathBuf]) -> WinResult<ShellDragData> {
    use windows::core::HSTRING;

    let mut pidls: Vec<*const ITEMIDLIST> = Vec::with_capacity(paths.len());
    for path in paths {
        let name = HSTRING::from(path.to_string_lossy().into_owned());
        let mut pidl: *mut ITEMIDLIST = ptr::null_mut();
        unsafe {
            SHParseDisplayName(&name, None, &mut pidl, 0, None)?;
        }
        if !pidl.is_null() {
            pidls.push(pidl);
        }
    }
    if pidls.is_empty() {
        return Err(Error::from_win32());
    }
    let array = match unsafe { SHCreateShellItemArrayFromIDLists(&pidls) } {
        Ok(array) => array,
        Err(error) => {
            for pidl in pidls.drain(..) {
                unsafe {
                    CoTaskMemFree(Some(pidl as *const c_void));
                }
            }
            return Err(error.into());
        }
    };
    let data = unsafe { array.BindToHandler(None, &BHID_DataObject) }?;
    Ok(ShellDragData { data, _array: array, pidls })
}

pub fn start_file_drag(
    paths: &[PathBuf],
    copy_if_shift: bool,
    source_hwnd: isize,
) -> Result<DragOutcome, String> {
    if paths.is_empty() {
        return Err("没有可拖动的项目".into());
    }
    ensure_ole();

    let mut canonical = Vec::with_capacity(paths.len());
    for path in paths {
        canonical.push(dunce::canonicalize(path).unwrap_or_else(|_| path.clone()));
    }

    let initial_preferred = if copy_if_shift {
        DROPEFFECT_COPY
    } else {
        DROPEFFECT_MOVE
    };
    let preferred = Arc::new(AtomicU32::new(initial_preferred.0));
    let shell = match shell_data_object(&canonical) {
        Ok(shell) => {
            log::info!("using shell CF_HDROP data object");
            Some(shell)
        }
        Err(error) => {
            log::warn!("shell data object failed: {error}; using custom object");
            None
        }
    };
    let data: IDataObject = match &shell {
        Some(shell) => shell.data.clone(),
        None => FileDataObject::new(canonical, preferred.clone()).into(),
    };
    let source: IDropSource = FileDropSource {
        preferred_effect: preferred,
        source_hwnd,
        left_source: std::sync::atomic::AtomicBool::new(false),
    }
    .into();

    let allowed = if copy_if_shift {
        DROPEFFECT_COPY
    } else {
        DROPEFFECT(DROPEFFECT_MOVE.0 | DROPEFFECT_COPY.0)
    };
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::{MSG, PM_NOREMOVE, PeekMessageW};
        let mut message = MSG::default();
        let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
    }
    let mut effect = DROPEFFECT(0);
    log::info!("DoDragDrop: allowed=0x{:X}", allowed.0);
    let result = unsafe { DoDragDrop(&data, &source, allowed, &mut effect) };
    log::info!("DoDragDrop returned hr=0x{:08X} effect=0x{:X}", result.0, effect.0);
    drop(shell);
    let result = if result == DRAGDROP_S_DROP {
        DragDropResult::Dropped
    } else if result == DRAGDROP_S_CANCEL {
        DragDropResult::Cancelled
    } else {
        DragDropResult::Failed
    };
    Ok(DragOutcome { result, effect })
}

/// Human readable description of the effect a target accepted.
pub fn effect_name(effect: DROPEFFECT) -> &'static str {
    if (effect.0 & DROPEFFECT_MOVE.0) != 0 && (effect.0 & DROPEFFECT_COPY.0) == 0 {
        "移动"
    } else if (effect.0 & DROPEFFECT_COPY.0) != 0 && (effect.0 & DROPEFFECT_MOVE.0) == 0 {
        "复制"
    } else if effect.0 == 0 {
        "无操作"
    } else {
        "复制/移动"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effect_names_are_sensible() {
        assert_eq!(effect_name(DROPEFFECT_MOVE), "移动");
        assert_eq!(effect_name(DROPEFFECT_COPY), "复制");
    }

    #[test]
    fn hdrop_payload_contains_utf16_paths_and_header() {
        let data = FileDataObject::new(
            vec![PathBuf::from(r"C:\中文目录\示例文件.txt")],
            Arc::new(AtomicU32::new(DROPEFFECT_MOVE.0)),
        );
        let medium = data.hdrop_medium().expect("hdrop");
        assert_eq!(medium.tymed, TYMED_HGLOBAL.0 as u32);
        unsafe {
            let handle = medium.u.hGlobal;
            let raw = GlobalLock(handle) as *const u8;
            assert!(!raw.is_null());
            let header = raw as *const DROPFILES;
            let p_files = std::ptr::addr_of!((*header).pFiles).read_unaligned();
            let f_wide = std::ptr::addr_of!((*header).fWide).read_unaligned();
            assert_eq!(p_files, std::mem::size_of::<DROPFILES>() as u32);
            assert!(f_wide.as_bool());
            let name_ptr = raw.add(p_files as usize) as *const u16;
            let mut len = 0usize;
            while *name_ptr.add(len) != 0 {
                len += 1;
            }
            let name = String::from_utf16_lossy(std::slice::from_raw_parts(name_ptr, len));
            assert_eq!(name, r"C:\中文目录\示例文件.txt");
            let _ = GlobalUnlock(handle);
            let _ = GlobalFree(Some(handle));
        }
    }

    #[test]
    fn preferred_effect_payload_is_a_dword() {
        let data = FileDataObject::new(
            vec![PathBuf::from(r"C:\a.txt")],
            Arc::new(AtomicU32::new(DROPEFFECT_COPY.0)),
        );
        let medium = data.preferred_medium().expect("preferred");
        unsafe {
            let handle = medium.u.hGlobal;
            let raw = GlobalLock(handle) as *const u8;
            assert_eq!((raw as *const u32).read_unaligned(), DROPEFFECT_COPY.0);
            let _ = GlobalUnlock(handle);
            let _ = GlobalFree(Some(handle));
        }
    }


    #[test]
    fn com_idataobject_exposes_hdrop_and_effect() {
        let paths = vec![PathBuf::from(r"C:\temp\中文.txt")];
        let preferred = Arc::new(AtomicU32::new(DROPEFFECT_COPY.0));
        let object: IDataObject =
            FileDataObject::new(paths.clone(), preferred).into();

        let hdrop_format = FORMATETC {
            cfFormat: CF_HDROP.0,
            ptd: ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        };
        unsafe {
            assert_eq!(object.QueryGetData(&hdrop_format), S_OK);
            let medium = object.GetData(&hdrop_format).expect("GetData CF_HDROP");
            assert_eq!(medium.tymed, TYMED_HGLOBAL.0 as u32);
            let handle = medium.u.hGlobal;
            let raw = GlobalLock(handle) as *const u8;
            let header = raw as *const DROPFILES;
            let p_files = std::ptr::addr_of!((*header).pFiles).read_unaligned();
            let wide = std::ptr::addr_of!((*header).fWide).read_unaligned();
            assert!(wide.as_bool());
            let name_ptr = raw.add(p_files as usize) as *const u16;
            let mut len = 0usize;
            while *name_ptr.add(len) != 0 {
                len += 1;
            }
            assert_eq!(
                String::from_utf16_lossy(std::slice::from_raw_parts(name_ptr, len)),
                r"C:\temp\中文.txt"
            );
            let _ = GlobalUnlock(handle);
            let _ = GlobalFree(Some(handle));
        }

        let effect_format = FORMATETC {
            cfFormat: FileDataObject::new(Vec::new(), Arc::new(AtomicU32::new(0)))
                .formats
                .preferred_effect,
            ptd: ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        };
        unsafe {
            let medium = object.GetData(&effect_format).expect("GetData effect");
            let handle = medium.u.hGlobal;
            let raw = GlobalLock(handle) as *const u8;
            assert_eq!((raw as *const u32).read_unaligned(), DROPEFFECT_COPY.0);
            let _ = GlobalUnlock(handle);
            let _ = GlobalFree(Some(handle));
        }
    }

    #[test]
    fn custom_data_object_implements_imarshal() {
        use windows::core::Interface;
        unsafe {
            let _ = OleInitialize(None);
        }
        let preferred = Arc::new(AtomicU32::new(DROPEFFECT_MOVE.0));
        let data: IDataObject =
            FileDataObject::new(vec![PathBuf::from(r"C:\Windows\notepad.exe")], preferred)
                .into();
        let marshal: windows::core::Result<windows::core::imp::IMarshal> =
            data.cast();
        assert!(
            marshal.is_ok(),
            "custom IDataObject should be agile / implement IMarshal (error {:?})",
            marshal.err()
        );
    }

    #[test]
    fn custom_data_object_can_be_marshaled_for_other_processes() {
        use windows::{
            Win32::System::{
                Com::{
                    IStream, MSHCTX_LOCAL, MSHLFLAGS_NORMAL,
                    Marshal::CoMarshalInterface,
                    StructuredStorage::CreateStreamOnHGlobal,
                },
                SystemServices::MK_LBUTTON,
            },
            core::Interface,
        };

        unsafe {
            let _ = OleInitialize(None);
        }
        let preferred = Arc::new(AtomicU32::new(DROPEFFECT_MOVE.0));
        let data: IDataObject =
            FileDataObject::new(vec![PathBuf::from(r"C:\Windows\notepad.exe")], preferred)
                .into();
        unsafe {
            let stream: IStream =
                CreateStreamOnHGlobal(HGLOBAL(std::ptr::null_mut()), true).expect("memory stream");
            let unknown: windows::core::IUnknown = data.cast().expect("IUnknown");
            let _ = &unknown;
            let result = CoMarshalInterface(
                &stream,
                &IDataObject::IID,
                &unknown,
                MSHCTX_LOCAL.0 as u32,
                None,
                MSHLFLAGS_NORMAL.0 as u32,
            );
            assert!(
                result.is_ok(),
                "custom IDataObject must marshal across processes: {result:?}"
            );
        }
        let _ = MK_LBUTTON;
    }


}
