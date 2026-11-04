use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    process::Command,
};

use windows::{
    Win32::{
        UI::{
            Shell::{SEE_MASK_INVOKEIDLIST, SHELLEXECUTEINFOW, ShellExecuteExW, ShellExecuteW},
            WindowsAndMessaging::SW_SHOWNORMAL,
        },
    },
    core::{HSTRING, PCWSTR},
};

#[derive(Debug, Default, Clone)]
pub struct OpReport {
    pub done: Vec<PathBuf>,
    /// `(source, target)` for each successful move, used to undo.
    pub moved: Vec<(PathBuf, PathBuf)>,
    pub errors: Vec<String>,
}

impl OpReport {
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }

    pub fn summary(&self, action: &str) -> String {
        if self.ok() {
            format!("{action} {} 项", self.done.len())
        } else if self.done.is_empty() {
            format!("{action}失败：{}", self.errors.join("；"))
        } else {
            format!(
                "{action} {} 项，{} 项失败：{}",
                self.done.len(),
                self.errors.len(),
                self.errors.join("；")
            )
        }
    }
}

pub fn validate_name(name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("名称不能为空".into());
    }
    if name == "." || name == ".." {
        return Err("名称无效".into());
    }
    if name
        .chars()
        .any(|c| matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'))
    {
        return Err("名称不能包含 < > : \" / \\ | ? *".into());
    }
    if name.ends_with(' ') || name.ends_with('.') {
        return Err("名称不能以空格或句点结尾".into());
    }
    let upper = name.to_ascii_uppercase();
    let reserved = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
        "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let base = upper.split('.').next().unwrap_or(&upper);
    if reserved.contains(&base) {
        return Err(format!("“{name}” 是 Windows 保留名称"));
    }
    Ok(())
}

pub fn unique_destination(dir: &Path, name: &str) -> PathBuf {
    let mut candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let path = Path::new(name);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| name.to_string());
    let extension = path
        .extension()
        .map(|s| s.to_string_lossy().into_owned());
    for index in 2..10_000 {
        let file_name = match &extension {
            Some(ext) if !ext.is_empty() => format!("{stem} ({index}).{ext}"),
            _ => format!("{stem} ({index})"),
        };
        candidate = dir.join(file_name);
        if !candidate.exists() {
            return candidate;
        }
    }
    candidate
}

pub fn copy_into(paths: &[PathBuf], dest_dir: &Path) -> OpReport {
    let mut report = OpReport::default();
    let dest_canon = dunce::canonicalize(dest_dir).ok();
    for source in paths {
        let Some(name) = source.file_name() else {
            report.errors.push(format!("无法复制：{}", source.display()));
            continue;
        };
        if let (Ok(source_canon), Some(dest_canon)) = (dunce::canonicalize(source), &dest_canon) {
            if dest_canon.starts_with(&source_canon) {
                report
                    .errors
                    .push("不能把文件夹复制到它自己内部".to_string());
                continue;
            }
        }
        let target = unique_destination(dest_dir, &name.to_string_lossy());
        match copy_entry(source, &target) {
            Ok(()) => report.done.push(target),
            Err(err) => report
                .errors
                .push(format!("{}：{err}", source.file_name().unwrap_or_default().to_string_lossy())),
        }
    }
    report
}

/// Sources moved out of an archiver's drag-and-drop temp folder.
pub struct StagedDrop {
    /// Stable paths inside `staging_root`, ready for an asynchronous copy.
    pub sources: Vec<PathBuf>,
    /// Private folder the payload was moved into; remove it after copying.
    pub staging_root: PathBuf,
}

/// True when `name` looks like the temp folder an archiver creates for
/// drag-and-drop extraction: 7-Zip uses `7zE`+hex (drag out) / `7zO`+hex (open)
/// and WinRAR uses `Rar$…`.
fn is_archiver_temp_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if let Some(rest) = lower
        .strip_prefix("7ze")
        .or_else(|| lower.strip_prefix("7zo"))
    {
        return rest.len() >= 4 && rest.bytes().all(|byte| byte.is_ascii_hexdigit());
    }
    lower.starts_with("rar$")
}

fn normalized_dir(path: &Path) -> String {
    path.to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .to_ascii_lowercase()
}

fn is_direct_child_of(path: &Path, parent: &Path) -> bool {
    path.parent()
        .is_some_and(|actual| normalized_dir(actual) == normalized_dir(parent))
}

fn is_archiver_temp_root_in(path: &Path, temp_root: &Path) -> bool {
    is_direct_child_of(path, temp_root)
        && path
            .file_name()
            .is_some_and(|name| is_archiver_temp_name(&name.to_string_lossy()))
}

/// True when the dragged item itself is an archiver's extraction folder. Such a
/// folder is empty while the drag runs and is deleted right after it, so
/// copying it only produces an empty `7zE…` folder in the destination.
pub fn is_archiver_temp_root(path: &Path) -> bool {
    is_archiver_temp_root_in(path, &std::env::temp_dir())
}

/// The archiver temp folder `path` lives in, if any.
fn archiver_temp_root_in(path: &Path, temp_root: &Path) -> Option<PathBuf> {
    path.ancestors()
        .skip(1)
        .find(|ancestor| is_archiver_temp_root_in(ancestor, temp_root))
        .map(Path::to_path_buf)
}

/// True when at least one path is an archiver's temporary drag folder or lives
/// inside one (for example a folder dragged straight out of an archive). Such a
/// path disappears with the drag and must not be pinned to quick access.
pub fn is_archiver_temp_payload(paths: &[PathBuf]) -> bool {
    let temp_root = std::env::temp_dir();
    paths.iter().any(|path| {
        is_archiver_temp_root_in(path, &temp_root)
            || archiver_temp_root_in(path, &temp_root).is_some()
    })
}

fn unique_staging_dir(temp_root: &Path) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    temp_root.join(format!("YKExplorer-drop-{}-{stamp}", std::process::id()))
}

/// Take ownership of a drop that only lives inside an archiver's temp folder.
///
/// 7-Zip (and WinRAR, …) extract dragged items into `%TEMP%\7zE…` and delete
/// that folder as soon as `IDropTarget::Drop` returns, which is before our
/// background copy starts. Renaming the payload into our own staging folder
/// inside `%TEMP%` is effectively instant, keeps the data alive and lets the
/// copy run asynchronously as usual.
///
/// Returns `None` when the paths are regular files (copy them in place).
pub fn stage_archiver_drop(paths: &[PathBuf]) -> Option<StagedDrop> {
    let temp_root = std::env::temp_dir();
    let staging_root = unique_staging_dir(&temp_root);
    stage_archiver_drop_within(paths, &temp_root, staging_root)
}

fn stage_archiver_drop_within(
    paths: &[PathBuf],
    temp_root: &Path,
    staging_root: PathBuf,
) -> Option<StagedDrop> {
    if paths.is_empty()
        || !paths
            .iter()
            .all(|path| archiver_temp_root_in(path, temp_root).is_some())
    {
        log::debug!("stage_archiver_drop: not an archiver temp payload");
        return None;
    }
    if let Err(error) = fs::create_dir_all(&staging_root) {
        log::warn!(
            "could not create staging folder {}: {error}",
            staging_root.display()
        );
        return None;
    }
    let mut sources = Vec::with_capacity(paths.len());
    for source in paths {
        let Some(name) = source.file_name() else {
            continue;
        };
        let target = unique_destination(&staging_root, &name.to_string_lossy());
        // Same volume in the normal case, so this is a rename; fall back to a
        // copy when the archiver's temp folder lives on another volume or the
        // source is already being deleted.
        let staged = match fs::rename(source, &target) {
            Ok(()) => Ok(()),
            Err(error) => {
                log::debug!(
                    "rename {} failed ({error}), falling back to a copy",
                    source.display()
                );
                copy_entry(source, &target)
            }
        };
        match staged {
            Ok(()) => sources.push(target),
            Err(error) => log::warn!("could not stage {}: {error}", source.display()),
        }
    }
    if sources.is_empty() {
        let _ = fs::remove_dir_all(&staging_root);
        return None;
    }
    Some(StagedDrop {
        sources,
        staging_root,
    })
}

/// Remove a staging folder created by [`stage_archiver_drop`].
pub fn discard_staged_drop(root: &Path) {
    if let Err(error) = fs::remove_dir_all(root) {
        log::warn!("could not remove staged drop {}: {error}", root.display());
    }
}

pub fn move_into(paths: &[PathBuf], dest_dir: &Path) -> OpReport {
    let mut report = OpReport::default();
    let dest_canon = dunce::canonicalize(dest_dir).ok();
    for source in paths {
        let Some(name) = source.file_name() else {
            report.errors.push(format!("无法移动：{}", source.display()));
            continue;
        };
        if source.parent().is_some_and(|parent| same_path(parent, dest_dir)) {
            report
                .errors
                .push(format!("{} 已在该文件夹中", name.to_string_lossy()));
            continue;
        }
        if let (Ok(source_canon), Some(dest_canon)) = (dunce::canonicalize(source), &dest_canon) {
            if dest_canon.starts_with(&source_canon) {
                report
                    .errors
                    .push("不能把文件夹移动到它自己内部".to_string());
                continue;
            }
        }
        let target = unique_destination(dest_dir, &name.to_string_lossy());
        match move_entry(source, &target) {
            Ok(()) => {
                report.moved.push((source.clone(), target.clone()));
                report.done.push(target);
            }
            Err(err) => report
                .errors
                .push(format!("{}：{err}", name.to_string_lossy())),
        }
    }
    report
}

pub fn copy_entry(source: &Path, target: &Path) -> Result<(), String> {
    let meta = fs::symlink_metadata(source).map_err(|e| e.to_string())?;
    if meta.file_type().is_symlink() {
        // Copying symlinks requires privileges; copy the target contents instead.
        let followed = fs::metadata(source).map_err(|e| e.to_string())?;
        if followed.is_dir() {
            copy_dir(source, target)
        } else {
            fs::copy(source, target).map(|_| ()).map_err(|e| e.to_string())
        }
    } else if meta.is_dir() {
        copy_dir(source, target)
    } else {
        fs::copy(source, target).map(|_| ()).map_err(|e| e.to_string())
    }
}

fn copy_dir(source: &Path, target: &Path) -> Result<(), String> {
    fs::create_dir_all(target).map_err(|e| e.to_string())?;
    for entry in fs::read_dir(source).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let child_target = target.join(entry.file_name());
        copy_entry(&entry.path(), &child_target)?;
    }
    Ok(())
}

pub fn move_entry(source: &Path, target: &Path) -> Result<(), String> {
    match fs::rename(source, target) {
        Ok(()) => Ok(()),
        Err(err) if is_cross_device(&err) => {
            copy_entry(source, target)?;
            if fs::symlink_metadata(source)
                .map(|m| m.is_dir())
                .unwrap_or(false)
            {
                fs::remove_dir_all(source).map_err(|e| e.to_string())?;
            } else {
                fs::remove_file(source).map_err(|e| e.to_string())?;
            }
            Ok(())
        }
        Err(err) => Err(err.to_string()),
    }
}

fn is_cross_device(err: &std::io::Error) -> bool {
    // ERROR_NOT_SAME_DEVICE
    err.raw_os_error() == Some(17) || err.kind() == ErrorKind::CrossesDevices
}

pub fn create_folder(parent: &Path, name: &str) -> Result<PathBuf, String> {
    validate_name(name)?;
    let path = unique_destination(parent, name);
    fs::create_dir(&path).map_err(|e| format!("无法创建文件夹：{e}"))?;
    Ok(path)
}

pub fn create_file(parent: &Path, name: &str) -> Result<PathBuf, String> {
    validate_name(name)?;
    let path = unique_destination(parent, name);
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| format!("无法创建文件：{e}"))?;
    Ok(path)
}

pub fn rename_path(path: &Path, new_name: &str) -> Result<PathBuf, String> {
    validate_name(new_name)?;
    let parent = path.parent().ok_or_else(|| "无法重命名根目录".to_string())?;
    let target = parent.join(new_name.trim());
    if same_path(path, &target) {
        return Ok(target);
    }
    if target.exists() {
        return Err(format!("“{}” 已存在", new_name.trim()));
    }
    fs::rename(path, &target).map_err(|e| format!("重命名失败：{e}"))?;
    Ok(target)
}

pub fn delete_to_trash(paths: &[PathBuf]) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }
    // `trash` calls CoInitializeEx itself. GPUI's executor threads already have
    // a COM apartment, which makes that call fail with RPC_E_CHANGED_MODE and
    // abort the process. Use a fresh thread with a clean COM state.
    let paths = paths.to_vec();
    std::thread::spawn(move || trash::delete_all(&paths))
        .join()
        .map_err(|_| "回收站操作线程异常退出".to_string())?
        .map_err(|e| format!("无法移入回收站：{e}"))
}

/// Restore previously trashed items by matching their original paths.
pub fn restore_trashed(original: &[PathBuf]) -> Result<usize, String> {
    let originals = original.to_vec();
    std::thread::spawn(move || -> Result<usize, String> {
        let items = trash::os_limited::list().map_err(|e| format!("无法读取回收站：{e}"))?;
        let targets: Vec<trash::TrashItem> = items
            .into_iter()
            .filter(|item| {
                let path = item.original_path();
                originals.iter().any(|original| same_path(&path, original))
            })
            .collect();
        let count = targets.len();
        if count == 0 {
            return Err("回收站中找不到这些项目".to_string());
        }
        trash::os_limited::restore_all(targets).map_err(|e| format!("恢复失败：{e}"))?;
        Ok(count)
    })
    .join()
    .map_err(|_| "回收站操作线程异常退出".to_string())?
}

pub fn delete_permanently(paths: &[PathBuf]) -> OpReport {
    let mut report = OpReport::default();
    for path in paths {
        let result = match fs::symlink_metadata(path) {
            Ok(meta) if meta.is_dir() => fs::remove_dir_all(path),
            Ok(_) => fs::remove_file(path),
            Err(err) => Err(err),
        };
        match result {
            Ok(()) => report.done.push(path.clone()),
            Err(err) => report
                .errors
                .push(format!("{}：{err}", path.to_string_lossy())),
        }
    }
    report
}

pub fn open_with_shell(path: &Path) -> Result<(), String> {
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
        );
    }
    let target = HSTRING::from(path.as_os_str());
    let operation = HSTRING::from("open");
    let result = unsafe {
        ShellExecuteW(
            None,
            &operation,
            &target,
            None,
            None,
            SW_SHOWNORMAL,
        )
    };
    if result.0 as isize > 32 {
        Ok(())
    } else {
        Err(format!("系统无法打开：{}", path.display()))
    }
}

pub fn reveal_in_explorer(path: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let mut command = Command::new("explorer.exe");
    if path.is_dir() {
        command.arg(path);
    } else {
        // `explorer /select,<path>` must be passed as one raw argument with
        // quotes preserved for paths that contain spaces.
        command.raw_arg(format!("/select,\"{}\"", path.display()));
    }
    command
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("无法打开资源管理器：{e}"))
}

pub fn same_path(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (dunce::canonicalize(a), dunce::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a
            .to_string_lossy()
            .eq_ignore_ascii_case(&b.to_string_lossy()),
    }
}


fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn shell_verb(path: &Path, verb: &str) -> Result<(), String> {
    let verb = wide(verb);
    let file = wide(&path.to_string_lossy());
    let mut info = SHELLEXECUTEINFOW::default();
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_INVOKEIDLIST;
    info.lpVerb = PCWSTR(verb.as_ptr());
    info.lpFile = PCWSTR(file.as_ptr());
    info.nShow = SW_SHOWNORMAL.0;
    unsafe { ShellExecuteExW(&mut info) }.map_err(|error| error.to_string())
}

/// Run a verb discovered from the registry (`*\shell`, `Directory\shell`, ...).
pub fn run_registry_verb(path: &Path, verb: &str) -> Result<(), String> {
    shell_verb(path, verb).map_err(|error| format!("系统菜单动作失败：{error}"))
}

pub fn is_executable(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "exe" | "msi" | "bat" | "cmd" | "ps1" | "com" | "scr"
            )
        })
}

pub fn open_with_dialog(path: &Path) -> Result<(), String> {
    if !path.is_file() {
        return Err("只能对文件使用“打开方式”".into());
    }
    shell_verb(path, "openas")
}

pub fn print_path(path: &Path) -> Result<(), String> {
    if !path.is_file() {
        return Err("只能打印文件".into());
    }
    shell_verb(path, "print")
}

pub fn show_properties(path: &Path) -> Result<(), String> {
    shell_verb(path, "properties")
}

pub fn run_as_admin(path: &Path) -> Result<(), String> {
    if !is_executable(path) {
        return Err("该文件类型不支持以管理员身份运行".into());
    }
    shell_verb(path, "runas")
}

pub fn open_cmd(path: &Path) -> Result<(), String> {
    if !path.is_dir() {
        return Err("必须选择一个文件夹".into());
    }
    Command::new("cmd.exe")
        .current_dir(path)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("无法启动命令提示符：{error}"))
}

pub fn open_powershell(path: &Path) -> Result<(), String> {
    if !path.is_dir() {
        return Err("必须选择一个文件夹".into());
    }
    Command::new("powershell.exe")
        .env("YKEXPLORER_TARGET", path.as_os_str())
        .args([
            "-NoExit",
            "-Command",
            "Set-Location -LiteralPath $env:YKEXPLORER_TARGET",
        ])
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("无法启动 PowerShell：{error}"))
}

pub fn open_terminal(path: &Path) -> Result<(), String> {
    if !path.is_dir() {
        return Err("必须选择一个文件夹".into());
    }
    match Command::new("wt.exe").arg("-d").arg(path).spawn() {
        Ok(_) => Ok(()),
        Err(_) => open_powershell(path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scratch_dir() -> PathBuf {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-scratch");
        let dir = root.join(format!(
            "ykexplorer-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn unique_destination_adds_suffix() {
        let dir = scratch_dir();
        fs::write(dir.join("a.txt"), b"x").unwrap();
        assert_eq!(unique_destination(&dir, "a.txt"), dir.join("a (2).txt"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn copy_and_move_work() {
        let dir = scratch_dir();
        let src = dir.join("source.txt");
        let out = dir.join("out");
        fs::create_dir(&out).unwrap();
        fs::write(&src, b"hello").unwrap();
        let report = copy_into(&[src.clone()], &out);
        assert!(report.ok(), "{:?}", report.errors);
        assert_eq!(fs::read_to_string(out.join("source.txt")).unwrap(), "hello");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn invalid_names_are_rejected() {
        assert!(validate_name("a/b").is_err());
        assert!(validate_name("CON").is_err());
        assert!(validate_name("正常名字").is_ok());
    }

    #[test]
    fn executable_extensions_are_detected() {
        assert!(is_executable(Path::new("a.exe")));
        assert!(is_executable(Path::new("A.PS1")));
        assert!(!is_executable(Path::new("a.txt")));
        assert!(!is_executable(Path::new("a.exe.txt")));
    }

}

/// One command exposed by a shell extension's own menu.
#[derive(Clone, Debug)]
pub struct HandlerMenuItem {
    pub id: u32,
    pub label: String,
}

/// A built (but hidden) shell-extension menu plus its flattened commands.
pub struct HandlerMenu {
    pub name: String,
    pub context: windows::Win32::UI::Shell::IContextMenu,
    pub menu: windows::Win32::UI::WindowsAndMessaging::HMENU,
    pub items: Vec<HandlerMenuItem>,
}

impl Drop for HandlerMenu {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::DestroyMenu(self.menu);
        }
    }
}

/// Build the hidden menu of one shell extension and flatten its items.
/// The menu is never shown; `InvokeCommand` is called with the stored ids.
pub fn build_handler_menu(
    clsid: &windows::core::GUID,
    display_name: &str,
    paths: &[PathBuf],
    background: Option<&Path>,
) -> Result<HandlerMenu, String> {
    use windows::Win32::{
        System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree},
        UI::{
            Shell::{CMF_NORMAL, IContextMenu, IShellExtInit},
            WindowsAndMessaging::CreatePopupMenu,
        },
    };
    use windows::core::{Interface, IUnknown};

    unsafe {
        let (folder_pidl, data, absolute_pidls) = build_handler_data(paths, background)?;
        let unknown: IUnknown = CoCreateInstance(clsid, None, CLSCTX_INPROC_SERVER)
            .map_err(|error| format!("无法创建 Shell 扩展：{error}"))?;
        let init: IShellExtInit = unknown
            .cast()
            .map_err(|error| format!("Shell 扩展不支持 IShellExtInit：{error}"))?;
        let context: IContextMenu = unknown
            .cast()
            .map_err(|error| format!("Shell 扩展不支持 IContextMenu：{error}"))?;
        init.Initialize(Some(folder_pidl), &data, None)
            .map_err(|error| format!("初始化 Shell 扩展失败：{error}"))?;
        let menu = CreatePopupMenu().map_err(|error| error.to_string())?;
        context
            .QueryContextMenu(menu, 0, 1, 0x7FFF, CMF_NORMAL)
            .ok()
            .map_err(|error| format!("扩展未能创建菜单：{error}"))?;
        let mut items = Vec::new();
        collect_handler_items(menu, "", 0, &mut items);
        let lower_name = display_name.to_lowercase();
        for item in &mut items {
            if !item.label.to_lowercase().starts_with(&lower_name) {
                item.label = format!("{display_name} · {}", item.label);
            }
        }
        for pidl in absolute_pidls {
            CoTaskMemFree(Some(pidl as *const _));
        }
        CoTaskMemFree(Some(folder_pidl as *const _));
        let _ = data;
        Ok(HandlerMenu {
            name: display_name.to_string(),
            context,
            menu,
            items,
        })
    }
}

/// Invoke one command of a previously built shell-extension menu.
pub fn invoke_handler_command(
    context: &windows::Win32::UI::Shell::IContextMenu,
    id: u32,
    hwnd: windows::Win32::Foundation::HWND,
) -> Result<(), String> {
    use windows::Win32::{
        Foundation::POINT,
        UI::{
            Shell::{CMIC_MASK_PTINVOKE, CMINVOKECOMMANDINFO, CMINVOKECOMMANDINFOEX},
            WindowsAndMessaging::{GetCursorPos, SW_SHOWNORMAL},
        },
    };
    use windows::core::{PCSTR, PCWSTR};

    if id == 0 {
        return Ok(());
    }
    unsafe {
        let mut point = POINT::default();
        let _ = GetCursorPos(&mut point);
        let mut info = CMINVOKECOMMANDINFOEX::default();
        info.cbSize = std::mem::size_of::<CMINVOKECOMMANDINFOEX>() as u32;
        info.fMask = 0x0000_4000 | CMIC_MASK_PTINVOKE;
        info.hwnd = hwnd;
        info.lpVerb = PCSTR((id as usize - 1) as *const u8);
        info.lpVerbW = PCWSTR((id as usize - 1) as *const u16);
        info.nShow = SW_SHOWNORMAL.0;
        info.ptInvoke = point;
        context
            .InvokeCommand(&info as *const _ as *const CMINVOKECOMMANDINFO)
            .map_err(|error| format!("执行扩展命令失败：{error}"))
    }
}

fn collect_handler_items(
    menu: windows::Win32::UI::WindowsAndMessaging::HMENU,
    prefix: &str,
    depth: usize,
    out: &mut Vec<HandlerMenuItem>,
) {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetMenuItemCount, GetMenuItemID, GetMenuStringW, GetSubMenu, MF_BYPOSITION,
    };
    if depth > 2 {
        return;
    }
    let count = unsafe { GetMenuItemCount(Some(menu)) }.max(0);
    for index in 0..count {
        let mut buffer = [0u16; 256];
        let length = unsafe { GetMenuStringW(menu, index as u32, Some(&mut buffer), MF_BYPOSITION) };
        let label = if length > 0 {
            String::from_utf16_lossy(&buffer[..length as usize]).replace('&', "")
        } else {
            String::new()
        };
        let submenu = unsafe { GetSubMenu(menu, index) };
        if !submenu.is_invalid() {
            let combined = if label.is_empty() {
                prefix.to_string()
            } else if prefix.is_empty() {
                label
            } else {
                format!("{prefix} · {label}")
            };
            collect_handler_items(submenu, &combined, depth + 1, out);
        } else {
            let id = unsafe { GetMenuItemID(menu, index) };
            if id == 0 || id == u32::MAX || label.is_empty() {
                continue;
            }
            let combined = if prefix.is_empty() {
                label
            } else {
                format!("{prefix} · {label}")
            };
            out.push(HandlerMenuItem { id, label: combined });
        }
    }
}

fn build_handler_data(
    paths: &[PathBuf],
    background: Option<&Path>,
) -> Result<
    (
        *mut windows::Win32::UI::Shell::Common::ITEMIDLIST,
        windows::Win32::System::Com::IDataObject,
        Vec<*mut windows::Win32::UI::Shell::Common::ITEMIDLIST>,
    ),
    String,
> {
    use windows::Win32::{
        System::Com::IDataObject,
        UI::Shell::{Common::ITEMIDLIST, SHBindToParent, SHCreateDataObject, SHParseDisplayName},
    };
    use windows::core::HSTRING;

    unsafe {
        if let Some(directory) = background {
            let name = HSTRING::from(directory.to_string_lossy().into_owned());
            let mut folder_pidl: *mut ITEMIDLIST = std::ptr::null_mut();
            SHParseDisplayName(&name, None, &mut folder_pidl, 0, None)
                .map_err(|error| format!("无法解析目录：{error}"))?;
            let data: IDataObject =
                SHCreateDataObject(Some(folder_pidl), None, None::<&IDataObject>)
                    .map_err(|error| format!("无法创建目录数据对象：{error}"))?;
            return Ok((folder_pidl, data, Vec::new()));
        }

        if paths.is_empty() {
            return Err("没有可操作的项目".into());
        }
        let parent = paths[0]
            .parent()
            .ok_or_else(|| "无法确定父文件夹".to_string())?;
        let parent_name = HSTRING::from(parent.to_string_lossy().into_owned());
        let mut folder_pidl: *mut ITEMIDLIST = std::ptr::null_mut();
        SHParseDisplayName(&parent_name, None, &mut folder_pidl, 0, None)
            .map_err(|error| format!("无法解析父文件夹：{error}"))?;

        let mut absolute_pidls: Vec<*mut ITEMIDLIST> = Vec::new();
        let mut children: Vec<*const ITEMIDLIST> = Vec::new();
        for path in paths {
            let name = HSTRING::from(path.to_string_lossy().into_owned());
            let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
            SHParseDisplayName(&name, None, &mut pidl, 0, None)
                .map_err(|error| format!("无法解析 {}：{error}", path.display()))?;
            let mut child: *mut ITEMIDLIST = std::ptr::null_mut();
            let _folder: windows::Win32::UI::Shell::IShellFolder =
                SHBindToParent(pidl, Some(&mut child))
                    .map_err(|error| format!("无法绑定 {}：{error}", path.display()))?;
            absolute_pidls.push(pidl);
            children.push(child as *const ITEMIDLIST);
        }
        let data: IDataObject =
            SHCreateDataObject(Some(folder_pidl), Some(&children), None::<&IDataObject>)
                .map_err(|error| format!("无法创建文件数据对象：{error}"))?;
        Ok((folder_pidl, data, absolute_pidls))
    }
}

#[cfg(test)]
mod drag_drop_tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// A stand-in for `%TEMP%` inside the workspace: the real temp folder is not
    /// writable for the test process on every machine.
    fn scratch_temp_root() -> PathBuf {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-scratch")
            .join(format!(
                "ykexplorer-drop-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn detects_archiver_temp_folders() {
        let root = scratch_temp_root();
        let dir = root.join("7zE1A2B3C4D");
        fs::create_dir(&dir).unwrap();
        let file = dir.join("报告.txt");
        fs::write(&file, b"payload").unwrap();

        assert_eq!(
            archiver_temp_root_in(&file, &root).as_deref(),
            Some(dir.as_path())
        );
        assert!(is_archiver_temp_root_in(&dir, &root));
        assert!(!is_archiver_temp_root_in(&file, &root));
        assert!(archiver_temp_root_in(&dir.join("子目录").join("a.txt"), &root).is_some());

        let plain = root.join("普通的临时目录");
        fs::create_dir(&plain).unwrap();
        fs::write(plain.join("a.txt"), b"x").unwrap();
        assert!(!is_archiver_temp_root_in(&plain, &root));
        assert!(archiver_temp_root_in(&plain.join("a.txt"), &root).is_none());

        // A folder with an archiver-like name outside the temp root is not one.
        let outside = root.join("elsewhere").join("7zE12345678").join("a.txt");
        assert!(archiver_temp_root_in(&outside, &root).is_none());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn stages_archiver_payload_before_the_source_disappears() {
        let root = scratch_temp_root();
        let dir = root.join("7zE0BADF00D");
        fs::create_dir(&dir).unwrap();
        let file = dir.join("数据.csv");
        fs::write(&file, b"a,b\n1,2\n").unwrap();
        let folder = dir.join("子目录");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("说明.txt"), b"hello").unwrap();
        let staging_root = root.join("YKExplorer-drop-test");

        let staged = stage_archiver_drop_within(
            &[file.clone(), folder.clone()],
            &root,
            staging_root.clone(),
        )
        .expect("should stage");
        assert_eq!(staged.sources.len(), 2);
        assert!(!file.exists());
        assert!(!folder.exists());

        // The archiver deletes its temp folder right after the drop.
        fs::remove_dir_all(&dir).unwrap();

        let csv = staged
            .sources
            .iter()
            .find(|path| path.extension().is_some_and(|ext| ext == "csv"))
            .expect("csv staged");
        assert_eq!(fs::read(csv).unwrap(), b"a,b\n1,2\n");
        let nested = staged
            .sources
            .iter()
            .find(|path| path.is_dir())
            .expect("folder staged");
        assert_eq!(fs::read(nested.join("说明.txt")).unwrap(), b"hello");

        discard_staged_drop(&staged.staging_root);
        assert!(!staging_root.exists());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn leaves_regular_drops_untouched() {
        let root = scratch_temp_root();
        let dir = root.join("regular");
        fs::create_dir(&dir).unwrap();
        let file = dir.join("a.txt");
        fs::write(&file, b"x").unwrap();

        assert!(
            stage_archiver_drop_within(
                std::slice::from_ref(&file),
                &root,
                root.join("YKExplorer-drop-test"),
            )
            .is_none()
        );
        assert!(file.exists());

        let _ = fs::remove_dir_all(&root);
    }
}
