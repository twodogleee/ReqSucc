//! 快捷键设置页、组合键录入和冲突反馈，修改后立即应用并通过现有会话保存流程持久化。
//!
//! 作者：Andersen

use super::*;

impl Workbench {
    /// 按当前工作空间重建快捷键，并清除录入状态；目录切换后不沿用旧工作空间的绑定。
    pub(super) fn refresh_shortcuts(&mut self, cx: &mut Context<Self>) {
        self.shortcut_recording = None;
        self.shortcut_capture = None;
        self.shortcut_error =
            shortcuts::apply(&self.doc.session, &self.standard_shortcuts, cx).join("\n");
    }

    /// 进入按键录入并拦截原快捷键；Esc 取消，校验失败保留旧绑定并允许继续录入。
    pub(super) fn record_shortcut(
        &mut self,
        shortcut: Shortcut,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.ready || window.has_active_dialog(cx) {
            return;
        }
        self.shortcut_recording = Some(shortcut);
        self.shortcut_error.clear();
        self.focus.focus(window, cx);
        let entity = cx.entity().downgrade();
        self.shortcut_capture = Some(cx.intercept_keystrokes(move |event, window, cx| {
            let consumed = entity
                .update(cx, |this, cx| {
                    if this.screen != "settings"
                        || this.settings_page != "shortcuts"
                        || this.shortcut_recording.is_none()
                    {
                        return false;
                    }
                    this.capture_shortcut(&event.keystroke, cx);
                    true
                })
                .unwrap_or(false);
            if consumed {
                window.prevent_default();
                cx.stop_propagation();
            }
        }));
    }

    /// 校验实际按下的组合键；成功时替换绑定并保存，修饰键单独按下不结束录入。
    fn capture_shortcut(&mut self, stroke: &Keystroke, cx: &mut Context<Self>) {
        let Some(shortcut) = self.shortcut_recording else {
            return;
        };
        if stroke.key == "escape" && !stroke.modifiers.modified() {
            self.shortcut_recording = None;
            self.shortcut_capture = None;
            self.shortcut_error.clear();
        } else if !stroke.key.is_empty()
            && !matches!(
                stroke.key.as_str(),
                "shift" | "control" | "alt" | "platform" | "function"
            )
        {
            let os = cx.platform_info().os;
            match shortcuts::validate(
                &self.doc.session,
                shortcut,
                &stroke.unparse(os),
                &self.standard_shortcuts,
                os,
            ) {
                Ok(key) => {
                    shortcuts::set(&mut self.doc.session, shortcut, &key, os);
                    self.refresh_shortcuts(cx);
                    self.changed(cx);
                }
                Err(error) => self.shortcut_error = error,
            }
        }
        cx.notify();
    }

    /// 为 HTTP 和 MQTT 空态追加当前发送快捷键；用户清除后隐藏快捷键说明。
    pub(super) fn shortcut_hint(
        &self,
        description: &str,
        action: &str,
        cx: &Context<Self>,
    ) -> String {
        let key = shortcuts::display(&self.doc.session, Shortcut::Send, cx.platform_info().os);
        if key.is_empty() {
            description.to_owned()
        } else {
            format!("{description} {key} {action}。")
        }
    }

    /// 展示全部应用操作的录入、清除和恢复入口；标准编辑快捷键不参与配置。
    pub(super) fn shortcuts_settings(&self, compact: bool, cx: &mut Context<Self>) -> AnyElement {
        let os = cx.platform_info().os;
        let mut card = self.setting_card(
            "应用操作快捷键",
            &format!(
                "点击组合键后按下新的快捷键，Esc 取消。{}",
                shortcuts::input_hint(os)
            ),
            cx,
        );
        for shortcut in shortcuts::ALL {
            let key = shortcuts::display(&self.doc.session, shortcut, os);
            let recording = self.shortcut_recording == Some(shortcut);
            let label = if recording {
                "请按下组合键…"
            } else if key.is_empty() {
                "未设置"
            } else {
                &key
            };
            let default = Keystroke::parse_with_os(&shortcut.default_key(os), os)
                .map(|key| gpui_kit::component::kbd::Kbd::format(&key))
                .unwrap_or_default();
            let control = h_flex()
                .w_full()
                .gap_1()
                .child(
                    self.button(
                        SharedString::from(format!("shortcut-{}", shortcut.id())),
                        label,
                        Action::RecordShortcut(shortcut),
                        cx,
                    )
                    .flex_1()
                    .min_w_0()
                    .disabled(!self.ready)
                    .tooltip(format!("设置{}快捷键", shortcut.label())),
                )
                .child(
                    self.icon_button(
                        SharedString::from(format!("clear-shortcut-{}", shortcut.id())),
                        "清除此快捷键",
                        IconName::Close,
                        Action::ClearShortcut(shortcut),
                        cx,
                    )
                    .disabled(!self.ready || key.is_empty()),
                );
            card = card.child(self.setting_row(
                shortcut.label(),
                &format!("默认：{default}"),
                control,
                compact,
                cx,
            ));
        }
        v_flex()
            .gap_3()
            .child(card)
            .when(self.shortcut_recording.is_some(), |el| {
                el.child(
                    h_flex()
                        .gap_2()
                        .child(self.help("正在录入快捷键，原快捷操作暂不执行。", cx))
                        .child(self.button(
                            "cancel-shortcut",
                            "取消录入",
                            Action::CancelShortcut,
                            cx,
                        )),
                )
            })
            .when(!self.shortcut_error.is_empty(), |el| {
                el.child(
                    self.help(&self.shortcut_error, cx)
                        .text_color(cx.theme().danger),
                )
            })
            .child(self.help(
                "快捷键按系统分别保存，清除或恢复默认只影响当前系统。复制、粘贴、撤销等标准编辑快捷键保持不变。",
                cx,
            ))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        self.button(
                            "clear-shortcuts",
                            "清除全部快捷键",
                            Action::ConfirmClearShortcuts,
                            cx,
                        )
                        .danger()
                        .outline()
                        .disabled(!self.ready),
                    )
                    .child(
                        self.button(
                            "reset-shortcuts",
                            "恢复默认快捷键",
                            Action::ResetShortcuts,
                            cx,
                        )
                        .disabled(!self.ready),
                    ),
            )
            .into_any_element()
    }
}
