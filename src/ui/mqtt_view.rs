//! MQTT 工作台视图，负责协议切换、连接与发布交互、订阅状态及有界消息详情展示。
//!
//! 作者：Andersen

use super::*;

/// 发布用例重命名弹窗内容，保留输入及校验失败提示。
struct MqttCaseRenameForm {
    /// 名称输入实体，失败后保留用户已填写的内容。
    input: Entity<InputState>,
    /// 重命名校验或上下文失效的错误说明，成功前在弹窗内展示。
    error: String,
    /// 打开弹窗时的界面缩放倍数，用于控件高度。
    scale: f32,
}

impl Render for MqttCaseRenameForm {
    /// 绘制名称输入与错误说明，沿用工作台控件高度和文字层级。
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_2()
            .child(
                Styled::h(
                    Input::new(&self.input).aria_label("发布用例名称").small(),
                    px(crate::theme::CONTROL_HEIGHT * self.scale),
                )
                .typography(TextRole::Control),
            )
            .when(!self.error.is_empty(), |form| {
                form.child(
                    div()
                        .typography(TextRole::Secondary)
                        .text_color(cx.theme().danger)
                        .child(self.error.clone()),
                )
            })
    }
}

impl Workbench {
    /// 打开用例重命名弹窗，确认时校验原标签身份和名称；失败保留弹窗及输入。
    pub(super) fn mqtt_case_rename_dialog(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx) {
            return;
        }
        let tab = text(&self.doc.session, "activeId");
        let name = model::active(&self.doc)
            .and_then(|r| r["mqtt"]["publishCases"].as_array())
            .and_then(|cases| cases.iter().find(|case| text(case, "id") == id))
            .map(|case| text(case, "name"))
            .unwrap_or_default();
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("发布用例名称")
                .default_value(name)
        });
        let scale = self.scale();
        let form = cx.new(|_| MqttCaseRenameForm {
            input: input.clone(),
            error: String::new(),
            scale,
        });
        let entity = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let state = input.clone();
            let form = form.clone();
            let form_for_ok = form.clone();
            let entity = entity.clone();
            let tab = tab.clone();
            let id = id.clone();
            dialog
                .title("重命名发布用例")
                .child(form)
                .confirm()
                .on_ok(move |_, _, cx| {
                    let name = state.read(cx).value().to_string();
                    let result = entity
                        .update(cx, |this, cx| {
                            if text(&this.doc.session, "activeId") != tab {
                                return Err("当前请求已切换，请重新打开此对话框".to_owned());
                            }
                            model::mqtt_rename_case(&mut this.doc, &id, &name)?;
                            this.reset_fields();
                            this.changed(cx);
                            Ok(())
                        })
                        .unwrap_or_else(|_| Err("当前请求已关闭".to_owned()));
                    if let Err(error) = result {
                        form_for_ok.update(cx, |form, cx| {
                            form.error = error;
                            cx.notify();
                        });
                        false
                    } else {
                        true
                    }
                })
        });
    }
    /// 展示 HTTP、MQTT、WebSocket 与 Kafka 协议切换入口。
    pub(super) fn protocol_picker(&self, cx: &Context<Self>) -> AnyElement {
        let protocol = model::active(&self.doc)
            .map(|r| text(r, "protocol"))
            .unwrap_or_default();
        let current = protocol == "mqtt";
        let entity = cx.entity().downgrade();
        let scale = self.scale();
        Button::new("request-protocol")
            .ghost()
            .small()
            .w(self.scaled(28.))
            .h(self.scaled(28.))
            .icon(
                Icon::new(if current {
                    Icon::new(Glyph::RadioTower)
                } else {
                    Icon::new(IconName::Globe)
                })
                .size(self.scaled(16.))
                .text_color(if current {
                    rgb(0xb99ae9).into()
                } else {
                    cx.theme().primary
                }),
            )
            .accessibility_label("切换请求协议")
            .tooltip("切换请求协议")
            .dropdown_menu(move |menu, _, _| {
                Self::populate_menu(
                    menu,
                    &[
                        (
                            "HTTP".into(),
                            Action::Protocol("http".into()),
                            protocol == "http",
                        ),
                        ("MQTT · V5".into(), Action::Protocol("mqtt".into()), current),
                        (
                            "WebSocket".into(),
                            Action::Protocol("websocket".into()),
                            protocol == "websocket",
                        ),
                        (
                            "Kafka".into(),
                            Action::Protocol("kafka".into()),
                            protocol == "kafka",
                        ),
                    ],
                    entity.clone(),
                    scale,
                )
            })
            .into_any_element()
    }
    /// 无活动请求或连接时切换协议并保留各协议配置，释放旧运行资源并重建编辑器。
    pub(super) fn change_protocol(&mut self, protocol: &str, cx: &mut Context<Self>) {
        if !["http", "mqtt", "websocket", "kafka"].contains(&protocol) {
            return;
        }
        let id = text(&self.doc.session, "activeId");
        if self.runs.get(&id).is_some_and(|r| r.pending)
            || self.mqtt_sessions.get(&id).is_some_and(|s| s.active())
            || self.message_sessions.get(&id).is_some_and(|s| s.active())
        {
            self.error = "请先取消请求或断开连接，再切换协议".into();
            return;
        }
        if let Some(r) = model::active_mut(&mut self.doc) {
            r["protocol"] = json!(protocol);
            if !r["mqtt"].is_object() {
                r["mqtt"] = json!(mqtt::Config::default());
            }
            model::normalize_mqtt_cases(r);
            if protocol == "websocket" && !r[protocol].is_object() {
                r[protocol] = json!(crate::websocket::Config::default());
            }
            if protocol == "kafka" && !r[protocol].is_object() {
                r[protocol] = json!(crate::kafka::Config::default());
            }
        }
        if let Some(i) = model::active_index(&self.doc) {
            self.doc.session["tabs"][i]["panel"] = json!(if protocol != "http" {
                "message"
            } else {
                "params"
            });
        }
        self.release(&id);
        self.reset_fields();
        self.error.clear();
        self.changed(cx);
    }
    /// 切换当前标签的连接状态：活动会话请求断开，否则解析连接配置并启动新会话。
    pub(super) fn mqtt_connect(&mut self, cx: &mut Context<Self>) {
        if !self.ready || self.busy {
            return;
        }
        let id = text(&self.doc.session, "activeId");
        if let Some(s) = self.mqtt_sessions.get(&id) {
            if s.active() {
                s.disconnect();
                cx.notify();
                return;
            }
        }
        match mqtt::resolve(&self.doc, mqtt::Operation::Connect)
            .and_then(|c| mqtt::Session::start(c, &self.runtime))
        {
            Ok(s) => {
                self.mqtt_sessions.insert(id.clone(), s);
                self.mqtt_selected.remove(&id);
                self.error.clear();
                self.response_key.clear();
            }
            Err(e) => self.error = e,
        }
        cx.notify();
    }
    /// 即时解析当前发布用例并提交给标签会话，解析或排队失败时展示错误。
    pub(super) fn mqtt_publish(&mut self) {
        let id = text(&self.doc.session, "activeId");
        let result = mqtt::resolve(&self.doc, mqtt::Operation::Publish).and_then(|c| {
            self.mqtt_sessions
                .get(&id)
                .ok_or("请先连接 MQTT 服务器".into())
                .and_then(|s| s.publish(c))
        });
        if let Err(e) = result {
            self.error = e;
        } else {
            self.error.clear();
        }
    }
    /// 按行下标设置订阅意图；有活动会话时先提交操作，成功入队后保存启用标记，实际结果等待回执。
    pub(super) fn mqtt_topic(&mut self, i: usize, on: bool, cx: &mut Context<Self>) {
        let id = text(&self.doc.session, "activeId");
        if let Some(s) = self.mqtt_sessions.get(&id).filter(|s| s.active()) {
            let result = mqtt::resolve(&self.doc, mqtt::Operation::Subscribe(i))
                .and_then(|c| c.topics.get(i).cloned().ok_or("主题不存在".into()))
                .and_then(|t| s.subscribe(t, on));
            if let Err(e) = result {
                self.error = e;
                return;
            }
        }
        if let Some(r) = model::active_mut(&mut self.doc) {
            if let Some(t) = r["mqtt"]["topics"].get_mut(i) {
                t["enabled"] = json!(on);
            }
        }
        self.changed(cx);
    }
    /// 将 MQTT 配置路径转换为请求绑定，复用敏感遮蔽、数值输入与连接锁定行为。
    pub(super) fn mqtt_field(
        &mut self,
        path: &str,
        label: &str,
        secret: bool,
        numeric: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Input {
        self.field(
            Binding::Request(format!("/mqtt/{path}")),
            label,
            secret,
            numeric,
            window,
            cx,
        )
    }
    /// 取得当前发布用例字段的稳定绑定，携带标签与用例标识；缺少有效用例时返回 None。
    pub(super) fn mqtt_case_binding(&self, field: &str) -> Option<Binding> {
        let index = model::mqtt_case_index(&self.doc)?;
        let tab = text(&self.doc.session, "activeId");
        let case = text(
            &model::active(&self.doc)?["mqtt"]["publishCases"][index],
            "id",
        );
        (!case.is_empty()).then(|| Binding::MqttCase {
            tab,
            case,
            field: field.into(),
        })
    }
    /// 创建当前发布用例的字段输入，数值字段由统一输入逻辑解析。
    ///
    /// # Panics
    /// 调用前必须已有经规范化的活动发布用例，否则无法建立绑定。
    fn mqtt_case_field(
        &mut self,
        path: &str,
        label: &str,
        numeric: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Input {
        let binding = self.mqtt_case_binding(path).expect("active MQTT case");
        self.field(binding, label, false, numeric, window, cx)
    }
    /// 绘制当前发布用例的 QoS 选择器，选值范围为 0–2。
    ///
    /// # Panics
    /// 调用前必须已有经规范化的活动发布用例。
    fn mqtt_case_qos(
        &mut self,
        value: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let binding = self.mqtt_case_binding("qos").expect("active MQTT case");
        self.select(
            "mqtt-case-qos",
            format!("QoS {value}"),
            (0..=2)
                .map(|q| (format!("QoS {q}"), Action::Set(binding.clone(), json!(q))))
                .collect(),
            window,
            cx,
        )
    }
    /// 为 MQTT 配置中的指定路径创建 QoS 选择器，用于遗嘱或其他非用例字段。
    pub(super) fn mqtt_qos(
        &mut self,
        path: &str,
        value: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.select(
            SharedString::from(format!("mqtt-qos-{path}")),
            format!("QoS {value}"),
            (0..=2)
                .map(|q| {
                    (
                        format!("QoS {q}"),
                        Action::Set(Binding::Request(format!("/mqtt/{path}")), json!(q)),
                    )
                })
                .collect(),
            window,
            cx,
        )
    }
    /// 按稳定绑定及文本格式复用消息编辑器，内容变化写回草稿，并应用行号与换行偏好。
    ///
    /// # Panics
    /// 编辑发布正文时必须已有经规范化的活动发布用例。
    pub(super) fn mqtt_editor(
        &mut self,
        path: &str,
        json_language: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let binding = if path == "payload" {
            self.mqtt_case_binding(path).expect("active MQTT case")
        } else {
            Binding::Request(format!("/mqtt/{path}"))
        };
        let key = format!("{}-{json_language}", binding.key());
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
                    if matches!(event, InputEvent::Change) {
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
            .aria_label(if path == "payload" {
                "MQTT 消息编辑器"
            } else {
                "遗嘱消息编辑器"
            })
            .into_any_element()
    }
    /// 绘制 MQTT 消息、订阅、认证、属性、遗嘱与连接设置分区，按连接状态限制配置编辑。
    pub(super) fn mqtt_request_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let r = model::active(&self.doc).unwrap().clone();
        let c = r["mqtt"].clone();
        let id = text(&r, "id");
        let snapshot = self.mqtt_sessions.get(&id).map(|s| s.snapshot());
        let live = snapshot
            .as_ref()
            .is_some_and(|s| s.state != mqtt::State::Disconnected);
        let connected = snapshot
            .as_ref()
            .is_some_and(|s| s.state == mqtt::State::Connected);
        let index = model::active_index(&self.doc).unwrap();
        let selected = text(&self.doc.session["tabs"][index], "panel");
        let folder = self
            .doc
            .folders
            .iter()
            .find(|f| f.id == text(&r, "folder"))
            .map(|f| f.name.clone())
            .unwrap_or("选择保存目录".into());
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
        let name = self.field(
            Binding::Request("/name".into()),
            "接口名称",
            false,
            false,
            window,
            cx,
        );
        let address = self
            .mqtt_field(
                "address",
                "mqtt://主机:1883 或 mqtts://主机:8883",
                false,
                false,
                window,
                cx,
            )
            .disabled(live);
        let client_id = self
            .mqtt_field("clientId", "客户端标识", false, false, window, cx)
            .disabled(live);
        let nav = self.section_tabs(
            "mqtt-panels",
            &selected,
            &[
                ("message", "消息"),
                ("topics", "主题订阅"),
                ("mqtt-auth", "认证"),
                ("properties", "属性"),
                ("will", "遗嘱"),
                ("mqtt-settings", "连接设置"),
                ("description", "说明"),
            ],
            false,
            cx,
        );
        let content = match selected.as_str() {
            "topics" => self.mqtt_topics(live, connected, snapshot.as_ref(), window, cx),
            "mqtt-auth" => {
                let username = self
                    .mqtt_field(
                        "username",
                        "用户名，支持 {{变量名}}",
                        false,
                        false,
                        window,
                        cx,
                    )
                    .disabled(live);
                let password = self
                    .mqtt_field("password", "密码，支持 {{变量名}}", true, false, window, cx)
                    .disabled(live);
                v_flex()
                    .gap_3()
                    .max_w(self.scaled(620.))
                    .child(
                        self.setting_switch(
                            "mqtt-auth-enabled",
                            Binding::Request("/mqtt/auth".into()),
                            "使用用户名和密码",
                            cx,
                        )
                        .disabled(live),
                    )
                    .child(
                        Form::vertical()
                            .small()
                            .child(Field::new().label("用户名").child(username))
                            .child(Field::new().label("密码").child(password)),
                    )
                    .child(
                        div()
                            .typography(TextRole::Secondary)
                            .text_color(cx.theme().muted_foreground)
                            .child("使用当前环境、集合和全局变量。认证信息仅随连接发送。"),
                    )
                    .into_any_element()
            }
            "properties" => v_flex()
                .gap_3()
                .child(self.section_title("连接用户属性（MQTT V5）", cx))
                .child(
                    div()
                        .typography(TextRole::Secondary)
                        .text_color(cx.theme().muted_foreground)
                        .child("属性随 CONNECT 发送；修改后需重新连接。发布属性在消息页设置。"),
                )
                .when(live, |el| {
                    el.child(self.help("连接期间不可修改连接属性", cx))
                })
                .child(self.rows(
                    Binding::Request("/mqtt/properties".into()),
                    false,
                    window,
                    cx,
                ))
                .into_any_element(),
            "will" => {
                let topic = self
                    .mqtt_field("willTopic", "遗嘱主题", false, false, window, cx)
                    .disabled(live);
                let payload = self
                    .mqtt_field(
                        "willPayload",
                        "遗嘱消息，支持 {{变量名}}",
                        false,
                        false,
                        window,
                        cx,
                    )
                    .disabled(live);
                v_flex()
                    .gap_3()
                    .child(
                        self.setting_switch(
                            "will-enabled",
                            Binding::Request("/mqtt/willEnabled".into()),
                            "启用遗嘱消息",
                            cx,
                        )
                        .disabled(live),
                    )
                    .child(self.help("异常断开时由服务器发布；正常点击断开不会触发遗嘱。", cx))
                    .child(
                        Form::vertical()
                            .small()
                            .max_w(self.scaled(620.))
                            .child(Field::new().label("遗嘱主题").child(topic))
                            .child(Field::new().label("遗嘱内容").child(payload)),
                    )
                    .child(
                        h_flex()
                            .gap_3()
                            .child(self.mqtt_qos(
                                "willQos",
                                c["willQos"].as_u64().unwrap_or(0),
                                window,
                                cx,
                            ))
                            .child(
                                self.toggle(
                                    "will-retain",
                                    Binding::Request("/mqtt/willRetain".into()),
                                    "保留遗嘱消息",
                                    cx,
                                )
                                .disabled(live),
                            ),
                    )
                    .into_any_element()
            }
            "mqtt-settings" => {
                let keep = self
                    .mqtt_field("keepAlive", "保活间隔", false, true, window, cx)
                    .disabled(live);
                let timeout = self
                    .mqtt_field("timeout", "连接超时", false, true, window, cx)
                    .disabled(live);
                v_flex().gap_3().max_w(self.scaled(620.)).child("协议版本：MQTT V5（固定）")
                    .child(Form::horizontal().small().label_width(self.scaled(220.))
                        .child(Field::new().label("保活间隔（秒，5–65535）").child(keep))
                        .child(Field::new().label("连接超时（秒，1–300）").child(timeout)))
                    .child(self.help("使用全新会话，断开时会话到期；连接中断后手动重连。", cx))
                    .child(self.help("mqtts 使用系统可信证书验证 TLS；当前不支持自定义 CA、双向证书或 MQTT over WebSocket。", cx))
                    .child(self.help("发布上限 256 KiB，接收报文上限 1 MiB；消息预览最多 8 KiB，内存保留最近 500 条记录。", cx)).into_any_element()
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
                let Some(case_index) = model::mqtt_case_index(&self.doc) else {
                    return div().child("无法加载 MQTT 发布用例").into_any_element();
                };
                let case = c["publishCases"][case_index].clone();
                let case_id = text(&case, "id");
                let case_name = text(&case, "name");
                let cases = c["publishCases"].as_array().cloned().unwrap_or_default();
                let selector = self.select(
                    "mqtt-publish-case",
                    case_name.clone(),
                    cases
                        .iter()
                        .map(|v| (text(v, "name"), Action::MqttCaseSelect(text(v, "id"))))
                        .collect(),
                    window,
                    cx,
                );
                let case_menu = self.dropdown(
                    "mqtt-case-menu",
                    "",
                    vec![
                        ("复制用例".into(), Action::MqttCaseNew(true)),
                        (
                            "重命名用例".into(),
                            Action::MqttCaseRenameDialog(case_id.clone()),
                        ),
                        ("删除用例…".into(), Action::MqttCaseConfirmDelete(case_id)),
                    ],
                    true,
                    cx,
                );
                let topic =
                    self.mqtt_case_field("topic", "发布主题，支持 {{变量名}}", false, window, cx);
                let editor = self.mqtt_editor("payload", case["format"] == "json", window, cx);
                let content_type =
                    self.mqtt_case_field("contentType", "内容类型（可选）", false, window, cx);
                let response_topic =
                    self.mqtt_case_field("responseTopic", "响应主题（可选）", false, window, cx);
                let expiry =
                    self.mqtt_case_field("expiry", "有效期（秒，0 表示不设置）", true, window, cx);
                v_flex()
                    .size_full()
                    .gap_2()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().typography(TextRole::Control).child("发布用例"))
                            .child(selector)
                            .child(
                                self.button(
                                    "mqtt-new-case",
                                    "新建",
                                    Action::MqttCaseNew(false),
                                    cx,
                                )
                                .ghost(),
                            )
                            .child(case_menu),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().flex_1().min_w_0().child(topic))
                            .child(self.mqtt_case_qos(
                                case["qos"].as_u64().unwrap_or(0),
                                window,
                                cx,
                            ))
                            .child(self.toggle(
                                "mqtt-retain",
                                self.mqtt_case_binding("retain").expect("active MQTT case"),
                                "保留",
                                cx,
                            ))
                            .child(
                                self.button("mqtt-publish", "发布消息", Action::MqttPublish, cx)
                                    .primary()
                                    .disabled(!connected),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(self.choose(
                                "mqtt-format",
                                self.mqtt_case_binding("format").expect("active MQTT case"),
                                &[("text", "文本"), ("json", "JSON")],
                                window,
                                cx,
                            ))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .typography(TextRole::Secondary)
                                    .text_color(cx.theme().muted_foreground)
                                    .child("消息内容支持 {{变量名}}，发布时解析"),
                            )
                            .child(
                                self.button(
                                    "mqtt-format-json",
                                    "格式化 JSON",
                                    Action::MqttFormat,
                                    cx,
                                )
                                .ghost()
                                .disabled(
                                    case["format"] != "json"
                                        || text(&case, "payload").trim().is_empty(),
                                ),
                            ),
                    )
                    .child(div().flex_1().min_h(self.scaled(80.)).child(editor))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().flex_1().min_w_0().child(content_type))
                            .child(div().flex_1().min_w_0().child(response_topic))
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child("有效期（秒）")
                                    .child(div().w(self.scaled(100.)).child(expiry)),
                            ),
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
                    .h(self.scaled(28.))
                    .gap_2()
                    .child(self.protocol_picker(cx))
                    .child(
                        div()
                            .typography(TextRole::Secondary)
                            .text_color(cx.theme().muted_foreground)
                            .child(folder.clone()),
                    )
                    .child(Icon::new(IconName::ChevronRight).size(self.scaled(16.)))
                    .child(div().flex_1().min_w_0().child(name.appearance(false)))
                    .child(self.select("mqtt-save-folder", folder, folders, window, cx))
                    .child(self.button("mqtt-save", "保存", Action::Save, cx).ghost()),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .h(self.scaled(34.))
                            .border_1()
                            .border_color(cx.theme().input)
                            .rounded(self.scaled(7.))
                            .px_2()
                            .gap_2()
                            .child(
                                div()
                                    .typography(TextRole::Control)
                                    .text_color(self.method_color("MQTT"))
                                    .child("V5"),
                            )
                            .child(div().flex_1().min_w_0().child(address.appearance(false)))
                            .child(
                                div()
                                    .w(self.scaled(200.))
                                    .border_l_1()
                                    .border_color(cx.theme().border)
                                    .child(client_id.appearance(false)),
                            ),
                    )
                    .child(
                        self.button(
                            "mqtt-connect",
                            if live {
                                if connected { "断开" } else { "取消" }
                            } else {
                                "连接"
                            },
                            Action::Send,
                            cx,
                        )
                        .icon(
                            Icon::new(if live {
                                Glyph::CircleStop
                            } else {
                                Glyph::RadioTower
                            })
                            .size(self.scaled(16.)),
                        )
                        .primary()
                        .h(self.scaled(34.))
                        .w(self.scaled(86.))
                        .disabled(self.busy),
                    ),
            )
            .child(nav)
            .child(
                div()
                    .id("mqtt-request-content")
                    .flex_1()
                    .min_h_0()
                    .when(selected != "message" && selected != "params", |el| {
                        el.overflow_y_scroll()
                    })
                    .child(content),
            )
            .into_any_element()
    }
    /// 绘制订阅主题列表，将保存的启用意图与服务器确认状态分开展示。
    pub(super) fn mqtt_topics(
        &mut self,
        live: bool,
        connected: bool,
        snapshot: Option<&mqtt::Live>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rows = model::active(&self.doc).unwrap()["mqtt"]["topics"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut panel = v_flex().gap_2().child(
            div()
                .typography(TextRole::Secondary)
                .text_color(cx.theme().muted_foreground)
                .child(if live {
                    "连接中可订阅或取消；修改主题、QoS 和说明需先断开。"
                } else {
                    "打开订阅开关后，连接成功时自动订阅。支持 +、# 通配符。"
                }),
        );
        let header = TableHeader::new().child(
            TableRow::new()
                .child(self.table_head("主题", None))
                .child(self.table_head("QoS", Some(100.)))
                .child(self.table_head("订阅状态", Some(170.)))
                .child(self.table_head("说明", None))
                .child(self.table_head("", Some(44.))),
        );
        let mut body = TableBody::new();
        for (i, row) in rows.iter().enumerate() {
            let topic = self
                .mqtt_field(
                    &format!("topics/{i}/topic"),
                    "输入主题",
                    false,
                    false,
                    window,
                    cx,
                )
                .disabled(live);
            let desc = self
                .mqtt_field(
                    &format!("topics/{i}/description"),
                    "说明",
                    false,
                    false,
                    window,
                    cx,
                )
                .disabled(live);
            let actual = snapshot.and_then(|s| s.subscriptions.get(&text(row, "id")));
            let on = if live {
                actual.is_some_and(|(on, _)| *on)
            } else {
                row["enabled"].as_bool().unwrap_or(false)
            };
            let state = actual.map(|(_, s)| s.as_str()).unwrap_or(if live {
                "未订阅"
            } else {
                "连接后订阅"
            });
            let pending = state.starts_with("正在");
            let qos = self.mqtt_qos(
                &format!("topics/{i}/qos"),
                row["qos"].as_u64().unwrap_or(0),
                window,
                cx,
            );
            body = body.child(
                TableRow::new()
                    .child(self.table_cell(None).child(topic.appearance(false)))
                    .child(self.table_cell(Some(100.)).child(qos))
                    .child(
                        self.table_cell(Some(170.)).child(
                            Switch::new(SharedString::from(format!("mqtt-sub-{i}")))
                                .small()
                                .label(state.to_owned())
                                .checked(on)
                                .disabled((live && !connected) || pending)
                                .on_click(cx.listener(move |this, on, w, cx| {
                                    this.act(Action::MqttTopic(i, *on), w, cx)
                                })),
                        ),
                    )
                    .child(self.table_cell(None).child(desc.appearance(false)))
                    .child(
                        self.table_cell(Some(44.)).child(
                            self.icon_button(
                                SharedString::from(format!("mqtt-del-{i}")),
                                "删除主题",
                                Glyph::Trash,
                                Action::RemoveRow(Binding::Request("/mqtt/topics".into()), i),
                                cx,
                            )
                            .disabled(live),
                        ),
                    ),
            );
        }
        panel = panel.child(
            v_flex()
                .w_full()
                .border_1()
                .border_color(cx.theme().border)
                .rounded(cx.theme().radius)
                .overflow_hidden()
                .child(
                    Table::new()
                        .small()
                        .w_full()
                        .accessibility_label("MQTT 主题订阅表")
                        .child(header)
                        .child(body),
                )
                .when(rows.is_empty(), |el| {
                    el.child(
                        div()
                            .p_4()
                            .typography(TextRole::Secondary)
                            .text_color(cx.theme().muted_foreground)
                            .child("暂无订阅主题，添加后可在连接时自动订阅。"),
                    )
                })
                .child(
                    h_flex()
                        .px_2()
                        .py_1()
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .justify_between()
                        .child(
                            self.button("mqtt-add-topic", "添加主题", Action::MqttAddTopic, cx)
                                .icon(IconName::Plus)
                                .ghost()
                                .disabled(live),
                        )
                        .child(
                            div()
                                .typography(TextRole::Caption)
                                .text_color(cx.theme().muted_foreground)
                                .child(format!("{} 项", rows.len())),
                        ),
                ),
        );
        panel.into_any_element()
    }
    /// 取得当前 MQTT 记录的原始详情供复制，保留正文原始格式。
    pub(super) fn mqtt_detail(&self) -> String {
        self.mqtt_detail_with_format(false)
    }
    /// 取得 MQTT 记录的展示详情，对完整 JSON 正文应用格式化。
    fn mqtt_display_detail(&self) -> String {
        self.mqtt_detail_with_format(true)
    }
    /// 按当前标签和记录标识生成详情，选择失效时回退到最新记录；无记录返回空字符串。
    fn mqtt_detail_with_format(&self, format_json: bool) -> String {
        let id = text(&self.doc.session, "activeId");
        self.mqtt_sessions
            .get(&id)
            .and_then(|s| {
                let live = s.live.lock().unwrap();
                let record = self
                    .mqtt_selected
                    .get(&id)
                    .and_then(|id| live.records.iter().find(|r| r.id == *id))
                    .or_else(|| live.records.front());
                record.map(|r| {
                    let detail = if format_json {
                        r.display_detail()
                    } else {
                        r.detail.clone()
                    };
                    format!("{} · {}\n{}\n\n{}", r.time, r.kind, r.topic, detail)
                })
            })
            .unwrap_or_default()
    }
    /// 绘制连接状态、消息记录及只读详情，区分数据流方向与生命周期事件并支持复制或清空。
    pub(super) fn mqtt_messages(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = text(&self.doc.session, "activeId");
        let snapshot = self.mqtt_sessions.get(&id).map(|s| s.snapshot());
        let summary = snapshot
            .as_ref()
            .map(|s| s.state.label())
            .unwrap_or("尚未连接");
        let records = snapshot
            .as_ref()
            .map(|s| s.records.iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        let mut panel = v_flex()
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .gap_2()
            .p_3()
            .child(
                h_flex()
                    .gap_3()
                    .child(self.section_title("消息记录", cx))
                    .child(div().text_color(cx.theme().primary).child(summary))
                    .child(
                        div()
                            .flex_1()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{} / 500 条 · 最新在前", records.len())),
                    )
                    .child(
                        self.button("mqtt-copy", "复制详情", Action::MqttCopy, cx)
                            .ghost()
                            .disabled(records.is_empty()),
                    )
                    .child(
                        self.button("mqtt-clear", "清空", Action::MqttClear, cx)
                            .ghost()
                            .disabled(records.is_empty()),
                    ),
            );
        if records.is_empty() {
            return panel
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(self.kit_empty(
                            "等待 MQTT 消息",
                            &self.shortcut_hint(
                                "连接后查看发布、订阅与接收记录，重启后不会自动连接。",
                                "连接或断开",
                                cx,
                            ),
                            Glyph::RadioTower,
                        )),
                )
                .into_any_element();
        }
        let selected = self
            .mqtt_selected
            .get(&id)
            .copied()
            .filter(|id| records.iter().any(|r| r.id == *id))
            .unwrap_or(records[0].id);
        let detail = self.mqtt_display_detail();
        let key = format!("mqtt:{id}:{selected}");
        if self.response_key != key || self.response_editor.is_none() {
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
            .id("mqtt-message-list")
            .w(self.scaled(310.))
            .h_full()
            .min_h_0()
            .flex_shrink_0()
            .overflow_y_scroll()
            .border_r_1()
            .border_color(cx.theme().border);
        for record in &records {
            // 数据流与生命周期事件使用不同图标：排队尚未发送，服务器确认也不等于接收应用消息。
            let (icon, color) = match record.kind.as_str() {
                "发送" => (Icon::new(IconName::ArrowUp), cx.theme().primary),
                "接收" => (
                    Icon::new(IconName::ArrowDown),
                    if cx.theme().mode.is_dark() {
                        rgb(0x80bcf4).into()
                    } else {
                        rgb(0x246ba4).into()
                    },
                ),
                "排队" => (Icon::new(Glyph::Clock), cx.theme().muted_foreground),
                "确认" => (Icon::new(IconName::CircleCheck), cx.theme().primary),
                "错误" => (Icon::new(IconName::CircleAlert), cx.theme().danger),
                "订阅" => (Icon::new(Glyph::RadioTower), cx.theme().muted_foreground),
                "连接" | "断开" => (Icon::new(IconName::Network), cx.theme().muted_foreground),
                _ => (Icon::new(IconName::Info), cx.theme().muted_foreground),
            };
            list = list.child(
                ListItem::new(SharedString::from(format!("mqtt-record-{}", record.id)))
                    .role(Role::Button)
                    .tab_index(0)
                    .on_key_down(cx.listener({
                        let id = record.id;
                        move |this, event: &KeyDownEvent, w, cx| {
                            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                this.act(Action::MqttRecord(id), w, cx);
                            }
                        }
                    }))
                    .on_click(cx.listener({
                        let id = record.id;
                        move |this, _, w, cx| this.act(Action::MqttRecord(id), w, cx)
                    }))
                    .typography(TextRole::Control)
                    .accessibility_label(format!(
                        "{} {} {}",
                        record.kind, record.time, record.topic
                    ))
                    .selected(record.id == selected)
                    .w_full()
                    .h(self.scaled(48.))
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
                                                div().text_color(color).child(record.kind.clone()),
                                            )
                                            .child(
                                                div()
                                                    .typography(TextRole::Secondary)
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(record.time.clone()),
                                            ),
                                    )
                                    .child(div().truncate().child(if record.topic.is_empty() {
                                        record.detail.lines().next().unwrap_or("").to_owned()
                                    } else {
                                        record.topic.clone()
                                    })),
                            ),
                    ),
            );
        }
        panel = panel.child(
            h_flex()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .gap_3()
                .child(list)
                .child(
                    div().flex_1().min_w_0().h_full().child(
                        Editor::new(self.response_editor.as_ref().unwrap())
                            .h(relative(1.))
                            .readonly(true)
                            .aria_label("MQTT 消息详情"),
                    ),
                ),
        );
        panel.into_any_element()
    }
}
