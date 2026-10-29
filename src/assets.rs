use std::borrow::Cow;

use anyhow::Result;
use gpui::{AssetSource, SharedString};

/// All UI icons are embedded at build time. `path` is the in-app asset path,
/// e.g. `icons/folder.svg`.
#[derive(Clone, Copy, Default)]
pub struct Assets;

macro_rules! icons {
    ($(($name:literal, $file:literal)),* $(,)?) => {
        const ICONS: &[(&str, &[u8])] = &[
            $(($name, include_bytes!(concat!("../assets/icons/", $file)))),*
        ];
    };
}

icons![
    ("icons/arrow-left.svg", "arrow-left.svg"),
    ("icons/arrow-right.svg", "arrow-right.svg"),
    ("icons/arrow-up.svg", "arrow-up.svg"),
    ("icons/brand.svg", "brand.svg"),
    ("icons/check.svg", "check.svg"),
    ("icons/chevron-down.svg", "chevron-down.svg"),
    ("icons/chevron-left.svg", "chevron-left.svg"),
    ("icons/chevron-right.svg", "chevron-right.svg"),
    ("icons/chevron-up.svg", "chevron-up.svg"),
    ("icons/close.svg", "close.svg"),
    ("icons/copy.svg", "copy.svg"),
    ("icons/cut.svg", "cut.svg"),
    ("icons/desktop.svg", "desktop.svg"),
    ("icons/download.svg", "download.svg"),
    ("icons/drive.svg", "drive.svg"),
    ("icons/edit.svg", "edit.svg"),
    ("icons/external.svg", "external.svg"),
    ("icons/file.svg", "file.svg"),
    ("icons/folder.svg", "folder.svg"),
    ("icons/grid.svg", "grid.svg"),
    ("icons/home.svg", "home.svg"),
    ("icons/image.svg", "image.svg"),
    ("icons/info.svg", "info.svg"),
    ("icons/list.svg", "list.svg"),
    ("icons/maximize.svg", "maximize.svg"),
    ("icons/minimize.svg", "minimize.svg"),
    ("icons/more.svg", "more.svg"),
    ("icons/music.svg", "music.svg"),
    ("icons/paste.svg", "paste.svg"),
    ("icons/pin.svg", "pin.svg"),
    ("icons/plus.svg", "plus.svg"),
    ("icons/print.svg", "print.svg"),
    ("icons/refresh.svg", "refresh.svg"),
    ("icons/restore.svg", "restore.svg"),
    ("icons/search.svg", "search.svg"),
    ("icons/settings.svg", "settings.svg"),
    ("icons/shield.svg", "shield.svg"),
    ("icons/sidebar.svg", "sidebar.svg"),
    ("icons/sort.svg", "sort.svg"),
    ("icons/split.svg", "split.svg"),
    ("icons/star.svg", "star.svg"),
    ("icons/terminal.svg", "terminal.svg"),
    ("icons/trash.svg", "trash.svg"),
    ("icons/video.svg", "video.svg"),
];

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(ICONS
            .iter()
            .find(|(name, _)| *name == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(ICONS
            .iter()
            .filter(|(name, _)| name.starts_with(path))
            .map(|(name, _)| SharedString::from(*name))
            .collect())
    }
}
