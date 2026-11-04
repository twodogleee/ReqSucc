//! 数据存储页的 Postman 本地导入流程，预览与提交均绑定当前工作空间。
//!
//! 作者：Andersen

use super::*;

impl Workbench {
    /// 选择本地文件并交给后台读取；文件选择期间切换数据目录会使本次选择失效。
    pub(super) fn choose_postman(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.ready || self.busy || self.postman_loading {
            return;
        }
        self.postman_plan = None;
        self.postman_notice.clear();
        self.postman_loading = true;
        let workspace = self.doc.workspace_id.clone();
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("选择 Postman 集合或环境 JSON（最多 20 个）".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let selected = receiver.await;
            let _ = this.update(cx, |this, cx| {
                if this.doc.workspace_id != workspace {
                    this.postman_loading = false;
                    cx.notify();
                    return;
                }
                match selected {
                    Ok(Ok(Some(paths))) if !paths.is_empty() => {
                        let tx = this.tx.clone();
                        let result = std::thread::Builder::new()
                            .name("reqsucc-postman-import".into())
                            .spawn(move || {
                                let result = crate::postman::read_files(&paths);
                                let _ = tx.send(Event::PostmanPrepared(workspace, result));
                            });
                        if result.is_err() {
                            this.postman_loading = false;
                            this.error = "无法启动导入读取任务".into();
                        }
                    }
                    Ok(Ok(_)) => this.postman_loading = false,
                    _ => {
                        this.postman_loading = false;
                        this.error = "无法打开 Postman 文件选择器".into();
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
    /// 确认导入后冻结编辑，后台以事务写入完整快照；失败保留预览以便重试。
    pub(super) fn import_postman(&mut self, cx: &mut Context<Self>) {
        if !self.ready || self.busy {
            return;
        }
        let Some((workspace, plan)) = &self.postman_plan else {
            return;
        };
        if workspace != &self.doc.workspace_id {
            self.postman_plan = None;
            self.error = "工作空间已切换，请重新选择导入文件".into();
            return;
        }
        match self.backend.import_postman(self.doc.clone(), plan.clone()) {
            Ok(()) => {
                self.postman_saving = true;
                self.busy = true;
                self.status = "正在导入 Postman 数据…".into();
            }
            Err(error) => self.error = error,
        }
        cx.notify();
    }
    /// 绘制导入入口、数量预览和兼容性说明，确认前不修改工作空间。
    pub(super) fn postman_settings(&self, cx: &Context<Self>) -> AnyElement {
        let mut content = v_flex()
            .p_3()
            .gap_3()
            .child(self.help(
                "从本地 JSON 导入 Collection v2.0 / v2.1 或环境，可同时选择多个文件。",
                cx,
            ))
            .child(
                h_flex().child(
                    self.button(
                        "storage-import-postman",
                        if self.postman_loading {
                            "正在读取…"
                        } else {
                            "导入 Postman"
                        },
                        Action::ChoosePostman,
                        cx,
                    )
                    .icon(
                        Icon::new(IconName::FolderOpen).size(self.scaled(crate::theme::ICON_SIZE)),
                    )
                    .disabled(!self.ready || self.busy || self.postman_loading),
                ),
            )
            .child(self.help(
                "以新增方式导入，同名集合和环境自动重命名。不会覆盖现有接口或切换当前环境。",
                cx,
            ));
        if let Some((_, plan)) = &self.postman_plan {
            content = content
                .child(self.section_title("导入预览", cx))
                .child(self.help(&plan.summary(), cx))
                .child(self.help(
                    "集合与目录的继承认证按有效值写入每条接口。变量占位符保留，导入不会发送请求。",
                    cx,
                ));
            for warning in &plan.warnings {
                content = content.child(self.help(warning, cx));
            }
            content = content.child(
                h_flex()
                    .gap_2()
                    .child(
                        self.button(
                            "confirm-postman-import",
                            "确认导入",
                            Action::ImportPostman,
                            cx,
                        )
                        .primary()
                        .disabled(self.busy),
                    )
                    .child(
                        self.button("cancel-postman-import", "取消", Action::CancelPostman, cx)
                            .disabled(self.busy),
                    ),
            );
        }
        if !self.postman_notice.is_empty() {
            content = content.child(self.help(&self.postman_notice, cx));
        }
        self.setting_card(
            "Postman 数据导入",
            "导入前预览，确认后保存到当前数据目录。",
            cx,
        )
        .child(content)
        .into_any_element()
    }
}
