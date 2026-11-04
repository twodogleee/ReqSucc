//! 全局主题与文字层级，统一控件尺寸、深浅配色和编辑器语法高亮。
//!
//! 作者：Andersen

use gpui_kit::{
    component::{Theme, ThemeMode, highlighter::ThemeStyle},
    *,
};
use std::sync::Arc;

// 图标和控件尺寸以用户缩放前的逻辑像素定义，由调用方统一缩放。
/// 工具栏图标的基准边长，单位为缩放前的逻辑像素。
pub const ICON_SIZE: f32 = 16.;
/// 常规工具栏控件的基准高度，单位为缩放前的逻辑像素。
pub const CONTROL_HEIGHT: f32 = 28.;
/// 标准组件圆角半径，单位为逻辑像素。
pub const CORNER_RADIUS: f32 = 6.;

/// GPUI Kit 控件与文字角色共用的 rem 基准字号；Root 统一应用一次缩放。
pub const BASE_FONT_SIZE: f32 = 14.;
/// 界面文字的语义层级，统一字号、字重和行高。
#[derive(Clone, Copy)]
pub enum TextRole {
    /// 页面主标题，基准字号为 18 逻辑像素。
    PageTitle,
    /// 分组标题，基准字号为 14 逻辑像素。
    SectionTitle,
    /// 正文内容，基准字号为 13 逻辑像素。
    Body,
    /// 按钮、输入和选择控件文字，基准字号为 12.25 逻辑像素。
    Control,
    /// 可换行的辅助说明，基准字号为 12 逻辑像素。
    Secondary,
    /// 状态、计数和表头等次要文字，基准字号为 11 逻辑像素。
    Caption,
}
/// 为可设置样式的元素应用统一文字角色，避免页面各自定义字号和强调方式。
pub trait TypographyExt: Styled + Sized {
    /// 按文字角色设置相对字号、字重和行高，使文字跟随全局字号缩放。
    fn typography(self, role: TextRole) -> Self {
        let (size, weight, line) = match role {
            TextRole::PageTitle => (18., FontWeight::SEMIBOLD, 1.4),
            TextRole::SectionTitle => (14., FontWeight::MEDIUM, 1.45),
            TextRole::Body => (13., FontWeight::NORMAL, 1.5),
            TextRole::Control => (12.25, FontWeight::NORMAL, 1.4),
            TextRole::Secondary => (12., FontWeight::NORMAL, 1.6),
            TextRole::Caption => (11., FontWeight::NORMAL, 1.45),
        };
        self.text_size(rems(size / BASE_FONT_SIZE))
            .font_weight(weight)
            .line_height(relative(line))
    }
}
impl<T: Styled> TypographyExt for T {}

/// 应用深浅主题与显示偏好，限制界面缩放和编辑器字号，并统一组件及语法高亮配色。
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
        t.font_size = px(BASE_FONT_SIZE * scale);
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
        // 次要文字颜色需同时适配主背景和浮层背景，保持深浅主题下的可读性。
        t.colors.muted_foreground = color(if light { 0x66717d } else { 0x9aa6b2 });
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
        // 标准组件共享背景层级和选中颜色，避免标签、表格与侧栏各用一套视觉语义。
        t.colors.tab = t.colors.sidebar;
        t.colors.tab_bar = t.colors.sidebar;
        t.colors.tab_bar_segmented = t.colors.muted;
        t.colors.tab_foreground = t.colors.muted_foreground;
        t.colors.tab_active = t.colors.background;
        t.colors.tab_active_foreground = t.colors.foreground;
        t.colors.table = t.colors.background;
        t.colors.table_head = t.colors.sidebar;
        t.colors.table_head_foreground = t.colors.muted_foreground;
        t.colors.table_foot = t.colors.sidebar;
        t.colors.table_foot_foreground = t.colors.muted_foreground;
        t.colors.table_even = t.colors.background;
        t.colors.table_hover = t.colors.muted;
        t.colors.table_active = t.colors.accent;
        t.colors.table_active_border = t.colors.primary;
        t.colors.table_row_border = t.colors.border;
        t.colors.list = t.colors.sidebar;
        t.colors.list_hover = t.colors.muted;
        t.colors.list_active = t.colors.accent;
        t.colors.list_active_border = t.colors.primary;
        t.colors.sidebar_accent = t.colors.accent;
        t.colors.sidebar_accent_foreground = t.colors.foreground;
        t.colors.sidebar_foreground = t.colors.foreground;
        t.colors.status_bar = t.colors.sidebar;
        t.colors.status_bar_border = t.colors.border;
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
