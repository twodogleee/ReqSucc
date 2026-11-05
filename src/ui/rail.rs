//! 工作台折叠活动栏的导航项，保持图标与实际菜单内容区同心。
//!
//! 作者：Andersen

use super::*;
use gpui_kit::component::{sidebar::SidebarItem, tooltip::Tooltip};
use std::rc::Rc;

/// 单个活动栏入口，保存与侧栏生命周期一致的导航状态。
#[derive(Clone)]
struct ActivityItem {
    /// 无障碍名称和悬浮提示。
    label: SharedString,
    /// 使用组件库现有图标资源。
    icon: Glyph,
    /// 入口对应的页面是否正在显示。
    active: bool,
    /// 入口不可用时保留提示和外观，但不接收点击。
    disabled: bool,
    /// 导航动作通过工作台监听器回传。
    handler: Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>,
}

/// 活动栏图标菜单；沿用 SidebarGroup 对折叠状态的传递方式。
#[derive(Clone)]
pub(super) struct ActivityMenu {
    /// 按展示顺序排列的导航入口。
    items: Vec<ActivityItem>,
    /// 侧栏是否折叠为图标栏。
    collapsed: bool,
}

impl ActivityMenu {
    /// 创建空菜单，入口由工作台按当前状态依次添加。
    pub(super) fn new() -> Self {
        Self {
            items: Vec::new(),
            collapsed: false,
        }
    }

    /// 添加导航入口，禁用入口不会调用 handler。
    pub(super) fn push(
        &mut self,
        label: &'static str,
        icon: Glyph,
        active: bool,
        disabled: bool,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) {
        self.items.push(ActivityItem {
            label: label.into(),
            icon,
            active,
            disabled,
            handler: Rc::new(handler),
        });
    }
}

impl Collapsible for ActivityMenu {
    /// 返回父侧栏传入的折叠状态。
    fn is_collapsed(&self) -> bool {
        self.collapsed
    }

    /// 接收 SidebarGroup 的折叠状态，不修改入口状态。
    fn collapsed(mut self, collapsed: bool) -> Self {
        self.collapsed = collapsed;
        self
    }
}

impl SidebarItem for ActivityMenu {
    /// 绘制与组件库菜单相同的间距和状态，图标在实际内容宽度内居中。
    fn render(
        self,
        id: impl Into<ElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> impl IntoElement {
        let id = id.into();
        // 组件库菜单使用 text_sm；图标无显式尺寸时为 0.875 rem。
        let icon_size = window.rem_size() * 0.875;
        v_flex()
            .gap_2()
            .children(self.items.into_iter().enumerate().map(|(ix, item)| {
                let id: SharedString = format!("{id}-{ix}").into();
                let label = item.label.clone();
                let handler = item.handler.clone();
                div().id(id).w_full().child(
                    h_flex()
                        .size_full()
                        .id("item")
                        .role(Role::TreeItem)
                        .aria_label(item.label.clone())
                        .aria_selected(item.active)
                        .overflow_x_hidden()
                        .flex_shrink_0()
                        .p_2()
                        .gap_x_2()
                        .rounded(cx.theme().radius)
                        .text_sm()
                        .when(!item.active && !item.disabled, |row| {
                            row.hover(|row| {
                                row.bg(cx.theme().sidebar_accent.opacity(0.8))
                                    .text_color(cx.theme().sidebar_accent_foreground)
                            })
                        })
                        .when(item.active, |row| {
                            row.font_medium()
                                .bg(cx.theme().tokens.sidebar_accent)
                                .text_color(cx.theme().sidebar_accent_foreground)
                        })
                        .when(item.disabled, |row| {
                            row.text_color(cx.theme().muted_foreground)
                        })
                        .when(!item.disabled, |row| {
                            row.on_click(move |event, window, cx| handler(event, window, cx))
                        })
                        .when(self.collapsed, |row| {
                            row.justify_center()
                                .child(div().w_full().h(icon_size).child(
                                    super::centered_icon::centered_icon(
                                        Icon::new(item.icon),
                                        icon_size,
                                    ),
                                ))
                                .tooltip(move |window, cx| {
                                    Tooltip::new(label.clone()).build(window, cx)
                                })
                        })
                        .when(!self.collapsed, |row| {
                            row.h_7().child(Icon::new(item.icon)).child(
                                h_flex()
                                    .flex_1()
                                    .gap_x_2()
                                    .justify_between()
                                    .overflow_x_hidden()
                                    .child(
                                        h_flex()
                                            .flex_1()
                                            .overflow_x_hidden()
                                            .child(item.label.clone()),
                                    ),
                            )
                        }),
                )
            }))
    }
}
