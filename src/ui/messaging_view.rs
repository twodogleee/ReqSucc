//! WebSocket 与 Kafka 工作台，复用请求草稿、输入组件和消息详情编辑器。
//!
//! 作者：Andersen

use super::*;

impl Workbench {
    /// 判断活动请求是否使用统一消息会话。
    pub(super) fn is_messaging(&self) -> bool {
        model::active(&self.doc)
            .is_some_and(|r| matches!(r["protocol"].as_str(), Some("websocket" | "kafka")))
    }
    /// 连接期间锁定握手、认证和消费组设置，允许编辑下一条消息与显式主题操作。
    pub(super) fn message_binding_locked(&self, binding: &Binding) -> bool {
        let Binding::Request(path) = binding else {
            return false;
        };
        let locked = if let Some(field) = path.strip_prefix("/websocket/") {
            !["payload", "format"].contains(&field)
        } else if let Some(field) = path.strip_prefix("/kafka/") {
            ![
                "payload",
                "format",
                "topic",
                "messageKey",
                "partition",
                "headers",
                "subscriptions",
                "newTopic",
                "partitions",
                "replication",
            ]
            .contains(&field.split('/').next().unwrap_or(""))
        } else {
            false
        };
        locked
            && self
                .message_sessions
                .get(&text(&self.doc.session, "activeId"))
                .is_some_and(|s| s.active())
    }
    /// 解析连接快照并启动所属协议，已有活动会话时请求取消或断开。
    pub(super) fn message_connect(&mut self, cx: &mut Context<Self>) {
        if !self.ready || self.busy {
            return;
        }
        let id = text(&self.doc.session, "activeId");
        if let Some(session) = self.message_sessions.get(&id).filter(|s| s.active()) {
            session.disconnect();
            cx.notify();
            return;
        }
        let kafka = model::active(&self.doc).is_some_and(|r| r["protocol"] == "kafka");
        let result = if kafka {
            kafka::resolve_connection(&self.doc).and_then(|c| kafka::start(c, self.runtime.clone()))
        } else {
            websocket::resolve_connection(&self.doc)
                .and_then(|c| websocket::start(c, &self.runtime))
        };
        match result {
            Ok(session) => {
                self.message_sessions.insert(id.clone(), session);
                self.message_selected.remove(&id);
                self.response_key.clear();
                self.error.clear();
            }
            Err(error) => self.error = error,
        }
        cx.notify();
    }
    /// 将当前草稿解析为操作快照；消费与主题管理只有显式点击才会执行。
    pub(super) fn message_operation(&mut self, operation: &str, cx: &mut Context<Self>) {
        let id = text(&self.doc.session, "activeId");
        if operation == "copy" {
            cx.write_to_clipboard(ClipboardItem::new_string(self.message_detail()));
            return;
        }
        if operation == "clear" {
            if let Some(session) = self.message_sessions.get(&id) {
                session.clear();
            }
            self.message_selected.remove(&id);
            self.response_key.clear();
            return;
        }
        let result = (|| {
            let command = match operation {
                "send" => messaging::Command::Send(
                    if model::active(&self.doc).is_some_and(|r| r["protocol"] == "kafka") {
                        kafka::outgoing(&self.doc)?
                    } else {
                        websocket::outgoing(&self.doc)?
                    },
                ),
                "ping" => messaging::Command::Ping,
                "refresh" => messaging::Command::Refresh,
                "unsubscribe" => messaging::Command::Subscribe(vec![]),
                "subscribe" => {
                    let config = messaging::resolve(&self.doc, "kafka", &["subscriptions"])?;
                    messaging::Command::Subscribe(kafka::subscription_names(&text(
                        &config,
                        "subscriptions",
                    ))?)
                }
                "create" => {
                    let config = messaging::resolve(&self.doc, "kafka", &["newTopic"])?;
                    let name = text(&config, "newTopic");
                    kafka::topic_name(&name)?;
                    let partitions = config["partitions"]
                        .as_i64()
                        .filter(|n| (1..=1024).contains(n))
                        .ok_or("分区数量须为 1–1024")?;
                    let replication = config["replication"]
                        .as_i64()
                        .filter(|n| (1..=1000).contains(n))
                        .ok_or("副本数量须为 1–1000")?;
                    messaging::Command::Create(name, partitions as i32, replication as i32)
                }
                _ => return Err("未知消息操作".to_owned()),
            };
            self.message_sessions
                .get(&id)
                .ok_or("请先连接服务器".to_owned())?
                .submit(command)
        })();
        self.error = result.err().unwrap_or_default();
        cx.notify();
    }
    /// 删除确认绑定原标签、会话与主题，重连、切换或关闭标签后拒绝执行。
    pub(super) fn message_delete_dialog(
        &mut self,
        topic: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx) {
            return;
        }
        let tab = text(&self.doc.session, "activeId");
        let Some(session) = self.message_sessions.get(&tab) else {
            return;
        };
        let identity = session.id.clone();
        let entity = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let (entity, tab, identity, topic) =
                (entity.clone(), tab.clone(), identity.clone(), topic.clone());
            dialog
                .title("删除 Kafka 主题")
                .child(format!(
                    "将从集群删除主题“{topic}”及其中的数据，此操作无法撤销。是否继续？"
                ))
                .confirm()
                .on_ok(move |_, _, cx| {
                    let _ = entity.update(cx, |this, cx| {
                        let result = if text(&this.doc.session, "activeId") != tab {
                            Err("当前标签已切换，请重新确认删除".into())
                        } else {
                            this.message_sessions
                                .get(&tab)
                                .filter(|s| s.id == identity)
                                .ok_or("连接已变化，请重新确认删除".to_owned())
                                .and_then(|s| s.submit(messaging::Command::Delete(topic.clone())))
                        };
                        this.error = result.err().unwrap_or_default();
                        cx.notify();
                    });
                    true
                })
        });
    }
    /// 创建协议字段输入；中文标签同时用于辅助功能和输入提示。
    fn message_field(
        &mut self,
        protocol: &str,
        field: &str,
        label: &str,
        secret: bool,
        numeric: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Input {
        self.field(
            Binding::Request(format!("/{protocol}/{field}")),
            label,
            secret,
            numeric,
            window,
            cx,
        )
    }
    /// 生成带固定标签的配置项，避免输入后丢失字段含义。
    fn message_setting(
        &mut self,
        protocol: &str,
        field: &str,
        label: &str,
        secret: bool,
        numeric: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let input = self.message_field(protocol, field, label, secret, numeric, window, cx);
        v_flex()
            .gap_1()
            .child(self.help(label, cx))
            .child(input)
            .into_any_element()
    }
    /// 生成协议枚举选择，菜单项写回同一请求草稿。
    fn message_select(
        &mut self,
        protocol: &str,
        field: &str,
        values: &[(&str, &str)],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let binding = Binding::Request(format!("/{protocol}/{field}"));
        let value = self.read(&binding).as_str().unwrap_or("").to_owned();
        let label = values
            .iter()
            .find(|(key, _)| *key == value)
            .map(|(_, label)| *label)
            .unwrap_or(&value)
            .to_owned();
        self.select(
            SharedString::from(format!("{protocol}-{field}")),
            label,
            values
                .iter()
                .map(|(key, label)| (label.to_string(), Action::Set(binding.clone(), json!(key))))
                .collect(),
            window,
            cx,
        )
    }
    /// 按标签、协议和格式复用正文编辑器，切换后不会写入其他标签。
    fn message_editor(
        &mut self,
        protocol: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let binding = Binding::Request(format!("/{protocol}/payload"));
        let id = text(&self.doc.session, "activeId");
        let json_language =
            model::active(&self.doc).is_some_and(|r| r[protocol]["format"] == "json");
        let key = format!("{id}:{protocol}:{json_language}");
        if self.body.is_none() || self.body_key != key {
            let content = self.read(&binding).as_str().unwrap_or("").to_owned();
            let editor = cx.new(|cx| {
                EditorState::new(window, cx)
                    .language(if json_language { "json" } else { "plain" })
                    .default_value(content)
                    .line_number(self.display_flag("lineNumbers"))
                    .soft_wrap(self.display_flag("softWrap"))
            });
            let sub = cx.subscribe_in(
                &editor,
                window,
                move |this, editor, event: &InputEvent, _, cx| {
                    if matches!(event, InputEvent::Change)
                        && text(&this.doc.session, "activeId") == id
                    {
                        this.write(&binding, json!(editor.read(cx).value().to_string()), cx);
                    }
                },
            );
            self.subscriptions.push(sub);
            self.body = Some(editor);
            self.body_key = key;
        }
        Editor::new(self.body.as_ref().unwrap())
            .h(relative(1.))
            .aria_label("消息正文")
            .into_any_element()
    }
    /// 绘制连接栏、保存入口和消息协议配置页。
    pub(super) fn message_request(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let request = model::active(&self.doc).cloned().unwrap_or_default();
        let protocol = text(&request, "protocol");
        let kafka = protocol == "kafka";
        let id = text(&self.doc.session, "activeId");
        let snapshot = self.message_sessions.get(&id).map(|s| s.snapshot());
        let active = snapshot
            .as_ref()
            .is_some_and(|s| s.phase != messaging::Phase::Disconnected);
        let ready = snapshot
            .as_ref()
            .is_some_and(|s| s.phase == messaging::Phase::Connected && !s.busy);
        let selected = model::active_index(&self.doc)
            .map(|i| text(&self.doc.session["tabs"][i], "panel"))
            .unwrap_or_default();
        let name = self.field(
            Binding::Request("/name".into()),
            "接口名称",
            false,
            false,
            window,
            cx,
        );
        let folders = self
            .doc
            .folders
            .iter()
            .map(|f| {
                (
                    f.name.clone(),
                    Action::Set(Binding::Request("/folder".into()), json!(f.id)),
                )
            })
            .collect();
        let folder = self
            .doc
            .folders
            .iter()
            .find(|f| f.id == text(&request, "folder"))
            .map(|f| f.name.clone())
            .unwrap_or("选择保存目录".into());
        let address = self.message_field(
            &protocol,
            if kafka { "brokers" } else { "address" },
            if kafka {
                "主机:9092，多个地址以逗号分隔"
            } else {
                "ws:// 或 wss:// 地址"
            },
            false,
            false,
            window,
            cx,
        );
        let panels = if kafka {
            vec![
                ("message", "发布消息"),
                ("subscriptions", "订阅消费"),
                ("topics", "主题管理"),
                ("headers", "消息头"),
                ("connection", "连接与认证"),
                ("description", "说明"),
            ]
        } else {
            vec![
                ("message", "消息"),
                ("headers", "请求头"),
                ("connection", "连接设置"),
                ("description", "说明"),
            ]
        };
        let content = match selected.as_str() {
            "headers" => self.rows(
                Binding::Request(format!("/{protocol}/headers")),
                false,
                window,
                cx,
            ),
            "connection" => self.message_connection_settings(&protocol, window, cx),
            "topics" if kafka => self.kafka_topics(snapshot.as_ref(), ready, window, cx),
            "subscriptions" if kafka => {
                let subscriptions = self.message_setting(
                    "kafka",
                    "subscriptions",
                    "订阅主题（逗号分隔，精确主题名）",
                    false,
                    false,
                    window,
                    cx,
                );
                v_flex()
                    .gap_3()
                    .child(subscriptions)
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                self.button(
                                    "kafka-subscribe",
                                    "订阅并消费",
                                    Action::MessageOp("subscribe".into()),
                                    cx,
                                )
                                .primary()
                                .disabled(!ready),
                            )
                            .child(
                                self.button(
                                    "kafka-unsubscribe",
                                    "取消订阅",
                                    Action::MessageOp("unsubscribe".into()),
                                    cx,
                                )
                                .disabled(!ready),
                            ),
                    )
                    .child(self.help(
                        &format!(
                                "当前订阅：{}；已分配 {} 个分区",
                                snapshot
                                    .as_ref()
                                    .map(|s| s.subscriptions.join(", "))
                                    .filter(|s| !s.is_empty())
                                    .unwrap_or("无".into()),
                                snapshot.as_ref().map(|s| s.assigned).unwrap_or(0)
                            ),
                        cx,
                    ))
                    .child(self.help(
                        "消费组与起始位置在连接设置中配置。调试消费不会自动提交位移。",
                        cx,
                    ))
                    .into_any_element()
            }
            "description" => self
                .field(
                    Binding::Request("/description".into()),
                    "接口说明",
                    false,
                    false,
                    window,
                    cx,
                )
                .into_any_element(),
            _ => {
                let mut content = v_flex().size_full().gap_2();
                if kafka {
                    let topic = self.message_setting(
                        "kafka",
                        "topic",
                        "发布主题",
                        false,
                        false,
                        window,
                        cx,
                    );
                    let key = self.message_setting(
                        "kafka",
                        "messageKey",
                        "消息键（可选）",
                        false,
                        false,
                        window,
                        cx,
                    );
                    let partition = self.message_setting(
                        "kafka",
                        "partition",
                        "分区（-1 自动）",
                        false,
                        true,
                        window,
                        cx,
                    );
                    content = content.child(
                        h_flex()
                            .gap_2()
                            .child(div().flex_1().child(topic))
                            .child(div().flex_1().child(key))
                            .child(div().w(self.scaled(140.)).child(partition)),
                    );
                }
                let format = self.message_select(
                    &protocol,
                    "format",
                    &[
                        ("text", "文本"),
                        ("json", "JSON"),
                        ("base64", "二进制（Base64）"),
                    ],
                    window,
                    cx,
                );
                let mut toolbar = h_flex().gap_2().child(format).child(
                    self.button(
                        "message-send",
                        if kafka {
                            "发布消息"
                        } else {
                            "发送消息"
                        },
                        Action::MessageOp("send".into()),
                        cx,
                    )
                    .primary()
                    .disabled(!ready),
                );
                if !kafka {
                    toolbar = toolbar.child(
                        self.button("message-ping", "Ping", Action::MessageOp("ping".into()), cx)
                            .disabled(!ready),
                    );
                }
                content
                    .child(toolbar)
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .overflow_hidden()
                            .child(self.message_editor(&protocol, window, cx)),
                    )
                    .into_any_element()
            }
        };
        v_flex()
            .size_full()
            .p_3()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .child(self.protocol_picker(cx))
                    .child(div().flex_1().min_w_0().child(name.appearance(false)))
                    .child(self.select("message-save-folder", folder, folders, window, cx))
                    .child(
                        self.button("message-save", "保存", Action::Save, cx)
                            .ghost(),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().min_w_0().child(address))
                    .child(
                        self.button(
                            "message-connect",
                            if active { "断开" } else { "连接" },
                            Action::Send,
                            cx,
                        )
                        .primary(),
                    ),
            )
            .child(self.section_tabs("message-panels", &selected, &panels, false, cx))
            .child(
                div()
                    .id("message-config-content")
                    .flex_1()
                    .min_h_0()
                    // 放大界面或缩小请求区时，编辑器应在分栏内收缩，不能覆盖下方消息工具栏。
                    .overflow_hidden()
                    .when(selected != "message" && selected != "params", |el| {
                        el.overflow_y_scroll()
                    })
                    .child(content),
            )
            .into_any_element()
    }
    /// 展示认证与 TLS 路径配置，保留证书验证且不记录密码。
    fn message_connection_settings(
        &mut self,
        protocol: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut content = v_flex().gap_3().max_w(self.scaled(760.));
        if protocol == "websocket" {
            content = content.child(self.message_setting(
                protocol,
                "subprotocols",
                "子协议（逗号分隔，可选）",
                false,
                false,
                window,
                cx,
            ));
            content = content.child(self.help(
                "WSS 使用系统信任的 CA 并验证服务器证书。认证信息可在请求头中配置。",
                cx,
            ));
        } else {
            for (field, label) in [("clientId", "客户端标识"), ("groupId", "消费组标识")]
            {
                content = content
                    .child(self.message_setting(protocol, field, label, false, false, window, cx));
            }
            content = content
                .child(self.help("没有已提交消费位移时的起始位置", cx))
                .child(self.message_select(
                    protocol,
                    "offset",
                    &[("latest", "最新消息"), ("earliest", "最早消息")],
                    window,
                    cx,
                ));
            content = content
                .child(self.help("安全协议", cx))
                .child(self.message_select(
                    protocol,
                    "security",
                    &[
                        ("PLAINTEXT", "PLAINTEXT"),
                        ("SSL", "SSL / TLS"),
                        ("SASL_PLAINTEXT", "SASL_PLAINTEXT"),
                        ("SASL_SSL", "SASL_SSL / TLS"),
                    ],
                    window,
                    cx,
                ));
            let security = model::active(&self.doc)
                .map(|r| text(&r[protocol], "security"))
                .unwrap_or_default();
            if security.starts_with("SASL") {
                content = content.child(self.message_select(
                    protocol,
                    "mechanism",
                    &[
                        ("PLAIN", "PLAIN"),
                        ("SCRAM-SHA-256", "SCRAM-SHA-256"),
                        ("SCRAM-SHA-512", "SCRAM-SHA-512"),
                    ],
                    window,
                    cx,
                ));
                content = content
                    .child(self.message_setting(
                        protocol,
                        "username",
                        "SASL 用户名",
                        false,
                        false,
                        window,
                        cx,
                    ))
                    .child(self.message_setting(
                        protocol,
                        "password",
                        "SASL 密码",
                        true,
                        false,
                        window,
                        cx,
                    ));
            }
            if security.ends_with("SSL") {
                for (field, label, secret) in [
                    ("caFile", "CA 证书 PEM 路径（可选）", false),
                    ("certFile", "客户端证书 PEM 路径（可选）", false),
                    ("keyFile", "客户端私钥 PEM 路径（与证书配对）", false),
                    ("keyPassword", "私钥口令（可选）", true),
                ] {
                    content = content.child(
                        self.message_setting(protocol, field, label, secret, false, window, cx),
                    );
                }
            }
        }
        content
            .child(self.message_setting(
                protocol,
                "timeout",
                "连接 / 操作超时（秒，1–60）",
                false,
                true,
                window,
                cx,
            ))
            .into_any_element()
    }
    /// 显示集群实际主题和分区数量，创建与删除均等待服务器确认。
    fn kafka_topics(
        &mut self,
        live: Option<&messaging::Live>,
        ready: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name =
            self.message_setting("kafka", "newTopic", "新主题名称", false, false, window, cx);
        let partitions =
            self.message_setting("kafka", "partitions", "分区数量", false, true, window, cx);
        let replication =
            self.message_setting("kafka", "replication", "副本数量", false, true, window, cx);
        let mut content = v_flex()
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(name))
                    .child(div().w(self.scaled(100.)).child(partitions))
                    .child(div().w(self.scaled(100.)).child(replication)),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        self.button(
                            "kafka-create",
                            "创建主题",
                            Action::MessageOp("create".into()),
                            cx,
                        )
                        .disabled(!ready),
                    )
                    .child(
                        self.button(
                            "kafka-refresh",
                            "刷新主题",
                            Action::MessageOp("refresh".into()),
                            cx,
                        )
                        .disabled(!ready),
                    ),
            );
        if let Some(live) = live {
            for topic in &live.topics {
                content = content.child(
                    h_flex()
                        .gap_3()
                        .child(
                            div()
                                .typography(TextRole::Control)
                                .flex_1()
                                .min_w_0()
                                .child(topic.name.clone()),
                        )
                        .child(
                            self.help(&format!("{} 个分区 {}", topic.partitions, topic.error), cx),
                        )
                        .child(
                            self.button(
                                SharedString::from(format!("delete-topic-{}", topic.name)),
                                "删除…",
                                Action::MessageDelete(topic.name.clone()),
                                cx,
                            )
                            .ghost()
                            .disabled(!ready),
                        ),
                );
            }
            if live.topics.is_empty() {
                content = content.child(self.help("没有可显示的主题，请连接后刷新。", cx));
            }
        }
        content.into_any_element()
    }
    /// 返回当前选择记录的有界详情，选择已被淘汰时回退到最新记录。
    fn message_detail(&self) -> String {
        let id = text(&self.doc.session, "activeId");
        self.message_sessions
            .get(&id)
            .and_then(|session| {
                messaging::with(&session.live, |live| {
                    self.message_selected
                        .get(&id)
                        .and_then(|id| live.records.iter().find(|r| r.id == *id))
                        .or_else(|| live.records.back())
                        .map(|r| {
                            format!(
                                "{} · {}\n{}\n\n{}",
                                messaging::format_timestamp(r.timestamp),
                                r.kind,
                                r.summary,
                                r.detail
                            )
                        })
                })
            })
            .unwrap_or_default()
    }
    /// 展示每个标签独立的有界记录列表与只读详情，支持复制和清空。
    pub(super) fn message_records(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = text(&self.doc.session, "activeId");
        let live = self.message_sessions.get(&id).map(|s| s.snapshot());
        let records = live
            .as_ref()
            .map(|s| s.records.iter().rev().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        let error = live.as_ref().map(|s| s.error.clone()).unwrap_or_default();
        let error_record = records
            .iter()
            .find(|r| r.kind == "错误")
            .map(|r| r.id)
            .unwrap_or(0);
        let panel = v_flex().size_full().min_h_0().p_3().gap_2().child(
            h_flex()
                .gap_3()
                .child(self.section_title("消息记录", cx))
                // 错误图标占用固定宽度，异常出现或消失不会挤动消息列表和详情。
                .child(
                    div()
                        .w(self.scaled(28.))
                        .h(self.scaled(28.))
                        .flex_shrink_0()
                        .when(!error.is_empty(), |el| {
                            el.child(
                                self.button(
                                    "message-error-detail",
                                    "",
                                    Action::MessageRecord(error_record),
                                    cx,
                                )
                                .icon(
                                    Icon::new(IconName::CircleAlert)
                                        .size(self.scaled(16.))
                                        .text_color(cx.theme().danger),
                                )
                                .accessibility_label(format!("查看消息错误：{error}"))
                                .tooltip(error.clone())
                                .ghost(),
                            )
                        }),
                )
                .child(div().flex_1().min_w_0().truncate().child(format!(
                    "{} · {} / 500 条",
                    live.as_ref().map(|s| s.phase.label()).unwrap_or("尚未连接"),
                    records.len()
                )))
                .child(
                    self.button(
                        "message-copy",
                        "复制详情",
                        Action::MessageOp("copy".into()),
                        cx,
                    )
                    .ghost()
                    .disabled(records.is_empty()),
                )
                .child(
                    self.button(
                        "message-clear",
                        "清空",
                        Action::MessageOp("clear".into()),
                        cx,
                    )
                    .ghost()
                    .disabled(records.is_empty()),
                ),
        );
        if records.is_empty() {
            return panel
                .child(self.help("连接后查看消息与操作结果。消息记录仅保留在本次会话中。", cx))
                .into_any_element();
        }
        let selected = self
            .message_selected
            .get(&id)
            .copied()
            .filter(|id| records.iter().any(|r| r.id == *id))
            .unwrap_or(records[0].id);
        let key = format!("message:{id}:{selected}");
        if self.response_key != key || self.response_editor.is_none() {
            let detail = self.message_detail();
            self.response_editor = Some(cx.new(|cx| {
                EditorState::new(window, cx)
                    .language("plain")
                    .default_value(detail)
                    .line_number(false)
                    .soft_wrap(true)
            }));
            self.response_key = key;
        }
        let mut list = v_flex()
            .id("message-record-list")
            .w(self.scaled(340.))
            .h_full()
            .min_h_0()
            .flex_shrink_0()
            .border_r_1()
            .border_color(cx.theme().border)
            .overflow_y_scroll();
        for record in records {
            let time = messaging::format_timestamp(record.timestamp);
            // 发送确认保留“发送成功”文字，并以方向箭头区别接收；控制事件采用独立状态图标。
            let (icon, color) = match record.kind.as_str() {
                "发送" | "发送成功" => (Icon::new(IconName::ArrowUp), cx.theme().primary),
                "接收" => (Icon::new(IconName::ArrowDown), self.method_color("WS")),
                "错误" => (Icon::new(IconName::CircleAlert), cx.theme().danger),
                "订阅" => (Icon::new(Glyph::RadioTower), cx.theme().muted_foreground),
                "连接" | "断开" => (Icon::new(IconName::Network), cx.theme().muted_foreground),
                "主题" => (Icon::new(IconName::Folder), cx.theme().muted_foreground),
                _ => (Icon::new(IconName::Info), cx.theme().muted_foreground),
            };
            list = list.child(
                ListItem::new(SharedString::from(format!("message-record-{}", record.id)))
                    .role(Role::Button)
                    .typography(TextRole::Control)
                    .accessibility_label(format!("{time} {} {}", record.kind, record.summary))
                    .tab_index(0)
                    .w_full()
                    .h(self.scaled(52.))
                    .selected(selected == record.id)
                    .on_click(cx.listener({
                        let id = record.id;
                        move |this, _, w, cx| this.act(Action::MessageRecord(id), w, cx)
                    }))
                    .on_key_down(cx.listener({
                        let id = record.id;
                        move |this, event: &KeyDownEvent, w, cx| {
                            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                this.act(Action::MessageRecord(id), w, cx);
                            }
                        }
                    }))
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .child(icon.size(self.scaled(16.)).text_color(color))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_1()
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .flex_shrink_0()
                                                    .text_color(color)
                                                    .child(record.kind.clone()),
                                            )
                                            .child(
                                                div()
                                                    .typography(TextRole::Caption)
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(time),
                                            ),
                                    )
                                    .child(div().truncate().child(record.summary.clone())),
                            ),
                    ),
            );
        }
        panel
            .child(
                h_flex().flex_1().min_h_0().gap_3().child(list).child(
                    div().flex_1().min_w_0().h_full().child(
                        Editor::new(self.response_editor.as_ref().unwrap())
                            .h(relative(1.))
                            .readonly(true)
                            .aria_label("消息详情"),
                    ),
                ),
            )
            .into_any_element()
    }
}
