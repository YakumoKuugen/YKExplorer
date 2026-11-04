//! Per-user registration that makes YKExplorer the default handler for opening
//! folders and drives, with a full backup/restore of the previous values.

use std::path::PathBuf;

use windows::{
    Win32::{
        Foundation::{ERROR_ACCESS_DENIED, ERROR_SUCCESS},
        System::Registry::{
            HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ,
            REG_VALUE_TYPE, RegCloseKey, RegCreateKeyExW, RegDeleteKeyW, RegDeleteTreeW,
            RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
        },
    },
    core::{HSTRING, PCWSTR},
};

const CLASSES: [&str; 3] = ["Folder", "Directory", "Drive"];
const FILE_EXPLORER_CLSID: &str = "{52205fd8-5dfb-447d-801a-d0b52f2e83e1}";
const YKEXPLORER_KEY: &str = r"Software\YKExplorer";
const MARKER_VALUE: &str = "ReplaceExplorer";

pub fn is_enabled() -> bool {
    read_string(YKEXPLORER_KEY, MARKER_VALUE).is_some()
}

pub fn enable() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|error| format!("无法获取程序路径：{error}"))?;
    let command = format!("\"{}\" \"%1\"", exe.display());
    for class in CLASSES {
        enable_one(
            &format!(r"Software\Classes\{class}\shell\open"),
            &format!(r"Software\YKExplorer\ExplorerBackup\{class}"),
            &command,
        )?;
        // `explore` is a separate verb (Windows' "open in a new window" path).
        // Launchers that ask for it explicitly would otherwise open Explorer.
        enable_one(
            &format!(r"Software\Classes\{class}\shell\explore"),
            &format!(r"Software\YKExplorer\ExplorerBackup\{class}-explore"),
            &command,
        )?;
    }
    // The Start menu / Win+E "File Explorer" entry is the shell folder with
    // this CLSID. Overriding its OpenNewWindow verb is what File Pilot does.
    enable_one(
        &format!(r"Software\Classes\CLSID\{FILE_EXPLORER_CLSID}\shell\opennewwindow"),
        r"Software\YKExplorer\ExplorerBackup\OpenNewWindow",
        &format!("\"{}\"", exe.display()),
    )?;
    write_string(
        YKEXPLORER_KEY,
        MARKER_VALUE,
        &exe.to_string_lossy().into_owned(),
    )?;
    Ok(())
}

pub fn disable() -> Result<(), String> {
    for class in CLASSES {
        disable_one(
            &format!(r"Software\Classes\{class}\shell\open"),
            &format!(r"Software\YKExplorer\ExplorerBackup\{class}"),
        )?;
        disable_one(
            &format!(r"Software\Classes\{class}\shell\explore"),
            &format!(r"Software\YKExplorer\ExplorerBackup\{class}-explore"),
        )?;
    }
    disable_one(
        &format!(r"Software\Classes\CLSID\{FILE_EXPLORER_CLSID}\shell\opennewwindow"),
        r"Software\YKExplorer\ExplorerBackup\OpenNewWindow",
    )?;
    let _ = delete_value(YKEXPLORER_KEY, MARKER_VALUE);
    Ok(())
}

fn enable_one(open: &str, backup: &str, command: &str) -> Result<(), String> {
    let command_key = format!(r"{open}\command");
    if read_string(backup, "CommandExisted").is_none() {
        let previous = read_string_typed(&command_key, "");
        let existed = previous.is_some();
        if let Some((previous, kind)) = &previous {
            write_string_typed(backup, "Command", previous, *kind)?;
            write_string(backup, "CommandType", &kind.0.to_string())?;
        }
        if let Some(delegate) = read_string(&command_key, "DelegateExecute") {
            write_string(backup, "DelegateExecute", &delegate)?;
        }
        write_string(backup, "CommandExisted", if existed { "1" } else { "0" })?;
        write_string(
            backup,
            "OpenExisted",
            if key_exists(open) { "1" } else { "0" },
        )?;
    }
    write_string(&command_key, "", command)?;
    // HKLM still contributes a DelegateExecute value that would win over our
    // command. Override it with an invalid CLSID so the shell falls back to
    // the command we just wrote.
    write_string(&command_key, "DelegateExecute", "YKExplorerDisabled")?;
    Ok(())
}

fn disable_one(open: &str, backup: &str) -> Result<(), String> {
    let command_key = format!(r"{open}\command");
    if read_string(backup, "OpenExisted").as_deref() == Some("0") {
        let _ = delete_tree(open);
        let _ = delete_key(backup);
        return Ok(());
    }
    let existed = read_string(backup, "CommandExisted").as_deref() == Some("1");
    if existed {
        if let Some(previous) = read_string(backup, "Command") {
            let kind = read_string(backup, "CommandType")
                .and_then(|value| value.parse::<u32>().ok())
                .map(REG_VALUE_TYPE)
                .unwrap_or(REG_SZ);
            write_string_typed(&command_key, "", &previous, kind)?;
        }
    } else {
        let _ = delete_tree(&command_key);
    }
    match read_string(backup, "DelegateExecute") {
        Some(delegate) => write_string(&command_key, "DelegateExecute", &delegate)?,
        None => {
            let _ = delete_value(&command_key, "DelegateExecute");
        }
    }
    let _ = delete_key(backup);
    Ok(())
}

fn key_exists(path: &str) -> bool {
    unsafe {
        let mut key = HKEY::default();
        let opened = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            &HSTRING::from(path),
            None,
            KEY_READ,
            &mut key,
        )
        .0 == ERROR_SUCCESS.0;
        if opened {
            let _ = RegCloseKey(key);
        }
        opened
    }
}

fn read_string(path: &str, name: &str) -> Option<String> {
    read_string_typed(path, name).map(|(value, _)| value)
}

fn read_string_typed(path: &str, name: &str) -> Option<(String, REG_VALUE_TYPE)> {
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            &HSTRING::from(path),
            None,
            KEY_READ,
            &mut key,
        )
        .0 != ERROR_SUCCESS.0
        {
            return None;
        }
        let mut kind = REG_VALUE_TYPE::default();
        let mut length = 0u32;
        let query = RegQueryValueExW(
            key,
            &HSTRING::from(name),
            None,
            Some(&mut kind),
            None,
            Some(&mut length),
        );
        if query.0 != ERROR_SUCCESS.0 || length == 0 {
            let _ = RegCloseKey(key);
            return None;
        }
        let mut buffer = vec![0u8; length as usize];
        let query = RegQueryValueExW(
            key,
            &HSTRING::from(name),
            None,
            Some(&mut kind),
            Some(buffer.as_mut_ptr()),
            Some(&mut length),
        );
        let _ = RegCloseKey(key);
        if query.0 != ERROR_SUCCESS.0 {
            return None;
        }
        buffer.truncate(length as usize);
        while buffer.len() >= 2 && buffer[buffer.len() - 2] == 0 && buffer[buffer.len() - 1] == 0 {
            buffer.truncate(buffer.len() - 2);
        }
        let value = String::from_utf16(
            &buffer
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect::<Vec<_>>(),
        )
        .ok()?;
        Some((value, kind))
    }
}

fn write_string(path: &str, name: &str, value: &str) -> Result<(), String> {
    write_string_typed(path, name, value, REG_SZ)
}

fn write_string_typed(
    path: &str,
    name: &str,
    value: &str,
    kind: REG_VALUE_TYPE,
) -> Result<(), String> {
    unsafe {
        let mut key = HKEY::default();
        let result = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            &HSTRING::from(path),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_READ | KEY_WRITE,
            None,
            &mut key,
            None,
        );
        if result.0 != ERROR_SUCCESS.0 {
            return Err(registry_write_error(path, result.0));
        }
        let wide: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes = std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2);
        let result = RegSetValueExW(key, &HSTRING::from(name), None, kind, Some(bytes));
        let _ = RegCloseKey(key);
        if result.0 != ERROR_SUCCESS.0 {
            return Err(registry_write_error(&format!("{path} / {name}"), result.0));
        }
        Ok(())
    }
}

/// Error 5 has a very common and very non-obvious cause in this app: a copy
/// launched from a folder carrying the `Low Mandatory Level` label (sandboxes,
/// some unzip tools) is forbidden by Windows from writing HKCU. Spell that out
/// instead of only showing the raw Win32 code.
fn registry_write_error(path: &str, code: u32) -> String {
    if code == ERROR_ACCESS_DENIED.0 {
        format!(
            "无法写入注册表 {path}：拒绝访问（错误 5）。\n\n这通常是因为 YKExplorer 从带有 Low 完整性标签的目录或沙箱中启动，Windows 会阻止低完整性进程写入 HKCU。\n请把 YKExplorer.exe 复制到普通目录（例如桌面或 %LOCALAPPDATA% 下的 YKExplorer 目录）后重新运行。"
        )
    } else {
        format!("无法写入注册表 {path}：{code}")
    }
}

fn delete_value(path: &str, name: &str) -> Result<(), String> {
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            &HSTRING::from(path),
            None,
            KEY_READ | KEY_WRITE,
            &mut key,
        )
        .0 != ERROR_SUCCESS.0
        {
            return Ok(());
        }
        let result = RegDeleteValueW(key, &HSTRING::from(name));
        let _ = RegCloseKey(key);
        if result.0 != ERROR_SUCCESS.0 {
            return Err(format!("无法删除注册表值 {path}\\{name}：{}", result.0));
        }
        Ok(())
    }
}

fn delete_tree(path: &str) -> Result<(), String> {
    unsafe {
        let result = RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(path));
        if result.0 != ERROR_SUCCESS.0 {
            return Err(format!("无法删除注册表树 {path}：{}", result.0));
        }
        Ok(())
    }
}

fn delete_key(path: &str) -> Result<(), String> {
    unsafe {
        let result = RegDeleteKeyW(HKEY_CURRENT_USER, &HSTRING::from(path));
        if result.0 != ERROR_SUCCESS.0 {
            return Err(format!("无法删除注册表键 {path}：{}", result.0));
        }
        Ok(())
    }
}

/// Path passed on the command line by Explorer (`"exe" "%1"`).
pub fn launch_path() -> Option<PathBuf> {
    std::env::args()
        .skip(1)
        .find(|argument| !argument.starts_with("--"))
        .map(PathBuf::from)
        .map(|path| {
            // A forwarded launch runs in the existing process, whose working
            // directory may differ; resolve relative arguments here.
            if path.is_absolute() {
                path
            } else {
                std::env::current_dir()
                    .map(|directory| directory.join(&path))
                    .unwrap_or(path)
            }
        })
}
