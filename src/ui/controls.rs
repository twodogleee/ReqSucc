//! 基于 GPUI Kit 语义组件构建统一控件组合，集中管理选择器生命周期、文字角色和表格尺寸。
//!
//! 作者：Andersen

use super::*;
use gpui_kit::component::select::{SearchableVec, Select, SelectEvent, SelectItem, SelectState};
pub(super) use gpui_kit::component::{
    alert::Alert,
    empty::{
        Empty as KitEmpty, EmptyContent, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle,
    },
    form::{Field, Form},
    label::Label,
    list::ListItem,
    sidebar::{Sidebar, SidebarGroup, SidebarHeader, SidebarMenu, SidebarMenuItem},
    status_bar::StatusBar,
    switch::Switch,
    tab::{Tab, TabBar},
    table::{Table, TableBody, TableCell, TableHead, TableHeader, TableRow},
    tag::Tag,
};
/// 统一选择器中的显示选项，以原始动作列表下标作为稳定选值。
#[derive(Clone)]
pub(super) struct Choice {
    /// 选项在菜单与当前值区域展示的文字。
    title: SharedString,
    /// 对应动作在当前选项列表中的零起始下标。
    index: usize,
}
impl SelectItem for Choice {
    /// 选择器返回的动作下标类型。
    type Value = usize;
    /// 返回选项展示文字，供组件绘制与搜索使用。
    fn title(&self) -> SharedString {
        self.title.clone()
    }
    /// 返回选项对应的动作下标引用。
    fn value(&self) -> &usize {
        &self.index
    }
}
/// 选择器缓存项，将选项签名、组件状态和事件订阅绑定为同一生命周期。
pub(super) struct SelectSlot {
    /// 包含标题及动作目标的选项签名，改变时重建状态和订阅。
    signature: Vec<String>,
    /// 支持搜索的选择器实体，保存选中项及交互状态。
    state: Entity<SelectState<SearchableVec<Choice>>>,
    /// 随选择器状态一起保留的监听订阅，选项重建时释放旧监听。
    _subscription: Subscription,
}
impl Workbench {
    /// 根据文字角色生成可自然换行的 Label，辅助角色使用主题次要文字色。
    pub(super) fn copy(&self, content: &str, role: TextRole, cx: &Context<Self>) -> Label {
        Label::new(content.to_owned())
            .typography(role)
            .min_w_0()
            .whitespace_normal()
            .text_color(if matches!(role, TextRole::Secondary | TextRole::Caption) {
                cx.theme().muted_foreground
            } else {
                cx.theme().foreground
            })
    }
    /// 生成统一字号、行高及次要颜色的辅助说明。
    pub(super) fn help(&self, content: &str, cx: &Context<Self>) -> Label {
        self.copy(content, TextRole::Secondary, cx)
    }
    /// 生成页面主标题，避免各页面自行设置字号。
    pub(super) fn page_title(&self, content: &str, cx: &Context<Self>) -> Label {
        self.copy(content, TextRole::PageTitle, cx)
    }
    /// 生成分组标题，使用统一文字角色。
    pub(super) fn section_title(&self, content: &str, cx: &Context<Self>) -> Label {
        self.copy(content, TextRole::SectionTitle, cx)
    }
    /// 创建带可换行辅助说明的表单字段容器。
    pub(super) fn described_field(&self, description: &str) -> Field {
        let description = description.to_owned();
        Field::new().description_fn(move |_, cx| {
            Label::new(description.clone())
                .typography(TextRole::Secondary)
                .whitespace_normal()
                .text_color(cx.theme().muted_foreground)
        })
    }
    /// 缓存并同步选择器状态，选项签名变化时重建监听；超过六项启用搜索，连接锁定时禁用。
    pub(super) fn select(
        &mut self,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        choices: Vec<(String, Action)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = format!("{:?}", id.into());
        let label = label.into();
        let signature: Vec<_> = choices
            .iter()
            .map(|(title, a)| {
                format!(
                    "{title}:{}",
                    match a {
                        Action::Set(b, v) => format!("{}={v}", b.key()),
                        Action::SelectEnv(id) => id.clone(),
                        Action::MqttCaseSelect(id) => id.clone(),
                        _ => String::new(),
                    }
                )
            })
            .collect();
        let selected = choices.iter().position(|(_, a)| match a {
            Action::Set(b, v) => self.read(b) == *v,
            Action::SelectEnv(id) => text(&self.doc.session, "environment") == *id,
            Action::MqttCaseSelect(id) => self
                .mqtt_case_binding("id")
                .is_some_and(|b| self.read(&b) == *id),
            _ => false,
        });
        let locked = self.busy || choices.iter().any(|(_, a)| self.locked_action(a));
        if self
            .selects
            .get(&key)
            .is_none_or(|s| s.signature != signature)
        {
            let items: Vec<_> = choices
                .iter()
                .enumerate()
                .map(|(index, (title, _))| Choice {
                    title: title.clone().into(),
                    index,
                })
                .collect();
            let state = cx.new(|cx| {
                SelectState::new(
                    SearchableVec::new(items),
                    selected.map(|row| IndexPath::default().row(row)),
                    window,
                    cx,
                )
                .searchable(choices.len() > 6)
            });
            let actions: Vec<_> = choices.into_iter().map(|(_, a)| a).collect();
            let subscription = cx.subscribe_in(
                &state,
                window,
                move |this, _, event: &SelectEvent<SearchableVec<Choice>>, w, cx| {
                    if let SelectEvent::Confirm(Some(index)) = event {
                        if let Some(action) = actions.get(*index) {
                            this.act(action.clone(), w, cx);
                        }
                    }
                },
            );
            self.selects.insert(
                key.clone(),
                SelectSlot {
                    signature,
                    state,
                    _subscription: subscription,
                },
            );
        }
        let state = self.selects[&key].state.clone();
        if state.read(cx).selected_value().copied() != selected {
            state.update(cx, |s, cx| {
                s.set_selected_index(
                    selected.map(|row| IndexPath::default().row(row)),
                    window,
                    cx,
                )
            });
        }
        let title = if label.is_empty() {
            "选择请求方法".into()
        } else {
            label.clone()
        };
        // SelectState 会渲染占满容器的实体，因此需限制外层容器尺寸，而不只是内部触发按钮。
        let width = if key.contains("qos") {
            84.
        } else if key.contains("format") {
            110.
        } else {
            180.
        };
        div()
            .w(self.scaled(width))
            .h(self.scaled(crate::theme::CONTROL_HEIGHT))
            .flex_shrink_0()
            .child(
                Select::new(&state)
                    .small()
                    .h(self.scaled(crate::theme::CONTROL_HEIGHT))
                    .typography(TextRole::Control)
                    .placeholder(title.clone())
                    .accessibility_label(title)
                    .search_placeholder("搜索选项…")
                    .menu_width(self.scaled(260.))
                    .menu_max_h(self.scaled(320.))
                    .disabled(locked),
            )
            .into_any_element()
    }
    /// 创建分区标签条；response 为 true 时切换响应分区，否则切换请求编辑分区。
    pub(super) fn section_tabs(
        &self,
        id: &'static str,
        active: &str,
        choices: &[(&str, &str)],
        response: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let values: Vec<_> = choices.iter().map(|(id, _)| id.to_string()).collect();
        TabBar::new(id)
            .small()
            .underline()
            .w_full()
            .selected_index(
                choices
                    .iter()
                    .position(|(key, _)| *key == active)
                    .unwrap_or(0),
            )
            .children(
                choices
                    .iter()
                    .map(|(_, name)| Tab::new().label(name.to_string())),
            )
            .on_click(cx.listener(move |this, index: &usize, w, cx| {
                if let Some(value) = values.get(*index) {
                    this.act(
                        if response {
                            Action::Response(value.clone())
                        } else {
                            Action::Panel(value.clone())
                        },
                        w,
                        cx,
                    );
                }
            }))
            .into_any_element()
    }
    /// 创建统一表头；width 为缩放前逻辑像素，None 表示弹性列。
    pub(super) fn table_head(&self, label: &str, width: Option<f32>) -> TableHead {
        TableHead::new()
            .min_w_0()
            .px_2()
            .py_1()
            .typography(TextRole::Caption)
            .when_some(width, |el, width| el.w(self.scaled(width)).flex_shrink_0())
            .when(width.is_none(), |el| el.flex_1().px_4())
            .when(width == Some(48.), |el| el.justify_center())
            .child(label.to_owned())
    }
    /// 创建与表头共用宽度规则的单元格；None 表示填充剩余宽度。
    pub(super) fn table_cell(&self, width: Option<f32>) -> TableCell {
        TableCell::new()
            .min_w_0()
            .px_2()
            .py_1()
            .when_some(width, |el, width| el.w(self.scaled(width)).flex_shrink_0())
            .when(width.is_none(), |el| el.flex_1())
    }
    /// 使用协议或 HTTP 方法的主题配色创建紧凑标记。
    pub(super) fn protocol_tag(&self, method: &str) -> Tag {
        let color = self.method_color(method);
        Tag::custom(color.opacity(0.1), color, color.opacity(0.22))
            .small()
            .child(method.to_owned())
    }
    /// 组合图标、标题和说明生成统一空状态，保持文字层级一致。
    pub(super) fn kit_empty(
        &self,
        title: &str,
        description: &str,
        icon: impl Into<Icon>,
    ) -> KitEmpty {
        KitEmpty::new().header(
            EmptyHeader::new()
                .media(EmptyMedia::new().child(icon.into().size_6()))
                .title(
                    EmptyTitle::new()
                        .typography(TextRole::SectionTitle)
                        .child(title.to_owned()),
                )
                .description(
                    EmptyDescription::new()
                        .typography(TextRole::Secondary)
                        .child(description.to_owned()),
                ),
        )
    }
    /// 将布尔字段绑定到统一开关，忙状态或 MQTT 配置锁定时禁用。
    pub(super) fn setting_switch(
        &self,
        id: impl Into<ElementId>,
        binding: Binding,
        label: &str,
        cx: &Context<Self>,
    ) -> Switch {
        Switch::new(id)
            .small()
            .label(label.to_owned())
            .checked(self.read(&binding).as_bool().unwrap_or(false))
            .disabled(self.busy || self.mqtt_binding_locked(&binding))
            .on_click(cx.listener(move |this, value, _, cx| this.write(&binding, json!(value), cx)))
    }
}
