use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, ExternalPaths, FocusHandle, Focusable,
    Animation, AnimationExt, Hsla, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ObjectFit, Pixels, Point, Render, ease_out_quint,
    ScrollStrategy, SharedString, Subscription, Timer, UniformListScrollHandle, Window,
    actions, div, img, prelude::*, px, svg, uniform_list,
};

use crate::{
    clipboard,
    drag_host,
    drag_out::{self, DragDropResult, DragOutcome},
    fs::{self, DriveInfo, FileEntry, FileKind, QuickPlace, SortKey},
    ops,
    shell_registry,
    text_input::{TextInput, TextInputEvent},
    theme,
};

actions!(
    fpilot,
    [
        Back,
        Forward,
        Up,
        Refresh,
        FocusPath,
        FocusSearch,
        NewTab,
        CloseTab,
        NextTab,
        PrevTab,
        OpenSelected,
        OpenSelectedNewTab,
        CopyFiles,
        CutFiles,
        PasteFiles,
        TrashSelection,
        PermanentDelete,
        RenameSelection,
        NewFolder,
        NewFile,
        SelectAll,
        SelectPrev,
        SelectNext,
        SelectExtendPrev,
        SelectExtendNext,
        SelectFirst,
        SelectLast,
        SelectExtendFirst,
        SelectExtendLast,
        ViewDetails,
        ViewGrid,
        ViewSplit,
        ToggleSidebar,
        ToggleHidden,
        CopyPath,
        Undo,
        RevealSelection,
        OpenSettings,
        Cancel,
    ]
);

pub const FILE_MANAGER_CONTEXT: &str = "FileManager";

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    Details,
    Grid,
    Split,
}

impl ViewMode {
    fn icon(self) -> &'static str {
        match self {
            Self::Details => "icons/list.svg",
            Self::Grid => "icons/grid.svg",
            Self::Split => "icons/split.svg",
        }
    }

}

struct TabState {
    id: u64,
    path: PathBuf,
    title: String,
    history: Vec<PathBuf>,
    history_index: usize,
    entries: Arc<Vec<FileEntry>>,
    visible: Arc<Vec<usize>>,
    selected: BTreeSet<PathBuf>,
    filter: String,
    cursor: usize,
    anchor: usize,
    loading: bool,
    load_id: u64,
    error: Option<String>,
    details_scroll: UniformListScrollHandle,
    grid_scroll: UniformListScrollHandle,
    /// Set while this tab shows recursive search results for `filter`.
    search_root: Option<PathBuf>,
    searching: bool,
    search_truncated: bool,
}

impl TabState {
    fn new(id: u64, path: PathBuf) -> Self {
        let title = fs::path_display_name(&path);
        Self {
            id,
            history: vec![path.clone()],
            history_index: 0,
            path,
            title,
            entries: Arc::new(Vec::new()),
            visible: Arc::new(Vec::new()),
            selected: BTreeSet::new(),
            filter: String::new(),
            cursor: 0,
            anchor: 0,
            loading: false,
            load_id: 0,
            error: None,
            details_scroll: UniformListScrollHandle::new(),
            grid_scroll: UniformListScrollHandle::new(),
            search_root: None,
            searching: false,
            search_truncated: false,
        }
    }

    fn selected_paths(&self) -> Vec<PathBuf> {
        self.visible
            .iter()
            .filter_map(|&index| {
                let entry = &self.entries[index];
                self.selected
                    .contains(&entry.path)
                    .then(|| entry.path.clone())
            })
            .collect()
    }

    fn selected_entries(&self) -> Vec<FileEntry> {
        self.visible
            .iter()
            .filter_map(|&index| {
                let entry = &self.entries[index];
                self.selected
                    .contains(&entry.path)
                    .then(|| entry.clone())
            })
            .collect()
    }

    fn selected_size(&self) -> u64 {
        self.selected_entries()
            .iter()
            .filter(|entry| !entry.is_dir)
            .map(|entry| entry.size)
            .sum()
    }

    fn entry_at_visible(&self, display_index: usize) -> Option<&FileEntry> {
        let entry_index = *self.visible.get(display_index)?;
        self.entries.get(entry_index)
    }

    fn selected_first_entry(&self) -> Option<&FileEntry> {
        let path = self.selected.iter().next()?;
        self.entries.iter().find(|entry| &entry.path == path)
    }

    fn preview_entry(&self) -> Option<&FileEntry> {
        self.selected_first_entry().or_else(|| {
            self.visible
                .get(self.cursor)
                .and_then(|index| self.entries.get(*index))
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum EditTarget {
    Search,
    Path,
    Rename(PathBuf),
    NewFolder,
    NewFile,
}

struct EditorSession {
    editor: Entity<TextInput>,
    target: EditTarget,
    _subscription: Subscription,
}

enum Modal {
    ConfirmPermanentDelete { paths: Vec<PathBuf> },
    Error { title: String, message: String },
    Settings,
}

#[derive(Clone, Copy, Debug)]
enum MenuCommand {
    Open,
    OpenInNewTab,
    Copy,
    Cut,
    Paste,
    Rename,
    Trash,
    CopyPath,
    Reveal,
    NewFolder,
    NewFile,
    Refresh,
    SelectAll,
    UndoAction,
    SystemOpenWith,
    SystemPrint,
    SystemRunAsAdmin,
    SystemPowerShell,
    SystemTerminal,
    SystemProperties,
    Registry(usize),
    ShellHandlerCommand { menu: usize, id: u32 },
}

struct ContextMenuState {
    position: Point<Pixels>,
    target: Option<PathBuf>,
    is_dir: bool,
}

#[derive(Clone, Copy)]
enum ToastKind {
    Info,
    Success,
    Error,
}

struct Toast {
    message: String,
    kind: ToastKind,
}

struct PendingDrag {
    start_screen: Option<(i32, i32)>,
    path: PathBuf,
}

struct DragRequest {
    paths: Vec<PathBuf>,
    shift: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsPage {
    General,
    ContextMenu,
    QuickAccess,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShellMenuItem {
    OpenWith,
    Print,
    RunAsAdmin,
    PowerShell,
    Terminal,
    Properties,
}

impl ShellMenuItem {
    const ALL: [Self; 6] = [
        Self::OpenWith,
        Self::Print,
        Self::RunAsAdmin,
        Self::PowerShell,
        Self::Terminal,
        Self::Properties,
    ];

    fn title(self) -> &'static str {
        match self {
            Self::OpenWith => "打开方式…",
            Self::Print => "打印",
            Self::RunAsAdmin => "以管理员身份运行",
            Self::PowerShell => "在此处打开 PowerShell",
            Self::Terminal => "在 Windows 终端中打开",
            Self::Properties => "属性",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::OpenWith => "使用 Windows“打开方式”选择程序，仅对单个文件显示。",
            Self::Print => "调用文件的系统打印动词，具体是否可用取决于文件关联。",
            Self::RunAsAdmin => "对 .exe/.msi/.bat/.cmd/.ps1 等以提升权限启动。",
            Self::PowerShell => "在目标文件夹中打开 PowerShell，并保留窗口。",
            Self::Terminal => "优先使用 Windows Terminal，未安装时回退到 PowerShell。",
            Self::Properties => "打开资源管理器的原生属性对话框。",
        }
    }

    fn enabled(self, prefs: &ShellMenuPrefs) -> bool {
        match self {
            Self::OpenWith => prefs.open_with,
            Self::Print => prefs.print,
            Self::RunAsAdmin => prefs.run_as_admin,
            Self::PowerShell => prefs.powershell,
            Self::Terminal => prefs.terminal,
            Self::Properties => prefs.properties,
        }
    }

    fn toggle(self, prefs: &mut ShellMenuPrefs) {
        let value = match self {
            Self::OpenWith => &mut prefs.open_with,
            Self::Print => &mut prefs.print,
            Self::RunAsAdmin => &mut prefs.run_as_admin,
            Self::PowerShell => &mut prefs.powershell,
            Self::Terminal => &mut prefs.terminal,
            Self::Properties => &mut prefs.properties,
        };
        *value = !*value;
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
struct ShellMenuPrefs {
    open_with: bool,
    print: bool,
    run_as_admin: bool,
    powershell: bool,
    terminal: bool,
    properties: bool,
}

impl Default for ShellMenuPrefs {
    fn default() -> Self {
        Self {
            open_with: true,
            print: true,
            run_as_admin: true,
            powershell: true,
            terminal: true,
            properties: true,
        }
    }
}

/// One reversible user action. Kept newest-last in `PilotApp::undo_stack`.
#[derive(Clone, Debug)]
enum UndoAction {
    Rename { from: PathBuf, to: PathBuf },
    Created { paths: Vec<PathBuf> },
    Moved { pairs: Vec<(PathBuf, PathBuf)> },
    Trashed { original: Vec<PathBuf> },
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct PersistedSettings {
    font_family: Option<String>,
    show_hidden: bool,
    shell_menu: ShellMenuPrefs,
    registry_menu: BTreeMap<String, bool>,
    shellex_menu: BTreeMap<String, bool>,
    replace_explorer: bool,
    quick_places: Option<Vec<PathBuf>>,
}

pub struct PilotApp {
    focus_handle: FocusHandle,
    tabs: Vec<TabState>,
    active_tab: usize,
    next_tab_id: u64,
    quick_places: Vec<QuickPlace>,
    drives: Vec<DriveInfo>,
    view_mode: ViewMode,
    sort_key: SortKey,
    sort_asc: bool,
    show_hidden: bool,
    show_sidebar: bool,
    font_family: String,
    fallback_fonts: Vec<String>,
    font_options: Vec<String>,
    editor: Option<EditorSession>,
    modal: Option<Modal>,
    context_menu: Option<ContextMenuState>,
    toast: Option<Toast>,
    toast_generation: u64,
    operation: Option<String>,
    pending_drag: Option<PendingDrag>,
    pending_select: Option<PathBuf>,
    shell_menu: ShellMenuPrefs,
    registry_items: Vec<shell_registry::RegistryMenuItem>,
    registry_menu: BTreeMap<String, bool>,
    shellex_items: Vec<shell_registry::ShellHandler>,
    shellex_menu: BTreeMap<String, bool>,
    shell_handler_menus: Vec<ops::HandlerMenu>,
    context_submenu: Option<usize>,
    context_submenu_position: Option<Point<Pixels>>,
    rubber_band: Option<(Point<Pixels>, Point<Pixels>)>,
    undo_stack: Vec<UndoAction>,
    close_hovered: bool,
    hovered_row: Option<usize>,
    hover_prev: Option<usize>,
    place_drag: Option<usize>,
    place_drag_start: Option<Point<Pixels>>,
    place_drag_pos: Option<Point<Pixels>>,
    place_drag_outside: bool,
    suppress_place_click: bool,
    hovered_side: Option<usize>,
    hover_prev_side: Option<usize>,
    side_generation: u64,
    hover_generation: u64,
    flash_counter: u64,
    open_flash: Option<(String, u64)>,
    replace_explorer: bool,
    settings_page: SettingsPage,
    pending_focus_list: bool,
    initial_focus_done: bool,
    window_handle: Option<windows::Win32::Foundation::HWND>,
}

impl Focusable for PilotApp {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl PilotApp {
    #[cfg(debug_assertions)]
    pub(crate) fn start_debug_worker(
        &mut self,
        requests: std::sync::mpsc::Receiver<crate::debug_ipc::DebugRequest>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_millis(25)).await;
                while let Ok(request) = requests.try_recv() {

                    if let Some(path) = request.command.strip_prefix("drag-host ") {
                        let path = PathBuf::from(path.trim().trim_matches('"'));
                        if path.exists() {
                            let hwnd = this
                                .update(cx, |app, _| app.window_handle)
                                .ok()
                                .flatten();
                            match hwnd {
                                Some(hwnd) => {
                                    let queued =
                                        drag_host::request_drag(hwnd, vec![path.clone()], false);
                                    let _ = request.response.send(format!(
                                        "ok drag-host queued={queued} path={}",
                                        path.display()
                                    ));
                                }
                                None => {
                                    let _ = request
                                        .response
                                        .send("error drag-host no hwnd".to_string());
                                }
                            }
                        } else {
                            let _ = request
                                .response
                                .send(format!("error drag-host missing {}", path.display()));
                        }
                        continue;
                    }
                    let response = this.update(cx, |app, cx| {
                        app.handle_debug_command(&request.command, cx)
                    });
                    let Ok(response) = response else {
                        return;
                    };
                    let _ = request.response.send(response);
                }
            }
        })
        .detach();
    }

    #[cfg(debug_assertions)]
    fn handle_debug_command(&mut self, command: &str, cx: &mut Context<Self>) -> String {
        let (verb, argument) = command
            .split_once(' ')
            .map(|(verb, argument)| (verb, argument.trim()))
            .unwrap_or((command.trim(), ""));
        match verb {
            "ping" => "pong".to_string(),
            "state" => serde_json::json!({
                "path": self.active().path.display().to_string(),
                "title": self.active().title,
                "view": match self.view_mode {
                    ViewMode::Details => "details",
                    ViewMode::Grid => "grid",
                    ViewMode::Split => "split",
                },
                "tabs": self.tabs.len(),
                "active_tab": self.active_tab,
                "entries": self.active().entries.len(),
                "visible": self.active().visible.len(),
                "selected": self.selected_paths().iter().map(|path| path.display().to_string()).collect::<Vec<_>>(),
                "filter": self.active().filter,
                "hovered_row": self.hovered_row,
                "hovered_side": self.hovered_side,
                "place_drag": self.place_drag,
                "place_drag_outside": self.place_drag_outside,
                "search_root": self.active().search_root.as_ref().map(|path| path.display().to_string()),
                "searching": self.active().searching,
                "search_truncated": self.active().search_truncated,
                "loading": self.active().loading,
                "error": self.active().error,
                "show_hidden": self.show_hidden,
                "show_sidebar": self.show_sidebar,
                "rubber_band": self.rubber_band.is_some(),
                "replace_explorer": self.replace_explorer,
                "font": self.font_family,
                "modal": match self.modal {
                    Some(Modal::ConfirmPermanentDelete { .. }) => "permanent-delete",
                    Some(Modal::Error { .. }) => "error",
                    Some(Modal::Settings) => "settings",
                    None => "none",
                },
                "editor": self.editor.as_ref().map(|editor| format!("{:?}", editor.target)),
                "settings_page": match self.settings_page {
                    SettingsPage::General => "general",
                    SettingsPage::ContextMenu => "context-menu",
                    SettingsPage::QuickAccess => "quick-access",
                },
                "shell_menu": {
                    "open_with": self.shell_menu.open_with,
                    "print": self.shell_menu.print,
                    "run_as_admin": self.shell_menu.run_as_admin,
                    "powershell": self.shell_menu.powershell,
                    "terminal": self.shell_menu.terminal,
                    "properties": self.shell_menu.properties,
                },
                "registry_items": self.registry_items.len(),
                "registry_enabled": self.registry_menu.len(),
                "shellex_items": self.shellex_items.len(),
                "shellex_enabled": self.shellex_menu.len(),
                "pending_drag": self.pending_drag.as_ref().map(|pending| pending.path.display().to_string()),
                "context_menu": self.context_menu.as_ref().and_then(|menu| menu.target.as_ref()).map(|path| path.display().to_string()),
            })
            .to_string(),
            "navigate" => {
                if argument.is_empty() {
                    return "error missing path".to_string();
                }
                let path = PathBuf::from(argument.trim_matches('"'));
                if !path.is_dir() {
                    return format!("error not a directory: {}", path.display());
                }
                self.navigate_to(path.clone(), cx);
                format!("ok navigate {}", path.display())
            }
            "view" => {
                let view = match argument {
                    "details" => ViewMode::Details,
                    "grid" => ViewMode::Grid,
                    "split" => ViewMode::Split,
                    _ => return "error view must be details|grid|split".to_string(),
                };
                self.set_view(view, cx);
                format!("ok view {argument}")
            }
            "hidden" => {
                match argument {
                    "on" => self.show_hidden = true,
                    "off" => self.show_hidden = false,
                    "toggle" => self.show_hidden = !self.show_hidden,
                    _ => return "error hidden must be on|off|toggle".to_string(),
                }
                self.rebuild_visible();
                self.save_settings();
                cx.notify();
                format!("ok hidden {}", self.show_hidden)
            }
            "refresh" => {
                self.reload_active(cx);
                "ok refresh".to_string()
            }
            "shellex" => {
                if argument == "list" {
                    let items: Vec<serde_json::Value> = shell_registry::enumerate_handlers()
                        .into_iter()
                        .map(|handler| {
                            serde_json::json!({
                                "id": handler.id,
                                "name": handler.name,
                                "clsid": format!("{:?}", handler.clsid),
                                "scopes": handler.scopes.iter().map(|scope| scope.slug()).collect::<Vec<_>>(),
                                "hive": if handler.from_hkcu { "HKCU" } else { "HKLM" },
                                "program": handler.program.as_ref().map(|path| path.display().to_string()),
                                "dll": handler.dll.as_ref().map(|path| path.display().to_string()),
                            })
                        })
                        .collect();
                    return serde_json::to_string(&items).unwrap_or_else(|_| "[]".to_string());
                }
                if let Some(name) = argument.strip_prefix("invoke ") {
                    let name = name.trim();
                    let handler = shell_registry::enumerate_handlers()
                        .into_iter()
                        .find(|handler| handler.name.eq_ignore_ascii_case(name));
                    let Some(handler) = handler else {
                        return format!("error handler not found: {name}");
                    };
                    let paths = self.selected_paths();
                    if paths.is_empty() {
                        return "error no selection".to_string();
                    }
                    let Some(hwnd) = self.window_handle else {
                        return "error no hwnd".to_string();
                    };
                    return match ops::build_handler_menu(
                        &handler.clsid,
                        &handler.name,
                        &paths,
                        None,
                    ) {
                        Ok(menu) => match menu.items.first() {
                            Some(item) => match ops::invoke_handler_command(
                                &menu.context,
                                item.id,
                                hwnd,
                            ) {
                                Ok(()) => format!("ok invoked {} -> {}", handler.name, item.label),
                                Err(error) => format!("error {}", error),
                            },
                            None => "error extension produced no commands".to_string(),
                        },
                        Err(error) => format!("error {}", error),
                    };
                }
                if let Some(name) = argument.strip_prefix("items ") {
                    let name = name.trim();
                    let handler = shell_registry::enumerate_handlers()
                        .into_iter()
                        .find(|handler| handler.name.eq_ignore_ascii_case(name));
                    let Some(handler) = handler else {
                        return format!("error handler not found: {name}");
                    };
                    let paths = self.selected_paths();
                    if paths.is_empty() {
                        return "error no selection".to_string();
                    }
                    return match ops::build_handler_menu(
                        &handler.clsid,
                        &handler.name,
                        &paths,
                        None,
                    ) {
                        Ok(menu) => menu
                            .items
                            .iter()
                            .map(|item| format!("{}={}", item.id, item.label))
                            .collect::<Vec<_>>()
                            .join(" | "),
                        Err(error) => format!("error {}", error),
                    };
                }
                if let Some(name) = argument.strip_prefix("toggle ") {
                    let name = name.trim();
                    let handler = shell_registry::enumerate_handlers()
                        .into_iter()
                        .find(|handler| handler.name.eq_ignore_ascii_case(name));
                    let Some(handler) = handler else {
                        return format!("error handler not found: {name}");
                    };
                    self.toggle_shellex(handler.id.clone(), cx);
                    return format!(
                        "ok shellex toggled {} enabled={}",
                        handler.id,
                        self.shellex_menu.contains_key(&handler.id)
                    );
                }
                if argument == "enabled" {
                    return format!(
                        "enabled: {}",
                        self.shellex_menu
                            .keys()
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
                "error shellex must be list|menu <name>|toggle <name>|enabled".to_string()
            }
            "registry" => {
                if argument == "refresh" || argument.is_empty() {
                    self.refresh_registry_menu(cx);
                    return format!(
                        "ok registry refreshed ({} items, {} enabled)",
                        self.registry_items.len(),
                        self.registry_menu.len()
                    );
                }
                if let Some(id) = argument.strip_prefix("toggle ") {
                    let id = id.trim().to_string();
                    if !self.registry_items.iter().any(|item| item.id == id) {
                        return format!("error unknown registry id {id}");
                    }
                    self.toggle_registry_menu(id.clone(), cx);
                    return format!(
                        "ok registry toggled {id} enabled={}",
                        self.registry_menu.contains_key(&id)
                    );
                }
                if argument == "enabled" {
                    return format!(
                        "enabled: {}",
                        self.registry_menu
                            .keys()
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
                if argument == "list" {
                    let items: Vec<serde_json::Value> = self
                        .registry_items
                        .iter()
                        .map(|item| {
                            serde_json::json!({
                                "id": item.id,
                                "scope": item.scope.slug(),
                                "verb": item.verb,
                                "title": item.title,
                                "hive": if item.from_hkcu { "HKCU" } else { "HKLM" },
                            })
                        })
                        .collect();
                    return serde_json::to_string(&items).unwrap_or_else(|_| "[]".to_string());
                }
                "error registry must be refresh|list".to_string()
            }
            "explorer" => match argument.trim() {
                "enable" => match crate::explorer::enable() {
                    Ok(()) => {
                        self.replace_explorer = true;
                        self.save_settings();
                        "ok explorer enabled".to_string()
                    }
                    Err(error) => format!("error {error}"),
                },
                "disable" => match crate::explorer::disable() {
                    Ok(()) => {
                        self.replace_explorer = false;
                        self.save_settings();
                        "ok explorer disabled".to_string()
                    }
                    Err(error) => format!("error {error}"),
                },
                _ => format!(
                    "explorer enabled={} marker={}",
                    self.replace_explorer,
                    crate::explorer::is_enabled()
                ),
            },
            "rubber" => {
                if argument.trim() == "off" {
                    self.rubber_band = None;
                    cx.notify();
                    "ok rubber off".to_string()
                } else {
                    self.rubber_band = Some((
                        Point::new(px(280.), px(230.)),
                        Point::new(px(760.), px(560.)),
                    ));
                    cx.notify();
                    "ok rubber on".to_string()
                }
            }
            "rename-test" => {
                let Some((path, name)) = argument.split_once('|') else {
                    return "error expected path|newname".to_string();
                };
                let path = PathBuf::from(path.trim().trim_matches('"'));
                if !path.exists() {
                    return format!("error missing {}", path.display());
                }
                self.submit_edit(
                    EditTarget::Rename(path.clone()),
                    name.trim().to_string(),
                    cx,
                );
                format!("ok rename-test {} stack={}", path.display(), self.undo_stack.len())
            }
            "mkdir-test" => {
                let name = argument.trim().to_string();
                self.submit_edit(EditTarget::NewFolder, name.clone(), cx);
                format!("ok mkdir-test {name} stack={}", self.undo_stack.len())
            }
            "config" => {
                let path = config_file().map(|p| p.display().to_string());
                format!("config={path:?}")
            }
            "places" => {
                let arg = argument.trim();
                if arg.is_empty() {
                    let list: Vec<String> = self
                        .quick_places
                        .iter()
                        .map(|place| place.label.clone())
                        .collect();
                    list.join(",")
                } else if arg == "reset" {
                    self.reset_quick_places(cx);
                    format!("ok places reset ({})", self.quick_places.len())
                } else if let Some(path) = arg.strip_prefix("add ") {
                    self.add_quick_place(PathBuf::from(path.trim().trim_matches('"')), cx);
                    format!("ok places add ({})", self.quick_places.len())
                } else if let Some(path) = arg.strip_prefix("toggle ") {
                    self.toggle_default_quick_place(
                        PathBuf::from(path.trim().trim_matches('"')),
                        cx,
                    );
                    format!("ok places toggle ({})", self.quick_places.len())
                } else if let Some(path) = arg.strip_prefix("remove ") {
                    let path = PathBuf::from(path.trim().trim_matches('"'));
                    self.remove_quick_place(&path, cx);
                    format!("ok places remove ({})", self.quick_places.len())
                } else {
                    "error places [add <path>|remove <path>|reset]".to_string()
                }
            }
            "hover" => {
                let value = argument.trim();
                match value.parse::<usize>() {
                    Ok(index) => {
                        let from = self.hovered_row.unwrap_or(index);
                        self.set_hovered_row(Some(index), from, cx);
                        format!("ok hover {index} gen={}", self.hover_generation)
                    }
                    Err(_) => {
                        let from = self.hovered_row.unwrap_or(0);
                        self.set_hovered_row(None, from, cx);
                        "ok hover cleared".to_string()
                    }
                }
            }
            "undo" => {
                self.undo(cx);
                format!("ok undo ({} left)", self.undo_stack.len())
            }
            "search" => {
                let query = argument.trim();
                if query.is_empty() || query == "off" {
                    self.clear_search(cx);
                    "ok search cleared".to_string()
                } else {
                    let query = query.to_string();
                    self.start_search(query.clone(), cx);
                    format!("ok search {query}")
                }
            }
            "trash" => {
                self.trash_selection(cx);
                "ok trash".to_string()
            }
            "open" => {
                let target = self
                    .active()
                    .selected_entries()
                    .into_iter()
                    .next()
                    .or_else(|| self.active().preview_entry().cloned());
                match target {
                    Some(entry) => {
                        let name = entry.name.clone();
                        self.open_entry(entry, false, cx);
                        format!("ok open {name}")
                    }
                    None => "error no selection".to_string(),
                }
            }
            "settings" => {
                match argument {
                    "open" => {
                        self.show_settings(cx);
                        return "ok settings open".to_string();
                    }
                    "context" => {
                        self.settings_page = SettingsPage::ContextMenu;
                        self.show_settings(cx);
                        return "ok settings context".to_string();
                    }
                    "quick" => {
                        self.settings_page = SettingsPage::QuickAccess;
                        self.show_settings(cx);
                        return "ok settings quick".to_string();
                    }
                    "registry" => {
                        self.registry_items = shell_registry::enumerate();
                        self.shellex_items = shell_registry::enumerate_handlers();
                        self.settings_page = SettingsPage::ContextMenu;
                        self.show_settings(cx);
                        return format!(
                            "ok settings registry ({} items, {} enabled)",
                            self.registry_items.len(),
                            self.registry_menu.len()
                        );
                    }
                    "general" => {
                        self.settings_page = SettingsPage::General;
                        self.show_settings(cx);
                        return "ok settings general".to_string();
                    }
                    "close" => {
                        self.modal = None;
                        cx.notify();
                        return "ok settings close".to_string();
                    }
                    _ => {}
                }
                "error settings must be open|general|context|quick|registry|close".to_string()
            }
            "font" => {
                if argument.is_empty() {
                    return "error missing font".to_string();
                }
                let family = argument.trim_matches('"').to_string();
                if !self.font_options.iter().any(|font| font.eq_ignore_ascii_case(&family)) {
                    return format!("error unknown font {family}");
                }
                self.choose_font(family.clone(), cx);
                format!("ok font {family}")
            }
            "copy-test" => {
                let Some((source, destination)) = argument.split_once('|') else {
                    return "error expected source|destination".to_string();
                };
                let cut = destination.contains(",move");
                let destination = destination.replace(",move", "");
                let source = PathBuf::from(source.trim().trim_matches('"'));
                let destination = PathBuf::from(destination.trim().trim_matches('"'));
                if !source.exists() {
                    return format!("error source missing {}", source.display());
                }
                if !destination.is_dir() {
                    return format!("error destination missing {}", destination.display());
                }
                self.transfer(vec![source.clone()], destination.clone(), cut, cx);
                format!(
                    "ok copy-test source={} dest={} cut={cut} stack={}",
                    source.display(),
                    destination.display(),
                    self.undo_stack.len()
                )
            }
            "list" => {
                let path = if argument.is_empty() {
                    self.active().path.clone()
                } else {
                    PathBuf::from(argument.trim_matches('"'))
                };
                match fs::read_directory(&path) {
                    Ok(entries) => {
                        let names = entries
                            .iter()
                            .map(|entry| entry.name.clone())
                            .collect::<Vec<_>>()
                            .join(" | ");
                        format!("ok list {} entries={} [{}]", path.display(), entries.len(), names)
                    }
                    Err(error) => format!("error list {}: {error}", path.display()),
                }
            }
            "select" => {
                let path = if Path::new(argument).is_absolute() {
                    PathBuf::from(argument.trim_matches('"'))
                } else {
                    self.active().path.join(argument.trim_matches('"'))
                };
                let wanted = path
                    .file_name()
                    .map(|name| name.to_string_lossy().to_ascii_lowercase())
                    .unwrap_or_default();
                let Some(index) = self
                    .active()
                    .entries
                    .iter()
                    .position(|entry| fs::path_eq(&entry.path, &path))
                    .or_else(|| {
                        // Search results live in nested folders; also match by name.
                        self.active().entries.iter().position(|entry| {
                            !wanted.is_empty()
                                && entry.name.to_ascii_lowercase() == wanted
                        })
                    })
                else {
                    return format!("error not in current directory: {}", path.display());
                };
                let path = self.active().entries[index].path.clone();
                self.active_mut().selected.clear();
                self.active_mut().selected.insert(path.clone());
                self.active_mut().cursor = index;
                self.active_mut().anchor = index;
                cx.notify();
                format!("ok select {}", path.display())
            }
            "shell-toggle" => {
                let item = match argument {
                    "open_with" => ShellMenuItem::OpenWith,
                    "print" => ShellMenuItem::Print,
                    "run_as_admin" => ShellMenuItem::RunAsAdmin,
                    "powershell" => ShellMenuItem::PowerShell,
                    "terminal" => ShellMenuItem::Terminal,
                    "properties" => ShellMenuItem::Properties,
                    _ => return "error shell-toggle needs open_with|print|run_as_admin|powershell|terminal|properties".to_string(),
                };
                self.toggle_shell_menu_item(item, cx);
                format!("ok shell-toggle {argument}")
            }
            "submenu" => {
                let name = argument.trim();
                let index = self
                    .shell_handler_menus
                    .iter()
                    .position(|menu| menu.name.eq_ignore_ascii_case(name));
                match index {
                    Some(index) => {
                        self.context_submenu = Some(index);
                        self.context_submenu_position =
                            Some(Point::new(px(560.), px(300.)));
                        cx.notify();
                        format!("ok submenu {} ({index})", self.shell_handler_menus[index].name)
                    }
                    None => format!("error submenu not found: {name}"),
                }
            }
            "context-menu" => {
                let Some(path) = self.selected_paths().into_iter().next() else {
                    return "error no selection".to_string();
                };
                self.context_menu = Some(ContextMenuState {
                    position: Point::new(px(320.), px(260.)),
                    is_dir: path.is_dir(),
                    target: Some(path.clone()),
                });
                self.rebuild_shell_handler_menus();
                cx.notify();
                format!("ok context-menu {}", path.display())
            }
            "cancel" => {
                self.context_menu = None;
                self.editor = None;
                self.modal = None;
                self.pending_drag = None;
                cx.notify();
                "ok cancel".to_string()
            }
            _ => format!("error unknown command {verb}"),
        }
    }

    pub fn new(cx: &mut Context<Self>) -> Self {
        let start = fs::default_start_dir();
        let persisted = load_persisted_settings();
        if persisted.replace_explorer {
            let _ = crate::explorer::enable();
        }
        let (mut font_family, mut fallback_fonts, font_options) = choose_fonts(cx);
        if let Some(saved) = &persisted.font_family {
            if font_options.iter().any(|font| font.eq_ignore_ascii_case(saved)) {
                font_family = saved.clone();
                fallback_fonts = build_fallbacks(&font_family, &font_options);
            }
        }
        let mut app = Self {
            focus_handle: cx.focus_handle(),
            tabs: vec![TabState::new(1, start)],
            active_tab: 0,
            next_tab_id: 2,
            quick_places: persisted
                .quick_places
                .as_ref()
                .map(|paths| paths.iter().map(|path| fs::quick_place_for(path)).collect())
                .unwrap_or_else(fs::quick_places),
            drives: fs::drives(),
            view_mode: ViewMode::Split,
            sort_key: SortKey::Name,
            sort_asc: true,
            show_hidden: persisted.show_hidden,
            show_sidebar: true,
            font_family,
            fallback_fonts,
            font_options,
            editor: None,
            modal: None,
            context_menu: None,
            toast: None,
            toast_generation: 0,
            operation: None,
            pending_drag: None,
            pending_select: None,
            shell_menu: persisted.shell_menu,
            registry_items: shell_registry::enumerate(),
            registry_menu: persisted.registry_menu,
            shellex_items: shell_registry::enumerate_handlers(),
            shellex_menu: persisted.shellex_menu,
            shell_handler_menus: Vec::new(),
            context_submenu: None,
            context_submenu_position: None,
            rubber_band: None,
            undo_stack: Vec::new(),
            close_hovered: false,
            hovered_row: None,
            hover_prev: None,
            place_drag: None,
            place_drag_start: None,
            place_drag_pos: None,
            place_drag_outside: false,
            suppress_place_click: false,
            hovered_side: None,
            hover_prev_side: None,
            side_generation: 0,
            hover_generation: 0,
            flash_counter: 0,
            open_flash: None,
            replace_explorer: persisted.replace_explorer && crate::explorer::is_enabled(),
            settings_page: SettingsPage::General,
            pending_focus_list: false,
            initial_focus_done: false,
            window_handle: None,
        };
        app.load_active(cx);
        app
    }

    fn save_settings(&self) {
        let data = PersistedSettings {
            font_family: Some(self.font_family.clone()),
            show_hidden: self.show_hidden,
            shell_menu: self.shell_menu.clone(),
            registry_menu: self.registry_menu.clone(),
            shellex_menu: self.shellex_menu.clone(),
            replace_explorer: self.replace_explorer,
            quick_places: Some(
                self.quick_places
                    .iter()
                    .map(|place| place.path.clone())
                    .collect(),
            ),
        };
        write_settings_file(&data);
    }

    fn active(&self) -> &TabState {
        &self.tabs[self.active_tab]
    }

    fn active_mut(&mut self) -> &mut TabState {
        &mut self.tabs[self.active_tab]
    }

    fn tab_index_by_id(&self, id: u64) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.id == id)
    }

    fn can_use_shortcuts(&self) -> bool {
        self.editor.is_none() && self.modal.is_none() && self.context_menu.is_none()
    }

    fn selected_paths(&self) -> Vec<PathBuf> {
        self.active().selected_paths()
    }

    fn rebuild_tab_visible(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        let (visible, cursor) = {
            let tab = &self.tabs[index];
            let visible = fs::visible_indices(
                &tab.entries,
                self.sort_key,
                self.sort_asc,
                &tab.filter,
                self.show_hidden,
            );
            let cursor = tab.cursor.min(visible.len().saturating_sub(1));
            (visible, cursor)
        };
        let tab = &mut self.tabs[index];
        tab.visible = Arc::new(visible);
        tab.cursor = cursor;
        tab.anchor = tab.anchor.min(tab.visible.len().saturating_sub(1));
    }

    /// Recursive search: results replace the directory listing until cleared.
    fn start_search(&mut self, query: String, cx: &mut Context<Self>) {
        let index = self.active_tab;
        let Some(tab) = self.tabs.get_mut(index) else {
            return;
        };
        let root = tab.path.clone();
        tab.search_root = Some(root.clone());
        tab.searching = true;
        tab.search_truncated = false;
        tab.load_id = tab.load_id.wrapping_add(1);
        let load_id = tab.load_id;
        let tab_id = tab.id;
        let show_hidden = self.show_hidden;
        cx.notify();

        let task = cx.background_executor().spawn(async move {
            fs::search_directory(&root, &query, show_hidden, 5000)
        });
        cx.spawn(async move |this, cx| {
            let (results, truncated) = task.await;
            let _ = this.update(cx, |this, cx| {
                let Some(index) = this.tab_index_by_id(tab_id) else {
                    return;
                };
                if this.tabs[index].load_id != load_id {
                    return;
                }
                this.tabs[index].searching = false;
                this.tabs[index].search_truncated = truncated;
                this.tabs[index].entries = Arc::new(results);
                this.tabs[index].visible = Arc::new(Vec::new());
                this.tabs[index].selected.clear();
                this.tabs[index].cursor = 0;
                this.tabs[index].anchor = 0;
                this.rebuild_tab_visible(index);
                cx.notify();
            });
        })
        .detach();
    }

    /// Leave search results and go back to the plain directory listing.
    fn clear_search(&mut self, cx: &mut Context<Self>) {
        let index = self.active_tab;
        let was_searching = self
            .tabs
            .get(index)
            .map(|tab| tab.search_root.is_some())
            .unwrap_or(false);
        if let Some(tab) = self.tabs.get_mut(index) {
            tab.search_root = None;
            tab.searching = false;
            tab.search_truncated = false;
            tab.filter.clear();
        }
        if was_searching {
            self.load_tab(index, cx);
        } else {
            self.rebuild_visible();
            cx.notify();
        }
    }

    fn rebuild_visible(&mut self) {
        self.rebuild_tab_visible(self.active_tab);
    }

    fn load_active(&mut self, cx: &mut Context<Self>) {
        self.load_tab(self.active_tab, cx);
    }

    fn load_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get_mut(index) else {
            return;
        };
        tab.loading = true;
        tab.error = None;
        tab.load_id = tab.load_id.wrapping_add(1);
        let load_id = tab.load_id;
        let tab_id = tab.id;
        let path = tab.path.clone();
        let load_path = path.clone();
        cx.notify();

        let task = cx
            .background_executor()
            .spawn(async move { fs::read_directory(&load_path) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                let Some(index) = this.tab_index_by_id(tab_id) else {
                    return;
                };
                if this.tabs[index].load_id != load_id || this.tabs[index].path != path {
                    return;
                }
                this.tabs[index].loading = false;
                match result {
                    Ok(entries) => {
                        this.tabs[index].error = None;
                        this.tabs[index].entries = Arc::new(entries);
                        this.rebuild_tab_visible(index);
                        let existing: BTreeSet<PathBuf> =
                            this.tabs[index].entries.iter().map(|e| e.path.clone()).collect();
                        this.tabs[index].selected.retain(|path| existing.contains(path));
                        this.tabs[index].cursor = this.tabs[index]
                            .cursor
                            .min(this.tabs[index].visible.len().saturating_sub(1));
                    }
                    Err(error) => {
                        this.tabs[index].error = Some(error);
                        this.tabs[index].entries = Arc::new(Vec::new());
                        this.tabs[index].visible = Arc::new(Vec::new());
                        this.tabs[index].selected.clear();
                        this.tabs[index].cursor = 0;
                    }
                }
                if index == this.active_tab {
                    if let Some(path) = this.pending_select.take() {
                        if let Some(visible_index) = this.tabs[index]
                            .visible
                            .iter()
                            .position(|&i| this.tabs[index].entries[i].path == path)
                        {
                            this.tabs[index].selected.clear();
                            this.tabs[index].selected.insert(path);
                            this.tabs[index].cursor = visible_index;
                            this.tabs[index].anchor = visible_index;
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn navigate_to(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        log::debug!("navigate: {}", path.display());
        if !path.exists() {
            self.show_error("无法打开", format!("路径不存在：{}", path.display()));
            cx.notify();
            return;
        }
        if !path.is_dir() {
            if let Err(error) = ops::open_with_shell(&path) {
                self.show_error("无法打开", error);
                cx.notify();
            }
            return;
        }
        let tab = self.active_mut();
        if tab.history.get(tab.history_index).is_some_and(|p| fs::path_eq(p, &path)) {
            self.reload_active(cx);
            return;
        }
        tab.history.truncate(tab.history_index + 1);
        tab.history.push(path.clone());
        tab.history_index = tab.history.len() - 1;
        tab.path = path;
        tab.title = fs::path_display_name(&tab.path);
        tab.selected.clear();
        tab.cursor = 0;
        tab.anchor = 0;
        tab.filter.clear();
        tab.search_root = None;
        tab.searching = false;
        tab.search_truncated = false;
        tab.entries = Arc::new(Vec::new());
        tab.visible = Arc::new(Vec::new());
        tab.error = None;
        self.context_menu = None;
        self.load_active(cx);
    }

    fn go_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editor.is_some() || self.modal.is_some() {
            return;
        }
        let tab = self.active_mut();
        if tab.history_index == 0 {
            return;
        }
        tab.history_index -= 1;
        tab.path = tab.history[tab.history_index].clone();
        tab.title = fs::path_display_name(&tab.path);
        tab.selected.clear();
        tab.cursor = 0;
        tab.anchor = 0;
        tab.filter.clear();
        tab.entries = Arc::new(Vec::new());
        tab.visible = Arc::new(Vec::new());
        window.focus(&self.focus_handle);
        self.load_active(cx);
    }

    fn go_forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editor.is_some() || self.modal.is_some() {
            return;
        }
        let tab = self.active_mut();
        if tab.history_index + 1 >= tab.history.len() {
            return;
        }
        tab.history_index += 1;
        tab.path = tab.history[tab.history_index].clone();
        tab.title = fs::path_display_name(&tab.path);
        tab.selected.clear();
        tab.cursor = 0;
        tab.anchor = 0;
        tab.filter.clear();
        tab.entries = Arc::new(Vec::new());
        tab.visible = Arc::new(Vec::new());
        window.focus(&self.focus_handle);
        self.load_active(cx);
    }

    fn go_up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_use_shortcuts() {
            return;
        }
        let Some(parent) = self.active().path.parent().map(Path::to_path_buf) else {
            return;
        };
        window.focus(&self.focus_handle);
        self.navigate_to(parent, cx);
    }

    pub fn open_start_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if path.is_dir() {
            self.navigate_to(path, cx);
        } else if let Some(parent) = path.parent().map(Path::to_path_buf) {
            self.pending_select = Some(path);
            self.navigate_to(parent, cx);
        }
    }

    fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_use_shortcuts() {
            return;
        }
        window.focus(&self.focus_handle);
        self.drives = fs::drives();
        self.reload_active(cx);
    }

    fn reload_active(&mut self, cx: &mut Context<Self>) {
        if self.active().search_root.is_some() {
            let query = self.active().filter.clone();
            self.start_search(query, cx);
            return;
        }
        self.load_tab(self.active_tab, cx);
    }

    fn new_tab(&mut self, path: Option<PathBuf>, cx: &mut Context<Self>) {
        let path = path.unwrap_or_else(|| self.active().path.clone());
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        self.tabs.push(TabState::new(id, path));
        self.active_tab = self.tabs.len() - 1;
        self.editor = None;
        self.context_menu = None;
        self.modal = None;
        self.load_active(cx);
    }

    fn new_tab_from_current(&mut self, cx: &mut Context<Self>) {
        let path = self.active().path.clone();
        self.new_tab(Some(path), cx);
    }

    fn close_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        log::info!("close tab: index={index}, active={}, count={}", self.active_tab, self.tabs.len());
        if index >= self.tabs.len() {
            return;
        }
        self.tabs.remove(index);
        if self.tabs.is_empty() {
            let path = dirs::home_dir()
                .or_else(|| dirs::download_dir())
                .unwrap_or_else(|| PathBuf::from("C:\\"));
            let id = self.next_tab_id;
            self.next_tab_id += 1;
            self.tabs.push(TabState::new(id, path));
            self.active_tab = 0;
            self.load_active(cx);
        } else if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len() - 1;
            self.reload_active(cx);
        } else if index < self.active_tab {
            self.active_tab -= 1;
        } else if index == self.active_tab {
            self.reload_active(cx);
        }
        self.editor = None;
        self.context_menu = None;
        self.pending_focus_list = true;
        cx.notify();
    }

    fn cycle_tab(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.tabs.len() <= 1 {
            return;
        }
        if forward {
            self.active_tab = (self.active_tab + 1) % self.tabs.len();
        } else {
            self.active_tab = (self.active_tab + self.tabs.len() - 1) % self.tabs.len();
        }
        self.editor = None;
        self.context_menu = None;
        self.modal = None;
        self.pending_focus_list = true;
        cx.notify();
    }

    fn set_view(&mut self, view: ViewMode, cx: &mut Context<Self>) {
        self.view_mode = view;
        cx.notify();
    }

    fn begin_edit(
        &mut self,
        target: EditTarget,
        initial: String,
        placeholder: &'static str,
        select_all: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let editor = cx.new(|cx| TextInput::new(initial, placeholder, cx));
        if select_all {
            editor.update(cx, |editor, cx| editor.select_all(cx));
        }
        let target_for_sub = target.clone();
        let subscription = cx.subscribe(&editor, move |this, _editor, event, cx| {
            this.on_editor_event(&target_for_sub, event, cx);
        });
        let focus = editor.read(cx).focus_handle.clone();
        log::debug!("begin edit: {:?}", target);
        self.editor = Some(EditorSession {
            editor,
            target,
            _subscription: subscription,
        });
        self.modal = None;
        window.focus(&focus);
        cx.notify();
    }

    fn begin_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.modal.is_some() {
            return;
        }
        let filter = self.active().filter.clone();
        self.begin_edit(
            EditTarget::Search,
            filter,
            "筛选当前文件夹，Enter 搜索子文件夹…",
            false,
            window,
            cx,
        );
    }

    fn begin_path_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.modal.is_some() {
            return;
        }
        let path = self.active().path.to_string_lossy().into_owned();
        self.begin_edit(EditTarget::Path, path, "输入路径…", true, window, cx);
    }

    fn close_editor(&mut self) {
        self.editor = None;
        self.pending_focus_list = true;
    }

    fn on_editor_event(
        &mut self,
        target: &EditTarget,
        event: &TextInputEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            TextInputEvent::Changed(text) => {
                if matches!(target, EditTarget::Search) {
                    if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                        tab.filter = text.clone();
                        tab.selected.clear();
                    }
                    self.rebuild_visible();
                    log::debug!("search changed, visible={}", self.active().visible.len());
                    cx.notify();
                }
            }
            TextInputEvent::Submitted(text) => {
                self.submit_edit(target.clone(), text.clone(), cx);
            }
            TextInputEvent::Cancelled => {
                if matches!(target, EditTarget::Search) {
                    let was_searching = self.active().search_root.is_some();
                    if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                        tab.filter.clear();
                        tab.search_root = None;
                        tab.searching = false;
                        tab.search_truncated = false;
                    }
                    if was_searching {
                        self.load_active(cx);
                    } else {
                        self.rebuild_visible();
                    }
                }
                self.close_editor();
                cx.notify();
            }
        }
    }

    fn submit_edit(&mut self, target: EditTarget, text: String, cx: &mut Context<Self>) {
        match target {
            EditTarget::Search => {
                self.close_editor();
                let query = text.trim().to_string();
                if query.is_empty() {
                    let was_searching = self.active().search_root.is_some();
                    if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                        tab.search_root = None;
                        tab.searching = false;
                        tab.search_truncated = false;
                    }
                    if was_searching {
                        self.load_active(cx);
                    }
                } else {
                    self.start_search(query, cx);
                }
                cx.notify();
            }
            EditTarget::Path => {
                self.close_editor();
                match fs::expand_user_path(&text) {
                    Ok(path) => {
                        if path.is_dir() {
                            self.navigate_to(path, cx);
                        } else {
                            self.show_error("无法打开", format!("不是文件夹：{}", path.display()));
                        }
                    }
                    Err(error) => self.show_error("路径无效", error),
                }
                cx.notify();
            }
            EditTarget::Rename(path) => {
                self.close_editor();
                match ops::rename_path(&path, &text) {
                    Ok(target) => {
                        self.push_undo(UndoAction::Rename {
                            from: path.clone(),
                            to: target.clone(),
                        });
                        self.pending_select = Some(target);
                        self.show_toast("已重命名", ToastKind::Success, cx);
                        self.reload_active(cx);
                    }
                    Err(error) => {
                        self.show_error("重命名失败", error);
                        cx.notify();
                    }
                }
            }
            EditTarget::NewFolder => {
                self.close_editor();
                let parent = self.active().path.clone();
                match ops::create_folder(&parent, &text) {
                    Ok(path) => {
                        self.push_undo(UndoAction::Created {
                            paths: vec![path.clone()],
                        });
                        self.pending_select = Some(path);
                        self.show_toast("已新建文件夹", ToastKind::Success, cx);
                        self.reload_active(cx);
                    }
                    Err(error) => {
                        self.show_error("新建文件夹失败", error);
                        cx.notify();
                    }
                }
            }
            EditTarget::NewFile => {
                self.close_editor();
                let parent = self.active().path.clone();
                match ops::create_file(&parent, &text) {
                    Ok(path) => {
                        self.push_undo(UndoAction::Created {
                            paths: vec![path.clone()],
                        });
                        self.pending_select = Some(path);
                        self.show_toast("已新建文件", ToastKind::Success, cx);
                        self.reload_active(cx);
                    }
                    Err(error) => {
                        self.show_error("新建文件失败", error);
                        cx.notify();
                    }
                }
            }
        }
    }

    fn submit_current_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = &self.editor else {
            return;
        };
        let text = session.editor.read(cx).content().to_string();
        let target = session.target.clone();
        window.focus(&self.focus_handle);
        self.submit_edit(target, text, cx);
    }

    fn show_error(&mut self, title: impl Into<String>, message: impl Into<String>) {
        self.modal = Some(Modal::Error {
            title: title.into(),
            message: message.into(),
        });
    }

    fn show_toast(&mut self, message: impl Into<String>, kind: ToastKind, cx: &mut Context<Self>) {
        self.toast_generation = self.toast_generation.wrapping_add(1);
        let generation = self.toast_generation;
        self.toast = Some(Toast {
            message: message.into(),
            kind,
        });
        cx.notify();
        cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_secs(3)).await;
            let _ = this.update(cx, |this, cx| {
                if this.toast_generation == generation {
                    this.toast = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn select_display_index(&mut self, index: usize, extend: bool, cx: &mut Context<Self>) {
        let tab = self.active_mut();
        let Some(entry) = tab.entry_at_visible(index) else {
            return;
        };
        let path = entry.path.clone();
        if extend {
            let start = tab.anchor.min(index);
            let end = tab.anchor.max(index);
            tab.selected.clear();
            for i in start..=end {
                if let Some(entry) = tab.entry_at_visible(i) {
                    tab.selected.insert(entry.path.clone());
                }
            }
            tab.cursor = index;
        } else {
            tab.selected.clear();
            tab.selected.insert(path);
            tab.cursor = index;
            tab.anchor = index;
        }
        cx.notify();
    }

    fn toggle_display_index(&mut self, index: usize, cx: &mut Context<Self>) {
        let tab = self.active_mut();
        let Some(path) = tab.entry_at_visible(index).map(|e| e.path.clone()) else {
            return;
        };
        if !tab.selected.remove(&path) {
            tab.selected.insert(path);
        }
        tab.cursor = index;
        tab.anchor = index;
        cx.notify();
    }

    fn row_mouse_down(
        &mut self,
        visible_index: usize,
        path: &Path,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.context_menu = None;
        if event.modifiers.control {
            self.toggle_display_index(visible_index, cx);
        } else if event.modifiers.shift {
            // Shift always means "rubber band": start a fresh marquee from this
            // point instead of dragging the file out. Whatever was selected
            // before is dropped, matching "select only what the box covers".
            self.active_mut().selected.clear();
            self.active_mut().selected.insert(path.to_path_buf());
            self.active_mut().cursor = visible_index;
            self.active_mut().anchor = visible_index;
            self.rubber_band = Some((event.position, event.position));
        } else {
            let already_selected = self.active().selected.contains(path);
            if !already_selected {
                self.active_mut().selected.clear();
                self.active_mut().selected.insert(path.to_path_buf());
                self.active_mut().cursor = visible_index;
                self.active_mut().anchor = visible_index;
                cx.notify();
            } else {
                self.active_mut().cursor = visible_index;
                self.active_mut().anchor = visible_index;
                cx.notify();
            }
        }
        if event.modifiers.shift {
            cx.notify();
            return;
        }
        self.pending_drag = Some(PendingDrag {
            start_screen: drag_out::cursor_position(),
            path: path.to_path_buf(),
        });
        self.spawn_drag_watch(window, cx);
    }

    fn spawn_drag_watch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let hwnd = match raw_window_handle::HasWindowHandle::window_handle(window) {
            Ok(handle) => match handle.as_raw() {
                raw_window_handle::RawWindowHandle::Win32(raw) => {
                    Some(windows::Win32::Foundation::HWND(raw.hwnd.get() as *mut _))
                }
                _ => None,
            },
            Err(_) => None,
        };
        cx.spawn(async move |this, cx| {
            let Some(start) = this
                .update(cx, |app, _| app.pending_drag.as_ref().and_then(|pending| pending.start_screen))
                .ok()
                .flatten()
            else {
                return;
            };

            for _ in 0..1200 {
                Timer::after(Duration::from_millis(16)).await;

                let pending_matches = this
                    .update(cx, |app, _| {
                        app.pending_drag
                            .as_ref()
                            .and_then(|pending| pending.start_screen)
                            == Some(start)
                    })
                    .unwrap_or(false);
                if !pending_matches {
                    return;
                }
                if !drag_out::left_button_down() {
                    let _ = this.update(cx, |app, _| {
                        app.pending_drag = None;
                    });
                    return;
                }
                let Some((x, y)) = drag_out::cursor_position() else {
                    continue;
                };
                let dx = (x - start.0) as f32;
                let dy = (y - start.1) as f32;
                if dx * dx + dy * dy < 16.0 {
                    continue;
                }

                // Ctrl (not Shift) is the copy modifier: Shift is reserved for marquee.
        let shift = drag_out::control_down();
                let Some(request) = this
                    .update(cx, |app, _| {
                        let pending = app.pending_drag.take()?;
                        Some(DragRequest {
                            paths: app.drag_selected_paths(&pending.path),
                            shift,
                        })
                    })
                    .ok()
                    .flatten()
                else {
                    return;
                };

                let count = request.paths.len();
                let shift = request.shift;
                log::info!("requesting native drag on UI thread: {count} items (shift={shift})");
                let Some(hwnd) = hwnd else {
                    log::error!("no window handle for native drag");
                    return;
                };
                if !drag_host::request_drag(hwnd, request.paths, shift) {
                    log::error!("failed to queue native drag request");
                    return;
                }
                for _ in 0..600 {
                    Timer::after(Duration::from_millis(16)).await;
                    if let Some(outcome) = drag_host::take_result() {
                        let _ = this.update(cx, |app, cx| {
                            app.finish_native_drag(outcome, count, cx);
                        });
                        return;
                    }
                }
                log::error!("native drag did not report a result in time");
                let _ = this.update(cx, |app, cx| {
                    app.show_toast("拖放未完成", ToastKind::Error, cx);
                });
            }
        })
        .detach();
    }

    fn row_right_click(
        &mut self,
        visible_index: usize,
        path: &Path,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        if !self.active().selected.contains(path) {
            let tab = self.active_mut();
            tab.selected.clear();
            tab.selected.insert(path.to_path_buf());
            tab.cursor = visible_index;
            tab.anchor = visible_index;
        }
        let is_dir = self
            .active()
            .entry_at_visible(visible_index)
            .is_some_and(|entry| entry.is_dir);
        self.context_menu = Some(ContextMenuState {
            position: event.position,
            target: Some(path.to_path_buf()),
            is_dir,
        });
        self.rebuild_shell_handler_menus();
        self.pending_drag = None;
        cx.notify();
    }

    fn open_entry(&mut self, entry: FileEntry, new_tab: bool, cx: &mut Context<Self>) {
        if entry.is_dir {
            if new_tab {
                self.new_tab(Some(entry.path), cx);
            } else {
                self.navigate_to(entry.path, cx);
            }
        } else {
            self.flash_open(entry.name.clone(), cx);
            let path = entry.path.clone();
            let task = cx
                .background_executor()
                .spawn(async move { ops::open_with_shell(&path) });
            cx.spawn(async move |this, cx| {
                if let Err(error) = task.await {
                    let _ = this.update(cx, |app, cx| {
                        app.show_error("无法打开文件", error);
                        cx.notify();
                    });
                }
            })
            .detach();
        }
    }

    fn open_selected(&mut self, new_tab: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_use_shortcuts() {
            return;
        }
        let target = self
            .active()
            .selected_entries()
            .into_iter()
            .next()
            .or_else(|| self.active().preview_entry().cloned());
        if let Some(entry) = target {
            self.open_entry(entry, new_tab, cx);
        } else {
            window.focus(&self.focus_handle);
        }
    }

    fn move_cursor(&mut self, delta: isize, extend: bool, cx: &mut Context<Self>) {
        let len = self.active().visible.len();
        if len == 0 {
            return;
        }
        let current = self.active().cursor.min(len - 1);
        let next = if delta.is_negative() {
            current.saturating_sub(delta.unsigned_abs())
        } else {
            (current + delta as usize).min(len - 1)
        };
        if extend {
            self.select_display_index(next, true, cx);
        } else {
            self.select_display_index(next, false, cx);
        }
        self.scroll_to_cursor();
    }

    fn move_cursor_to(&mut self, index: usize, extend: bool, cx: &mut Context<Self>) {
        let len = self.active().visible.len();
        if len == 0 {
            return;
        }
        let index = index.min(len - 1);
        self.select_display_index(index, extend, cx);
        self.scroll_to_cursor();
    }

    fn scroll_to_cursor(&self) {
        let tab = self.active();
        if tab.visible.is_empty() {
            return;
        }
        match self.view_mode {
            ViewMode::Grid => {
                // Grid columns are dynamic; scrolling by visible index is still
                // a useful approximation for keyboard navigation.
                tab.grid_scroll
                    .scroll_to_item(tab.cursor / 5, ScrollStrategy::Center);
            }
            _ => {
                tab.details_scroll
                    .scroll_to_item(tab.cursor, ScrollStrategy::Center);
            }
        }
    }

    fn select_all_visible(&mut self, cx: &mut Context<Self>) {
        let tab = self.active_mut();
        tab.selected = tab
            .visible
            .iter()
            .map(|&i| tab.entries[i].path.clone())
            .collect();
        if !tab.visible.is_empty() {
            tab.anchor = 0;
            tab.cursor = 0;
        }
        cx.notify();
    }

    fn copy_selection(&mut self, cut: bool, cx: &mut Context<Self>) {
        if !self.can_use_shortcuts() {
            return;
        }
        let paths = self.selected_paths();
        if paths.is_empty() {
            return;
        }
        match clipboard::write_files(&paths, cut) {
            Ok(()) => {
                let message = if cut {
                    format!("已剪切 {} 项", paths.len())
                } else {
                    format!("已复制 {} 项", paths.len())
                };
                self.show_toast(message, ToastKind::Info, cx);
            }
            Err(error) => {
                self.show_error("剪贴板写入失败", error);
                cx.notify();
            }
        }
    }

    fn paste_into(&mut self, destination: PathBuf, cx: &mut Context<Self>) {
        let Some(files) = clipboard::read_files() else {
            self.show_toast("剪贴板中没有文件", ToastKind::Info, cx);
            return;
        };
        if !destination.is_dir() {
            return;
        }
        self.transfer(files.paths, destination, files.cut, cx);
    }

    /// Copy or move `paths` into `destination`, recording an undo step.
    fn transfer(
        &mut self,
        paths: Vec<PathBuf>,
        destination: PathBuf,
        cut: bool,
        cx: &mut Context<Self>,
    ) {
        let count = paths.len();
        let clipboard_paths = paths.clone();
        self.operation = Some(format!(
            "正在{} {} 项…",
            if cut { "移动" } else { "复制" },
            count
        ));
        cx.notify();

        let future = cx.background_executor().spawn(async move {
            if cut {
                ops::move_into(&paths, &destination)
            } else {
                ops::copy_into(&paths, &destination)
            }
        });
        cx.spawn(async move |this, cx| {
            let report = future.await;
            let _ = this.update(cx, |this, cx| {
                this.operation = None;
                if cut && report.ok() {
                    clipboard::clear_file_clipboard_if_matches(&clipboard_paths);
                }
                if cut {
                    if !report.moved.is_empty() {
                        this.push_undo(UndoAction::Moved {
                            pairs: report.moved.clone(),
                        });
                    }
                } else if !report.done.is_empty() {
                    this.push_undo(UndoAction::Created {
                        paths: report.done.clone(),
                    });
                }
                let action = if cut { "移动" } else { "复制" };
                let message = report.summary(action);
                let kind = if report.ok() {
                    ToastKind::Success
                } else {
                    ToastKind::Error
                };
                this.show_toast(message, kind, cx);
                this.reload_active(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn paste_selection(&mut self, cx: &mut Context<Self>) {
        if !self.can_use_shortcuts() {
            return;
        }
        let destination = self
            .active()
            .selected_entries()
            .into_iter()
            .find(|entry| entry.is_dir)
            .map(|entry| entry.path)
            .unwrap_or_else(|| self.active().path.clone());
        self.paste_into(destination, cx);
    }

    fn trash_selection(&mut self, cx: &mut Context<Self>) {
        if !self.can_use_shortcuts() {
            return;
        }
        let paths = self.selected_paths();
        if paths.is_empty() {
            return;
        }
        let count = paths.len();
        let originals = paths.clone();
        self.operation = Some(format!("正在删除 {count} 项…"));
        cx.notify();
        let future = cx
            .background_executor()
            .spawn(async move { ops::delete_to_trash(&paths) });
        cx.spawn(async move |this, cx| {
            let result = future.await;
            let _ = this.update(cx, |this, cx| {
                this.operation = None;
                match result {
                    Ok(()) => {
                        this.push_undo(UndoAction::Trashed {
                            original: originals.clone(),
                        });
                        this.show_toast(format!("已将 {count} 项移到回收站"), ToastKind::Success, cx);
                        this.reload_active(cx);
                    }
                    Err(error) => {
                        this.show_error("删除失败", error);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn permanent_delete(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if paths.is_empty() {
            return;
        }
        let count = paths.len();
        self.operation = Some(format!("正在永久删除 {count} 项…"));
        let future = cx
            .background_executor()
            .spawn(async move { ops::delete_permanently(&paths) });
        cx.spawn(async move |this, cx| {
            let report = future.await;
            let _ = this.update(cx, |this, cx| {
                this.operation = None;
                if report.ok() {
                    this.show_toast(
                        format!("已永久删除 {count} 项"),
                        ToastKind::Success,
                        cx,
                    );
                } else {
                    this.show_toast(report.summary("删除"), ToastKind::Error, cx);
                }
                this.reload_active(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn request_permanent_delete(&mut self, cx: &mut Context<Self>) {
        if !self.can_use_shortcuts() {
            return;
        }
        let paths = self.selected_paths();
        if paths.is_empty() {
            return;
        }
        self.modal = Some(Modal::ConfirmPermanentDelete { paths });
        cx.notify();
    }

    fn begin_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_use_shortcuts() {
            return;
        }
        let Some(entry) = self
            .active()
            .selected_entries()
            .into_iter()
            .next()
            .or_else(|| self.active().preview_entry().cloned())
        else {
            return;
        };
        let name = entry
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.begin_edit(
            EditTarget::Rename(entry.path),
            name,
            "新名称",
            true,
            window,
            cx,
        );
    }

    fn begin_new_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.modal.is_some() {
            return;
        }
        self.begin_edit(
            EditTarget::NewFolder,
            "新建文件夹".to_string(),
            "文件夹名称",
            true,
            window,
            cx,
        );
    }

    fn begin_new_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.modal.is_some() {
            return;
        }
        self.begin_edit(
            EditTarget::NewFile,
            "未命名.txt".to_string(),
            "文件名",
            true,
            window,
            cx,
        );
    }

    fn copy_paths_to_clipboard(&mut self, cx: &mut Context<Self>) {
        let paths = self.selected_paths();
        let paths = if paths.is_empty() {
            self.active()
                .preview_entry()
                .map(|e| vec![e.path.clone()])
                .unwrap_or_default()
        } else {
            paths
        };
        if paths.is_empty() {
            return;
        }
        let text = paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("\r\n");
        match clipboard::write_text(&text) {
            Ok(()) => self.show_toast("路径已复制", ToastKind::Success, cx),
            Err(error) => {
                self.show_error("复制路径失败", error);
                cx.notify();
            }
        }
    }

    fn reveal_selected(&mut self, cx: &mut Context<Self>) {
        let target = self
            .selected_paths()
            .into_iter()
            .next()
            .or_else(|| self.active().preview_entry().map(|e| e.path.clone()));
        if let Some(path) = target {
            if let Err(error) = ops::reveal_in_explorer(&path) {
                self.show_error("无法定位文件", error);
                cx.notify();
            }
        }
    }

    fn drag_selected_paths(&self, clicked: &Path) -> Vec<PathBuf> {
        let selected = self.selected_paths();
        if selected.iter().any(|path| path == clicked) {
            selected
        } else {
            vec![clicked.to_path_buf()]
        }
    }

    fn finish_native_drag(
        &mut self,
        outcome: Result<DragOutcome, String>,
        count: usize,
        cx: &mut Context<Self>,
    ) {
        match outcome {
            Ok(outcome) => {
                log::info!(
                    "drag finished: {:?}, effect={}",
                    outcome.result,
                    outcome.effect.0
                );
                if outcome.result == DragDropResult::Dropped {
                    let effect = drag_out::effect_name(outcome.effect);
                    let message = format!("已拖放 {count} 项（{effect}）");
                    self.show_toast(message, ToastKind::Success, cx);
                } else if outcome.result == DragDropResult::Failed {
                    self.show_toast("拖放未完成", ToastKind::Error, cx);
                }
                self.reload_active(cx);
            }
            Err(error) => {
                self.show_error("拖放失败", error);
                cx.notify();
            }
        }
    }

    fn handle_external_drop(
        &mut self,
        paths: &ExternalPaths,
        cx: &mut Context<Self>,
    ) {
        let destination = self.active().path.clone();
        self.copy_external_paths(paths.paths().to_vec(), destination, cx);
    }

    fn copy_external_paths(
        &mut self,
        files: Vec<PathBuf>,
        destination: PathBuf,
        cx: &mut Context<Self>,
    ) {
        if files.is_empty() {
            return;
        }
        let count = files.len();
        let destination_for_log = destination.clone();
        self.operation = Some(format!("正在复制 {count} 项到 {}…", destination.display()));
        cx.notify();
        let future = cx.background_executor().spawn(async move {
            ops::copy_into(&files, &destination)
        });
        cx.spawn(async move |this, cx| {
            let report = future.await;
            if !report.ok() {
                log::warn!(
                    "copy into {} failed: {:?}",
                    destination_for_log.display(),
                    report.errors
                );
            }
            let _ = this.update(cx, |this, cx| {
                this.operation = None;
                if report.ok() {
                    this.show_toast(report.summary("复制"), ToastKind::Success, cx);
                } else {
                    let hint = if report.errors.iter().any(|error| {
                        error.contains("拒绝访问")
                            || error.contains("Access is denied")
                            || error.contains("os error 5")
                    }) {
                        "\n\n提示：目标目录可能没有写权限。请检查文件夹安全设置；若系统策略限制当前进程，可尝试以管理员身份运行 FPilot。"
                    } else {
                        ""
                    };
                    this.show_error(
                        "复制失败",
                        format!(
                            "目标：{}\n{}{hint}",
                            destination_for_log.display(),
                            report.errors.join("\n")
                        ),
                    );
                    cx.notify();
                }
                this.reload_active(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn show_settings(&mut self, cx: &mut Context<Self>) {
        self.context_menu = None;
        self.editor = None;
        self.modal = Some(Modal::Settings);
        cx.notify();
    }

    fn handle_menu_command(
        &mut self,
        command: MenuCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let menu_target = self.context_menu.as_ref().and_then(|menu| menu.target.clone());
        self.context_menu = None;
        match command {
            MenuCommand::Open => {
                if let Some(path) = self
                    .active()
                    .selected_entries()
                    .into_iter()
                    .next()
                    .map(|e| e.path)
                {
                    if path.is_dir() {
                        self.navigate_to(path, cx);
                    } else if let Err(error) = ops::open_with_shell(&path) {
                        self.show_error("无法打开", error);
                        cx.notify();
                    }
                }
            }
            MenuCommand::OpenInNewTab => {
                if let Some(entry) = self
                    .active()
                    .selected_entries()
                    .into_iter()
                    .find(|e| e.is_dir)
                {
                    self.new_tab(Some(entry.path), cx);
                }
            }
            MenuCommand::Copy => self.copy_selection(false, cx),
            MenuCommand::Cut => self.copy_selection(true, cx),
            MenuCommand::Paste => self.paste_selection(cx),
            MenuCommand::Rename => self.begin_rename(window, cx),
            MenuCommand::UndoAction => self.undo(cx),
            MenuCommand::Trash => self.trash_selection(cx),
            MenuCommand::CopyPath => self.copy_paths_to_clipboard(cx),
            MenuCommand::Reveal => self.reveal_selected(cx),
            MenuCommand::NewFolder => self.begin_new_folder(window, cx),
            MenuCommand::NewFile => self.begin_new_file(window, cx),
            MenuCommand::Refresh => {
                self.reload_active(cx);
            }
            MenuCommand::SelectAll => self.select_all_visible(cx),
            MenuCommand::SystemOpenWith => {
                if let Some(path) = menu_target.filter(|path| path.is_file()) {
                    if let Err(error) = ops::open_with_dialog(&path) {
                        self.show_error("打开方式失败", error);
                        cx.notify();
                    }
                }
            }
            MenuCommand::SystemPrint => {
                if let Some(path) = menu_target.filter(|path| path.is_file()) {
                    if let Err(error) = ops::print_path(&path) {
                        self.show_error("打印失败", error);
                        cx.notify();
                    }
                }
            }
            MenuCommand::SystemRunAsAdmin => {
                if let Some(path) = menu_target.filter(|path| ops::is_executable(path)) {
                    if let Err(error) = ops::run_as_admin(&path) {
                        self.show_error("无法以管理员身份运行", error);
                        cx.notify();
                    }
                }
            }
            MenuCommand::SystemPowerShell => {
                let directory = menu_target
                    .filter(|path| path.is_dir())
                    .unwrap_or_else(|| self.active().path.clone());
                if let Err(error) = ops::open_powershell(&directory) {
                    self.show_error("无法打开 PowerShell", error);
                    cx.notify();
                }
            }
            MenuCommand::SystemTerminal => {
                let directory = menu_target
                    .filter(|path| path.is_dir())
                    .unwrap_or_else(|| self.active().path.clone());
                if let Err(error) = ops::open_terminal(&directory) {
                    self.show_error("无法打开终端", error);
                    cx.notify();
                }
            }
            MenuCommand::SystemProperties => {
                let target = menu_target.unwrap_or_else(|| self.active().path.clone());
                if let Err(error) = ops::show_properties(&target) {
                    self.show_error("无法打开属性", error);
                    cx.notify();
                }
            }
            MenuCommand::Registry(index) => {
                if let Some(item) = self.registry_items.get(index).cloned() {
                    let target = menu_target.unwrap_or_else(|| self.active().path.clone());
                    if let Err(error) = shell_registry::invoke(&item, &target) {
                        self.show_error("系统菜单动作失败", error);
                        cx.notify();
                    }
                }
            }
            MenuCommand::ShellHandlerCommand { menu, id } => {
                if let Some(state) = self.shell_handler_menus.get(menu) {
                    if let Some(hwnd) = self.window_handle {
                        if let Err(error) = ops::invoke_handler_command(&state.context, id, hwnd) {
                            self.show_error("执行扩展命令失败", error);
                            cx.notify();
                        }
                    }
                }
            }
        }
        self.context_submenu = None;
        self.context_submenu_position = None;
    }

    fn rebuild_shell_handler_menus(&mut self) {
        self.shell_handler_menus.clear();
        self.context_submenu = None;
        self.context_submenu_position = None;
        let target = match &self.context_menu {
            Some(menu) => menu.target.clone(),
            None => return,
        };
        if self.active().selected.len() > 1 {
            return;
        }
        let target_entry = target
            .as_ref()
            .and_then(|target| self.active().entries.iter().find(|entry| &entry.path == target))
            .cloned();
        let allow_drive = target
            .as_ref()
            .map(|path| path.parent().is_none())
            .unwrap_or(false);
        let mut enabled: Vec<shell_registry::ShellHandler> = Vec::new();
        for handler in &self.shellex_items {
            let matches = if target.is_none() {
                handler
                    .scopes
                    .contains(&shell_registry::HandlerScope::Background)
            } else {
                match &target_entry {
                    Some(entry) if entry.is_dir => {
                        handler
                            .scopes
                            .contains(&shell_registry::HandlerScope::Directory)
                            || handler
                                .scopes
                                .contains(&shell_registry::HandlerScope::Folder)
                            || (allow_drive
                                && handler
                                    .scopes
                                    .contains(&shell_registry::HandlerScope::Drive))
                    }
                    Some(_) => handler
                        .scopes
                        .contains(&shell_registry::HandlerScope::File),
                    None => false,
                }
            };
            if matches && self.shellex_menu.contains_key(&handler.id) {
                enabled.push(handler.clone());
            }
        }
        for handler in enabled {
            let background = if target.is_none() {
                Some(self.active().path.clone())
            } else {
                None
            };
            let paths = if background.is_some() {
                Vec::new()
            } else {
                let mut selected = self.selected_paths();
                if let Some(target) = &target {
                    if !selected.iter().any(|path| path == target) {
                        selected.push(target.clone());
                    }
                }
                selected
            };
            match ops::build_handler_menu(
                &handler.clsid,
                &handler.name,
                &paths,
                background.as_deref(),
            ) {
                Ok(menu) => {
                    if !menu.items.is_empty() {
                        self.shell_handler_menus.push(menu);
                    }
                }
                Err(error) => {
                    log::warn!("cannot build shell handler menu {}: {error}", handler.name)
                }
            }
        }
    }

    fn update_context_menu(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        self.context_menu = Some(ContextMenuState {
            position: event.position,
            target: None,
            is_dir: true,
        });
        self.rebuild_shell_handler_menus();
        cx.notify();
    }

    fn cancel_or_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(session) = &self.editor {
            if matches!(session.target, EditTarget::Search) {
                if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                    tab.filter.clear();
                }
                self.rebuild_visible();
            }
            self.close_editor();
            window.focus(&self.focus_handle);
            cx.notify();
            return;
        }
        if self.context_menu.take().is_some() {
            cx.notify();
            return;
        }
        if self.modal.take().is_some() {
            self.pending_focus_list = true;
            cx.notify();
            return;
        }
        if self.active().search_root.is_some() {
            self.clear_search(cx);
            return;
        }
        self.active_mut().selected.clear();
        cx.notify();
    }

    fn set_hovered_row(&mut self, row: Option<usize>, from: usize, cx: &mut Context<Self>) {
        let next = match row {
            Some(index) => Some(index),
            None if self.hovered_row == Some(from) => None,
            None => return,
        };
        if self.hovered_row == next {
            return;
        }
        self.hover_prev = self.hovered_row;
        self.hovered_row = next;
        self.hover_generation = self.hover_generation.wrapping_add(1);
        cx.notify();
    }

    /// Real pointer position translated into window client coordinates.
    fn cursor_client_point(&self) -> Option<Point<Pixels>> {
        #[cfg(target_os = "windows")]
        {
            use windows::Win32::{
                Foundation::POINT,
                Graphics::Gdi::ScreenToClient,
                UI::WindowsAndMessaging::GetCursorPos,
            };
            let hwnd = self.window_handle?;
            unsafe {
                let mut point = POINT::default();
                GetCursorPos(&mut point).ok()?;
                let _ = ScreenToClient(hwnd, &mut point);
                return Some(Point::new(px(point.x as f32), px(point.y as f32)));
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            None
        }
    }

    /// Polls the real cursor while a sidebar item is being dragged. Window
    /// events stop arriving once the pointer leaves the window, so the ghost
    /// and the drop decision cannot depend on them.
    fn track_place_drag(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            Timer::after(Duration::from_millis(16)).await;
            let Some((slot, start_y)) = this
                .update(cx, |app, _| {
                    app.place_drag
                        .map(|slot| (slot, app.place_drag_start.map(|p| f32::from(p.y))))
                })
                .ok()
                .flatten()
            else {
                break;
            };

            let Some(position) = this
                .update(cx, |app, _| app.cursor_client_point())
                .ok()
                .flatten()
            else {
                break;
            };
            let held = crate::drag_out::left_button_down();
            let outside = f32::from(position.x) > 232.0
                || start_y.is_none_or(|y| (f32::from(position.y) - y).abs() > 18.0);

            if held {
                let _ = slot;
                let _ = this.update(cx, |app, cx| {
                    app.place_drag_pos = Some(position);
                    if app.place_drag_outside != outside {
                        app.place_drag_outside = outside;
                        cx.notify();
                    }
                });
            } else {
                let _ = this.update(cx, |app, cx| {
                    app.finish_place_drag(position, cx);
                });
                break;
            }
        })
        .detach();
    }

    /// Capture the mouse so moves and the release keep coming even when the
    /// pointer leaves the window.
    fn begin_mouse_capture(&self) {
        #[cfg(target_os = "windows")]
        if let Some(hwnd) = self.window_handle {
            unsafe {
                windows::Win32::UI::Input::KeyboardAndMouse::SetCapture(hwnd);
            }
        }
    }

    fn end_mouse_capture(&self) {
        #[cfg(target_os = "windows")]
        unsafe {
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture();
        }
    }

    /// Resolve a sidebar drag: released over the sidebar means "put it back",
    /// anywhere else (including outside the window) means "remove".
    fn finish_place_drag(&mut self, fallback: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(slot) = self.place_drag.take() else {
            return;
        };
        self.end_mouse_capture();
        let position = fallback;
        let started = self.place_drag_start.take();
        self.place_drag_pos = None;
        self.place_drag_outside = false;
        let moved = started.is_none_or(|start| {
            (f32::from(position.x) - f32::from(start.x)).abs() > 5.0
                || (f32::from(position.y) - f32::from(start.y)).abs() > 5.0
        });
        // Put back only when released over the row it came from; anything else
        // (another row, the file list, outside the window) means remove.
        let outside_window = f32::from(position.x) < 0.0 || f32::from(position.y) < 0.0;
        let back_home = !outside_window
            && started.is_some_and(|start| {
                f32::from(position.x) <= 232.0
                    && (f32::from(position.y) - f32::from(start.y)).abs() <= 18.0
            });
        if moved && !back_home {
            self.suppress_place_click = true;
            if let Some(path) = self.quick_places.get(slot).map(|place| place.path.clone()) {
                self.remove_quick_place(&path, cx);
            }
        }
        cx.notify();
    }

    fn add_quick_place(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if !path.is_dir() {
            self.show_toast("只能把文件夹加入快速访问", ToastKind::Info, cx);
            return;
        }
        if self
            .quick_places
            .iter()
            .any(|place| fs::path_eq(&place.path, &path))
        {
            self.show_toast("该文件夹已在快速访问中", ToastKind::Info, cx);
            return;
        }
        self.quick_places.push(fs::quick_place_for(&path));
        self.save_settings();
        self.show_toast("已加入快速访问", ToastKind::Success, cx);
    }

    fn remove_quick_place(&mut self, path: &Path, cx: &mut Context<Self>) {
        let before = self.quick_places.len();
        self.quick_places.retain(|place| !fs::path_eq(&place.path, path));
        if self.quick_places.len() == before {
            return;
        }
        self.save_settings();
        self.show_toast("已从快速访问移除", ToastKind::Success, cx);
    }

    fn reset_quick_places(&mut self, cx: &mut Context<Self>) {
        self.quick_places = fs::quick_places();
        self.save_settings();
        self.show_toast("已恢复默认快速访问", ToastKind::Success, cx);
    }

    /// Settings only toggles the built-in entries; custom ones are managed by
    /// dragging in the sidebar.
    fn toggle_default_quick_place(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self
            .quick_places
            .iter()
            .any(|place| fs::path_eq(&place.path, &path))
        {
            self.remove_quick_place(&path, cx);
        } else {
            self.add_quick_place(path, cx);
        }
    }

    fn set_hovered_side(&mut self, row: Option<usize>, from: usize, cx: &mut Context<Self>) {
        let next = match row {
            Some(index) => Some(index),
            None if self.hovered_side == Some(from) => None,
            None => return,
        };
        if self.hovered_side == next {
            return;
        }
        self.hover_prev_side = self.hovered_side;
        self.hovered_side = next;
        // Sidebar keeps its own counter so list animations are not restarted.
        self.side_generation = self.side_generation.wrapping_add(1);
        cx.notify();
    }

    /// Brief animated pill shown when a file is launched.
    fn flash_open(&mut self, name: String, cx: &mut Context<Self>) {
        self.flash_counter = self.flash_counter.wrapping_add(1);
        let generation = self.flash_counter;
        self.open_flash = Some((name, generation));
        cx.notify();
        cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_millis(620)).await;
            let _ = this.update(cx, |app, cx| {
                if app.open_flash.as_ref().is_some_and(|(_, g)| *g == generation) {
                    app.open_flash = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn push_undo(&mut self, action: UndoAction) {
        const LIMIT: usize = 64;
        self.undo_stack.push(action);
        if self.undo_stack.len() > LIMIT {
            self.undo_stack.remove(0);
        }
    }

    /// Reverse the most recent rename / create / move.
    fn undo(&mut self, cx: &mut Context<Self>) {
        if !self.can_use_shortcuts() {
            return;
        }
        let Some(action) = self.undo_stack.pop() else {
            self.show_toast("没有可撤销的操作", ToastKind::Info, cx);
            return;
        };
        let result: Result<String, String> = match &action {
            UndoAction::Rename { from, to } => {
                let name = from
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                match ops::rename_path(to, &name) {
                    Ok(path) => Ok(format!("已撤销重命名：{}", path.display())),
                    Err(error) => Err(error),
                }
            }
            UndoAction::Created { paths } => {
                let mut failed = Vec::new();
                for path in paths {
                    if path.is_dir() {
                        if std::fs::remove_dir_all(path).is_err() {
                            failed.push(path.display().to_string());
                        }
                    } else if std::fs::remove_file(path).is_err() {
                        failed.push(path.display().to_string());
                    }
                }
                if failed.is_empty() {
                    Ok(format!("已撤销创建 {} 项", paths.len()))
                } else {
                    Err(format!("部分项目无法移除：{}", failed.join("；")))
                }
            }
            UndoAction::Trashed { original } => {
                match ops::restore_trashed(original) {
                    Ok(count) => Ok(format!("已从回收站还原 {count} 项")),
                    Err(error) => Err(error),
                }
            }
            UndoAction::Moved { pairs } => {
                let mut failed = Vec::new();
                for (from, to) in pairs {
                    let target = if from.exists() {
                        match (from.parent(), to.file_name()) {
                            (Some(parent), Some(name)) => {
                                ops::unique_destination(parent, &name.to_string_lossy())
                            }
                            _ => from.clone(),
                        }
                    } else {
                        from.clone()
                    };
                    if std::fs::rename(to, &target).is_err() {
                        failed.push(to.display().to_string());
                    }
                }
                if failed.is_empty() {
                    Ok(format!("已撤销移动 {} 项", pairs.len()))
                } else {
                    Err(format!("部分项目无法移回：{}", failed.join("；")))
                }
            }
        };
        match result {
            Ok(message) => {
                self.show_toast(message, ToastKind::Success, cx);
                self.reload_active(cx);
            }
            Err(error) => {
                self.show_error("撤销失败", error);
                cx.notify();
            }
        }
    }

    fn toggle_replace_explorer(&mut self, cx: &mut Context<Self>) {
        if self.replace_explorer {
            match crate::explorer::disable() {
                Ok(()) => {
                    self.replace_explorer = false;
                    self.save_settings();
                    self.show_toast("已恢复资源管理器的打开方式", ToastKind::Success, cx);
                }
                Err(error) => self.show_error("恢复资源管理器失败", error),
            }
        } else {
            match crate::explorer::enable() {
                Ok(()) => {
                    self.replace_explorer = true;
                    self.save_settings();
                    self.show_toast("已将 FPilot 设为默认文件夹打开方式", ToastKind::Success, cx);
                }
                Err(error) => self.show_error("设置默认打开方式失败", error),
            }
        }
        cx.notify();
    }

    fn toggle_hidden(&mut self, cx: &mut Context<Self>) {
        self.show_hidden = !self.show_hidden;
        self.rebuild_visible();
        self.save_settings();
        cx.notify();
    }

    fn toggle_shell_menu_item(&mut self, item: ShellMenuItem, cx: &mut Context<Self>) {
        item.toggle(&mut self.shell_menu);
        self.save_settings();
        cx.notify();
    }

    fn reset_shell_menu(&mut self, cx: &mut Context<Self>) {
        self.shell_menu = ShellMenuPrefs::default();
        self.save_settings();
        cx.notify();
    }

    fn toggle_registry_menu(&mut self, id: String, cx: &mut Context<Self>) {
        if self.registry_menu.remove(&id).is_none() {
            self.registry_menu.insert(id, true);
        }
        self.save_settings();
        cx.notify();
    }

    fn refresh_registry_menu(&mut self, cx: &mut Context<Self>) {
        self.registry_items = shell_registry::enumerate();
        self.registry_menu
            .retain(|id, _| self.registry_items.iter().any(|item| &item.id == id));
        self.shellex_items = shell_registry::enumerate_handlers();
        self.shellex_menu
            .retain(|id, _| self.shellex_items.iter().any(|item| &item.id == id));
        self.save_settings();
        cx.notify();
    }

    fn toggle_shellex(&mut self, id: String, cx: &mut Context<Self>) {
        if self.shellex_menu.remove(&id).is_none() {
            self.shellex_menu.insert(id, true);
        }
        self.save_settings();
        cx.notify();
    }

    fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.show_sidebar = !self.show_sidebar;
        cx.notify();
    }

    fn sort_by(&mut self, key: SortKey, cx: &mut Context<Self>) {
        if self.sort_key == key {
            self.sort_asc = !self.sort_asc;
        } else {
            self.sort_key = key;
            self.sort_asc = true;
        }
        self.rebuild_visible();
        cx.notify();
    }

    fn choose_font(&mut self, family: String, cx: &mut Context<Self>) {
        if family == self.font_family {
            return;
        }
        self.font_family = family;
        self.fallback_fonts = build_fallbacks(&self.font_family, &self.font_options);
        self.save_settings();
        cx.notify();
    }

    // ---- Actions ---------------------------------------------------------

    fn action_back(&mut self, _: &Back, window: &mut Window, cx: &mut Context<Self>) {
        if self.editor.is_some() || self.modal.is_some() {
            return;
        }
        self.go_back(window, cx);
    }

    fn action_forward(&mut self, _: &Forward, window: &mut Window, cx: &mut Context<Self>) {
        if self.editor.is_some() || self.modal.is_some() {
            return;
        }
        self.go_forward(window, cx);
    }

    fn action_up(&mut self, _: &Up, window: &mut Window, cx: &mut Context<Self>) {
        self.go_up(window, cx);
    }

    fn action_refresh(&mut self, _: &Refresh, window: &mut Window, cx: &mut Context<Self>) {
        self.refresh(window, cx);
    }

    fn action_focus_path(&mut self, _: &FocusPath, window: &mut Window, cx: &mut Context<Self>) {
        if self.modal.is_none() {
            self.begin_path_edit(window, cx);
        }
    }

    fn action_focus_search(&mut self, _: &FocusSearch, window: &mut Window, cx: &mut Context<Self>) {
        if self.modal.is_none() {
            self.begin_search(window, cx);
        }
    }

    fn action_new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.editor.is_some() || self.modal.is_some() {
            return;
        }
        window.focus(&self.focus_handle);
        self.new_tab_from_current(cx);
    }

    fn action_close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.modal.is_some() {
            return;
        }
        window.focus(&self.focus_handle);
        self.close_tab(self.active_tab, cx);
    }

    fn action_next_tab(&mut self, _: &NextTab, _: &mut Window, cx: &mut Context<Self>) {
        if self.editor.is_some() || self.modal.is_some() {
            return;
        }
        self.cycle_tab(true, cx);
    }

    fn action_prev_tab(&mut self, _: &PrevTab, _: &mut Window, cx: &mut Context<Self>) {
        if self.editor.is_some() || self.modal.is_some() {
            return;
        }
        self.cycle_tab(false, cx);
    }

    fn action_open(&mut self, _: &OpenSelected, window: &mut Window, cx: &mut Context<Self>) {
        self.open_selected(false, window, cx);
    }

    fn action_open_new_tab(
        &mut self,
        _: &OpenSelectedNewTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_selected(true, window, cx);
    }

    fn action_copy(&mut self, _: &CopyFiles, _: &mut Window, cx: &mut Context<Self>) {
        self.copy_selection(false, cx);
    }

    fn action_cut(&mut self, _: &CutFiles, _: &mut Window, cx: &mut Context<Self>) {
        self.copy_selection(true, cx);
    }

    fn action_paste(&mut self, _: &PasteFiles, _: &mut Window, cx: &mut Context<Self>) {
        self.paste_selection(cx);
    }

    fn action_trash(&mut self, _: &TrashSelection, _: &mut Window, cx: &mut Context<Self>) {
        self.trash_selection(cx);
    }

    fn action_permanent_delete(
        &mut self,
        _: &PermanentDelete,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.request_permanent_delete(cx);
    }

    fn action_rename(&mut self, _: &RenameSelection, window: &mut Window, cx: &mut Context<Self>) {
        self.begin_rename(window, cx);
    }

    fn action_new_folder(&mut self, _: &NewFolder, window: &mut Window, cx: &mut Context<Self>) {
        self.begin_new_folder(window, cx);
    }

    fn action_new_file(&mut self, _: &NewFile, window: &mut Window, cx: &mut Context<Self>) {
        self.begin_new_file(window, cx);
    }

    fn action_select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        if !self.can_use_shortcuts() {
            return;
        }
        self.select_all_visible(cx);
    }

    fn action_select_prev(&mut self, _: &SelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        if self.can_use_shortcuts() {
            self.move_cursor(-1, false, cx);
        }
    }

    fn action_select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        if self.can_use_shortcuts() {
            self.move_cursor(1, false, cx);
        }
    }

    fn action_select_prev_extend(
        &mut self,
        _: &SelectExtendPrev,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.can_use_shortcuts() {
            self.move_cursor(-1, true, cx);
        }
    }

    fn action_select_next_extend(
        &mut self,
        _: &SelectExtendNext,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.can_use_shortcuts() {
            self.move_cursor(1, true, cx);
        }
    }

    fn action_select_first(&mut self, _: &SelectFirst, _: &mut Window, cx: &mut Context<Self>) {
        if self.can_use_shortcuts() {
            self.move_cursor_to(0, false, cx);
        }
    }

    fn action_select_last(&mut self, _: &SelectLast, _: &mut Window, cx: &mut Context<Self>) {
        if self.can_use_shortcuts() {
            let len = self.active().visible.len();
            if len > 0 {
                self.move_cursor_to(len - 1, false, cx);
            }
        }
    }

    fn action_select_first_extend(
        &mut self,
        _: &SelectExtendFirst,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.can_use_shortcuts() {
            self.move_cursor_to(0, true, cx);
        }
    }

    fn action_select_last_extend(
        &mut self,
        _: &SelectExtendLast,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.can_use_shortcuts() {
            let len = self.active().visible.len();
            if len > 0 {
                self.move_cursor_to(len - 1, true, cx);
            }
        }
    }

    fn action_view_details(
        &mut self,
        _: &ViewDetails,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_view(ViewMode::Details, cx);
    }

    fn action_view_grid(&mut self, _: &ViewGrid, _: &mut Window, cx: &mut Context<Self>) {
        self.set_view(ViewMode::Grid, cx);
    }

    fn action_view_split(&mut self, _: &ViewSplit, _: &mut Window, cx: &mut Context<Self>) {
        self.set_view(ViewMode::Split, cx);
    }

    fn action_toggle_sidebar(
        &mut self,
        _: &ToggleSidebar,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.editor.is_none() {
            self.toggle_sidebar(cx);
        }
    }

    fn action_toggle_hidden(
        &mut self,
        _: &ToggleHidden,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.editor.is_none() {
            self.toggle_hidden(cx);
        }
    }

    fn action_copy_path(&mut self, _: &CopyPath, _: &mut Window, cx: &mut Context<Self>) {
        if self.can_use_shortcuts() {
            self.copy_paths_to_clipboard(cx);
        }
    }

    fn action_undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        self.undo(cx);
    }

    fn action_reveal(&mut self, _: &RevealSelection, _: &mut Window, cx: &mut Context<Self>) {
        if self.can_use_shortcuts() {
            self.reveal_selected(cx);
        }
    }

    fn action_settings(&mut self, _: &OpenSettings, _: &mut Window, cx: &mut Context<Self>) {
        self.show_settings(cx);
    }

    fn action_cancel(&mut self, _: &Cancel, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_or_close(window, cx);
    }
}

/// The single config file. It lives in the user profile, so replacing or
/// reinstalling the application folder never resets it.
fn config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|base| base.join("FPilot"))
}

fn config_file() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join("config.json"))
}

/// Older builds wrote these; they are only read once for migration.
fn legacy_settings_files() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(dir) = config_dir() {
        paths.push(dir.join("settings.json"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            paths.push(parent.join("fpilot.settings.json"));
        }
    }
    paths
}

fn read_settings_file(path: &Path) -> Option<PersistedSettings> {
    let text = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str::<PersistedSettings>(&text) {
        Ok(settings) => Some(settings),
        Err(error) => {
            log::warn!("ignore invalid config {}: {error}", path.display());
            None
        }
    }
}

fn load_persisted_settings() -> PersistedSettings {
    if let Some(path) = config_file() {
        if let Some(settings) = read_settings_file(&path) {
            return settings;
        }
    }
    // Migrate a config written by an older build, then keep using config.json.
    for path in legacy_settings_files() {
        if let Some(settings) = read_settings_file(&path) {
            log::info!("migrating config from {}", path.display());
            if write_settings_file(&settings).is_some() {
                // Keep the old file as a backup so nothing is silently lost.
                let backup = path.with_extension("json.migrated");
                let _ = std::fs::rename(&path, &backup);
            }
            return settings;
        }
    }
    PersistedSettings::default()
}

fn write_settings_file(settings: &PersistedSettings) -> Option<PathBuf> {
    let path = config_file()?;
    if let Some(parent) = path.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            log::warn!("cannot create config dir {}: {error}", parent.display());
            return None;
        }
    }
    let Ok(json) = serde_json::to_string_pretty(settings) else {
        log::warn!("cannot serialize config");
        return None;
    };
    match std::fs::write(&path, json) {
        Ok(()) => Some(path),
        Err(error) => {
            log::warn!("cannot write config {}: {error}", path.display());
            None
        }
    }
}

fn choose_fonts(cx: &App) -> (String, Vec<String>, Vec<String>) {
    let names = cx.text_system().all_font_names();
    let has = |candidate: &str| names.iter().any(|name| name.eq_ignore_ascii_case(candidate));
    let mut chosen = theme::UI_FONT_CANDIDATES
        .iter()
        .find(|candidate| has(candidate))
        .copied()
        .unwrap_or("Segoe UI")
        .to_string();

    let mut options: Vec<String> = theme::UI_FONT_CANDIDATES
        .iter()
        .filter(|candidate| has(candidate))
        .map(|s| (*s).to_string())
        .collect();
    for name in names.iter() {
        if options.len() >= 24 {
            break;
        }
        if name.starts_with('.') || name.contains('@') || name.trim().is_empty() {
            continue;
        }
        if !options.iter().any(|option| option.eq_ignore_ascii_case(name)) {
            options.push(name.clone());
        }
    }
    options.sort_by_key(|name| {
        theme::UI_FONT_CANDIDATES
            .iter()
            .position(|candidate| candidate.eq_ignore_ascii_case(name))
            .unwrap_or(100)
    });
    options.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    if options.is_empty() {
        options.push(chosen.clone());
    }
    if !options.iter().any(|option| option.eq_ignore_ascii_case(&chosen)) {
        chosen = options[0].clone();
    }
    let fallbacks = build_fallbacks(&chosen, &options);
    (chosen, fallbacks, options)
}

fn build_fallbacks(chosen: &str, available: &[String]) -> Vec<String> {
    let has = |candidate: &str| {
        candidate.eq_ignore_ascii_case(chosen)
            || available
                .iter()
                .any(|name| name.eq_ignore_ascii_case(candidate))
    };
    let mut fallbacks = Vec::new();
    fallbacks.push(chosen.to_string());
    for family in theme::CJK_FALLBACKS {
        if has(family) && !fallbacks.iter().any(|f| f.eq_ignore_ascii_case(family)) {
            fallbacks.push((*family).to_string());
        }
    }
    if !fallbacks.iter().any(|f| f.eq_ignore_ascii_case("Segoe UI")) {
        fallbacks.push("Segoe UI".to_string());
    }
    fallbacks
}
impl PilotApp {
    fn render_titlebar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let active = self.active_tab;
        let tabs_data: Vec<_> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| (index, tab.title.clone()))
            .collect();
        let viewport_width = f32::from(window.viewport_size().width);
        let tab_strip_width = (tabs_data.len() as f32 * 180.0 + 26.0)
            .min((viewport_width - 330.0).max(180.0));

        let tabs = tabs_data.into_iter().map(|(index, title)| {
            let is_active = index == active;
            div()
                .id(("tab", index))
                .flex()
                .flex_none()
                .items_center()
                .gap_1p5()
                .h(px(32.))
                .w(px(176.))
                .px_2()
                .rounded_md()
                .cursor_pointer()
                .overflow_hidden()
                .when(is_active, |this| {
                    this.bg(theme::surface())
                        .shadow_sm()
                        .text_color(theme::text())
                })
                .when(!is_active, |this| {
                    this.text_color(theme::text_muted())
                        .hover(|this| this.bg(theme::hover()).text_color(theme::text()))
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    if index < this.tabs.len() {
                        this.active_tab = index;
                        this.editor = None;
                        this.context_menu = None;
                        this.modal = None;
                        this.pending_focus_list = true;
                        cx.notify();
                    }
                    window.focus(&this.focus_handle);
                }))
                .child(
                    svg()
                        .path("icons/folder.svg")
                        .size(px(14.))
                        .text_color(if is_active {
                            theme::folder()
                        } else {
                            theme::text_faint()
                        }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .truncate()
                        .text_size(px(12.))
                        .child(title),
                )
                .child(
                    div()
                        .id(("tab-close", index))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(18.))
                        .rounded_sm()
                        .cursor_pointer()
                        .hover(|this| this.bg(theme::pressed()).text_color(theme::text()))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            log::info!("tab close button clicked: {index}");
                            cx.stop_propagation();
                            this.close_tab(index, cx);
                            window.focus(&this.focus_handle);
                        }))
                        .child(
                            svg()
                                .path("icons/close.svg")
                                .size(px(11.))
                                .text_color(theme::text_muted()),
                        ),
                )
        });

        div()
            .h(px(theme::TITLEBAR_HEIGHT))
            .w_full()
            .flex()
            .items_center()
            .gap_1()
            .px(px(8.))
            .bg(theme::chrome())
            .border_b_1()
            .border_color(theme::border())
            .child(
                div()
                    .id("sidebar-toggle")
                    .size(px(30.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .cursor_pointer()
                    .text_color(theme::text_muted())
                    .hover(|this| {
                        this.bg(theme::hover())
                            .text_color(theme::text())
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_sidebar(cx)))
                    .child(
                        svg()
                            .path("icons/sidebar.svg")
                            .size(px(16.))
                            .text_color(theme::text_muted()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .w(px(tab_strip_width))
                    .min_w(px(0.))
                    .h_full()
                    .overflow_hidden()
                    .children(tabs)
                    .child(
                        div()
                            .id("new-tab")
                            .flex_none()
                            .size(px(26.))
                            .ml_1()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .cursor_pointer()
                            .text_color(theme::text_faint())
                            .hover(|this| {
                                this.bg(theme::hover())
                                    .text_color(theme::text())
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                window.focus(&this.focus_handle);
                                this.new_tab_from_current(cx);
                            }))
                            .child(
                                svg()
                                    .path("icons/plus.svg")
                                    .size(px(14.))
                                    .text_color(theme::text_faint()),
                            ),
                    ),
            )
            .child(
                div()
                    .id("titlebar-drag")
                    .flex_1()
                    .min_w(px(64.))
                    .h_full()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _, window, cx| request_window_move(window, cx)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(2.))
                    .child(
                        div()
                            .id("window-min")
                            .size(px(30.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .cursor_pointer()
                            .text_color(theme::text_muted())
                            .hover(|this| this.bg(theme::control_hover()))
                            .on_click(|_, window, _| { log::info!("window button: minimize"); window.minimize_window(); })
                            .child(
                                svg()
                                    .path("icons/minimize.svg")
                                    .size(px(15.))
                                    .text_color(theme::text_muted()),
                            ),
                    )
                    .child(
                        div()
                            .id("window-max")
                            .size(px(30.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .cursor_pointer()
                            .text_color(theme::text_muted())
                            .hover(|this| this.bg(theme::control_hover()))
                            .on_click(|_, window, _| { log::info!("window button: maximize/restore"); toggle_window_maximize(window); })
                            .child(
                                svg()
                                    .path(if window.is_maximized() {
                                        "icons/restore.svg"
                                    } else {
                                        "icons/maximize.svg"
                                    })
                                    .size(px(14.))
                                    .text_color(theme::text_muted()),
                            ),
                    )
                    .child(
                        div()
                            .id("window-close")
                            .size(px(30.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|this| this.bg(theme::danger_muted()))
                            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                                if this.close_hovered != *hovered {
                                    this.close_hovered = *hovered;
                                    cx.notify();
                                }
                            }))
                            .on_click(|_, window, _| { log::info!("window button: close"); window.remove_window(); })
                            .child(
                                svg()
                                    .path("icons/close.svg")
                                    .size(px(15.))
                                    .text_color(if self.close_hovered {
                                        theme::white()
                                    } else {
                                        theme::text_muted()
                                    }),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_toolbar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let can_back = self.active().history_index > 0;
        let can_forward = self.active().history_index + 1 < self.active().history.len();
        let path = self.active().path.clone();
        let filter = self.active().filter.clone();
        let editing_path = self
            .editor
            .as_ref()
            .is_some_and(|session| matches!(session.target, EditTarget::Path));
        let editing_search = self
            .editor
            .as_ref()
            .is_some_and(|session| matches!(session.target, EditTarget::Search));

        let mut breadcrumb: Vec<AnyElement> = Vec::new();
        let home = dirs::home_dir();
        if let Some(home) = home.clone() {
            breadcrumb.push(
                div()
                    .id("crumb-home")
                    .flex_none()
                    .size(px(26.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .cursor_pointer()
                    .text_color(theme::text_muted())
                    .hover(|this| {
                        this.bg(theme::hover())
                            .text_color(theme::text())
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        window.focus(&this.focus_handle);
                        this.navigate_to(home.clone(), cx);
                    }))
                    .child(
                        svg()
                            .path("icons/home.svg")
                            .size(px(14.))
                            .text_color(theme::text_muted()),
                    )
                    .into_any_element(),
            );
        }
        let segments = fs::path_segments(&path);
        for (index, (label, segment_path)) in segments.iter().enumerate() {
            let is_last = index == segments.len() - 1;
            let segment_path = segment_path.clone();
            breadcrumb.push(
                div()
                    .flex_none()
                    .text_color(theme::text_faint())
                    .child(
                        svg()
                            .path("icons/chevron-right.svg")
                            .size(px(10.))
                            .text_color(theme::text_faint()),
                    )
                    .into_any_element(),
            );
            breadcrumb.push(
                div()
                    .id(("crumb", index))
                    .flex_none()
                    .max_w(px(170.))
                    .px_1p5()
                    .py_0p5()
                    .rounded_sm()
                    .truncate()
                    .text_size(px(12.))
                    .text_color(if is_last {
                        theme::text()
                    } else {
                        theme::text_muted()
                    })
                    .cursor_pointer()
                    .hover(|this| this.bg(theme::hover()).text_color(theme::text()))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        window.focus(&this.focus_handle);
                        this.navigate_to(segment_path.clone(), cx);
                    }))
                    .child(label.clone())
                    .into_any_element(),
            );
        }

        let breadcrumb_child: AnyElement = if editing_path {
            let session = self.editor.as_ref().expect("path editor");
            let editor = session.editor.clone();
            let focus = editor.read(cx).focus_handle.clone();
            div()
                .id("path-editor")
                .flex_1()
                .min_w(px(120.))
                .h(px(30.))
                .px_2()
                .flex()
                .items_center()
                .gap_2()
                .rounded_md()
                .bg(theme::surface())
                .border_1()
                .border_color(theme::accent())
                .child(svg().path("icons/folder.svg").size(px(14.)).text_color(theme::folder()))
                .child(editor)
                .child(
                    div()
                        .id("path-submit")
                        .flex_none()
                        .size(px(22.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_sm()
                        .cursor_pointer()
                        .text_color(theme::accent())
                        .hover(|this| this.bg(theme::accent_soft()))
                        .on_click(cx.listener(|this, _, window, cx| {
                            window.focus(&this.focus_handle);
                            this.submit_current_editor(window, cx);
                        }))
                        .child(
                            svg()
                                .path("icons/arrow-right.svg")
                                .size(px(13.))
                                .text_color(theme::accent()),
                        ),
                )
                .on_mouse_down(MouseButton::Left, move |_, window, _| {
                    window.focus(&focus);
                })
                .into_any_element()
        } else {
            div()
                .id("breadcrumb")
                .flex_1()
                .min_w(px(120.))
                .h(px(34.))
                .px_1()
                .flex()
                .items_center()
                .gap_0p5()
                .rounded_md()
                .bg(theme::chrome())
                .min_w(px(0.))
                .overflow_hidden()
                .children(breadcrumb)
                .child(
                    div()
                        .id("breadcrumb-blank")
                        .flex_1()
                        .h_full()
                        .min_w(px(12.))
                        .cursor_text()
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.begin_path_edit(window, cx);
                        })),
                )
                .child(
                    div()
                        .id("edit-path")
                        .flex_none()
                        .size(px(24.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_sm()
                        .cursor_pointer()
                        .text_color(theme::text_faint())
                        .hover(|this| {
                            this.bg(theme::hover())
                                .text_color(theme::text())
                        })
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.begin_path_edit(window, cx);
                        }))
                        .child(
                            svg()
                                .path("icons/edit.svg")
                                .size(px(12.))
                                .text_color(theme::text_faint()),
                        ),
                )
                .into_any_element()
        };

        let search_child: AnyElement = if editing_search {
            let session = self.editor.as_ref().expect("search editor");
            let editor = session.editor.clone();
            let focus = editor.read(cx).focus_handle.clone();
            div()
                .id("search-editor")
                .w(px(238.))
                .h(px(30.))
                .px_2()
                .flex()
                .items_center()
                .gap_2()
                .rounded_md()
                .bg(theme::surface())
                .border_1()
                .border_color(theme::accent())
                .child(svg().path("icons/search.svg").size(px(14.)).text_color(theme::text_faint()))
                .child(editor)
                .on_mouse_down(MouseButton::Left, move |_, window, _| {
                    window.focus(&focus);
                })
                .into_any_element()
        } else {
            div()
                .id("search-box")
                .w(px(238.))
                .h(px(30.))
                .px_2()
                .flex()
                .items_center()
                .gap_2()
                .rounded_md()
                .bg(theme::chrome())
                .cursor_text()
                .text_color(if filter.is_empty() {
                    theme::text_faint()
                } else {
                    theme::text()
                })
                .hover(|this| this.bg(theme::hover()))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.begin_search(window, cx);
                }))
                .child(
                    svg()
                        .path("icons/search.svg")
                        .size(px(14.))
                        .text_color(theme::text_faint()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .truncate()
                        .text_size(px(12.))
                        .child(if filter.is_empty() {
                            SharedString::from("筛选当前文件夹…")
                        } else {
                            SharedString::from(filter)
                        }),
                )
                .when(!self.active().filter.is_empty(), |this| {
                    this.child(
                        div()
                            .id("clear-filter")
                            .flex_none()
                            .size(px(18.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_sm()
                            .cursor_pointer()
                            .hover(|this| this.bg(theme::pressed()))
                            .on_click(cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                if this.active().search_root.is_some() {
                                    this.clear_search(cx);
                                    return;
                                }
                                this.active_mut().filter.clear();
                                this.rebuild_visible();
                                cx.notify();
                            }))
                            .child(
                                svg()
                                    .path("icons/close.svg")
                                    .size(px(10.))
                                    .text_color(theme::text_faint()),
                            ),
                    )
                })
                .into_any_element()
        };

        let view_button = |view: ViewMode,
                           id: &'static str,
                           cx: &mut Context<Self>|
         -> AnyElement {
            let active = self.view_mode == view;
            let icon_color = if active {
                theme::accent()
            } else {
                theme::text_muted()
            };
            div()
                .id(id)
                .size(px(26.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_md()
                .cursor_pointer()
                .when(active, |this| this.bg(theme::surface()).shadow_sm())
                .when(!active, |this| {
                    this.hover(|this| this.bg(theme::hover()))
                })
                .on_click(cx.listener(move |this, _, _, cx| this.set_view(view, cx)))
                .child(svg().path(view.icon()).size(px(14.)).text_color(icon_color))
                .into_any_element()
        };

        div()
            .h(px(theme::TOOLBAR_HEIGHT))
            .flex_none()
            .mx(px(6.))
            .mt(px(6.))
            .mb(px(6.))
            .overflow_hidden()
            .flex()
            .items_center()
            .gap_1()
            .px(px(10.))
            .rounded_lg()
            .bg(theme::surface())
            .border_1()
            .border_color(theme::border())
            .shadow_sm()
            .child(
                self.nav_button("nav-back", "icons/arrow-left.svg", can_back, window, cx, |this, window, cx| {
                    this.go_back(window, cx)
                }),
            )
            .child(
                self.nav_button("nav-forward", "icons/arrow-right.svg", can_forward, window, cx, |this, window, cx| {
                    this.go_forward(window, cx)
                }),
            )
            .child(
                self.nav_button("nav-up", "icons/arrow-up.svg", self.active().path.parent().is_some(), window, cx, |this, window, cx| {
                    this.go_up(window, cx)
                }),
            )
            .child(
                self.nav_button("nav-refresh", "icons/refresh.svg", true, window, cx, |this, window, cx| {
                    this.refresh(window, cx)
                }),
            )
            .child(div().w(px(4.)))
            .child(breadcrumb_child)
            .child(search_child)
            .child(div().w(px(4.)))
            .child(div().flex().items_center().gap(px(2.)).rounded_lg().bg(theme::hover()).border_1().border_color(theme::border()).p(px(2.))
                .child(view_button(ViewMode::Details, "view-details", cx))
                .child(view_button(ViewMode::Grid, "view-grid", cx))
                .child(view_button(ViewMode::Split, "view-split", cx)))
            .child(div().w(px(1.)).h(px(18.)).bg(theme::border()))
            .child(
                div()
                    .id("toolbar-more")
                    .size(px(26.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|this| this.bg(theme::hover()))
                    .on_click(cx.listener(|this, _, _, cx| this.show_settings(cx)))
                    .child(
                        svg()
                            .path("icons/settings.svg")
                            .size(px(15.))
                            .text_color(theme::text_muted()),
                    ),
            )
            .into_any_element()
    }

    fn nav_button(
        &self,
        id: &'static str,
        icon: &'static str,
        enabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        let _ = window;
        div()
            .id(id)
            .size(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .when(enabled, |this| {
                this.cursor_pointer()
                    .text_color(theme::text_muted())
                    .hover(|this| {
                        this.bg(theme::hover())
                            .text_color(theme::text())
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        action(this, window, cx);
                    }))
            })
            .when(!enabled, |this| this.opacity(0.28))
            .child(
                svg()
                    .path(icon)
                    .size(px(15.))
                    .text_color(if enabled {
                        theme::text_muted()
                    } else {
                        theme::text_faint()
                    }),
            )
            .into_any_element()
    }


    fn render_sidebar(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let current = self.active().path.clone();
        let places = self.quick_places.clone();
        let drives = self.drives.clone();
        let place_count = places.len();
        let hovered_side = self.hovered_side;
        let hover_prev_side = self.hover_prev_side;
        let side_generation = self.side_generation;
        let side_hover = move |index: usize| -> Option<(bool, bool)> {
            if hovered_side == Some(index) {
                Some((true, hover_prev_side.is_none_or(|from| from < index)))
            } else if hover_prev_side == Some(index) {
                Some((false, hovered_side.is_none_or(|to| to < index)))
            } else {
                None
            }
        };
        let mut rows: Vec<AnyElement> = Vec::new();

        rows.push(
            div()
                .id("quick-access-header")
                .mx_1()
                .px_2()
                .mt_1()
                .py_1()
                .flex()
                .items_center()
                .justify_between()
                .rounded_md()
                .text_size(px(10.5))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme::text_faint())
                .child("快速访问")
                .child(
                    div()
                        .text_size(px(9.))
                        .text_color(theme::text_faint())
                        .child("拖入文件夹可固定"),
                )
                .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                    cx.stop_propagation();
                    for path in paths.paths() {
                        this.add_quick_place(path.clone(), cx);
                    }
                }))
                .into_any_element(),
        );
        for (index, place) in places.into_iter().enumerate() {
            let selected = fs::path_eq(&current, &place.path);
            let path = place.path.clone();
            let drop_path = path.clone();
            let slot = index;
            let place_dragging = self.place_drag == Some(slot) && self.place_drag_outside;
            let highlight = slide_block(
                if selected { None } else { side_hover(slot) },
                None,
                slot,
                side_generation,
                0.0,
                "side-hl",
                |element| element.rounded_md(),
            );
            rows.push(
                div()
                    .id(("sidebar-place", index))
                    .relative()
                    .h(px(30.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_md()
                    .cursor_pointer()
                    .text_size(px(12.5))
                    .when(selected, |this| {
                        this.bg(theme::surface())
                            .text_color(theme::text())
                            .shadow_sm()
                    })
                    .when(!selected, |this| {
                        this.text_color(theme::text_muted())
                            .hover(|this| this.text_color(theme::text()))
                    })
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        let entered = *hovered;
                        this.set_hovered_side(entered.then_some(slot), slot, cx);
                    }))
                    .when(place_dragging, |this| this.opacity(0.45))
                    .child(highlight)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            this.begin_mouse_capture();
                            this.track_place_drag(cx);
                            this.place_drag = Some(slot);
                            this.place_drag_start = Some(event.position);
                            this.place_drag_pos = Some(event.position);
                            this.place_drag_outside = false;
                            this.suppress_place_click = false;
                            cx.notify();
                        }),
                    )
                    .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                        if this.place_drag != Some(slot) {
                            return;
                        }
                        if !crate::drag_out::left_button_down() {
                            this.place_drag = None;
                            this.place_drag_outside = false;
                            cx.notify();
                            return;
                        }
                        this.place_drag_pos = Some(event.position);
                        let start_y = this.place_drag_start.map(|start| f32::from(start.y));
                        let outside = f32::from(event.position.x) > 232.0
                            || start_y.is_none_or(|y| {
                                (f32::from(event.position.y) - y).abs() > 18.0
                            });
                        if outside != this.place_drag_outside {
                            this.place_drag_outside = outside;
                        }
                        cx.notify();
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseUpEvent, _, cx| {
                            if this.place_drag == Some(slot) {
                                this.finish_place_drag(event.position, cx);
                            }
                        }),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if this.suppress_place_click {
                            this.suppress_place_click = false;
                            return;
                        }
                        window.focus(&this.focus_handle);
                        this.navigate_to(path.clone(), cx);
                    }))
                    .on_drop(cx.listener(move |this, paths: &ExternalPaths, _, cx| {
                        cx.stop_propagation();
                        let items = paths.paths().to_vec();
                        // Folders dropped on the sidebar are pinned; files are
                        // copied into the place under the pointer.
                        if !items.is_empty() && items.iter().all(|path| path.is_dir()) {
                            for path in items {
                                this.add_quick_place(path, cx);
                            }
                        } else {
                            this.copy_external_paths(items, drop_path.clone(), cx);
                        }
                    }))
                    .child(
                        svg()
                            .path(place.icon)
                            .size(px(15.))
                            .text_color(if selected {
                                theme::accent()
                            } else {
                                theme::text_muted()
                            }),
                    )
                    .child(div().truncate().child(place.label))
                    .into_any_element(),
            );
        }

        rows.push(
            div()
                .mt_3()
                .px_3()
                .pb_1()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(px(10.5))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme::text_faint())
                        .child("此电脑"),
                )
                .child(
                    div()
                        .id("refresh-drives")
                        .size(px(18.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_sm()
                        .cursor_pointer()
                        .text_color(theme::text_faint())
                        .hover(|this| this.bg(theme::hover()).text_color(theme::text()))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.drives = fs::drives();
                            cx.notify();
                        }))
                        .child(svg().path("icons/refresh.svg").size(px(11.))),
                )
                .into_any_element(),
        );
        for (index, drive) in drives.into_iter().enumerate() {
            let selected = fs::path_eq(&current, &drive.path);
            let path = drive.path.clone();
            let drop_path = path.clone();
            let used = drive.used_fraction();
            let free = fs::format_size(drive.free);
            let name = drive.display_name();
            let slot = place_count + index;
            let highlight = slide_block(
                if selected { None } else { side_hover(slot) },
                None,
                slot,
                side_generation,
                0.0,
                "side-hl",
                |element| element.rounded_md(),
            );
            rows.push(
                div()
                    .id(("drive", index))
                    .relative()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .text_size(px(12.))
                    .when(selected, |this| {
                        this.bg(theme::surface())
                            .text_color(theme::text())
                            .shadow_sm()
                    })
                    .when(!selected, |this| {
                        this.text_color(theme::text_muted())
                            .hover(|this| this.text_color(theme::text()))
                    })
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        let entered = *hovered;
                        this.set_hovered_side(entered.then_some(slot), slot, cx);
                    }))
                    .child(highlight)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        window.focus(&this.focus_handle);
                        this.navigate_to(path.clone(), cx);
                    }))
                    .on_drop(cx.listener(move |this, paths: &ExternalPaths, _, cx| {
                        cx.stop_propagation();
                        this.copy_external_paths(paths.paths().to_vec(), drop_path.clone(), cx);
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                svg()
                                    .path("icons/drive.svg")
                                    .size(px(15.))
                                    .text_color(if selected {
                                        theme::accent()
                                    } else {
                                        theme::text_faint()
                                    }),
                            )
                            .child(div().flex_1().truncate().child(name))
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(9.5))
                                    .text_color(theme::text_faint())
                                    .child(free),
                            ),
                    )
                    .child(
                        div()
                            .ml(px(28.))
                            .h(px(3.))
                            .w(px(154.))
                            .rounded_full()
                            .bg(theme::border_soft())
                            .child(
                                div()
                                    .h(px(3.))
                                    .w(px(154.0 * used.clamp(0.02, 1.0)))
                                    .rounded_full()
                                    .bg(if used > 0.88 {
                                        theme::warning()
                                    } else {
                                        theme::accent().opacity(0.65)
                                    }),
                            ),
                    )
                    .into_any_element(),
            );
        }

        div()
            .w(px(theme::SIDEBAR_WIDTH))
            .h_full()
            .flex_none()
            .overflow_hidden()
            .flex()
            .flex_col()
            .rounded_lg()
            .bg(theme::surface())
            .border_1()
            .border_color(theme::border())
            .shadow_sm()
            .child(
                div()
                    .id("sidebar-scroll")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .px_2()
                    .pb_2()
                    .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                        cx.stop_propagation();
                        for path in paths.paths() {
                            this.add_quick_place(path.clone(), cx);
                        }
                    }))
                    .children(rows),
            )
            .into_any_element()
    }

    fn render_statusbar(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let tab = self.active();
        let selected = tab.selected.len();
        let total = tab.visible.len();
        let selected_size = tab.selected_size();
        let left = if selected > 0 {
            format!(
                "已选 {} 项 · {}",
                selected,
                if selected_size > 0 {
                    fs::format_size(selected_size)
                } else {
                    "—".to_string()
                }
            )
        } else {
            format!("{total} 项")
        };
        let search_note = if tab.searching {
            Some("正在搜索子文件夹…".to_string())
        } else if tab.search_root.is_some() {
            Some(format!(
                "搜索{}：{total} 项{}",
                if tab.search_truncated { "（已截断）" } else { "" },
                if tab.filter.is_empty() {
                    String::new()
                } else {
                    format!("“{}”", tab.filter)
                }
            ))
        } else {
            None
        };
        let operation = self
            .operation
            .clone()
            .or_else(|| tab.loading.then(|| "正在加载…".to_string()))
            .or(search_note);

        div()
            .h(px(theme::STATUS_HEIGHT))
            .w_full()
            .flex()
            .items_center()
            .px_3()
            .gap_3()
            .bg(theme::chrome())
            .border_t_1()
            .border_color(theme::border())
            .text_size(px(10.5))
            .text_color(theme::text_muted())
            .child(left)
            .when_some(operation, |this, text| {
                this.child(div().text_color(theme::accent()).child(text))
            })
            .child(div().flex_1())
            .child(
                div()
                    .text_color(theme::text_faint())
                    .child("拖动：移动 · 按住 Ctrl：复制 · Shift+拖动：框选"),
            )
            .child(
                div()
                    .id("status-settings")
                    .size(px(20.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_sm()
                    .cursor_pointer()
                    .text_color(theme::text_faint())
                    .hover(|this| this.bg(theme::hover()).text_color(theme::text()))
                    .on_click(cx.listener(|this, _, _, cx| this.show_settings(cx)))
                    .child(
                        svg()
                            .path("icons/settings.svg")
                            .size(px(12.))
                            .text_color(theme::text_faint()),
                    ),
            )
            .into_any_element()
    }
}

impl Render for PilotApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.window_handle.is_none() {
            if let Ok(handle) = raw_window_handle::HasWindowHandle::window_handle(window) {
                if let raw_window_handle::RawWindowHandle::Win32(raw) = handle.as_raw() {
                    self.window_handle =
                        Some(windows::Win32::Foundation::HWND(raw.hwnd.get() as *mut _));
                }
            }
        }
        if !self.initial_focus_done {
            self.initial_focus_done = true;
            let focus = self.focus_handle.clone();
            window.on_next_frame(move |window, _| {
                window.focus(&focus);
            });
        }
        if self.pending_focus_list {
            self.pending_focus_list = false;
            let focus = self.focus_handle.clone();
            window.on_next_frame(move |window, _| {
                window.focus(&focus);
            });
        }

        let content = self.render_content(window, cx);
        let overlays = self.render_overlays(window, cx);
        let viewport = window.viewport_size();
        let ghost_moved = match (self.place_drag_start, self.place_drag_pos) {
            (Some(start), Some(position)) => {
                (f32::from(position.x) - f32::from(start.x)).abs() > 5.0
                    || (f32::from(position.y) - f32::from(start.y)).abs() > 5.0
            }
            _ => false,
        };
        let ghost_visible = self.place_drag_pos.is_some_and(|position| {
            f32::from(position.x) >= 0.0
                && f32::from(position.y) >= 0.0
                && f32::from(position.x) <= f32::from(viewport.width)
                && f32::from(position.y) <= f32::from(viewport.height)
        });
        let drag_ghost: AnyElement = match (self.place_drag, self.place_drag_pos) {
            (Some(slot), Some(position)) if ghost_moved && ghost_visible => {
                match self.quick_places.get(slot) {
                    Some(place) => {
                        let removing = self.place_drag_outside;
                        div()
                            .absolute()
                            .left(px(f32::from(position.x) - 46.0))
                            .top(px(f32::from(position.y) - 16.0))
                            .h(px(30.))
                            .px_2()
                            .flex()
                            .items_center()
                            .gap_2()
                            .rounded_md()
                            .bg(theme::surface())
                            .border_1()
                            .border_color(if removing {
                                theme::danger()
                            } else {
                                theme::accent_border()
                            })
                            .shadow_lg()
                            .opacity(0.96)
                            .text_size(px(12.))
                            .text_color(theme::text())
                            .child(
                                svg()
                                    .path(place.icon)
                                    .size(px(14.))
                                    .text_color(theme::text_muted()),
                            )
                            .child(div().max_w(px(150.)).truncate().child(place.label.clone()))
                            .child(
                                div()
                                    .text_size(px(9.5))
                                    .text_color(if removing {
                                        theme::danger()
                                    } else {
                                        theme::text_faint()
                                    })
                                    .child(if removing { "松开移除" } else { "拖出移除" }),
                            )
                            .into_any_element()
                    }
                    None => div().into_any_element(),
                }
            }
            _ => div().into_any_element(),
        };
        let band_overlay: AnyElement = match self.rubber_band {
            Some((start, current)) => {
                let x1 = f32::from(start.x);
                let y1 = f32::from(start.y);
                let x2 = f32::from(current.x);
                let y2 = f32::from(current.y);
                div()
                    .absolute()
                    .left(px(x1.min(x2)))
                    .top(px(y1.min(y2)))
                    .w(px((x1 - x2).abs().max(1.0)))
                    .h(px((y1 - y2).abs().max(1.0)))
                    .border_1()
                    .border_color(theme::accent())
                    .bg(theme::accent().opacity(0.12))
                    .into_any_element()
            }
            None => div().into_any_element(),
        };

        div()
            .id("app-root")
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(theme::canvas())
            .font(theme::app_font(self.font_family.clone(), self.fallback_fonts.clone()))
            .text_color(theme::text())
            .key_context(FILE_MANAGER_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if this.place_drag.is_none() {
                    return;
                }
                this.place_drag_pos = Some(event.position);
                let start_y = this
                    .place_drag_start
                    .map(|position| f32::from(position.y));
                let outside = f32::from(event.position.x) > 232.0
                    || start_y.is_none_or(|y| (f32::from(event.position.y) - y).abs() > 18.0);
                this.place_drag_outside = outside;
                if !crate::drag_out::left_button_down() {
                    // The button was released without us seeing the mouse-up
                    // (e.g. released outside the window): settle right here.
                    this.finish_place_drag(event.position, cx);
                    return;
                }
                cx.notify();
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, event: &MouseUpEvent, window, cx| {
                // Fallback for dropping a quick-access row outside the sidebar:
                // the row's own mouse-up may never fire once the pointer leaves it,
                // so decide from the position where the button was released.
                if this.place_drag.is_some() {
                    this.finish_place_drag(event.position, cx);
                }
                if this.rubber_band.take().is_some() {
                    cx.notify();
                }
                if this.editor.is_none() {
                    window.focus(&this.focus_handle);
                }
            }))
            .on_action(cx.listener(Self::action_back))
            .on_action(cx.listener(Self::action_forward))
            .on_action(cx.listener(Self::action_up))
            .on_action(cx.listener(Self::action_refresh))
            .on_action(cx.listener(Self::action_focus_path))
            .on_action(cx.listener(Self::action_focus_search))
            .on_action(cx.listener(Self::action_new_tab))
            .on_action(cx.listener(Self::action_close_tab))
            .on_action(cx.listener(Self::action_next_tab))
            .on_action(cx.listener(Self::action_prev_tab))
            .on_action(cx.listener(Self::action_open))
            .on_action(cx.listener(Self::action_open_new_tab))
            .on_action(cx.listener(Self::action_copy))
            .on_action(cx.listener(Self::action_cut))
            .on_action(cx.listener(Self::action_paste))
            .on_action(cx.listener(Self::action_trash))
            .on_action(cx.listener(Self::action_permanent_delete))
            .on_action(cx.listener(Self::action_rename))
            .on_action(cx.listener(Self::action_new_folder))
            .on_action(cx.listener(Self::action_new_file))
            .on_action(cx.listener(Self::action_select_all))
            .on_action(cx.listener(Self::action_select_prev))
            .on_action(cx.listener(Self::action_select_next))
            .on_action(cx.listener(Self::action_select_prev_extend))
            .on_action(cx.listener(Self::action_select_next_extend))
            .on_action(cx.listener(Self::action_select_first))
            .on_action(cx.listener(Self::action_select_last))
            .on_action(cx.listener(Self::action_select_first_extend))
            .on_action(cx.listener(Self::action_select_last_extend))
            .on_action(cx.listener(Self::action_view_details))
            .on_action(cx.listener(Self::action_view_grid))
            .on_action(cx.listener(Self::action_view_split))
            .on_action(cx.listener(Self::action_toggle_sidebar))
            .on_action(cx.listener(Self::action_toggle_hidden))
            .on_action(cx.listener(Self::action_copy_path))
            .on_action(cx.listener(Self::action_undo))
            .on_action(cx.listener(Self::action_reveal))
            .on_action(cx.listener(Self::action_settings))
            .on_action(cx.listener(Self::action_cancel))
            .child(self.render_titlebar(window, cx))
            .child(self.render_toolbar(window, cx))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .gap(px(6.))
                    .px(px(6.))
                    .pb(px(6.))
                    .when(self.show_sidebar, |this| {
                        this.child(self.render_sidebar(window, cx))
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .child(content),
                    ),
            )
            .child(self.render_statusbar(window, cx))
            .children(overlays)
            .child(drag_ghost)
            .child(band_overlay)
    }
}

impl PilotApp {
    fn render_content(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let error = self.active().error.clone();
        let loading = self.active().loading;
        let visible_len = self.active().visible.len();
        let filter = self.active().filter.clone();
        let searching = self.active().searching;
        let search_root = self.active().search_root.clone();

        let current_path_label = self.active().path.to_string_lossy().into_owned();
        let inner: AnyElement = if let Some(error) = error {
            self.render_empty_state(
                "icons/folder.svg",
                "无法显示此文件夹",
                &error,
                Some(("重试", EmptyAction::Retry)),
                window,
                cx,
            )
        } else if visible_len == 0 {
            if searching {
                self.render_empty_state(
                    "icons/search.svg",
                    "正在搜索子文件夹…",
                    &current_path_label,
                    None,
                    window,
                    cx,
                )
            } else if search_root.is_some() {
                self.render_empty_state(
                    "icons/search.svg",
                    "没有匹配的项目",
                    &format!("已在 {} 及其子文件夹中搜索", current_path_label),
                    Some(("退出搜索", EmptyAction::ClearFilter)),
                    window,
                    cx,
                )
            } else if loading {
                self.render_empty_state(
                    "icons/folder.svg",
                    "正在加载…",
                    &current_path_label,
                    None,
                    window,
                    cx,
                )
            } else if !filter.is_empty() {
                self.render_empty_state(
                    "icons/search.svg",
                    "没有匹配的项目",
                    &format!("当前筛选：{filter}"),
                    Some(("清除筛选", EmptyAction::ClearFilter)),
                    window,
                    cx,
                )
            } else {
                self.render_empty_state(
                    "icons/folder.svg",
                    "此文件夹为空",
                    "拖入文件，或新建一个文件夹开始整理。",
                    Some(("新建文件夹", EmptyAction::NewFolder)),
                    window,
                    cx,
                )
            }
        } else {
            match self.view_mode {
                ViewMode::Details => self.render_details_section(true, window, cx),
                ViewMode::Grid => self.render_grid(window, cx),
                ViewMode::Split => div()
                    .flex()
                    .size_full()
                    .min_h(px(0.))
                    .child(self.render_details_section(true, window, cx))
                    .child(self.render_preview(window, cx))
                    .into_any_element(),
            }
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .min_h(px(0.))
            .overflow_hidden()
            .rounded_lg()
            .bg(theme::surface())
            .border_1()
            .border_color(theme::border())
            .shadow_sm()
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.handle_external_drop(paths, cx);
            }))
            .child(inner)
            .into_any_element()
    }

    fn render_empty_state(
        &mut self,
        icon: &'static str,
        title: &str,
        message: &str,
        action: Option<(&'static str, EmptyAction)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let action_element: AnyElement = match action {
            Some((label, EmptyAction::Retry)) => div()
                .id("empty-retry")
                .mt_2()
                .px_3()
                .py_1p5()
                .rounded_md()
                .bg(theme::accent())
                .text_color(theme::white())
                .text_size(px(12.))
                .cursor_pointer()
                .hover(|this| this.bg(theme::accent_hover()))
                .on_click(cx.listener(|this, _, window, cx| {
                    window.focus(&this.focus_handle);
                    this.reload_active(cx);
                }))
                .child(label)
                .into_any_element(),
            Some((label, EmptyAction::ClearFilter)) => div()
                .id("empty-clear")
                .mt_2()
                .px_3()
                .py_1p5()
                .rounded_md()
                .bg(theme::accent())
                .text_color(theme::white())
                .text_size(px(12.))
                .cursor_pointer()
                .hover(|this| this.bg(theme::accent_hover()))
                .on_click(cx.listener(|this, _, _, cx| this.clear_search(cx)))
                .child(label)
                .into_any_element(),
            Some((label, EmptyAction::NewFolder)) => div()
                .id("empty-new-folder")
                .mt_2()
                .px_3()
                .py_1p5()
                .rounded_md()
                .bg(theme::accent())
                .text_color(theme::white())
                .text_size(px(12.))
                .cursor_pointer()
                .hover(|this| this.bg(theme::accent_hover()))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.begin_new_folder(window, cx);
                }))
                .child(label)
                .into_any_element(),
            None => div().into_any_element(),
        };
        let _ = window;
        div()
            .flex_1()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .bg(theme::surface())
            .child(
                div()
                    .size(px(64.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(theme::chrome())
                    .child(svg().path(icon).size(px(30.)).text_color(theme::text_faint())),
            )
            .child(
                div()
                    .text_size(px(14.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme::text())
                    .child(title.to_string()),
            )
            .child(
                div()
                    .max_w(px(440.))
                    .text_center()
                    .text_size(px(11.5))
                    .line_height(px(18.))
                    .text_color(theme::text_muted())
                    .child(message.to_string()),
            )
            .child(action_element)
            .into_any_element()
    }

    fn render_details_section(
        &mut self,
        include_header: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let header = if include_header {
            self.render_list_header(cx)
        } else {
            div().into_any_element()
        };
        let list = self.render_details_list(window, cx);
        div()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .flex()
            .flex_col()
            .child(header)
            .child(list)
            .into_any_element()
    }

    fn render_list_header(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let key = self.sort_key;
        let asc = self.sort_asc;
        let cell = |label: &'static str,
                    sort: SortKey,
                    width: Option<Pixels>,
                    cx: &mut Context<Self>|
         -> AnyElement {
            let active = key == sort;
            let is_flex = width.is_none();
            div()
                .id(SharedString::from(format!("header-{}", sort.label())))
                .h_full()
                .when_some(width, |this, width| this.w(width))
                .when(is_flex, |this| this.flex_1().min_w(px(0.)))
                .flex()
                .items_center()
                .gap_1()
                .text_size(px(10.5))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(if active { theme::text() } else { theme::text_faint() })
                .cursor_pointer()
                .hover(|this| this.text_color(theme::text()))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.sort_by(sort, cx);
                }))
                .child(label)
                .when(active, |this| {
                    this.child(
                        svg()
                            .path(if asc {
                                "icons/chevron-up.svg"
                            } else {
                                "icons/chevron-down.svg"
                            })
                            .size(px(10.))
                            .text_color(theme::text_faint()),
                    )
                })
                .into_any_element()
        };

        div()
            .h(px(34.))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .gap_3()
            .px_3()
            .bg(theme::chrome())
            .border_b_1()
            .border_color(theme::border())
            .child(cell("名称", SortKey::Name, None, cx))
            .child(cell("类型", SortKey::Type, Some(px(122.)), cx))
            .child(cell("修改日期", SortKey::Modified, Some(px(148.)), cx))
            .child(cell("大小", SortKey::Size, Some(px(82.)), cx))
            .into_any_element()
    }

    fn render_details_list(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let _ = window;
        let tab = self.active();
        let entries = tab.entries.clone();
        let visible = tab.visible.clone();
        let selected = Arc::new(tab.selected.clone());
        let scroll = tab.details_scroll.clone();
        let weak = cx.entity().downgrade();
        let focus = self.focus_handle.clone();
        let search_root = tab.search_root.clone();
        let hovered_row = self.hovered_row;
        let hover_prev = self.hover_prev;
        let hover_generation = self.hover_generation;
        let hover_state = move |index: usize| -> Option<(bool, bool)> {
            if hovered_row == Some(index) {
                // Growing into this row: anchor at the edge we came from.
                Some((true, hover_prev.is_none_or(|from| from < index)))
            } else if hover_prev == Some(index) {
                // Shrinking away: keep the edge the highlight is heading to.
                Some((false, hovered_row.is_none_or(|to| to < index)))
            } else {
                None
            }
        };
        let item_count = visible.len();

        let list = uniform_list(
            "details-list",
            item_count,
            move |range, _window, _cx| {
                range
                    .filter_map(|display_index| {
                        let entry_index = *visible.get(display_index)?;
                        let entry = entries.get(entry_index)?.clone();
                        let is_selected = selected.contains(&entry.path);
                        Some(details_row(
                            display_index,
                            entry,
                            is_selected,
                            hover_state(display_index),
                            hover_generation,
                            search_root.clone(),
                            weak.clone(),
                            focus.clone(),
                        ))
                    })
                    .collect::<Vec<_>>()
            },
        )
        .track_scroll(scroll)
        .size_full();

        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                window.focus(&this.focus_handle);
                this.active_mut().selected.clear();
                this.context_menu = None;
                this.rubber_band = Some((event.position, event.position));
                cx.notify();
            }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if let Some((start, _)) = this.rubber_band {
                    if !crate::drag_out::left_button_down() {
                        this.rubber_band = None;
                    } else {
                        this.rubber_band = Some((start, event.position));
                    }
                    cx.notify();
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| {
                if this.rubber_band.take().is_some() {
                    cx.notify();
                }
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(|this, event, _, cx| {
                this.update_context_menu(event, cx);
            }))
            .child(list)
            .into_any_element()
    }

    fn render_grid(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let tab = self.active();
        let entries = tab.entries.clone();
        let visible = tab.visible.clone();
        let selected = Arc::new(tab.selected.clone());
        let scroll = tab.grid_scroll.clone();
        let search_root = tab.search_root.clone();
        let hovered_row = self.hovered_row;
        let hover_prev = self.hover_prev;
        let hover_generation = self.hover_generation;
        let hover_state = move |index: usize| -> Option<(bool, bool)> {
            if hovered_row == Some(index) {
                Some((true, hover_prev.is_none_or(|from| from < index)))
            } else if hover_prev == Some(index) {
                Some((false, hovered_row.is_none_or(|to| to < index)))
            } else {
                None
            }
        };
        let weak = cx.entity().downgrade();
        let focus = self.focus_handle.clone();

        let viewport_width = f32::from(window.viewport_size().width);
        let sidebar = if self.show_sidebar {
            theme::SIDEBAR_WIDTH
        } else {
            0.0
        };
        let available = (viewport_width - sidebar - 44.0).max(320.0);
        let columns = ((available + 12.0) / 168.0).floor().clamp(2.0, 8.0) as usize;
        let gap = 12.0;
        let card_width = ((available - gap * (columns as f32 - 1.0)) / columns as f32).max(132.0);
        let row_count = visible.len().div_ceil(columns);

        let list = uniform_list(
            "grid-list",
            row_count,
            move |range, _, _cx| {
                range
                    .map(|row| {
                        let start = row * columns;
                        let end = (start + columns).min(visible.len());
                        let mut row_element = div().h(px(178.)).w_full().flex().gap(px(gap));
                        for display_index in start..end {
                            let Some(entry) = visible
                                .get(display_index)
                                .and_then(|index| entries.get(*index))
                                .cloned()
                            else {
                                continue;
                            };
                            let is_selected = selected.contains(&entry.path);
                            row_element = row_element.child(grid_card(
                                display_index,
                                entry,
                                is_selected,
                                hover_state(display_index),
                                hover_generation,
                                search_root.clone(),
                                weak.clone(),
                                focus.clone(),
                                card_width,
                            ));
                        }
                        row_element.into_any_element()
                    })
                    .collect::<Vec<_>>()
            },
        )
        .track_scroll(scroll)
        .size_full();

        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .p(px(12.))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                window.focus(&this.focus_handle);
                this.active_mut().selected.clear();
                this.context_menu = None;
                this.rubber_band = Some((event.position, event.position));
                cx.notify();
            }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if let Some((start, _)) = this.rubber_band {
                    if !crate::drag_out::left_button_down() {
                        this.rubber_band = None;
                    } else {
                        this.rubber_band = Some((start, event.position));
                    }
                    cx.notify();
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| {
                if this.rubber_band.take().is_some() {
                    cx.notify();
                }
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(|this, event, _, cx| {
                this.update_context_menu(event, cx);
            }))
            .child(list)
            .into_any_element()
    }

    fn render_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let _ = window;
        let tab = self.active();
        let selected_count = tab.selected.len();
        let preview = tab.preview_entry().cloned();
        let preview_name = preview.as_ref().map(|entry| entry.name.clone());
        let selected_entries = tab.selected_entries();
        let total_selected_size = tab.selected_size();

        let body: AnyElement = if selected_count > 1 {
            let mut names: Vec<AnyElement> = Vec::new();
            for entry in selected_entries.iter().take(9) {
                names.push(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            svg()
                                .path(entry.kind.icon())
                                .size(px(13.))
                                .text_color(kind_color(entry.kind)),
                        )
                        .child(div().truncate().child(entry.name.clone()))
                        .into_any_element(),
                );
            }
            if selected_entries.len() > 9 {
                names.push(
                    div()
                        .text_color(theme::text_faint())
                        .child(format!("…以及另外 {} 项", selected_entries.len() - 9))
                        .into_any_element(),
                );
            }
            div()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .h(px(150.))
                        .w_full()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap_2()
                        .rounded_md()
                        .bg(theme::chrome())
                        .child(
                            div()
                                .text_size(px(30.))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(theme::accent())
                                .child(selected_count.to_string()),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(theme::text_muted())
                                .child("个项目已选择"),
                        ),
                )
                .child(meta_row("总大小", &fs::format_size(total_selected_size)))
                .child(div().h(px(1.)).bg(theme::border_soft()))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1p5()
                        .text_size(px(11.5))
                        .text_color(theme::text_muted())
                        .children(names),
                )
                .into_any_element()
        } else if let Some(entry) = preview {
            let is_image = entry.kind == FileKind::Image
                && !entry.is_dir
                && entry.size < 64 * 1024 * 1024
                && matches!(
                    entry.extension.as_str(),
                    "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico"
                );
            let tile: AnyElement = if is_image {
                let icon = entry.kind.icon();
                let color = kind_color(entry.kind);
                let label = entry.extension.to_ascii_uppercase();
                img(entry.path.clone())
                    .w_full()
                    .h_full()
                    .object_fit(ObjectFit::Contain)
                    .with_fallback(move || {
                        type_tile(icon, color, &label, 180.0).into_any_element()
                    })
                    .into_any_element()
            } else if entry.is_dir {
                div()
                    .w_full()
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        svg()
                            .path("icons/folder.svg")
                            .size(px(72.))
                            .text_color(theme::folder()),
                    )
                    .into_any_element()
            } else {
                type_tile(
                    entry.kind.icon(),
                    kind_color(entry.kind),
                    &entry
                        .extension
                        .to_ascii_uppercase()
                        .chars()
                        .take(5)
                        .collect::<String>(),
                    180.0,
                )
                .into_any_element()
            };

            let mut meta = vec![
                meta_row("类型", &entry.type_label),
                meta_row(
                    "大小",
                    if entry.is_dir {
                        "—".to_string()
                    } else {
                        fs::format_size(entry.size)
                    }
                    .as_str(),
                ),
                meta_row("修改时间", &fs::format_datetime(entry.modified)),
                meta_row("访问时间", &fs::format_datetime(entry.accessed)),
                meta_row("创建时间", &fs::format_datetime(entry.created)),
            ];
            if entry.is_symlink {
                meta.push(meta_row("链接", "符号链接"));
            }

            let open_path = entry.path.clone();
            let reveal_path = entry.path.clone();
            div()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .h(px(216.))
                        .w_full()
                        .p(px(10.))
                        .rounded_lg()
                        .bg(theme::canvas())
                        .border_1()
                        .border_color(theme::border_soft())
                        .child(tile),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_size(px(14.))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .line_clamp(2)
                                .child(entry.name.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(10.5))
                                .text_color(theme::text_faint())
                                .line_clamp(2)
                                .child(entry.path.to_string_lossy().into_owned()),
                        ),
                )
                .child(div().h(px(1.)).bg(theme::border_soft()))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .text_size(px(11.5))
                        .children(meta),
                )
                .child(
                    div()
                        .mt_1()
                        .flex()
                        .gap_2()
                        .child(
                            div()
                                .id("preview-open")
                                .flex_1()
                                .h(px(30.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .gap_1p5()
                                .rounded_md()
                                .bg(theme::accent())
                                .text_color(theme::white())
                                .text_size(px(11.5))
                                .cursor_pointer()
                                .hover(|this| this.bg(theme::accent_hover()))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    window.focus(&this.focus_handle);
                                    if open_path.is_dir() {
                                        this.navigate_to(open_path.clone(), cx);
                                    } else if let Err(error) = ops::open_with_shell(&open_path) {
                                        this.show_error("无法打开", error);
                                        cx.notify();
                                    }
                                }))
                                .child(svg().path("icons/external.svg").size(px(12.)))
                                .child("打开"),
                        )
                        .child(
                            div()
                                .id("preview-reveal")
                                .flex_1()
                                .h(px(30.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .gap_1p5()
                                .rounded_md()
                                .bg(theme::chrome())
                                .border_1()
                                .border_color(theme::border())
                                .text_color(theme::text_muted())
                                .text_size(px(11.5))
                                .cursor_pointer()
                                .hover(|this| this.bg(theme::hover()).text_color(theme::text()))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Err(error) = ops::reveal_in_explorer(&reveal_path) {
                                        this.show_error("无法定位", error);
                                        cx.notify();
                                    }
                                }))
                                .child("定位"),
                        ),
                )
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .h_full()
                .text_color(theme::text_faint())
                .child(svg().path("icons/file.svg").size(px(34.)))
                .child(
                    div()
                        .text_size(px(11.5))
                        .child("选择一个文件进行预览"),
                )
                .into_any_element()
        };

        let header = match (selected_count, preview_name) {
            (0, _) => "预览".to_string(),
            (1, Some(name)) => format!("预览 · {name}"),
            (n, _) => format!("已选择 {n} 项"),
        };

        div()
            .w(px(328.))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .gap_3()
            .p(px(14.))
            .bg(theme::surface())
            .border_l_1()
            .border_color(theme::border())
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(10.5))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme::text_faint())
                            .child(header),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(theme::text_faint())
                            .child("Shift+点击可多选"),
                    ),
            )
            .child(
                div()
                    .id("preview-scroll")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .child(body),
            )
            .into_any_element()
    }
}

#[derive(Clone, Copy)]
enum EmptyAction {
    Retry,
    ClearFilter,
    NewFolder,
}

fn meta_row(label: &str, value: &str) -> AnyElement {
    div()
        .flex()
        .items_start()
        .gap_2()
        .child(
            div()
                .w(px(58.))
                .flex_none()
                .text_color(theme::text_faint())
                .child(label.to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .text_color(theme::text_muted())
                .line_clamp(2)
                .child(value.to_string()),
        )
        .into_any_element()
}

fn kind_color(kind: FileKind) -> gpui::Hsla {
    match kind {
        FileKind::Folder => theme::folder(),
        FileKind::Image => theme::blue(),
        FileKind::Video => theme::purple(),
        FileKind::Audio => theme::teal(),
        FileKind::Document => theme::danger(),
        FileKind::Archive => theme::warning(),
        FileKind::Code => theme::purple(),
        FileKind::Executable => theme::success(),
        FileKind::Font => theme::text_muted(),
        FileKind::Other => theme::text_faint(),
    }
}

fn type_tile(icon: &'static str, color: gpui::Hsla, label: &str, icon_size: f32) -> impl IntoElement {
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_2()
        .rounded_lg()
        .bg(color.opacity(0.10))
        .child(svg().path(icon).size(px(icon_size * 0.36)).text_color(color))
        .child(
            div()
                .max_w_full()
                .truncate()
                .text_size(px(11.))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(color)
                .child(label.to_string()),
        )
}

fn open_entry_from_card(weak: gpui::WeakEntity<PilotApp>, entry: FileEntry, cx: &mut App) {
    if let Some(app) = weak.upgrade() {
        let _ = app.update(cx, |app, cx| app.open_entry(entry, false, cx));
    }
}

fn card_row_click_count(event: &ClickEvent) -> usize {
    match event {
        ClickEvent::Mouse(mouse) => mouse.down.click_count,
        ClickEvent::Keyboard(_) => 0,
    }
}

/// Highlight used by list rows, grid cards and sidebar entries.
///
/// It grows from slightly inset and transparent to a full-size rounded block,
/// so it reads as the highlight floating up from behind the UI instead of
/// painting over the item.
///
/// The element is *always* emitted (transparent when idle) and always
/// `absolute`, so the row keeps a stable element tree and stable flex layout.
/// Emitting it conditionally made the hitbox change every frame, which made
/// GPUI fire hover events in a loop and the highlight never settled.
fn slide_block(
    hover: Option<(bool, bool)>,
    selected_tint: Option<Hsla>,
    index: usize,
    generation: u64,
    side_inset: f32,
    idle_key: &str,
    decorate: impl Fn(gpui::Div) -> gpui::Div + 'static,
) -> AnyElement {
    let base = decorate(div().absolute());
    if let Some(tint) = selected_tint {
        return base
            .left(px(side_inset))
            .right(px(side_inset))
            .top(px(0.))
            .bottom(px(0.))
            .bg(tint)
            .with_animation(
                SharedString::from(format!("{idle_key}-sel-{index}")),
                Animation::new(Duration::from_millis(1)),
                |element, _| element,
            )
            .into_any_element();
    }
    match hover {
        Some((entering, _)) => {
            let animation =
                Animation::new(Duration::from_millis(200)).with_easing(gpui::ease_out_quint());
            base.bg(theme::row_hover())
                .with_animation(
                    SharedString::from(format!("{idle_key}-{index}-{generation}")),
                    animation,
                    move |element, delta| {
                        let shown = if entering { delta } else { 1.0 - delta };
                        let shrink = 1.0 - shown;
                        element
                            .top(px(shrink * 9.0))
                            .bottom(px(shrink * 1.0))
                            .left(px(side_inset + shrink * 6.0))
                            .right(px(side_inset + shrink * 6.0))
                            .opacity(shown)
                    },
                )
                .into_any_element()
        }
        None => base
            .left(px(side_inset))
            .right(px(side_inset))
            .top(px(0.))
            .bottom(px(0.))
            .opacity(0.0)
            .with_animation(
                SharedString::from(format!("{idle_key}-idle-{index}")),
                Animation::new(Duration::from_millis(1)),
                |element, _| element.opacity(0.0),
            )
            .into_any_element(),
    }
}

/// Folder shown in the "type" column while search results are displayed.
fn search_location(root: &Path, path: &Path) -> String {
    let Some(parent) = path.parent() else {
        return String::new();
    };
    match parent.strip_prefix(root) {
        Ok(relative) if relative.as_os_str().is_empty() => ".".to_string(),
        Ok(relative) => format!("{}{}", relative.to_string_lossy(), std::path::MAIN_SEPARATOR),
        Err(_) => parent.to_string_lossy().into_owned(),
    }
}

fn details_row(
    display_index: usize,
    entry: FileEntry,
    selected: bool,
    hover: Option<(bool, bool)>,
    hover_generation: u64,
    search_root: Option<PathBuf>,
    weak: gpui::WeakEntity<PilotApp>,
    focus: FocusHandle,
) -> AnyElement {
    let path_left = entry.path.clone();
    let path_right = entry.path.clone();
    let path_move = entry.path.clone();
    let weak_down = weak.clone();
    let weak_right = weak.clone();
    let weak_click = weak.clone();
    let weak_up = weak.clone();
    let weak_move = weak.clone();
    let weak_hover = weak.clone();
    let focus_down = focus.clone();
    let focus_click = focus.clone();
    let focus_right = focus.clone();
    let click_entry = entry.clone();
    let row_hidden = entry.is_hidden;
    let row_selected = selected;
    let size = if entry.is_dir {
        "—".to_string()
    } else {
        fs::format_size(entry.size)
    };
    let modified = fs::format_datetime(entry.modified);
    let highlight = slide_block(
        hover,
        selected.then(|| theme::accent_soft()),
        display_index,
        hover_generation,
        6.0,
        "row-hl",
        |element| element.rounded_md(),
    );

    let base = div()
        .id(("file-row", display_index))
        .relative()
        .h(px(theme::ROW_HEIGHT))
        .w_full()
        .flex()
        .items_center()
        .gap_3()
        .px_3()
        .when(row_hidden, |this| this.opacity(0.55))
        .on_hover(move |hovered: &bool, _, cx| {
            if let Some(app) = weak_hover.upgrade() {
                let index = display_index;
                let entered = *hovered;
                let _ = app.update(cx, |app, cx| {
                    app.set_hovered_row(entered.then_some(index), index, cx);
                });
            }
        })
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, move |event, window, cx| {
            window.focus(&focus_down);
            if let Some(app) = weak_down.upgrade() {
                let path = path_left.clone();
                let _ = app.update(cx, |app, cx| {
                    app.row_mouse_down(display_index, &path, event, window, cx);
                });
            }
            cx.stop_propagation();
        })
        .on_mouse_down(MouseButton::Right, move |event, window, cx| {
            window.focus(&focus_right);
            if let Some(app) = weak_right.upgrade() {
                let path = path_right.clone();
                let _ = app.update(cx, |app, cx| {
                    app.row_right_click(display_index, &path, event, cx);
                });
            }
            cx.stop_propagation();
        })
        .on_mouse_move(move |_, _, cx| {
            if let Some(app) = weak_move.upgrade() {
                let path = path_move.clone();
                let _ = app.update(cx, |app, cx| {
                    if app.rubber_band.is_some() && crate::drag_out::left_button_down() {
                        if app.active_mut().selected.insert(path) {
                            cx.notify();
                        }
                    }
                });
            }
        })
        .on_mouse_up(MouseButton::Left, move |_, _, cx| {
            if let Some(app) = weak_up.upgrade() {
                let _ = app.update(cx, |app, cx| {
                    app.pending_drag = None;
                    if app.rubber_band.take().is_some() {
                        cx.notify();
                    }
                });
            }
        })
        .on_click(move |event, window, cx| {
            if card_row_click_count(event) >= 2 {
                window.focus(&focus_click);
                open_entry_from_card(weak_click.clone(), click_entry.clone(), cx);
            }
        })
        .child(highlight)
        .child(
            div()
                .w(px(2.))
                .h(px(18.))
                .rounded_full()
                .bg(if row_selected {
                    theme::accent()
                } else {
                    theme::surface().opacity(0.0)
                }),
        )
        .child(
            svg()
                .path(entry.kind.icon())
                .size(px(17.))
                .text_color(kind_color(entry.kind)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .text_size(px(12.))
                .text_color(theme::text())
                .child(entry.name.clone()),
        )
        .child(
            div()
                .w(px(122.))
                .flex_none()
                .truncate()
                .text_size(px(10.5))
                .text_color(theme::text_faint())
                .child(match &search_root {
                    Some(root) => search_location(root, &entry.path),
                    None => entry.type_label.clone(),
                }),
        )
        .child(
            div()
                .w(px(148.))
                .flex_none()
                .truncate()
                .text_size(px(10.5))
                .text_color(theme::text_faint())
                .child(modified),
        )
        .child(
            div()
                .w(px(82.))
                .flex_none()
                .text_right()
                .truncate()
                .text_size(px(10.5))
                .text_color(theme::text_faint())
                .child(size),
        );

    base.into_any_element()
}

fn grid_card(
    display_index: usize,
    entry: FileEntry,
    selected: bool,
    hover: Option<(bool, bool)>,
    hover_generation: u64,
    search_root: Option<PathBuf>,
    weak: gpui::WeakEntity<PilotApp>,
    focus: FocusHandle,
    card_width: f32,
) -> AnyElement {
    let path_left = entry.path.clone();
    let path_right = entry.path.clone();
    let path_move = entry.path.clone();
    let weak_down = weak.clone();
    let weak_right = weak.clone();
    let weak_click = weak.clone();
    let weak_up = weak.clone();
    let weak_move = weak.clone();
    let weak_hover = weak.clone();
    let focus_down = focus.clone();
    let focus_click = focus.clone();
    let focus_right = focus.clone();
    let click_entry = entry.clone();
    let is_image = entry.kind == FileKind::Image
        && !entry.is_dir
        && entry.size < 64 * 1024 * 1024
        && matches!(
            entry.extension.as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico"
        );
    let color = kind_color(entry.kind);
    let icon = entry.kind.icon();
    let extension = entry.extension.to_ascii_uppercase();

    let image_area: AnyElement = if entry.is_dir {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(
                svg()
                    .path("icons/folder.svg")
                    .size(px(42.))
                    .text_color(theme::folder()),
            )
            .into_any_element()
    } else if is_image {
        let fallback_icon = icon;
        let fallback_label = extension.clone();
        img(entry.path.clone())
            .size_full()
            .object_fit(ObjectFit::Cover)
            .with_fallback(move || {
                type_tile(fallback_icon, color, &fallback_label, 120.0).into_any_element()
            })
            .into_any_element()
    } else {
        type_tile(icon, color, &extension, 120.0).into_any_element()
    };

    let date = fs::format_relative(entry.modified);
    let highlight = slide_block(
        if selected { None } else { hover },
        None,
        display_index,
        hover_generation,
        0.0,
        "card-hl",
        |element| element.rounded_lg(),
    );

    let base = div()
        .id(("grid-card", display_index))
        .relative()
        .w(px(card_width))
        .h(px(168.))
        .flex()
        .flex_col()
        .gap_2()
        .p_2()
        .rounded_lg()
        .border_1()
        .border_color(if selected {
            theme::accent_border()
        } else {
            theme::border_soft()
        })
        .bg(if selected {
            theme::accent_soft()
        } else {
            theme::surface()
        })
        .cursor_pointer()
        .hover(move |this| {
            this.border_color(if selected {
                theme::accent_border()
            } else {
                theme::border()
            })
            .shadow_sm()
        })
        .on_hover(move |hovered: &bool, _, cx| {
            if let Some(app) = weak_hover.upgrade() {
                let index = display_index;
                let entered = *hovered;
                let _ = app.update(cx, |app, cx| {
                    app.set_hovered_row(entered.then_some(index), index, cx);
                });
            }
        })
        .on_mouse_down(MouseButton::Left, move |event, window, cx| {
            window.focus(&focus_down);
            if let Some(app) = weak_down.upgrade() {
                let path = path_left.clone();
                let _ = app.update(cx, |app, cx| {
                    app.row_mouse_down(display_index, &path, event, window, cx);
                });
            }
            cx.stop_propagation();
        })
        .on_mouse_down(MouseButton::Right, move |event, window, cx| {
            window.focus(&focus_right);
            if let Some(app) = weak_right.upgrade() {
                let path = path_right.clone();
                let _ = app.update(cx, |app, cx| {
                    app.row_right_click(display_index, &path, event, cx);
                });
            }
            cx.stop_propagation();
        })
        .on_mouse_move(move |_, _, cx| {
            if let Some(app) = weak_move.upgrade() {
                let path = path_move.clone();
                let _ = app.update(cx, |app, cx| {
                    if app.rubber_band.is_some() && crate::drag_out::left_button_down() {
                        if app.active_mut().selected.insert(path) {
                            cx.notify();
                        }
                    }
                });
            }
        })
        .on_mouse_up(MouseButton::Left, move |_, _, cx| {
            if let Some(app) = weak_up.upgrade() {
                let _ = app.update(cx, |app, cx| {
                    app.pending_drag = None;
                    if app.rubber_band.take().is_some() {
                        cx.notify();
                    }
                });
            }
        })
        .on_click(move |event, window, cx| {
            if card_row_click_count(event) >= 2 {
                window.focus(&focus_click);
                open_entry_from_card(weak_click.clone(), click_entry.clone(), cx);
            }
        })
        .child(highlight)
        .child(
            div()
                .w_full()
                .h(px(104.))
                .flex_none()
                .overflow_hidden()
                .rounded_md()
                .bg(if is_image {
                    theme::canvas()
                } else {
                    color.opacity(0.06)
                })
                .child(image_area),
        )
        .child(
            div()
                .w_full()
                .truncate()
                .text_size(px(11.5))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(theme::text())
                .child(entry.name),
        )
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_1()
                .text_size(px(9.5))
                .text_color(theme::text_faint())
                .child(div().truncate().child(match &search_root {
                    Some(root) => search_location(root, &entry.path),
                    None => entry.type_label.clone(),
                }))
                .child(div().flex_none().child(date)),
        );

    base.into_any_element()
}

impl PilotApp {
    fn render_overlays(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut overlays = Vec::new();
        if self.context_menu.is_some() {
            overlays.push(self.render_context_menu(window, cx));
        }
        let editor_dialog = self.editor.as_ref().is_some_and(|session| {
            matches!(
                session.target,
                EditTarget::Rename(_) | EditTarget::NewFolder | EditTarget::NewFile
            )
        });
        if editor_dialog {
            overlays.push(self.render_editor_dialog(window, cx));
        }
        if self.modal.is_some() {
            overlays.push(self.render_modal(window, cx));
        }
        if let Some((name, generation)) = self.open_flash.clone() {
            let pill = div()
                .min_h(px(38.))
                .px_3()
                .flex()
                .items_center()
                .gap_2()
                .rounded_lg()
                .bg(theme::surface())
                .border_1()
                .border_color(theme::border())
                .shadow_lg()
                .text_size(px(11.5))
                .text_color(theme::text())
                .child(
                    svg()
                        .path("icons/external.svg")
                        .size(px(14.))
                        .text_color(theme::accent()),
                )
                .child(div().max_w(px(420.)).truncate().child(name))
                .child(
                    div()
                        .text_size(px(10.))
                        .text_color(theme::text_faint())
                        .child("正在打开…"),
                );
            overlays.push(
                div()
                    .absolute()
                    .left(px(0.))
                    .right(px(0.))
                    .bottom(px(84.))
                    .flex()
                    .justify_center()
                    .child(
                        pill.with_animation(
                            SharedString::from(format!("open-flash-{generation}")),
                            Animation::new(Duration::from_millis(160))
                                .with_easing(ease_out_quint()),
                            |element, delta| {
                                element
                                    .opacity(delta)
                                    .mt(px((1.0 - delta) * 10.0))
                            },
                        ),
                    )
                    .into_any_element(),
            );
        }
        if let Some(toast) = &self.toast {
            let (bg, icon) = match toast.kind {
                ToastKind::Info => (theme::text(), "icons/check.svg"),
                ToastKind::Success => (theme::success(), "icons/check.svg"),
                ToastKind::Error => (theme::danger(), "icons/close.svg"),
            };
            overlays.push(
                div()
                    .absolute()
                    .left(px(0.))
                    .right(px(0.))
                    .bottom(px(42.))
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .max_w(px(640.))
                            .min_h(px(34.))
                            .px_3()
                            .py_1p5()
                            .rounded_lg()
                            .bg(bg)
                            .shadow_lg()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_size(px(11.5))
                            .text_color(theme::white())
                            .child(
                                svg()
                                    .path(icon)
                                    .size(px(13.))
                                    .text_color(theme::white()),
                            )
                            .child(div().truncate().child(toast.message.clone())),
                    )
                    .into_any_element(),
            );
        }
        overlays
    }

    fn render_editor_dialog(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(session) = &self.editor else {
            return div().into_any_element();
        };
        let editor = session.editor.clone();
        let target = session.target.clone();
        let (title, subtitle, icon) = match &target {
            EditTarget::Rename(path) => (
                "重命名".to_string(),
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                "icons/edit.svg",
            ),
            EditTarget::NewFolder => (
                "新建文件夹".to_string(),
                "在当前目录中创建文件夹".to_string(),
                "icons/folder.svg",
            ),
            EditTarget::NewFile => (
                "新建文件".to_string(),
                "在当前目录中创建空白文件".to_string(),
                "icons/file.svg",
            ),
            _ => return div().into_any_element(),
        };
        let panel = div()
            .w(px(420.))
            .rounded_lg()
            .bg(theme::surface())
            .border_1()
            .border_color(theme::border())
            .shadow_lg()
            .p(px(18.))
            .flex()
            .flex_col()
            .gap_3()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .size(px(34.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .bg(theme::accent_soft())
                            .child(svg().path(icon).size(px(17.)).text_color(theme::accent())),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(title),
                            )
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(theme::text_faint())
                                    .child(subtitle),
                            ),
                    ),
            )
            .child(
                div()
                    .h(px(34.))
                    .w_full()
                    .px_2()
                    .flex()
                    .items_center()
                    .rounded_md()
                    .bg(theme::chrome())
                    .border_1()
                    .border_color(theme::accent())
                    .child(editor),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(theme::text_faint())
                            .child("Enter 确认 · Esc 取消"),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                div()
                                    .id("dialog-cancel")
                                    .h(px(28.))
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .text_size(px(11.5))
                                    .text_color(theme::text_muted())
                                    .hover(|this| this.bg(theme::hover()).text_color(theme::text()))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.close_editor();
                                        window.focus(&this.focus_handle);
                                        cx.notify();
                                    }))
                                    .child("取消"),
                            )
                            .child(
                                div()
                                    .id("dialog-confirm")
                                    .h(px(28.))
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .rounded_md()
                                    .bg(theme::accent())
                                    .text_color(theme::white())
                                    .cursor_pointer()
                                    .text_size(px(11.5))
                                    .hover(|this| this.bg(theme::accent_hover()))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.submit_current_editor(window, cx);
                                    }))
                                    .child("确定"),
                            ),
                    ),
            );

        div()
            .absolute()
            .left(px(0.))
            .top(px(0.))
            .size_full()
            .bg(theme::black().opacity(0.24))
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                this.close_editor();
                window.focus(&this.focus_handle);
                cx.notify();
            }))
            .child(panel)
            .into_any_element()
    }

    fn render_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match &self.modal {
            Some(Modal::ConfirmPermanentDelete { paths }) => {
                let paths = paths.clone();
                let count = paths.len();
                let panel = div()
                    .w(px(420.))
                    .rounded_lg()
                    .bg(theme::surface())
                    .border_1()
                    .border_color(theme::border())
                    .shadow_lg()
                    .p(px(18.))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .size(px(34.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_md()
                                    .bg(theme::danger_soft())
                                    .child(
                                        svg()
                                            .path("icons/trash.svg")
                                            .size(px(17.))
                                            .text_color(theme::danger()),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(format!("永久删除 {count} 项？")),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .line_height(px(18.))
                            .text_color(theme::text_muted())
                            .child("此操作不会经过回收站，且无法撤销。"),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                div()
                                    .id("delete-cancel")
                                    .h(px(28.))
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .text_size(px(11.5))
                                    .text_color(theme::text_muted())
                                    .hover(|this| this.bg(theme::hover()).text_color(theme::text()))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.modal = None;
                                        window.focus(&this.focus_handle);
                                        cx.notify();
                                    }))
                                    .child("取消"),
                            )
                            .child(
                                div()
                                    .id("delete-confirm")
                                    .h(px(28.))
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .rounded_md()
                                    .bg(theme::danger())
                                    .text_color(theme::white())
                                    .cursor_pointer()
                                    .text_size(px(11.5))
                                    .hover(|this| this.bg(theme::danger().opacity(0.85)))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.modal = None;
                                        window.focus(&this.focus_handle);
                                        this.permanent_delete(paths.clone(), cx);
                                    }))
                                    .child("永久删除"),
                            ),
                    );
                self.modal_backdrop(panel, cx)
            }
            Some(Modal::Error { title, message }) => {
                let title = title.clone();
                let message = message.clone();
                let panel = div()
                    .w(px(460.))
                    .rounded_lg()
                    .bg(theme::surface())
                    .border_1()
                    .border_color(theme::border())
                    .shadow_lg()
                    .p(px(18.))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .size(px(34.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_md()
                                    .bg(theme::danger_soft())
                                    .child(
                                        svg()
                                            .path("icons/close.svg")
                                            .size(px(17.))
                                            .text_color(theme::danger()),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(title),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .line_height(px(18.))
                            .text_color(theme::text_muted())
                            .child(message),
                    )
                    .child(
                        div().flex().justify_end().child(
                            div()
                                .id("error-ok")
                                .h(px(28.))
                                .px_4()
                                .flex()
                                .items_center()
                                .rounded_md()
                                .bg(theme::accent())
                                .text_color(theme::white())
                                .cursor_pointer()
                                .text_size(px(11.5))
                                .hover(|this| this.bg(theme::accent_hover()))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.modal = None;
                                    window.focus(&this.focus_handle);
                                    cx.notify();
                                }))
                                .child("知道了"),
                        ),
                    );
                self.modal_backdrop(panel, cx)
            }
            Some(Modal::Settings) => self.render_settings_dialog(window, cx),
            None => div().into_any_element(),
        }
    }

    fn modal_backdrop(&self, panel: impl IntoElement, cx: &mut Context<Self>) -> AnyElement {
        div()
            .absolute()
            .left(px(0.))
            .top(px(0.))
            .size_full()
            .bg(theme::black().opacity(0.24))
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                this.modal = None;
                window.focus(&this.focus_handle);
                cx.notify();
            }))
            .child(panel)
            .into_any_element()
    }

    fn render_settings_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let _ = window;
        let page = self.settings_page;
        let fonts = self.font_options.clone();
        let current = self.font_family.clone();
        let show_hidden = self.show_hidden;
        let replace_explorer = self.replace_explorer;
        let quick_places = self.quick_places.clone();
        let prefs = self.shell_menu.clone();
        let registry_items = self.registry_items.clone();
        let registry_map = self.registry_menu.clone();
        let shellex_items = self.shellex_items.clone();
        let shellex_map = self.shellex_menu.clone();

        let mut tab_items: Vec<AnyElement> = Vec::new();
        for (tab_page, label) in [
            (SettingsPage::General, "常规"),
            (SettingsPage::ContextMenu, "上下文菜单"),
            (SettingsPage::QuickAccess, "快速访问"),
        ] {
            let active = page == tab_page;
            tab_items.push(
                div()
                    .id(("settings-tab", tab_page as usize))
                    .h(px(26.))
                    .px_3()
                    .flex()
                    .items_center()
                    .rounded_md()
                    .cursor_pointer()
                    .text_size(px(11.))
                    .when(active, |this| {
                        this.bg(theme::surface())
                            .shadow_sm()
                            .text_color(theme::accent())
                    })
                    .when(!active, |this| {
                        this.text_color(theme::text_muted())
                            .hover(|this| this.bg(theme::hover()).text_color(theme::text()))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings_page = tab_page;
                        cx.notify();
                    }))
                    .child(label)
                    .into_any_element(),
            );
        }
        let tab_bar = div()
            .flex()
            .gap_1()
            .p_1()
            .rounded_md()
            .bg(theme::chrome())
            .children(tab_items);

        let page_body: AnyElement = if page == SettingsPage::General {
            let general_body;
            let mut font_rows: Vec<AnyElement> = Vec::new();
            for (index, family) in fonts.iter().enumerate() {
                let is_current = family.eq_ignore_ascii_case(&current);
                let family_for_click = family.clone();
                font_rows.push(
                    div()
                        .id(("font-option", index))
                        .h(px(28.))
                        .px_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded_md()
                        .cursor_pointer()
                        .when(is_current, |this| this.bg(theme::accent_soft()))
                        .when(!is_current, |this| {
                            this.hover(|this| this.bg(theme::hover()))
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.choose_font(family_for_click.clone(), cx);
                        }))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .truncate()
                                .text_size(px(11.))
                                .text_color(if is_current {
                                    theme::accent()
                                } else {
                                    theme::text_muted()
                                })
                                .child(family.clone()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(10.))
                                .text_color(theme::text_faint())
                                .child("中文 Aa"),
                        )
                        .when(is_current, |this| {
                            this.child(
                                svg()
                                    .path("icons/check.svg")
                                    .size(px(12.))
                                    .text_color(theme::accent()),
                            )
                        })
                        .into_any_element(),
                );
            }

            general_body = div()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_size(px(10.5))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(theme::text_faint())
                                .child("界面字体（原生系统字体）"),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child("中文排版示例：下载 · 文档 · 图片 · 重命名"),
                        )
                        .child(
                            div()
                                .id("font-scroll")
                                .mt_1()
                                .max_h(px(170.))
                                .overflow_y_scroll()
                                .rounded_md()
                                .bg(theme::chrome())
                                .p_1()
                                .children(font_rows),
                        ),
                )
                .child(div().h(px(1.)).bg(theme::border_soft()))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .text_size(px(11.5))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .child("显示隐藏文件"),
                                )
                                .child(
                                    div()
                                        .text_size(px(10.))
                                        .text_color(theme::text_faint())
                                        .child("快捷键 Ctrl+Shift+H"),
                                ),
                        )
                        .child(
                            div()
                                .id("settings-hidden-toggle")
                                .w(px(36.))
                                .h(px(20.))
                                .p(px(2.))
                                .rounded_full()
                                .cursor_pointer()
                                .when(show_hidden, |this| this.bg(theme::accent()))
                                .when(!show_hidden, |this| this.bg(theme::border()))
                                .on_click(cx.listener(|this, _, _, cx| this.toggle_hidden(cx)))
                                .child(
                                    div()
                                        .size(px(16.))
                                        .rounded_full()
                                        .bg(theme::white())
                                        .shadow_sm()
                                        .when(show_hidden, |this| this.ml(px(16.))),
                                ),
                        ),
                )
                .child(div().h(px(1.)).bg(theme::border_soft()))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .text_size(px(11.5))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .child("替换系统资源管理器"),
                                )
                                .child(
                                    div()
                                        .text_size(px(10.))
                                        .text_color(theme::text_faint())
                                        .child("接管文件夹双击、Win+E 和开始菜单“文件管理器”；仅当前用户，可随时关闭恢复"),
                                ),
                        )
                        .child(
                            div()
                                .id("settings-replace-explorer")
                                .w(px(36.))
                                .h(px(20.))
                                .p(px(2.))
                                .rounded_full()
                                .cursor_pointer()
                                .when(replace_explorer, |this| this.bg(theme::accent()))
                                .when(!replace_explorer, |this| this.bg(theme::border()))
                                .on_click(cx.listener(|this, _, _, cx| this.toggle_replace_explorer(cx)))
                                .child(
                                    div()
                                        .size(px(16.))
                                        .rounded_full()
                                        .bg(theme::white())
                                        .shadow_sm()
                                        .when(replace_explorer, |this| this.ml(px(16.))),
                                ),
                        ),
                )
                .child(div().h(px(1.)).bg(theme::border_soft()))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_0p5()
                                .min_w(px(0.))
                                .child(
                                    div()
                                        .text_size(px(11.5))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .child("配置文件"),
                                )
                                .child(
                                    div()
                                        .text_size(px(10.))
                                        .text_color(theme::text_faint())
                                        .truncate()
                                        .child(
                                            config_file()
                                                .map(|path| path.display().to_string())
                                                .unwrap_or_else(|| "无法定位配置目录".to_string()),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .id("settings-open-config")
                                .flex_none()
                                .h(px(24.))
                                .px_2()
                                .flex()
                                .items_center()
                                .rounded_md()
                                .border_1()
                                .border_color(theme::border())
                                .cursor_pointer()
                                .text_size(px(10.))
                                .text_color(theme::text_muted())
                                .hover(|this| this.bg(theme::hover()).text_color(theme::text()))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(dir) = config_dir() {
                                        let _ = std::process::Command::new("explorer").arg(&dir).spawn();
                                    } else {
                                        this.show_toast("无法定位配置目录", ToastKind::Error, cx);
                                    }
                                }))
                                .child("打开所在文件夹"),
                        ),
                )
                .child(div().h(px(1.)).bg(theme::border_soft()))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(1.))
                        .p_2()
                        .rounded_md()
                        .bg(theme::chrome())
                        .text_size(px(10.5))
                        .text_color(theme::text_muted())
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(theme::text_faint())
                                .child("拖放行为"),
                        )
                        .child("从 FPilot 拖到资源管理器 / 桌面 / 其他程序：默认移动。")
                        .child("拖动开始时按住 Ctrl：改为复制。")
                        .child("按住 Shift 拖动：强制框选，不拖出文件。"),
                );
            div()
                .id("general-settings-page")
                .max_h(px(520.))
                .overflow_y_scroll()
                .child(general_body)
                .into_any_element()
        } else if page == SettingsPage::ContextMenu {
            let mut builtin_rows: Vec<AnyElement> = Vec::new();
            for (index, item) in ShellMenuItem::ALL.iter().copied().enumerate() {
                let enabled = item.enabled(&prefs);
                builtin_rows.push(
                    div()
                        .id(("builtin-menu-row", index))
                        .min_h(px(42.))
                        .px_2()
                        .py_1()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .rounded_md()
                        .cursor_pointer()
                        .hover(|this| this.bg(theme::hover()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.toggle_shell_menu_item(item, cx);
                        }))
                        .child(
                            div()
                                .flex()
                                .flex_1()
                                .flex_col()
                                .gap_0p5()
                                .min_w(px(0.))
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .child(item.title()),
                                )
                                .child(
                                    div()
                                        .text_size(px(9.2))
                                        .text_color(theme::text_faint())
                                        .truncate()
                                        .child(item.description()),
                                ),
                        )
                        .child(settings_toggle(enabled))
                        .into_any_element(),
                );
            }

            let mut registry_rows: Vec<AnyElement> = Vec::new();
            let mut current_scope: Option<shell_registry::Scope> = None;
            for (index, item) in registry_items.iter().enumerate() {
                if current_scope != Some(item.scope) {
                    current_scope = Some(item.scope);
                    registry_rows.push(
                        div()
                            .pt_1()
                            .text_size(px(9.5))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme::text_faint())
                            .child(item.scope.label())
                            .into_any_element(),
                    );
                }
                let enabled = registry_map.contains_key(&item.id);
                let id_for_click = item.id.clone();
                registry_rows.push(
                    div()
                        .id(("registry-menu-row", index))
                        .min_h(px(42.))
                        .px_2()
                        .py_1()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .rounded_md()
                        .cursor_pointer()
                        .hover(|this| this.bg(theme::hover()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.toggle_registry_menu(id_for_click.clone(), cx);
                        }))
                        .child(
                            div()
                                .flex()
                                .flex_1()
                                .flex_col()
                                .gap_0p5()
                                .min_w(px(0.))
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .truncate()
                                        .child(item.title.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(px(9.2))
                                        .text_color(theme::text_faint())
                                        .truncate()
                                        .child(format!(
                                            "{} · {}",
                                            item.verb,
                                            if item.from_hkcu { "HKCU" } else { "HKLM" }
                                        )),
                                ),
                        )
                        .child(settings_toggle(enabled))
                        .into_any_element(),
                );
            }

            let mut shellex_rows: Vec<AnyElement> = Vec::new();
            for (index, handler) in shellex_items.iter().enumerate() {
                let enabled = shellex_map.contains_key(&handler.id);
                let id_for_click = handler.id.clone();
                let scopes = handler
                    .scopes
                    .iter()
                    .map(|scope| scope.label())
                    .collect::<Vec<_>>()
                    .join(" / ");
                let program = handler
                    .program
                    .as_ref()
                    .and_then(|program| program.file_name())
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "未找到程序".to_string());
                shellex_rows.push(
                    div()
                        .id(("shellex-menu-row", index))
                        .min_h(px(42.))
                        .px_2()
                        .py_1()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .rounded_md()
                        .cursor_pointer()
                        .hover(|this| this.bg(theme::hover()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.toggle_shellex(id_for_click.clone(), cx);
                        }))
                        .child(
                            div()
                                .flex()
                                .flex_1()
                                .flex_col()
                                .gap_0p5()
                                .min_w(px(0.))
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .truncate()
                                        .child(handler.name.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(px(9.2))
                                        .text_color(theme::text_faint())
                                        .truncate()
                                        .child(format!(
                                            "{} · {} · {}",
                                            program,
                                            scopes,
                                            if handler.from_hkcu { "HKCU" } else { "HKLM" }
                                        )),
                                ),
                        )
                        .child(settings_toggle(enabled))
                        .into_any_element(),
                );
            }

            div()
                .id("context-settings-page")
                .max_h(px(500.))
                .overflow_y_scroll()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap_0p5()
                                        .child(
                                            div()
                                                .text_size(px(12.))
                                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                                .child("上下文菜单"),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(10.))
                                                .text_color(theme::text_muted())
                                                .child("内置动作、注册表动词、Shell 扩展统一管理；切换后立即保存。"),
                                        ),
                                )
                                .child(
                                    div()
                                        .id("context-settings-refresh")
                                        .flex_none()
                                        .h(px(24.))
                                        .px_2()
                                        .flex()
                                        .items_center()
                                        .rounded_md()
                                        .border_1()
                                        .border_color(theme::border())
                                        .cursor_pointer()
                                        .text_size(px(10.))
                                        .text_color(theme::text_muted())
                                        .hover(|this| this.bg(theme::hover()).text_color(theme::text()))
                                        .on_click(cx.listener(|this, _, _, cx| this.refresh_registry_menu(cx)))
                                        .child("刷新列表"),
                                ),
                        )
                        .child(div().h(px(1.)).bg(theme::border_soft()))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .child(format!("内置系统动作（{}）", ShellMenuItem::ALL.len())),
                                )
                                .child(div().flex().flex_col().gap_0p5().children(builtin_rows))
                                .child(
                                    div()
                                        .flex()
                                        .justify_end()
                                        .child(
                                            div()
                                                .id("reset-shell-menu")
                                                .h(px(22.))
                                                .px_2()
                                                .flex()
                                                .items_center()
                                                .rounded_md()
                                                .border_1()
                                                .border_color(theme::border())
                                                .cursor_pointer()
                                                .text_size(px(9.5))
                                                .text_color(theme::text_muted())
                                                .hover(|this| this.bg(theme::hover()).text_color(theme::text()))
                                                .on_click(cx.listener(|this, _, _, cx| this.reset_shell_menu(cx)))
                                                .child("恢复默认"),
                                        ),
                                ),
                        )
                        .child(div().h(px(1.)).bg(theme::border_soft()))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .child(format!("注册表动词（{}）", registry_items.len())),
                                )
                                .child(
                                    div()
                                        .text_size(px(9.5))
                                        .text_color(theme::text_faint())
                                        .child("HKCU/HKLM 的 *\\\\shell、Directory\\\\shell、Directory\\\\Background\\\\shell。"),
                                )
                                .child(div().flex().flex_col().gap_0p5().children(registry_rows)),
                        )
                        .child(div().h(px(1.)).bg(theme::border_soft()))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .child(format!("Shell 扩展（COM）（{}）", shellex_items.len())),
                                )
                                .child(
                                    div()
                                        .text_size(px(9.5))
                                        .text_color(theme::text_faint())
                                        .child("右键菜单中显示为二级菜单，悬停展开；点击具体子项才执行。"),
                                )
                                .child(div().flex().flex_col().gap_0p5().children(shellex_rows)),
                        )
                        .child(div().h(px(1.)).bg(theme::border_soft()))
                        .child(
                            div()
                                .text_size(px(9.5))
                                .text_color(theme::text_faint())
                                .child("只读取注册表，不修改系统设置。"),
                        ),
                )
                .into_any_element()
        } else {
            let defaults = fs::quick_places();
            let mut grid: Vec<AnyElement> = Vec::new();
            let mut cursor = 0;
            while cursor < defaults.len() {
                let mut row = div().flex().items_center().gap(px(6.));
                for offset in 0..2 {
                    match defaults.get(cursor + offset) {
                        Some(place) => {
                            let enabled = quick_places
                                .iter()
                                .any(|current| fs::path_eq(&current.path, &place.path));
                            let path = place.path.clone();
                            row = row.child(
                                div()
                                    .id(SharedString::from(format!(
                                        "settings-quick-tab-{cursor}-{offset}"
                                    )))
                                    .flex_1()
                                    .min_w(px(0.))
                                    .h(px(26.))
                                    .px_1()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_2()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .hover(|this| this.bg(theme::hover()))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.toggle_default_quick_place(path.clone(), cx);
                                    }))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.))
                                            .truncate()
                                            .text_size(px(11.))
                                            .child(place.label.clone()),
                                    )
                                    .child(settings_toggle(enabled)),
                            );
                        }
                        None => {
                            row = row.child(div().flex_1());
                        }
                    }
                }
                grid.push(row.into_any_element());
                cursor += 2;
            }

            div()
                .id("quick-access-settings-page")
                .max_h(px(500.))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_0p5()
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .child("快速访问"),
                                )
                                .child(
                                    div()
                                        .text_size(px(10.))
                                        .text_color(theme::text_muted())
                                        .child("只管理默认项；侧栏里把条目往外拖可移除，拖入文件夹可新增。"),
                                ),
                        )
                        .child(
                            div()
                                .id("settings-quick-reset")
                                .flex_none()
                                .h(px(24.))
                                .px_2()
                                .flex()
                                .items_center()
                                .rounded_md()
                                .border_1()
                                .border_color(theme::border())
                                .cursor_pointer()
                                .text_size(px(10.))
                                .text_color(theme::text_muted())
                                .hover(|this| this.bg(theme::hover()).text_color(theme::text()))
                                .on_click(cx.listener(|this, _, _, cx| this.reset_quick_places(cx)))
                                .child("恢复默认"),
                        ),
                )
                .child(div().h(px(1.)).bg(theme::border_soft()))
                .child(div().flex().flex_col().gap_0p5().children(grid))
                .into_any_element()
        };

        let panel = div()
            .w(px(590.))
            .max_h(px(720.))
            .rounded_lg()
            .bg(theme::surface())
            .border_1()
            .border_color(theme::border())
            .shadow_lg()
            .p(px(18.))
            .flex()
            .flex_col()
            .gap_3()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .size(px(34.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_md()
                                    .bg(theme::chrome())
                                    .child(
                                        svg()
                                            .path("icons/settings.svg")
                                            .size(px(17.))
                                            .text_color(theme::text_muted()),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .text_size(px(14.))
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .child("设置"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(10.5))
                                            .text_color(theme::text_faint())
                                            .child("FPilot · GPUI 原生"),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .id("settings-close-x")
                            .size(px(26.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .cursor_pointer()
                            .text_color(theme::text_faint())
                            .hover(|this| this.bg(theme::hover()).text_color(theme::text()))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.modal = None;
                                window.focus(&this.focus_handle);
                                cx.notify();
                            }))
                            .child(
                                svg()
                                    .path("icons/close.svg")
                                    .size(px(13.))
                                    .text_color(theme::text_faint()),
                            ),
                    ),
            )
            .child(div().h(px(1.)).bg(theme::border_soft()))
            .child(tab_bar)
            .child(page_body)
            .child(
                div().flex().justify_end().child(
                    div()
                        .id("settings-done")
                        .h(px(28.))
                        .px_4()
                        .flex()
                        .items_center()
                        .rounded_md()
                        .bg(theme::accent())
                        .text_color(theme::white())
                        .cursor_pointer()
                        .text_size(px(11.5))
                        .hover(|this| this.bg(theme::accent_hover()))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.modal = None;
                            window.focus(&this.focus_handle);
                            cx.notify();
                        }))
                        .child("完成"),
                ),
            );

        self.modal_backdrop(panel, cx)
    }

    fn render_context_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(menu) = &self.context_menu else {
            return div().into_any_element();
        };
        let pos = menu.position;
        let target = menu.target.clone();
        let is_dir = menu.is_dir;
        let selected_count = self.active().selected.len();
        let target_entry = target
            .as_ref()
            .and_then(|target| self.active().entries.iter().find(|entry| &entry.path == target))
            .cloned();
        let prefs = self.shell_menu.clone();
        let viewport = window.viewport_size();
        let viewport_w = f32::from(viewport.width);
        let viewport_h = f32::from(viewport.height);

        let mut items: Vec<AnyElement> = Vec::new();
        if !self.undo_stack.is_empty() {
            items.push(self.menu_item(
                MenuCommand::UndoAction,
                "icons/refresh.svg",
                "撤销",
                "Ctrl+Z",
                false,
                cx,
            ));
            items.push(menu_separator());
        }
        if target.is_some() {
            items.push(self.menu_item(
                MenuCommand::Open,
                "icons/external.svg",
                "打开",
                "Enter",
                false,
                cx,
            ));
            if is_dir {
                items.push(self.menu_item(
                    MenuCommand::OpenInNewTab,
                    "icons/folder.svg",
                    "在新标签页中打开",
                    "Ctrl+Enter",
                    false,
                    cx,
                ));
            }
            items.push(menu_separator());
            items.push(self.menu_item(
                MenuCommand::Copy,
                "icons/copy.svg",
                if selected_count > 1 {
                    "复制所选项目"
                } else {
                    "复制"
                },
                "Ctrl+C",
                false,
                cx,
            ));
            items.push(self.menu_item(
                MenuCommand::Cut,
                "icons/cut.svg",
                if selected_count > 1 {
                    "剪切所选项目"
                } else {
                    "剪切"
                },
                "Ctrl+X",
                false,
                cx,
            ));
            if is_dir {
                items.push(self.menu_item(
                    MenuCommand::Paste,
                    "icons/paste.svg",
                    "粘贴到此文件夹",
                    "Ctrl+V",
                    false,
                    cx,
                ));
            }
            items.push(menu_separator());
            if selected_count == 1 {
                items.push(self.menu_item(
                    MenuCommand::Rename,
                    "icons/edit.svg",
                    "重命名",
                    "F2",
                    false,
                    cx,
                ));
            }
            items.push(self.menu_item(
                MenuCommand::Trash,
                "icons/trash.svg",
                "移到回收站",
                "Delete",
                true,
                cx,
            ));
            items.push(menu_separator());
            items.push(self.menu_item(
                MenuCommand::CopyPath,
                "icons/file.svg",
                "复制路径",
                "Ctrl+Shift+C",
                false,
                cx,
            ));
            items.push(self.menu_item(
                MenuCommand::Reveal,
                "icons/external.svg",
                "在资源管理器中显示",
                "Ctrl+Shift+E",
                false,
                cx,
            ));
        } else {
            items.push(self.menu_item(
                MenuCommand::NewFolder,
                "icons/plus.svg",
                "新建文件夹",
                "Ctrl+Shift+N",
                false,
                cx,
            ));
            items.push(self.menu_item(
                MenuCommand::NewFile,
                "icons/file.svg",
                "新建文件",
                "Ctrl+Alt+N",
                false,
                cx,
            ));
            items.push(menu_separator());
            items.push(self.menu_item(
                MenuCommand::Paste,
                "icons/paste.svg",
                "粘贴",
                "Ctrl+V",
                false,
                cx,
            ));
            items.push(self.menu_item(
                MenuCommand::SelectAll,
                "icons/check.svg",
                "全选",
                "Ctrl+A",
                false,
                cx,
            ));
            items.push(menu_separator());
            items.push(self.menu_item(
                MenuCommand::Refresh,
                "icons/refresh.svg",
                "刷新",
                "F5",
                false,
                cx,
            ));
        }

        let mut system_items: Vec<AnyElement> = Vec::new();
        if selected_count <= 1 {
            match &target_entry {
                Some(entry) if !entry.is_dir => {
                    if prefs.open_with {
                        system_items.push(self.menu_item(
                            MenuCommand::SystemOpenWith,
                            "icons/external.svg",
                            "打开方式…",
                            "",
                            false,
                            cx,
                        ));
                    }
                    if prefs.print {
                        system_items.push(self.menu_item(
                            MenuCommand::SystemPrint,
                            "icons/print.svg",
                            "打印",
                            "",
                            false,
                            cx,
                        ));
                    }
                    if prefs.run_as_admin && ops::is_executable(&entry.path) {
                        system_items.push(self.menu_item(
                            MenuCommand::SystemRunAsAdmin,
                            "icons/shield.svg",
                            "以管理员身份运行",
                            "",
                            false,
                            cx,
                        ));
                    }
                    if prefs.properties {
                        system_items.push(self.menu_item(
                            MenuCommand::SystemProperties,
                            "icons/info.svg",
                            "属性",
                            "",
                            false,
                            cx,
                        ));
                    }
                }
                Some(_) | None => {
                    if prefs.terminal {
                        system_items.push(self.menu_item(
                            MenuCommand::SystemTerminal,
                            "icons/terminal.svg",
                            "在 Windows 终端中打开",
                            "",
                            false,
                            cx,
                        ));
                    }
                    if prefs.powershell {
                        system_items.push(self.menu_item(
                            MenuCommand::SystemPowerShell,
                            "icons/terminal.svg",
                            "在此处打开 PowerShell",
                            "",
                            false,
                            cx,
                        ));
                    }
                    if prefs.properties {
                        system_items.push(self.menu_item(
                            MenuCommand::SystemProperties,
                            "icons/info.svg",
                            "属性",
                            "",
                            false,
                            cx,
                        ));
                    }
                }
            }
        }
        if !system_items.is_empty() {
            items.push(menu_separator());
            items.extend(system_items);
        }

        if selected_count <= 1 {
            let scope = match &target_entry {
                Some(entry) if entry.is_dir => shell_registry::Scope::Directory,
                Some(_) => shell_registry::Scope::File,
                None => shell_registry::Scope::Background,
            };
            let enabled_items: Vec<(usize, String)> = self
                .registry_items
                .iter()
                .enumerate()
                .filter(|(_, item)| {
                    item.scope == scope && self.registry_menu.contains_key(&item.id)
                })
                .map(|(index, item)| (index, item.title.clone()))
                .collect();
            if !enabled_items.is_empty() {
                items.push(menu_separator());
                for (index, title) in enabled_items {
                    items.push(self.menu_item_owned(
                        MenuCommand::Registry(index),
                        "icons/external.svg",
                        title,
                        "",
                        false,
                        cx,
                    ));
                }
            }
        }

        if !self.shell_handler_menus.is_empty() {
            items.push(menu_separator());
            for (menu_index, state) in self.shell_handler_menus.iter().enumerate() {
                items.push(self.submenu_parent_item(menu_index, state.name.clone(), cx));
            }
        }

        let menu_width = 238.0_f32;
        let raw_items = items.len() as f32;
        let menu_height = 8.0_f32 + raw_items * 28.0;
        let left = f32::from(pos.x).clamp(8.0, (viewport_w - menu_width - 8.0).max(8.0));
        let top = f32::from(pos.y).clamp(8.0, (viewport_h - menu_height - 8.0).max(8.0));

        let mut root = div()
            .absolute()
            .left(px(0.))
            .top(px(0.))
            .size_full()
            .child(
                div()
                    .absolute()
                    .left(px(0.))
                    .top(px(0.))
                    .size_full()
                    .bg(theme::black().opacity(0.001))
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                        this.context_menu = None;
                        this.shell_handler_menus.clear();
                        this.context_submenu = None;
                        this.context_submenu_position = None;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id("context-menu-panel")
                    .absolute()
                    .left(px(left))
                    .top(px(top))
                    .w(px(menu_width))
                    .max_h(px((viewport_h - 16.0).max(120.0)))
                    .overflow_y_scroll()
                    .rounded_lg()
                    .bg(theme::surface())
                    .border_1()
                    .border_color(theme::border())
                    .shadow_lg()
                    .p_1()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .children(items),
            );

        if let Some(menu_index) = self.context_submenu {
            if let Some(state) = self.shell_handler_menus.get(menu_index) {
                let name = state.name.clone();
                let prefix = format!("{name} · ");
                let commands: Vec<(u32, String)> = state
                    .items
                    .iter()
                    .map(|item| {
                        let label = item
                            .label
                            .strip_prefix(&prefix)
                            .unwrap_or(&item.label)
                            .to_string();
                        (item.id, label)
                    })
                    .collect();
                let anchor = self.context_submenu_position.unwrap_or(pos);
                let sub_width = 270.0_f32;
                let sub_height = 8.0 + 26.0 + commands.len() as f32 * 28.0;
                let sub_left = (left + menu_width - 2.0)
                    .clamp(8.0, (viewport_w - sub_width - 8.0).max(8.0));
                let sub_top = (f32::from(anchor.y) - 14.0)
                    .clamp(8.0, (viewport_h - sub_height - 8.0).max(8.0));
                let mut sub_items: Vec<AnyElement> = Vec::new();
                for (id, label) in commands {
                    sub_items.push(self.menu_item_owned(
                        MenuCommand::ShellHandlerCommand { menu: menu_index, id },
                        "icons/external.svg",
                        label,
                        "",
                        false,
                        cx,
                    ));
                }
                root = root.child(
                    div()
                        .id("context-submenu-panel")
                        .absolute()
                        .left(px(sub_left))
                        .top(px(sub_top))
                        .w(px(sub_width))
                        .max_h(px((viewport_h - 16.0).max(120.0)))
                        .overflow_y_scroll()
                        .rounded_lg()
                        .bg(theme::surface())
                        .border_1()
                        .border_color(theme::border())
                        .shadow_lg()
                        .p_1()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .child(
                            div()
                                .h(px(24.))
                                .px_2()
                                .flex()
                                .items_center()
                                .text_size(px(10.))
                                .text_color(theme::text_faint())
                                .child(name),
                        )
                        .children(sub_items),
                );
            }
        }

        root.into_any_element()
    }

    fn submenu_parent_item(
        &self,
        index: usize,
        name: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let element_id = SharedString::from(format!("submenu-parent-{index}-{name}"));
        div()
            .id(element_id)
            .h(px(28.))
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .rounded_md()
            .cursor_pointer()
            .text_size(px(11.5))
            .text_color(theme::text_muted())
            .hover(|this| this.bg(theme::hover()).text_color(theme::text()))
            .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                if *hovered {
                    this.context_submenu = Some(index);
                    this.context_submenu_position = Some(window.mouse_position());
                    cx.notify();
                }
            }))
            .child(
                svg()
                    .path("icons/external.svg")
                    .size(px(13.))
                    .text_color(theme::text_faint()),
            )
            .child(div().flex_1().child(name))
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(theme::text_faint())
                    .child("›"),
            )
            .into_any_element()
    }

    fn menu_item(
        &self,
        command: MenuCommand,
        icon: &'static str,
        label: &'static str,
        shortcut: &'static str,
        danger: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.menu_item_owned(command, icon, label.to_string(), shortcut, danger, cx)
    }

    fn menu_item_owned(
        &self,
        command: MenuCommand,
        icon: &'static str,
        label: String,
        shortcut: &'static str,
        danger: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let element_id = SharedString::from(format!("menu-item-{command:?}-{label}"));
        div()
            .id(element_id)
            .h(px(28.))
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .rounded_md()
            .cursor_pointer()
            .text_size(px(11.5))
            .text_color(if danger {
                theme::danger()
            } else {
                theme::text_muted()
            })
            .hover(move |this| {
                this.bg(if danger {
                    theme::danger_soft()
                } else {
                    theme::hover()
                })
                .text_color(if danger {
                    theme::danger()
                } else {
                    theme::text()
                })
            })
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, window, cx| {
                this.handle_menu_command(command, window, cx);
            }))
            .child(
                svg()
                    .path(icon)
                    .size(px(13.))
                    .text_color(if danger {
                        theme::danger()
                    } else {
                        theme::text_faint()
                    }),
            )
            .child(div().flex_1().child(SharedString::from(label)))
            .child(
                div()
                    .text_size(px(9.5))
                    .text_color(theme::text_faint())
                    .child(shortcut),
            )
            .into_any_element()
    }
}

fn settings_toggle(enabled: bool) -> AnyElement {
    div()
        .w(px(34.))
        .h(px(19.))
        .flex_none()
        .p(px(2.))
        .rounded_full()
        .when(enabled, |this| this.bg(theme::accent()))
        .when(!enabled, |this| this.bg(theme::border()))
        .child(
            div()
                .size(px(15.))
                .rounded_full()
                .bg(theme::white())
                .shadow_sm()
                .when(enabled, |this| this.ml(px(15.))),
        )
        .into_any_element()
}

fn menu_separator() -> AnyElement {
    div()
        .h(px(9.))
        .my_0p5()
        .flex()
        .items_center()
        .child(div().h(px(1.)).w_full().bg(theme::border_soft()))
        .into_any_element()
}

fn request_window_move(window: &Window, _cx: &mut App) {
    #[cfg(target_os = "windows")]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        use windows::Win32::{
            Foundation::{HWND, RECT},
            UI::WindowsAndMessaging::{
                GetWindowRect, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SetWindowPos,
            },
        };

        let Some((start_x, start_y)) = drag_out::cursor_position() else {
            return;
        };
        let Ok(handle) = <Window as HasWindowHandle>::window_handle(window) else {
            return;
        };
        let RawWindowHandle::Win32(raw) = handle.as_raw() else {
            return;
        };
        let hwnd_value = raw.hwnd.get() as isize;
        let hwnd = HWND(hwnd_value as *mut _);
        let mut rect = RECT::default();
        if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
            return;
        }
        log::info!(
            "titlebar drag requested at {}x{}, window at {},{}",
            start_x,
            start_y,
            rect.left,
            rect.top
        );
        std::thread::spawn(move || {
            let hwnd = HWND(hwnd_value as *mut _);
            loop {
                std::thread::sleep(Duration::from_millis(8));
                if !drag_out::left_button_down() {
                    break;
                }
                let Some((x, y)) = drag_out::cursor_position() else {
                    break;
                };
                let target_x = rect.left + (x - start_x);
                let target_y = rect.top + (y - start_y);
                let _ = unsafe {
                    SetWindowPos(
                        hwnd,
                        None,
                        target_x,
                        target_y,
                        0,
                        0,
                        SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                    )
                };
            }
        });
    }
}

fn toggle_window_maximize(window: &Window) {
    #[cfg(target_os = "windows")]
    {
        use std::ffi::c_void;

        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        use windows::Win32::{
            Foundation::HWND,
            UI::WindowsAndMessaging::{IsZoomed, SW_MAXIMIZE, SW_RESTORE, ShowWindowAsync},
        };

        let Ok(handle) = <Window as HasWindowHandle>::window_handle(window) else {
            return;
        };
        let RawWindowHandle::Win32(raw) = handle.as_raw() else {
            return;
        };
        let hwnd = HWND(raw.hwnd.get() as *mut c_void);
        unsafe {
            let command = if IsZoomed(hwnd).as_bool() {
                SW_RESTORE
            } else {
                SW_MAXIMIZE
            };
            let _ = ShowWindowAsync(hwnd, command);
        }
    }
}

#[cfg(test)]
mod settings_tests {
    use super::*;

    #[test]
    fn shell_menu_defaults_and_round_trip() {
        let prefs = ShellMenuPrefs::default();
        assert!(prefs.open_with && prefs.print && prefs.properties);
        let persisted = PersistedSettings {
            font_family: Some("Microsoft YaHei UI".into()),
            show_hidden: true,
            shell_menu: prefs,
            registry_menu: BTreeMap::from([("file:7-zip".to_string(), true)]),
            shellex_menu: BTreeMap::from([("shellex:7-zip".to_string(), true)]),
            replace_explorer: false,
            quick_places: None,
        };
        let json = serde_json::to_string(&persisted).unwrap();
        let loaded: PersistedSettings = serde_json::from_str(&json).unwrap();
        assert!(loaded.show_hidden);
        assert!(loaded.shell_menu.terminal);
        assert_eq!(
            loaded.registry_menu.get("file:7-zip").copied(),
            Some(true)
        );
        assert_eq!(
            loaded.shellex_menu.get("shellex:7-zip").copied(),
            Some(true)
        );
        assert_eq!(loaded.font_family.as_deref(), Some("Microsoft YaHei UI"));
    }

    #[test]
    fn shell_menu_toggle_is_independent() {
        let mut prefs = ShellMenuPrefs::default();
        ShellMenuItem::Print.toggle(&mut prefs);
        assert!(!prefs.print);
        assert!(prefs.open_with);
        assert!(prefs.properties);
    }
}
