//! Enumerate context-menu verbs registered in the Windows registry.
//!
//! Explorer builds its "classic" context menu from `shell` subkeys below
//! `HKCU/HKLM\Software\Classes\...`. The same entries can be shown in YKExplorer
//! and invoked through `ShellExecuteExW` with the verb name.

use std::path::{Path, PathBuf};

use windows::{
    Win32::{
        Foundation::{ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS},
        System::Registry::{
            HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, REG_EXPAND_SZ, REG_SZ,
            REG_VALUE_TYPE, RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW,
        },
        UI::Shell::SHLoadIndirectString,
    },
    core::{PCWSTR, PWSTR},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scope {
    File,
    Directory,
    Background,
}

impl Scope {
    pub const ALL: [Self; 3] = [Self::File, Self::Directory, Self::Background];

    pub fn label(self) -> &'static str {
        match self {
            Self::File => "文件",
            Self::Directory => "文件夹",
            Self::Background => "文件夹空白处",
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Directory => "dir",
            Self::Background => "background",
        }
    }

    fn key_path(self) -> &'static str {
        match self {
            Self::File => "Software\\Classes\\*\\shell",
            Self::Directory => "Software\\Classes\\Directory\\shell",
            Self::Background => "Software\\Classes\\Directory\\Background\\shell",
        }
    }
}

#[derive(Clone, Debug)]
pub struct RegistryMenuItem {
    /// Stable identifier persisted in settings, e.g. `file:7-zip`.
    pub id: String,
    pub scope: Scope,
    /// Verb name passed to `ShellExecuteExW`.
    pub verb: String,
    pub title: String,
    pub from_hkcu: bool,
}

/// Enumerate registered verbs. HKCU wins over HKLM for the same verb.
pub fn enumerate() -> Vec<RegistryMenuItem> {
    let mut seen = std::collections::BTreeSet::new();
    let mut items = Vec::new();
    for (hive, from_hkcu) in [(HKEY_CURRENT_USER, true), (HKEY_LOCAL_MACHINE, false)] {
        for scope in Scope::ALL {
            enumerate_scope(hive, scope, from_hkcu, &mut seen, &mut items);
        }
    }
    items.sort_by(|a, b| {
        a.scope
            .cmp(&b.scope)
            .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
    });
    items
}

fn registry_cache() -> &'static std::sync::RwLock<Option<Vec<RegistryMenuItem>>> {
    static CACHE: std::sync::OnceLock<std::sync::RwLock<Option<Vec<RegistryMenuItem>>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::RwLock::new(None))
}

/// Cached variant used when a window opens. Registry enumeration (plus icon
/// and version lookups) is slow enough to be noticeable when opening several
/// windows, but the verb list changes rarely.
pub fn enumerate_cached() -> Vec<RegistryMenuItem> {
    if let Some(items) = registry_cache()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
    {
        return items.clone();
    }
    let items = enumerate();
    *registry_cache()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(items.clone());
    items
}

/// Force a fresh enumeration and update the shared cache.
pub fn enumerate_refresh() -> Vec<RegistryMenuItem> {
    let items = enumerate();
    *registry_cache()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(items.clone());
    items
}

/// Invoke a registered verb on `path` (a file, folder, or the current folder
/// for background verbs).
pub fn invoke(item: &RegistryMenuItem, path: &Path) -> Result<(), String> {
    crate::ops::run_registry_verb(path, &item.verb)
}

fn enumerate_scope(
    hive: HKEY,
    scope: Scope,
    from_hkcu: bool,
    seen: &mut std::collections::BTreeSet<String>,
    items: &mut Vec<RegistryMenuItem>,
) {
    let path = wide(scope.key_path());
    let mut root = HKEY(std::ptr::null_mut());
    let status = unsafe {
        RegOpenKeyExW(hive, PCWSTR(path.as_ptr()), Some(0), KEY_READ, &mut root)
    };
    if status != ERROR_SUCCESS {
        return;
    }

    let mut index = 0u32;
    loop {
        let mut name = [0u16; 512];
        let mut length = name.len() as u32;
        let status = unsafe {
            RegEnumKeyExW(
                root,
                index,
                Some(PWSTR(name.as_mut_ptr())),
                &mut length,
                None,
                None,
                None,
                None,
            )
        };
        if status == ERROR_NO_MORE_ITEMS {
            break;
        }
        if status == ERROR_MORE_DATA {
            index += 1;
            continue;
        }
        if status != ERROR_SUCCESS {
            break;
        }
        index += 1;

        let verb = String::from_utf16_lossy(&name[..length as usize]);
        if verb.is_empty() || verb.eq_ignore_ascii_case("open") {
            continue;
        }

        let sub_path = format!("{}\\{}", scope.key_path(), verb);
        let sub_path_w = wide(&sub_path);
        let mut key = HKEY(std::ptr::null_mut());
        let status = unsafe {
            RegOpenKeyExW(
                hive,
                PCWSTR(sub_path_w.as_ptr()),
                Some(0),
                KEY_READ,
                &mut key,
            )
        };
        if status != ERROR_SUCCESS {
            continue;
        }

        let legacy = value_exists(key, "LegacyDisable");
        let display = read_string(key, "MUIVerb")
            .or_else(|| read_string(key, ""))
            .filter(|text| !text.is_empty());
        unsafe {
            let _ = RegCloseKey(key);
        }
        if legacy {
            continue;
        }

        let title = display
            .map(|text| resolve_indirect(&text))
            .unwrap_or_else(|| verb.clone());
        let id = format!("{}:{}", scope.slug(), verb.to_lowercase());
        if !seen.insert(id.clone()) {
            continue;
        }
        items.push(RegistryMenuItem {
            id,
            scope,
            verb,
            title,
            from_hkcu,
        });
    }

    unsafe {
        let _ = RegCloseKey(root);
    }
}

fn read_raw(key: HKEY, name: &str) -> Option<(REG_VALUE_TYPE, Vec<u8>)> {
    let name_w = wide(name);
    let mut kind = REG_VALUE_TYPE::default();
    let mut size = 0u32;
    let status = unsafe {
        RegQueryValueExW(
            key,
            PCWSTR(name_w.as_ptr()),
            None,
            Some(&mut kind),
            None,
            Some(&mut size),
        )
    };
    if status != ERROR_SUCCESS || size == 0 {
        return None;
    }
    let mut buffer = vec![0u8; size as usize + 2];
    let mut actual = size;
    let status = unsafe {
        RegQueryValueExW(
            key,
            PCWSTR(name_w.as_ptr()),
            None,
            Some(&mut kind),
            Some(buffer.as_mut_ptr()),
            Some(&mut actual),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    buffer.truncate(actual.min(size) as usize);
    Some((kind, buffer))
}

fn read_string(key: HKEY, name: &str) -> Option<String> {
    let (kind, buffer) = read_raw(key, name)?;
    if kind != REG_SZ && kind != REG_EXPAND_SZ {
        return None;
    }
    let wide: Vec<u16> = buffer
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();
    let end = wide.iter().position(|&ch| ch == 0).unwrap_or(wide.len());
    let text = String::from_utf16_lossy(&wide[..end]);
    let text = text.trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn value_exists(key: HKEY, name: &str) -> bool {
    read_raw(key, name).is_some()
}

fn resolve_indirect(text: &str) -> String {
    if !text.starts_with('@') {
        return text.to_string();
    }
    let source = wide(text);
    let mut output = [0u16; 1024];
    let result =
        unsafe { SHLoadIndirectString(PCWSTR(source.as_ptr()), &mut output, None) };
    if result.is_ok() {
        let end = output.iter().position(|&ch| ch == 0).unwrap_or(output.len());
        let resolved = String::from_utf16_lossy(&output[..end]).trim().to_string();
        if !resolved.is_empty() {
            return resolved;
        }
    }
    text.trim_start_matches('@').to_string()
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_paths_are_distinct() {
        let paths: Vec<&str> = Scope::ALL.iter().map(|scope| scope.key_path()).collect();
        assert_eq!(paths.len(), 3);
        assert!(paths.iter().all(|path| path.starts_with("Software\\Classes")));
        assert!(paths[0].contains("*\\shell"));
        assert!(paths[1].contains("Directory\\shell"));
        assert!(paths[2].contains("Directory\\Background\\shell"));
    }

    #[test]
    fn ids_are_stable_and_scoped() {
        let item = RegistryMenuItem {
            id: format!("{}:7-zip", Scope::File.slug()),
            scope: Scope::File,
            verb: "7-Zip".to_string(),
            title: "7-Zip".to_string(),
            from_hkcu: true,
        };
        assert_eq!(item.id, "file:7-zip");
    }

    #[test]
    fn enumeration_does_not_panic() {
        let _ = enumerate();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum HandlerScope {
    File,
    Directory,
    Folder,
    Drive,
    Background,
}

impl HandlerScope {
    pub const ALL: [Self; 5] = [
        Self::File,
        Self::Directory,
        Self::Folder,
        Self::Drive,
        Self::Background,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::File => "文件",
            Self::Directory => "文件夹",
            Self::Folder => "所有文件夹",
            Self::Drive => "驱动器",
            Self::Background => "文件夹空白处",
        }
    }

    #[cfg_attr(not(debug_assertions), allow(dead_code))]
    pub fn slug(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Directory => "dir",
            Self::Folder => "folder",
            Self::Drive => "drive",
            Self::Background => "background",
        }
    }

    fn class_path(self) -> &'static str {
        match self {
            Self::File => "*",
            Self::Directory => "Directory",
            Self::Folder => "Folder",
            Self::Drive => "Drive",
            Self::Background => "Directory\\Background",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ShellHandler {
    /// Stable identifier persisted in settings, e.g. `shellex:7-zip`.
    pub id: String,
    pub name: String,
    #[cfg_attr(not(debug_assertions), allow(dead_code))]
    pub clsid: windows::core::GUID,
    pub scopes: Vec<HandlerScope>,
    pub from_hkcu: bool,
    /// Main executable found next to the extension DLL (e.g. 7zFM.exe,
    /// BCompare.exe). The registry does not store a command line for in-proc
    /// shell extensions, so this is what a click launches directly.
    pub program: Option<PathBuf>,
    /// The `InprocServer32` DLL that implements the extension.
    pub dll: Option<PathBuf>,
}

/// Enumerate registered COM context-menu handlers (`shellex\ContextMenuHandlers`).
/// Entries with the same display name are merged across scopes.
pub fn enumerate_handlers() -> Vec<ShellHandler> {
    let mut map: std::collections::BTreeMap<String, ShellHandler> =
        std::collections::BTreeMap::new();
    for (hive, from_hkcu) in [(HKEY_CURRENT_USER, true), (HKEY_LOCAL_MACHINE, false)] {
        for view in ["", "Wow6432Node\\"] {
            for scope in HandlerScope::ALL {
                enumerate_handlers_in(hive, view, scope, from_hkcu, &mut map);
            }
        }
    }
    let mut items: Vec<ShellHandler> = map.into_values().collect();
    items.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    items
}

fn handler_cache() -> &'static std::sync::RwLock<Option<Vec<ShellHandler>>> {
    static CACHE: std::sync::OnceLock<std::sync::RwLock<Option<Vec<ShellHandler>>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::RwLock::new(None))
}

pub fn enumerate_handlers_cached() -> Vec<ShellHandler> {
    if let Some(items) = handler_cache()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
    {
        return items.clone();
    }
    let items = enumerate_handlers();
    *handler_cache()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(items.clone());
    items
}

pub fn enumerate_handlers_refresh() -> Vec<ShellHandler> {
    let items = enumerate_handlers();
    *handler_cache()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(items.clone());
    items
}

fn enumerate_handlers_in(
    hive: HKEY,
    view: &str,
    scope: HandlerScope,
    from_hkcu: bool,
    map: &mut std::collections::BTreeMap<String, ShellHandler>,
) {
    let path = format!(
        "Software\\Classes\\{view}{}\\shellex\\ContextMenuHandlers",
        scope.class_path()
    );
    let path_w = wide(&path);
    let mut root = HKEY(std::ptr::null_mut());
    let status = unsafe {
        RegOpenKeyExW(hive, PCWSTR(path_w.as_ptr()), Some(0), KEY_READ, &mut root)
    };
    if status != ERROR_SUCCESS {
        return;
    }

    let mut index = 0u32;
    loop {
        let mut name = [0u16; 512];
        let mut length = name.len() as u32;
        let status = unsafe {
            RegEnumKeyExW(
                root,
                index,
                Some(PWSTR(name.as_mut_ptr())),
                &mut length,
                None,
                None,
                None,
                None,
            )
        };
        if status == ERROR_NO_MORE_ITEMS {
            break;
        }
        if status == ERROR_MORE_DATA {
            index += 1;
            continue;
        }
        if status != ERROR_SUCCESS {
            break;
        }
        index += 1;

        let key_name = String::from_utf16_lossy(&name[..length as usize]);
        if key_name.is_empty() {
            continue;
        }
        let sub_path = format!("{path}\\{key_name}");
        let sub_path_w = wide(&sub_path);
        let mut key = HKEY(std::ptr::null_mut());
        let status = unsafe {
            RegOpenKeyExW(hive, PCWSTR(sub_path_w.as_ptr()), Some(0), KEY_READ, &mut key)
        };
        if status != ERROR_SUCCESS {
            continue;
        }
        let clsid_text = read_string(key, "");
        unsafe {
            let _ = RegCloseKey(key);
        }
        let Some(clsid_text) = clsid_text else {
            continue;
        };
        let Some(clsid) = parse_clsid(&clsid_text) else {
            continue;
        };
        let Some(inproc) = clsid_inproc_path_64(&clsid) else {
            // 32-bit-only handler: this 64-bit process cannot load it.
            continue;
        };
        if is_system_dll(&inproc) {
            // Windows' own shell handlers (Sharing, Open With, ...) have no
            // useful standalone program; skip them.
            continue;
        }
        let Some(program) = main_program(&inproc) else {
            continue;
        };
        let display = handler_display(&key_name, &clsid);
        let id = format!("shellex:{}", display.to_lowercase());
        match map.get_mut(&id) {
            Some(existing) => {
                if !existing.scopes.contains(&scope) {
                    existing.scopes.push(scope);
                    existing.scopes.sort();
                }
                if existing.program.is_none() {
                    existing.program = Some(program.clone());
                    existing.dll = Some(PathBuf::from(&inproc));
                }
            }
            None => {
                map.insert(
                    id.clone(),
                    ShellHandler {
                        id,
                        name: display,
                        clsid,
                        scopes: vec![scope],
                        from_hkcu,
                        program: Some(program.clone()),
                        dll: Some(PathBuf::from(&inproc)),
                    },
                );
            }
        }
    }

    unsafe {
        let _ = RegCloseKey(root);
    }
}

fn parse_clsid(text: &str) -> Option<windows::core::GUID> {
    use windows::core::HSTRING;
    use windows::Win32::System::Com::CLSIDFromString;

    let text = text.trim();
    if !text.starts_with('{') {
        return None;
    }
    let wide = HSTRING::from(text);
    unsafe { CLSIDFromString(&wide) }.ok()
}

fn clsid_display(clsid: &windows::core::GUID) -> Option<String> {
    let path = format!("Software\\Classes\\CLSID\\{}", guid_string(clsid));
    for hive in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        for view in ["", "Wow6432Node\\"] {
            let full = format!("Software\\Classes\\{view}CLSID\\{}", guid_string(clsid));
            let _ = &path;
            let full_w = wide(&full);
            let mut key = HKEY(std::ptr::null_mut());
            let status = unsafe {
                RegOpenKeyExW(hive, PCWSTR(full_w.as_ptr()), Some(0), KEY_READ, &mut key)
            };
            if status == ERROR_SUCCESS {
                let text = read_string(key, "");
                unsafe {
                    let _ = RegCloseKey(key);
                }
                if let Some(text) = text {
                    let text = resolve_indirect(&text);
                    if !text.is_empty() {
                        return Some(text);
                    }
                }
            }
        }
    }
    None
}

fn guid_string(guid: &windows::core::GUID) -> String {
    let d4 = guid.data4;
    format!(
        "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
        guid.data1, guid.data2, guid.data3, d4[0], d4[1], d4[2], d4[3], d4[4], d4[5], d4[6], d4[7]
    )
}

fn clsid_inproc_path_64(clsid: &windows::core::GUID) -> Option<String> {
    for hive in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        let path = format!(
            "Software\\Classes\\CLSID\\{}\\InprocServer32",
            guid_string(clsid)
        );
        let path_w = wide(&path);
        let mut key = HKEY(std::ptr::null_mut());
        let status = unsafe {
            RegOpenKeyExW(hive, PCWSTR(path_w.as_ptr()), Some(0), KEY_READ, &mut key)
        };
        if status == ERROR_SUCCESS {
            let text = read_string(key, "");
            unsafe {
                let _ = RegCloseKey(key);
            }
            if let Some(text) = text {
                let text = resolve_indirect(&text);
                if !text.is_empty() {
                    return Some(text);
                }
            }
        }
    }
    None
}

fn handler_display(key_name: &str, clsid: &windows::core::GUID) -> String {
    let generic_key = key_name.starts_with('{')
        || key_name.to_ascii_lowercase().contains("shellex")
        || key_name.eq_ignore_ascii_case("ContextMenuHandlers");
    let mut candidate = key_name.trim().to_string();
    if generic_key {
        let dll_name = clsid_inproc_path_64(clsid)
            .and_then(|path| dll_product_name(&path));
        if let Some(name) = dll_name.or_else(|| clsid_display(clsid)) {
            candidate = name;
        }
    }
    let candidate = candidate
        .trim_end_matches(" Shell Extension")
        .trim()
        .to_string();
    if candidate.is_empty() {
        key_name.to_string()
    } else {
        candidate
    }
}

fn dll_product_name(path: &str) -> Option<String> {
    use windows::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
    };
    use windows::core::HSTRING;

    let wide_path = HSTRING::from(path);
    let size = unsafe { GetFileVersionInfoSizeW(&wide_path, None) };
    if size == 0 {
        return None;
    }
    let mut buffer = vec![0u8; size as usize];
    unsafe {
        GetFileVersionInfoW(&wide_path, None, size, buffer.as_mut_ptr() as *mut _).ok()?;
    }
    let mut pointer: *mut std::ffi::c_void = std::ptr::null_mut();
    let mut length = 0u32;
    let translation = HSTRING::from("\\VarFileInfo\\Translation");
    if !unsafe {
        VerQueryValueW(
            buffer.as_ptr() as *const _,
            &translation,
            &mut pointer,
            &mut length,
        )
    }
    .as_bool()
        || length < 4
        || pointer.is_null()
    {
        return None;
    }
    let language = unsafe { *(pointer as *const u16) };
    let codepage = unsafe { *((pointer as *const u16).add(1)) };
    let query = HSTRING::from(format!(
        "\\StringFileInfo\\{language:04x}{codepage:04x}\\ProductName"
    ));
    let mut value_pointer: *mut std::ffi::c_void = std::ptr::null_mut();
    let mut value_length = 0u32;
    if !unsafe {
        VerQueryValueW(
            buffer.as_ptr() as *const _,
            &query,
            &mut value_pointer,
            &mut value_length,
        )
    }
    .as_bool()
        || value_pointer.is_null()
        || value_length == 0
    {
        return None;
    }
    let slice = unsafe {
        std::slice::from_raw_parts(value_pointer as *const u16, value_length as usize)
    };
    let end = slice.iter().position(|&ch| ch == 0).unwrap_or(slice.len());
    let text = String::from_utf16_lossy(&slice[..end]).trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn is_system_dll(path: &str) -> bool {
    let system_root = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let lower = path.to_lowercase();
    let root = system_root.to_string_lossy().to_lowercase();
    lower.starts_with(&root)
}

fn main_program(dll: &str) -> Option<PathBuf> {
    let directory = Path::new(dll).parent()?;
    let mut best: Option<(u64, PathBuf)> = None;
    for entry in std::fs::read_dir(directory).ok()? {
        let entry = entry.ok()?;
        let path = entry.path();
        if !path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
        {
            continue;
        }
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if name.contains("unins")
            || name.contains("setup")
            || name.contains("update")
            || name.contains("crash")
        {
            continue;
        }
        let size = entry.metadata().ok()?.len();
        if best
            .as_ref()
            .map_or(true, |(best_size, _)| size > *best_size)
        {
            best = Some((size, path));
        }
    }
    best.map(|(_, path)| path)
}
