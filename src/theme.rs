use gpui::{Font, FontFallbacks, FontStyle, FontWeight, Hsla, rgb};

// Core palette. Keep colors in one place so the whole app stays coherent.
pub fn canvas() -> Hsla {
    rgb(0xF5F6F9).into()
}
pub fn surface() -> Hsla {
    rgb(0xFFFFFF).into()
}
pub fn chrome() -> Hsla {
    rgb(0xFAFAFC).into()
}
pub fn border() -> Hsla {
    rgb(0xE7E8EC).into()
}
pub fn border_soft() -> Hsla {
    rgb(0xEFF0F3).into()
}
pub fn text() -> Hsla {
    rgb(0x1F2024).into()
}
pub fn text_muted() -> Hsla {
    rgb(0x71727A).into()
}
pub fn text_faint() -> Hsla {
    rgb(0xA1A2AA).into()
}
pub fn accent() -> Hsla {
    rgb(0x2F7CF6).into()
}
pub fn accent_hover() -> Hsla {
    rgb(0x1F6AE8).into()
}
pub fn accent_soft() -> Hsla {
    rgb(0xEAF4FF).into()
}
pub fn accent_border() -> Hsla {
    rgb(0xA8D3FF).into()
}
pub fn hover() -> Hsla {
    rgb(0xF2F3F6).into()
}
pub fn pressed() -> Hsla {
    rgb(0xE9EBF0).into()
}
pub fn folder() -> Hsla {
    rgb(0xF4B740).into()
}
pub fn danger() -> Hsla {
    rgb(0xE5484D).into()
}
/// Slightly deeper wash used for the animated row highlight.
pub fn row_hover() -> Hsla {
    rgb(0xE4E7EF).into()
}
/// Hover wash for title-bar controls: clearly darker than `hover()`.
pub fn control_hover() -> Hsla {
    rgb(0xE3E5EC).into()
}
/// Muted red for the close button so it does not glare.
pub fn danger_muted() -> Hsla {
    rgb(0xC94B4B).into()
}
pub fn danger_soft() -> Hsla {
    rgb(0xFFF0F0).into()
}
pub fn success() -> Hsla {
    rgb(0x2FA36B).into()
}
pub fn warning() -> Hsla {
    rgb(0xD97706).into()
}
pub fn purple() -> Hsla {
    rgb(0x8B5CF6).into()
}
pub fn blue() -> Hsla {
    rgb(0x3B82F6).into()
}
pub fn teal() -> Hsla {
    rgb(0x0D9488).into()
}
pub fn white() -> Hsla {
    rgb(0xFFFFFF).into()
}
pub fn black() -> Hsla {
    rgb(0x000000).into()
}

pub const UI_FONT_CANDIDATES: &[&str] = &[
    "Microsoft YaHei UI",
    "Microsoft YaHei",
    "Noto Sans SC",
    "Noto Sans CJK SC",
    "Source Han Sans SC",
    "Source Han Sans CN",
    "SimHei",
    "SimSun",
    "NSimSun",
    "Segoe UI",
];

pub const CJK_FALLBACKS: &[&str] = &[
    "Microsoft YaHei UI",
    "Microsoft YaHei",
    "Noto Sans SC",
    "Noto Sans CJK SC",
    "Source Han Sans SC",
    "SimHei",
    "SimSun",
    "Segoe UI",
    "Arial",
];


pub fn app_font(family: impl Into<gpui::SharedString>, fallbacks: Vec<String>) -> Font {
    Font {
        family: family.into(),
        features: Default::default(),
        fallbacks: Some(FontFallbacks::from_fonts(fallbacks)),
        weight: FontWeight::NORMAL,
        style: FontStyle::Normal,
    }
}

pub const TITLEBAR_HEIGHT: f32 = 46.0;
pub const TOOLBAR_HEIGHT: f32 = 52.0;
pub const STATUS_HEIGHT: f32 = 30.0;
pub const SIDEBAR_WIDTH: f32 = 218.0;
pub const ROW_HEIGHT: f32 = 36.0;
