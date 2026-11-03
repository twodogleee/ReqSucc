use gpui_kit::{
    component::{Theme, ThemeMode, highlighter::ThemeStyle},
    *,
};
use std::sync::Arc;

// Shared UI metrics, in logical pixels before the user-selected zoom.
pub const ICON_SIZE: f32 = 16.;
pub const NAV_ICON_SIZE: f32 = 20.;
pub const CONTROL_HEIGHT: f32 = 28.;
pub const CORNER_RADIUS: f32 = 6.;

pub fn apply(light: bool, prefs: &serde_json::Value, cx: &mut App) {
    Theme::change(
        if light {
            ThemeMode::Light
        } else {
            ThemeMode::Dark
        },
        None,
        cx,
    );
    Theme::update(cx, |t| {
        let scale = prefs["scale"].as_f64().unwrap_or(100.).clamp(90., 125.) as f32 / 100.;
        t.font_size = px(14. * scale);
        #[cfg(target_os = "macos")]
        {
            t.font_family = "PingFang SC".into();
        }
        t.mono_font_size = px(prefs["editorFontSize"]
            .as_f64()
            .unwrap_or(14.)
            .clamp(12., 20.) as f32
            * scale);
        let color = |hex: u32| -> Hsla { rgb(hex).into() };
        t.colors.primary = color(if light { 0x18856b } else { 0x79d3b5 });
        t.colors.primary_foreground = color(if light { 0xffffff } else { 0x102f25 });
        if !light {
            t.colors.background = color(0x1d2023);
            t.colors.sidebar = color(0x202326);
            t.colors.foreground = color(0xe2e5e8);
            t.colors.muted_foreground = color(0xa1acb6);
            t.colors.border = color(0x353c43);
            t.colors.muted = color(0x2b3035);
        }
        t.colors.button_primary = t.colors.primary;
        t.colors.button_primary_foreground = t.colors.primary_foreground;
        t.colors.button_primary_hover = color(if light { 0x168368 } else { 0x93e5ca });
        t.colors.button_primary_active = color(if light { 0x10745b } else { 0x62bfa0 });
        if prefs["strongBorders"].as_bool().unwrap_or(false) {
            t.colors.border = color(if light { 0x9aa5af } else { 0x53616c });
        } else {
            t.colors.border = color(if light { 0xd9dee4 } else { 0x30373d });
        }
        t.colors.sidebar_border = t.colors.border;
        t.colors.title_bar_border = t.colors.border;
        t.colors.title_bar = color(if light { 0xf6f7f9 } else { 0x24272a });
        t.radius = px(CORNER_RADIUS);
        t.radius_lg = px(8.);
        t.colors.popover = color(if light { 0xffffff } else { 0x252a2f });
        t.colors.popover_foreground = t.colors.foreground;
        t.colors.accent = color(if light { 0xe8f3ef } else { 0x34463f });
        t.colors.accent_foreground = t.colors.foreground;
        t.colors.input = if prefs["strongBorders"].as_bool().unwrap_or(false) {
            t.colors.border
        } else {
            color(if light { 0xc8d0d8 } else { 0x414a52 })
        };
        let mut highlight = (*t.highlight_theme).clone();
        let style = |hex: u32| -> ThemeStyle {
            serde_json::from_value(serde_json::json!({"color":color(hex),"font_weight":400}))
                .unwrap()
        };
        let s = &mut highlight.style.syntax;
        s.property = Some(style(if light { 0x205f8b } else { 0x9cdcfe }));
        s.string = Some(style(if light { 0x28703b } else { 0xa8d5a2 }));
        s.number = Some(style(if light { 0x835218 } else { 0xefc48d }));
        s.boolean = Some(style(if light { 0x6842a0 } else { 0xc7b0f5 }));
        s.constant = Some(style(if light { 0x596477 } else { 0xb8c5d6 }));
        s.punctuation = Some(style(if light { 0x465566 } else { 0xcbd2da }));
        highlight.style.editor_background = Some(t.colors.background);
        highlight.style.editor_foreground = Some(t.colors.foreground);
        highlight.style.editor_line_number = Some(t.colors.muted_foreground);
        highlight.style.editor_active_line = Some(t.colors.muted);
        t.highlight_theme = Arc::new(highlight);
    });
}
