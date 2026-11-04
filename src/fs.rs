use std::{
    path::{Component, Path, PathBuf},
    time::{Duration, SystemTime},
};

use chrono::{DateTime, Local};
use windows::{
    Win32::Storage::FileSystem::{
        GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
    },
    core::HSTRING,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Folder,
    Image,
    Video,
    Audio,
    Document,
    Archive,
    Code,
    Executable,
    Font,
    Other,
}

impl FileKind {
    pub fn from_entry(is_dir: bool, extension: &str) -> Self {
        if is_dir {
            return Self::Folder;
        }
        match extension {
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "svg" | "avif" | "heic"
            | "tif" | "tiff" | "psd" => Self::Image,
            "mp4" | "mkv" | "mov" | "avi" | "wmv" | "webm" | "flv" | "m4v" => Self::Video,
            "mp3" | "wav" | "flac" | "aac" | "ogg" | "m4a" | "wma" | "opus" => Self::Audio,
            "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "txt" | "rtf" | "md"
            | "csv" | "epub" | "odt" | "ods" | "odp" => Self::Document,
            "zip" | "rar" | "7z" | "tar" | "gz" | "bz2" | "xz" | "zst" | "iso" => Self::Archive,
            "rs" | "c" | "h" | "cpp" | "hpp" | "cs" | "js" | "jsx" | "ts" | "tsx" | "py"
            | "go" | "java" | "kt" | "rb" | "php" | "html" | "css" | "scss" | "json" | "toml"
            | "yaml" | "yml" | "xml" | "sql" | "sh" | "ps1" => Self::Code,
            "exe" | "msi" | "com" | "bat" | "cmd" | "lnk" | "appx" | "msix" => Self::Executable,
            "ttf" | "otf" | "woff" | "woff2" | "ttc" => Self::Font,
            _ => Self::Other,
        }
    }

    pub fn label(self, extension: &str) -> String {
        match self {
            Self::Folder => "文件夹".to_string(),
            Self::Image => format!("{} 图像", extension.to_ascii_uppercase()),
            Self::Video => format!("{} 视频", extension.to_ascii_uppercase()),
            Self::Audio => format!("{} 音频", extension.to_ascii_uppercase()),
            Self::Document => match extension {
                "pdf" => "PDF 文档".to_string(),
                "doc" | "docx" => "Word 文档".to_string(),
                "xls" | "xlsx" | "csv" => "Excel 表格".to_string(),
                "ppt" | "pptx" => "PowerPoint 演示".to_string(),
                "txt" => "文本文档".to_string(),
                "md" => "Markdown 文档".to_string(),
                _ => format!("{} 文档", extension.to_ascii_uppercase()),
            },
            Self::Archive => format!("{} 压缩文件", extension.to_ascii_uppercase()),
            Self::Code => match extension {
                "rs" => "Rust 源文件".to_string(),
                "py" => "Python 源文件".to_string(),
                "js" | "jsx" => "JavaScript 文件".to_string(),
                "ts" | "tsx" => "TypeScript 文件".to_string(),
                "json" => "JSON 文件".to_string(),
                "toml" => "TOML 文件".to_string(),
                _ => format!("{} 源文件", extension.to_ascii_uppercase()),
            },
            Self::Executable => "应用程序".to_string(),
            Self::Font => "字体文件".to_string(),
            Self::Other => {
                if extension.is_empty() {
                    "文件".to_string()
                } else {
                    format!("{} 文件", extension.to_ascii_uppercase())
                }
            }
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Self::Folder => "icons/folder.svg",
            Self::Image => "icons/image.svg",
            Self::Video => "icons/video.svg",
            Self::Audio => "icons/music.svg",
            Self::Document => "icons/file.svg",
            Self::Archive => "icons/file.svg",
            Self::Code => "icons/file.svg",
            Self::Executable => "icons/file.svg",
            Self::Font => "icons/file.svg",
            Self::Other => "icons/file.svg",
        }
    }
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub is_hidden: bool,
    pub is_symlink: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub created: Option<SystemTime>,
    pub accessed: Option<SystemTime>,
    pub extension: String,
    pub kind: FileKind,
    pub type_label: String,
    /// Pre-formatted display strings. Formatting dates and sizes during every
    /// frame is surprisingly expensive (chrono conversions per visible row);
    /// these are computed once when the directory is read.
    pub size_label: String,
    pub modified_label: String,
    pub modified_relative: String,
}

impl FileEntry {
    pub fn read(path: PathBuf) -> Option<Self> {
        let link_meta = std::fs::symlink_metadata(&path).ok()?;
        let is_symlink = link_meta.file_type().is_symlink();
        let meta = if is_symlink {
            std::fs::metadata(&path).unwrap_or(link_meta)
        } else {
            link_meta
        };
        let is_dir = meta.is_dir();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        let extension = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let kind = FileKind::from_entry(is_dir, &extension);
        let type_label = kind.label(&extension);
        let size = if is_dir { 0 } else { meta.len() };
        let modified = meta.modified().ok();

        #[cfg(windows)]
        let is_hidden = {
            use std::os::windows::fs::MetadataExt;
            const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
            (meta.file_attributes() & FILE_ATTRIBUTE_HIDDEN) != 0 || name.starts_with('.')
        };
        #[cfg(not(windows))]
        let is_hidden = name.starts_with('.');

        Some(Self {
            path,
            name,
            is_dir,
            is_hidden,
            is_symlink,
            size,
            modified,
            created: meta.created().ok(),
            accessed: meta.accessed().ok(),
            extension,
            kind,
            type_label,
            size_label: if is_dir {
                "—".to_string()
            } else {
                format_size(size)
            },
            modified_label: format_datetime(modified),
            modified_relative: format_relative(modified),
        })
    }
}

pub fn read_directory(path: &Path) -> Result<Vec<FileEntry>, String> {
    let read = std::fs::read_dir(path).map_err(|err| friendly_io_error(err, path))?;
    // Metadata is the expensive part of listing a folder. Large folders are
    // split across a few threads; sorting happens later anyway, so the order
    // in which the chunks finish does not matter.
    let mut paths = Vec::new();
    for entry in read.flatten() {
        paths.push(entry.path());
    }
    if paths.len() < 512 {
        return Ok(paths.into_iter().filter_map(FileEntry::read).collect());
    }

    let workers = std::thread::available_parallelism()
        .map(|count| count.get().clamp(2, 4))
        .unwrap_or(2)
        .min(paths.len());
    let chunk_size = paths.len().div_ceil(workers);
    let mut entries = Vec::with_capacity(paths.len());
    std::thread::scope(|scope| {
        let handles: Vec<_> = paths
            .chunks(chunk_size)
            .map(|chunk| {
                scope.spawn(move || {
                    chunk
                        .iter()
                        .filter_map(|path| FileEntry::read(path.clone()))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        for handle in handles {
            match handle.join() {
                Ok(items) => entries.extend(items),
                Err(_) => log::warn!("directory worker panicked while reading metadata"),
            }
        }
    });
    Ok(entries)
}

pub fn friendly_io_error(err: std::io::Error, path: &Path) -> String {
    use std::io::ErrorKind;
    let name = path.to_string_lossy();
    match err.kind() {
        ErrorKind::NotFound => format!("找不到位置：{name}"),
        ErrorKind::PermissionDenied => format!("没有权限访问：{name}"),
        _ => format!("无法访问 {name}：{err}"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Type,
    Modified,
    Size,
}

impl SortKey {
    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "名称",
            Self::Type => "类型",
            Self::Modified => "修改日期",
            Self::Size => "大小",
        }
    }
}

pub fn compare_entries(a: &FileEntry, b: &FileEntry, key: SortKey, ascending: bool) -> std::cmp::Ordering {
    if a.is_dir != b.is_dir {
        return if a.is_dir {
            std::cmp::Ordering::Less
        } else {
            std::cmp::Ordering::Greater
        };
    }
    let ordering = match key {
        SortKey::Name => natural_cmp(&a.name, &b.name),
        SortKey::Type => cmp_ignore_case(&a.type_label, &b.type_label)
            .then_with(|| natural_cmp(&a.name, &b.name)),
        SortKey::Modified => a
            .modified
            .cmp(&b.modified)
            .then_with(|| natural_cmp(&a.name, &b.name)),
        SortKey::Size => a
            .size
            .cmp(&b.size)
            .then_with(|| natural_cmp(&a.name, &b.name)),
    };
    if ascending { ordering } else { ordering.reverse() }
}

/// A query prepared once and reused for every entry. `visible_indices` used to
/// lowercase the query (and each file name) for every entry on every keystroke.
pub struct FilterMatcher {
    query: String,
}

impl FilterMatcher {
    pub fn new(query: &str) -> Self {
        Self {
            query: query.trim().to_lowercase(),
        }
    }

    pub fn matches(&self, entry: &FileEntry) -> bool {
        if self.query.is_empty() {
            return true;
        }
        if self.query.contains('*') || self.query.contains('?') {
            return wildcard_match(&self.query, &entry.name.to_lowercase());
        }
        contains_ignore_case(&entry.name, &self.query)
            || contains_ignore_case(&entry.type_label, &self.query)
    }
}

fn lower_char(character: char) -> char {
    if character.is_ascii() {
        character.to_ascii_lowercase()
    } else {
        character.to_lowercase().next().unwrap_or(character)
    }
}

fn cmp_ignore_case(a: &str, b: &str) -> std::cmp::Ordering {
    let mut left = a.chars();
    let mut right = b.chars();
    loop {
        match (left.next(), right.next()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(a), Some(b)) => {
                let ordering = lower_char(a).cmp(&lower_char(b));
                if ordering != std::cmp::Ordering::Equal {
                    return ordering;
                }
            }
        }
    }
}

/// Case-insensitive substring test without allocating a lowercase copy of the
/// file name. ASCII fast path covers the overwhelming majority of queries.
fn contains_ignore_case(haystack: &str, needle_lower: &str) -> bool {
    if needle_lower.is_empty() {
        return true;
    }
    if haystack.is_ascii() && needle_lower.is_ascii() {
        let needle = needle_lower.as_bytes();
        return haystack
            .as_bytes()
            .windows(needle.len())
            .any(|window| window.eq_ignore_ascii_case(needle));
    }
    'outer: for (start, _) in haystack.char_indices() {
        let mut needle = needle_lower.chars();
        let mut text = haystack[start..].chars();
        loop {
            match (text.next(), needle.next()) {
                (Some(text), Some(needle)) => {
                    if lower_char(text) != needle {
                        continue 'outer;
                    }
                }
                (_, None) => return true,
                (None, Some(_)) => return false,
            }
        }
    }
    false
}

/// Folders still come first, but the returned indices preserve the original
/// entries array so the UI can avoid cloning every FileEntry on each frame.
pub fn visible_indices(
    entries: &[FileEntry],
    key: SortKey,
    ascending: bool,
    filter: &str,
    show_hidden: bool,
) -> Vec<usize> {
    let matcher = FilterMatcher::new(filter);
    let mut indices: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| (show_hidden || !entry.is_hidden) && matcher.matches(entry))
        .map(|(index, _)| index)
        .collect();
    indices.sort_by(|&a, &b| compare_entries(&entries[a], &entries[b], key, ascending));
    indices
}

/// Lightweight natural ordering: runs of ASCII digits compare numerically.
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let mut ai = a.char_indices().peekable();
    let mut bi = b.char_indices().peekable();
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some((a_pos, ac)), Some((b_pos, bc))) => {
                if ac.is_ascii_digit() && bc.is_ascii_digit() {
                    let a_end = digit_run_end(a, a_pos);
                    let b_end = digit_run_end(b, b_pos);
                    let a_num = a[a_pos..a_end].trim_start_matches('0');
                    let b_num = b[b_pos..b_end].trim_start_matches('0');
                    let ord = a_num.len().cmp(&b_num.len()).then_with(|| a_num.cmp(b_num));
                    if ord != std::cmp::Ordering::Equal {
                        return ord;
                    }
                    while let Some(&(idx, _)) = ai.peek() {
                        if idx >= a_end {
                            break;
                        }
                        ai.next();
                    }
                    while let Some(&(idx, _)) = bi.peek() {
                        if idx >= b_end {
                            break;
                        }
                        bi.next();
                    }
                    continue;
                }
                let a_lower = lower_char(ac);
                let b_lower = lower_char(bc);
                let ord = a_lower.cmp(&b_lower);
                if ord != std::cmp::Ordering::Equal {
                    return ord;
                }
                ai.next();
                bi.next();
            }
        }
    }
}

fn digit_run_end(s: &str, start: usize) -> usize {
    s[start..]
        .char_indices()
        .find(|(_, c)| !c.is_ascii_digit())
        .map(|(i, _)| start + i)
        .unwrap_or(s.len())
}

/// Recursively search `root` (and every subfolder) for entries whose name
/// matches `query`. Symlinked directories are listed but not descended into,
/// which avoids cycles. Returns the hits plus whether `limit` cut the walk short.
pub fn search_directory(
    root: &Path,
    query: &str,
    show_hidden: bool,
    limit: usize,
) -> (Vec<FileEntry>, bool) {
    let mut results = Vec::new();
    if query.trim().is_empty() {
        return (results, false);
    }
    let matcher = FilterMatcher::new(query);
    let mut pending = std::collections::VecDeque::new();
    pending.push_back(root.to_path_buf());
    while let Some(directory) = pending.pop_front() {
        let Ok(read) = std::fs::read_dir(&directory) else {
            continue;
        };
        for item in read.flatten() {
            let Some(entry) = FileEntry::read(item.path()) else {
                continue;
            };
            if !show_hidden && entry.is_hidden {
                continue;
            }
            if entry.is_dir && !entry.is_symlink {
                pending.push_back(entry.path.clone());
            }
            if matcher.matches(&entry) {
                results.push(entry);
                if results.len() >= limit {
                    return (results, true);
                }
            }
        }
    }
    (results, false)
}

fn wildcard_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut mark) = (None, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else if value >= 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

pub fn format_datetime(time: Option<SystemTime>) -> String {
    let Some(time) = time else {
        return "—".to_string();
    };
    let dt: DateTime<Local> = time.into();
    dt.format("%Y/%m/%d %H:%M").to_string()
}

pub fn format_relative(time: Option<SystemTime>) -> String {
    let Some(time) = time else {
        return "—".to_string();
    };
    let now = SystemTime::now();
    match now.duration_since(time) {
        Ok(delta) if delta < Duration::from_secs(60) => "刚刚".to_string(),
        Ok(delta) if delta < Duration::from_secs(3600) => {
            format!("{} 分钟前", delta.as_secs() / 60)
        }
        Ok(delta) if delta < Duration::from_secs(86_400) => {
            format!("{} 小时前", delta.as_secs() / 3600)
        }
        Ok(delta) if delta < Duration::from_secs(86_400 * 7) => {
            format!("{} 天前", delta.as_secs() / 86_400)
        }
        _ => {
            let dt: DateTime<Local> = time.into();
            dt.format("%Y/%m/%d").to_string()
        }
    }
}

#[derive(Debug, Clone)]
pub struct QuickPlace {
    pub label: String,
    pub path: PathBuf,
    pub icon: &'static str,
}

pub fn quick_places() -> Vec<QuickPlace> {
    let mut places = Vec::new();
    if let Some(home) = dirs::home_dir() {
        places.push(QuickPlace {
            label: "主目录".into(),
            path: home,
            icon: "icons/home.svg",
        });
    }
    let candidates = [
        ("桌面", dirs::desktop_dir(), "icons/desktop.svg"),
        ("下载", dirs::download_dir(), "icons/download.svg"),
        ("文档", dirs::document_dir(), "icons/file.svg"),
        ("图片", dirs::picture_dir(), "icons/image.svg"),
        ("音乐", dirs::audio_dir(), "icons/music.svg"),
        ("视频", dirs::video_dir(), "icons/video.svg"),
    ];
    for (label, path, icon) in candidates {
        if let Some(path) = path {
            if path.exists() {
                places.push(QuickPlace {
                    label: label.into(),
                    path,
                    icon,
                });
            }
        }
    }
    places
}

#[derive(Debug, Clone)]
pub struct DriveInfo {
    pub path: PathBuf,
    pub letter: String,
    pub label: String,
    pub total: u64,
    pub free: u64,
    pub is_removable: bool,
    pub is_network: bool,
}

impl DriveInfo {
    pub fn display_name(&self) -> String {
        if self.label.trim().is_empty() {
            if self.is_removable {
                format!("可移动磁盘 ({})", self.letter)
            } else if self.is_network {
                format!("网络驱动器 ({})", self.letter)
            } else {
                format!("本地磁盘 ({})", self.letter)
            }
        } else {
            format!("{} ({})", self.label, self.letter)
        }
    }

    pub fn used_fraction(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            ((self.total.saturating_sub(self.free)) as f64 / self.total as f64) as f32
        }
    }
}

pub fn drives() -> Vec<DriveInfo> {
    let mask = unsafe { GetLogicalDrives() };
    if mask == 0 {
        return Vec::new();
    }
    let mut result = Vec::new();
    for i in 0..26 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = (b'A' + i as u8) as char;
        let root = format!("{letter}:\\");
        let wide = HSTRING::from(&root);
        let drive_type = unsafe { GetDriveTypeW(&wide) };
        const DRIVE_REMOVABLE: u32 = 2;
        const DRIVE_FIXED: u32 = 3;
        const DRIVE_REMOTE: u32 = 4;
        const DRIVE_CDROM: u32 = 5;
        if !matches!(
            drive_type,
            DRIVE_REMOVABLE | DRIVE_FIXED | DRIVE_REMOTE | DRIVE_CDROM
        ) {
            continue;
        }
        let mut name_buf = [0u16; 261];
        let mut fs_buf = [0u16; 32];
        let label = unsafe {
            GetVolumeInformationW(
                &wide,
                Some(&mut name_buf),
                None,
                None,
                None,
                Some(&mut fs_buf),
            )
            .ok()
            .map(|_| utf16z_to_string(&name_buf))
            .unwrap_or_default()
        };
        let mut free_to_caller = 0u64;
        let mut total = 0u64;
        let mut free = 0u64;
        unsafe {
            let _ = GetDiskFreeSpaceExW(
                &wide,
                Some(&mut free_to_caller),
                Some(&mut total),
                Some(&mut free),
            );
        }
        result.push(DriveInfo {
            path: PathBuf::from(&root),
            letter: format!("{letter}:"),
            label,
            total,
            free,
            is_removable: drive_type == DRIVE_REMOVABLE || drive_type == DRIVE_CDROM,
            is_network: drive_type == DRIVE_REMOTE,
        });
    }
    result
}

fn utf16z_to_string(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

/// Drive enumeration costs several shell calls per volume. Besides the startup
/// window, every new explorer window asks for the same list, so cache it for a
/// few seconds (an explicit refresh still forces a fresh scan).
pub fn drives_cached() -> Vec<DriveInfo> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<(std::time::Instant, Vec<DriveInfo>)>> =
        std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(|| {
        std::sync::Mutex::new((std::time::Instant::now(), drives()))
    });
    let mut guard = cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard.0.elapsed() > Duration::from_secs(5) {
        guard.1 = drives();
        guard.0 = std::time::Instant::now();
    }
    guard.1.clone()
}

/// Build a quick-access entry for an arbitrary path, reusing the built-in
/// label/icon when it is one of the defaults.
pub fn quick_place_for(path: &Path) -> QuickPlace {
    for place in quick_places() {
        if path_eq(&place.path, path) {
            return place;
        }
    }
    let label = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    QuickPlace {
        label,
        path: path.to_path_buf(),
        icon: "icons/folder.svg",
    }
}

pub fn default_start_dir() -> PathBuf {
    if let Some(path) = std::env::args()
        .skip(1)
        .find(|argument| !argument.starts_with("--"))
        .map(PathBuf::from)
    {
        if path.exists() {
            return path;
        }
    }
    dirs::download_dir()
        .or_else(dirs::home_dir)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("C:\\"))
}

pub fn expand_user_path(input: &str) -> Result<PathBuf, String> {
    let input = input.trim().trim_matches('"');
    if input.is_empty() {
        return Err("请输入路径".into());
    }

    let mut expanded = input.to_string();
    if expanded == "~" {
        if let Some(home) = dirs::home_dir() {
            return Ok(home);
        }
    } else if let Some(rest) = expanded.strip_prefix("~\\").or_else(|| expanded.strip_prefix("~/"))
    {
        if let Some(home) = dirs::home_dir() {
            expanded = home.join(rest).to_string_lossy().into_owned();
        }
    }

    if expanded.contains('%') {
        let wide = HSTRING::from(&expanded);
        let mut buf = vec![0u16; 32768];
        let len = unsafe {
            windows::Win32::System::Environment::ExpandEnvironmentStringsW(&wide, Some(&mut buf))
        };
        if len > 0 && (len as usize) <= buf.len() {
            expanded = utf16z_to_string(&buf);
        }
    }

    let path = PathBuf::from(expanded);
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map_err(|err| err.to_string())?
            .join(path)
    };
    if path.exists() {
        Ok(path)
    } else {
        Err(format!("路径不存在：{}", path.display()))
    }
}


pub fn path_segments(path: &Path) -> Vec<(String, PathBuf)> {
    let mut segments = Vec::new();
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => {
                if let Some(letter) = prefix.as_os_str().to_str() {
                    let clean = letter.trim_end_matches('\\');
                    current.push(format!("{clean}\\"));
                    segments.push((clean.to_string(), current.clone()));
                }
            }
            Component::RootDir => {}
            Component::Normal(part) => {
                current.push(part);
                let label = part.to_string_lossy().into_owned();
                segments.push((label, current.clone()));
            }
            Component::CurDir | Component::ParentDir => {}
        }
    }
    segments
}

pub fn path_display_name(path: &Path) -> String {
    if let Some(segments) = path_segments(path).last() {
        segments.0.clone()
    } else {
        path.to_string_lossy().into_owned()
    }
}


pub fn path_eq(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy())
}



#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_human_friendly() {
        assert_eq!(format_size(500), "500 B");
        assert_eq!(format_size(2048), "2.00 KB");
        assert_eq!(format_size(10 * 1024 * 1024), "10.0 MB");
    }

    #[test]
    fn search_walks_subfolders() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-scratch")
            .join(format!("ykexplorer-search-{}", std::process::id()));
        let nested = root.join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(root.join("报告-根.txt"), b"1").unwrap();
        std::fs::write(nested.join("报告-深.txt"), b"2").unwrap();
        std::fs::write(nested.join("其他.txt"), b"3").unwrap();

        let (hits, truncated) = search_directory(&root, "报告", true, 100);
        let mut names: Vec<String> = hits.iter().map(|entry| entry.name.clone()).collect();
        names.sort();
        assert_eq!(names, vec!["报告-根.txt", "报告-深.txt"]);
        assert!(!truncated);

        let (limited, cut) = search_directory(&root, "*.txt", true, 2);
        assert_eq!(limited.len(), 2);
        assert!(cut);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn wildcard_filter_works() {
        assert!(wildcard_match("*.txt", "readme.txt"));
        assert!(wildcard_match("照片?.png", "照片1.png"));
        assert!(!wildcard_match("*.txt", "readme.md"));
    }

    #[test]
    fn path_segments_handle_windows_roots() {
        let segments = path_segments(Path::new(r"C:\Users\Alice\Downloads"));
        assert_eq!(segments[0].0, "C:");
        assert_eq!(segments.last().unwrap().0, "Downloads");
    }
}

#[cfg(test)]
mod visible_tests {
    use super::*;

    fn entry(name: &str, is_dir: bool, hidden: bool, ext: &str) -> FileEntry {
        FileEntry {
            path: PathBuf::from(name),
            name: name.to_string(),
            is_dir,
            is_hidden: hidden,
            is_symlink: false,
            size: 0,
            modified: None,
            created: None,
            accessed: None,
            extension: ext.to_string(),
            kind: FileKind::from_entry(is_dir, ext),
            type_label: FileKind::from_entry(is_dir, ext).label(ext),
            size_label: if is_dir { "—".into() } else { "0 B".into() },
            modified_label: "—".into(),
            modified_relative: "—".into(),
        }
    }

    #[test]
    fn visible_indices_folders_first_filter_and_hidden() {
        let entries = vec![
            entry("b.txt", false, false, "txt"),
            entry("folder", true, false, ""),
            entry(".hidden", false, true, ""),
            entry("a.png", false, false, "png"),
        ];
        let all = visible_indices(&entries, SortKey::Name, true, "", false);
        assert_eq!(
            all.iter().map(|&i| entries[i].name.as_str()).collect::<Vec<_>>(),
            vec!["folder", "a.png", "b.txt"]
        );
        let filtered = visible_indices(&entries, SortKey::Name, true, "*.txt", false);
        assert_eq!(filtered.len(), 1);
        assert_eq!(entries[filtered[0]].name, "b.txt");
        let hidden = visible_indices(&entries, SortKey::Name, true, "", true);
        assert_eq!(hidden.len(), 4);
    }

    #[test]
    fn filter_is_case_insensitive_and_supports_wildcards() {
        let file = entry("Quarterly Report.TXT", false, false, "txt");
        let matches = |query: &str| FilterMatcher::new(query).matches(&file);
        assert!(matches("report"));
        assert!(matches("REPORT"));
        assert!(matches("*.txt"));
        assert!(matches("文本文档"));
        assert!(!matches("missing"));
    }

    #[test]
    fn entry_display_labels_are_precomputed() {
        let file = entry("a.txt", false, false, "txt");
        assert_eq!(file.size_label, "0 B");
        assert_eq!(file.modified_label, "—");
    }
}
