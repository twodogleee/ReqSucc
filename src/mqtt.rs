//! MQTT V5 配置校验与临时连接会话；仅配置持久化到 SQLite，连接状态和消息记录保存在内存。
//!
//! 作者：Andersen

use crate::{http, model, storage::Snapshot};
use chrono::{DateTime, Local, TimeZone};
use rumqttc::{
    Outgoing, Transport,
    v5::{
        AsyncClient, Event, MqttOptions,
        mqttbytes::{
            QoS,
            v5::{
                LastWill, Packet, PubAckReason, PubCompReason, PubRecReason, PublishProperties,
                SubscribeReasonCode, UnsubAckReason,
            },
        },
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

/// 单个连接可保存的发布用例数量上限。
pub const MAX_PUBLISH_CASES: usize = 100;
/// 发布用例与旧版单消息配置之间同步的字段名，避免选择状态污染兼容表示。
pub const PUBLISH_CASE_FIELDS: [&str; 8] = [
    "topic",
    "payload",
    "format",
    "qos",
    "retain",
    "contentType",
    "responseTopic",
    "expiry",
];

/// 用于校验持久化发布用例的完整字段结构，允许尚未填写完成的消息草稿。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PublishCase {
    /// 请求内唯一的发布用例标识。
    id: String,
    /// 用户可见用例名称，去除首尾空白后须非空且不重复。
    name: String,
    /// 尚未解析变量的发布主题，保存草稿时允许为空。
    topic: String,
    /// 尚未解析变量的消息正文，保存时不强制 JSON 完整。
    payload: String,
    /// 消息编辑格式，支持 text 和 json。
    format: String,
    /// 发布服务质量等级，允许 0、1、2。
    qos: u8,
    /// 是否请求服务器保留该消息供后续订阅者接收。
    retain: bool,
    /// 发布属性中的内容类型模板，空字符串表示不附加。
    content_type: String,
    /// 发布属性中的响应主题模板，空字符串表示不附加。
    response_topic: String,
    /// 消息有效期，单位秒；0 表示不设置有效期属性。
    expiry: u32,
}

/// 可持久化的 MQTT 连接与消息配置；执行操作前按用途展开变量。
#[derive(Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    /// mqtt:// 或 mqtts:// 连接地址模板，禁止内嵌认证、路径及查询参数。
    pub address: String,
    /// 客户端标识模板，连接时须非空。
    pub client_id: String,
    /// 用户名模板，仅启用认证时参与解析与连接。
    pub username: String,
    /// 密码模板，仅启用认证时发送，不可输出日志。
    pub password: String,
    /// 是否启用用户名密码认证，默认关闭。
    pub auth: bool,
    /// 连接保活间隔，单位秒，允许 5–65535，默认 60。
    pub keep_alive: u16,
    /// 首次连接等待超时，单位秒，允许 1–300，默认 10。
    pub timeout: u64,
    /// 保存的订阅行，连接后自动提交其中已启用项，最多 100 条。
    pub topics: Vec<Topic>,
    /// 当前待发布消息的主题；持久化兼容表示取第一个用例。
    pub topic: String,
    /// 当前待发布消息正文；发送时按 UTF-8 编码并限制为 256 KiB。
    pub payload: String,
    /// 当前发布正文格式，text 不校验 JSON，json 在发送时校验。
    pub format: String,
    /// 当前发布消息的 QoS 等级，允许 0、1、2。
    pub qos: u8,
    /// 是否对当前发布消息设置保留标记。
    pub retain: bool,
    /// CONNECT 报文用户属性行，仅发送已启用且名称非空的项。
    pub properties: Vec<http::Row>,
    /// 当前发布内容类型，空字符串表示省略该属性。
    pub content_type: String,
    /// 当前发布响应主题，非空时按无通配符主题校验。
    pub response_topic: String,
    /// 当前发布消息有效期，单位秒；0 表示不附加有效期属性。
    pub expiry: u32,
    /// 是否在连接时注册遗嘱消息，默认关闭。
    pub will_enabled: bool,
    /// 遗嘱发布主题模板，启用遗嘱时按无通配符主题校验。
    pub will_topic: String,
    /// 遗嘱正文模板，展开后最多 256 KiB。
    pub will_payload: String,
    /// 遗嘱的 QoS 等级，允许 0、1、2。
    pub will_qos: u8,
    /// 是否请求服务器保留遗嘱消息。
    pub will_retain: bool,
}
impl Default for Config {
    /// 创建随机客户端标识及默认连接参数，初始不启用认证、订阅或遗嘱。
    fn default() -> Self {
        Self {
            address: String::new(),
            client_id: format!("reqsucc-{}", &model::id()[..8]),
            username: String::new(),
            password: String::new(),
            auth: false,
            keep_alive: 60,
            timeout: 10,
            topics: vec![],
            topic: String::new(),
            payload: String::new(),
            format: "text".into(),
            qos: 0,
            retain: false,
            properties: vec![],
            content_type: String::new(),
            response_topic: String::new(),
            expiry: 0,
            will_enabled: false,
            will_topic: String::new(),
            will_payload: String::new(),
            will_qos: 0,
            will_retain: false,
        }
    }
}
/// 可持久化的订阅行，稳定标识用于对应异步服务器确认。
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Topic {
    /// 订阅行稳定标识，用于关联订阅状态和操作回执。
    pub id: String,
    /// 主题过滤器模板，支持合法通配符与共享订阅格式。
    pub topic: String,
    /// 请求的订阅 QoS 等级，允许 0、1、2。
    pub qos: u8,
    /// 是否希望连接后订阅该主题；实际订阅成功状态由服务器回执决定。
    pub enabled: bool,
    /// 用户维护的订阅说明，不发送到服务器。
    pub description: String,
}
/// 创建默认不启用、QoS 为 0 的空白订阅行。
pub fn topic_row() -> Value {
    json!({"id":model::id(),"topic":"","qos":0,"enabled":false,"description":""})
}
/// 判断请求是否明确选择 MQTT 协议，缺失协议字段时返回 false。
pub fn is_mqtt(r: &Value) -> bool {
    r["protocol"] == "mqtt"
}
/// 生成侧栏协议标记；MQTT 返回协议名，HTTP 返回方法名。
pub fn badge(r: &Value) -> String {
    if is_mqtt(r) {
        "MQTT".into()
    } else {
        model::text(r, "method")
    }
}
/// 按协议读取请求列表展示用地址，不进行变量解析。
pub fn address(r: &Value) -> String {
    if is_mqtt(r) {
        model::text(&r["mqtt"], "address")
    } else {
        model::text(r, "url")
    }
}
/// 从请求的 mqtt 字段反序列化配置，缺省字段沿用 Config 默认值，类型无效时返回错误。
pub fn config(r: &Value) -> Result<Config, String> {
    serde_json::from_value(r["mqtt"].clone()).map_err(|_| "MQTT 配置格式不正确".into())
}

/// 校验并格式化 JSON 模板，保留原始数值、转义、重复键和变量占位符，不展开变量。
///
/// # Errors
/// 将裸值占位符暂替为 null 后仍不是合法 JSON 时返回解析错误，调用方应保留原文。
pub fn format_json_template(source: &str) -> Result<String, serde_json::Error> {
    // 仅在校验副本中将裸值占位符替换为 null，字符串内的占位符保持原样。
    // 后续排版使用原始 token，避免改写重复键、数值表示和转义。
    let mut checked = String::with_capacity(source.len());
    let mut iter = source.char_indices().peekable();
    let mut quoted = false;
    let mut escaped = false;
    while let Some((pos, ch)) = iter.next() {
        if quoted {
            checked.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                quoted = false;
            }
        } else if ch == '"' {
            quoted = true;
            checked.push(ch);
        } else if source[pos..].starts_with("{{") {
            if let Some(end) = source[pos + 2..].find("}}") {
                let end = pos + 2 + end + 2;
                checked.push_str("null");
                while iter.peek().is_some_and(|(index, _)| *index < end) {
                    iter.next();
                }
            } else {
                checked.push(ch);
            }
        } else {
            checked.push(ch);
        }
    }
    serde_json::from_str::<serde::de::IgnoredAny>(&checked)?;

    let mut output = String::with_capacity(source.len());
    let mut iter = source.char_indices().peekable();
    let mut indent = 0usize;
    let mut quoted = false;
    let mut escaped = false;
    while let Some((pos, ch)) = iter.next() {
        if quoted {
            output.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                quoted = false;
            }
            continue;
        }
        if ch == '"' {
            quoted = true;
            output.push(ch);
        } else if source[pos..].starts_with("{{") {
            if let Some(end) = source[pos + 2..].find("}}") {
                let end = pos + 2 + end + 2;
                output.push_str(&source[pos..end]);
                while iter.peek().is_some_and(|(index, _)| *index < end) {
                    iter.next();
                }
            } else {
                output.push(ch);
            }
        } else if ch.is_whitespace() {
            continue;
        } else if ch == '{' || ch == '[' {
            output.push(ch);
            indent += 1;
            let closing = if ch == '{' { '}' } else { ']' };
            if iter
                .clone()
                .find(|(_, c)| !c.is_whitespace())
                .map(|(_, c)| c)
                != Some(closing)
            {
                output.push('\n');
                output.push_str(&"  ".repeat(indent));
            }
        } else if ch == '}' || ch == ']' {
            indent -= 1;
            let opening = if ch == '}' { '{' } else { '[' };
            if !output.ends_with(opening) {
                output.push('\n');
                output.push_str(&"  ".repeat(indent));
            }
            output.push(ch);
        } else if ch == ',' {
            output.push_str(",\n");
            output.push_str(&"  ".repeat(indent));
        } else if ch == ':' {
            output.push_str(": ");
        } else {
            output.push(ch);
        }
    }
    Ok(output)
}
/// 校验持久化协议和发布用例的结构、标识、名称及数量，不要求草稿主题或 JSON 已可发送。
pub fn validate_saved(r: &Value) -> Result<(), String> {
    match r.get("protocol").and_then(Value::as_str).unwrap_or("http") {
        "http" => {}
        "mqtt" => {
            config(r)?;
            // 旧版请求没有 publishCases，加载时由模型层补齐，持久化校验仍允许该格式。
            if let Some(value) = r["mqtt"].get("publishCases") {
                let cases: Vec<PublishCase> = serde_json::from_value(value.clone())
                    .map_err(|_| "MQTT 发布用例格式无效".to_string())?;
                if cases.is_empty() || cases.len() > MAX_PUBLISH_CASES {
                    return Err(format!("MQTT 发布用例须为 1–{MAX_PUBLISH_CASES} 条"));
                }
                let mut ids = std::collections::HashSet::new();
                let mut names = std::collections::HashSet::new();
                for case in &cases {
                    if case.id.trim().is_empty()
                        || !ids.insert(&case.id)
                        || case.name.trim().is_empty()
                        || !names.insert(case.name.trim())
                    {
                        return Err("MQTT 发布用例的标识或名称为空或重复".into());
                    }
                    if !["text", "json"].contains(&case.format.as_str()) || case.qos > 2 {
                        return Err("MQTT 发布用例的消息类型或 QoS 无效".into());
                    }
                    // 反序列化负责字段类型校验；保存草稿时允许主题或 JSON 尚未填写完整。
                    let _ = (
                        &case.topic,
                        &case.payload,
                        case.retain,
                        &case.content_type,
                        &case.response_topic,
                        case.expiry,
                    );
                }
            }
        }
        _ => return Err("尚不支持此请求协议".into()),
    }
    Ok(())
}
/// 一次 MQTT 操作的用途，决定发送前需要解析哪些配置字段。
pub enum Operation {
    /// 解析连接、认证、启用的遗嘱、订阅和 CONNECT 用户属性。
    Connect,
    /// 解析当前选中发布用例的主题、正文与发布属性。
    Publish,
    /// 解析指定下标的订阅行，供订阅或取消订阅使用。
    Subscribe(usize),
}
/// 按操作用途即时解析配置，复用 HTTP 的变量优先级；缺失变量或无效配置返回错误，不修改原草稿。
pub fn resolve(doc: &Snapshot, operation: Operation) -> Result<Config, String> {
    let input = model::input(doc)?;
    let (variables, _) = http::variables(&input).map_err(|e| e.message)?;
    let mut missing = vec![];
    let mut value = model::active(doc).ok_or("请先新建请求")?["mqtt"].clone();
    if matches!(operation, Operation::Publish) {
        if let Some(index) = model::mqtt_case_index(doc) {
            let case = value["publishCases"][index].clone();
            for key in PUBLISH_CASE_FIELDS {
                value[key] = case[key].clone();
            }
        }
    }
    /// 递归展开选中配置中的字符串变量，跳过对象的 id 和 description，并累积缺失项。
    fn walk(v: &mut Value, map: &HashMap<String, String>, missing: &mut Vec<String>) {
        match v {
            Value::String(s) => *s = http::expand(s, "MQTT 配置", map, missing),
            Value::Array(a) => {
                for v in a {
                    walk(v, map, missing)
                }
            }
            Value::Object(o) => {
                for (k, v) in o {
                    if k != "id" && k != "description" {
                        walk(v, map, missing)
                    }
                }
            }
            _ => {}
        }
    }
    match operation {
        Operation::Connect => {
            for key in ["address", "clientId"] {
                walk(&mut value[key], &variables, &mut missing);
            }
            if value["auth"] == true {
                for key in ["username", "password"] {
                    walk(&mut value[key], &variables, &mut missing);
                }
            }
            if value["willEnabled"] == true {
                for key in ["willTopic", "willPayload"] {
                    walk(&mut value[key], &variables, &mut missing);
                }
            }
            for key in ["topics", "properties"] {
                if let Some(rows) = value[key].as_array_mut() {
                    for row in rows.iter_mut().filter(|r| r["enabled"] == true) {
                        walk(row, &variables, &mut missing);
                    }
                }
            }
        }
        Operation::Publish => {
            for key in ["topic", "payload", "contentType", "responseTopic"] {
                walk(&mut value[key], &variables, &mut missing);
            }
        }
        Operation::Subscribe(i) => {
            if let Some(row) = value["topics"].get_mut(i) {
                walk(row, &variables, &mut missing);
            }
        }
    }
    if !missing.is_empty() {
        return Err(format!("未定义变量：{}", missing.join("、")));
    }
    serde_json::from_value(value).map_err(|_| "MQTT 配置格式不正确".into())
}
/// 将 0、1、2 转换为协议 QoS，其他数值返回中文校验错误。
fn qos(v: u8) -> Result<QoS, String> {
    match v {
        0 => Ok(QoS::AtMostOnce),
        1 => Ok(QoS::AtLeastOnce),
        2 => Ok(QoS::ExactlyOnce),
        _ => Err("QoS 只能为 0、1 或 2".into()),
    }
}
/// 检查 MQTT 字符串的 UTF-8 字节长度及禁止字符，错误中使用 label 标明字段。
fn mqtt_string(s: &str, label: &str) -> Result<(), String> {
    if s.len() > 65535
        || s.chars().any(|c| {
            c == '\0' || (0xfdd0..=0xfdef).contains(&(c as u32)) || c as u32 & 0xffff >= 0xfffe
        })
    {
        return Err(format!("{label}包含无效字符或超过 65535 字节"));
    }
    Ok(())
}
/// 校验非空主题及 MQTT 字符限制；filter 为 true 时允许合法通配符和共享订阅格式。
pub fn validate_topic(s: &str, filter: bool) -> Result<(), String> {
    mqtt_string(s, "主题")?;
    if s.is_empty() {
        return Err("主题不能为空".into());
    }
    let s = if filter && s.starts_with("$share/") {
        let (group, topic) = s[7..]
            .split_once('/')
            .ok_or("共享订阅格式应为 $share/组名/主题")?;
        if group.is_empty() || group.contains(['+', '#']) || topic.is_empty() {
            return Err("共享订阅组名或主题无效".into());
        }
        topic
    } else {
        s
    };
    let levels: Vec<_> = s.split('/').collect();
    for (i, level) in levels.iter().enumerate() {
        if level.contains(['+', '#'])
            && (!filter || (*level != "+" && !(*level == "#" && i + 1 == levels.len())))
        {
            return Err("订阅主题中 + 须占完整层级，# 只能位于末级；发布主题不能包含通配符".into());
        }
    }
    Ok(())
}
impl Config {
    /// 校验连接参数并生成 V5 全新会话选项，TLS 使用系统信任，接收报文上限为 1 MiB。
    fn options(&self) -> Result<MqttOptions, String> {
        let url = url::Url::parse(self.address.trim())
            .map_err(|_| "请输入 mqtt://主机:1883 或 mqtts://主机:8883")?;
        if !["mqtt", "mqtts"].contains(&url.scheme())
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || !["", "/"].contains(&url.path())
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(
                "连接地址仅支持 mqtt:// 或 mqtts://，认证请填写在认证页；不支持路径与查询参数"
                    .into(),
            );
        }
        if self.client_id.is_empty() {
            return Err("客户端标识不能为空".into());
        }
        mqtt_string(&self.client_id, "客户端标识")?;
        if !(5..=65535).contains(&self.keep_alive) || !(1..=300).contains(&self.timeout) {
            return Err("保活须为 5–65535 秒，连接超时须为 1–300 秒".into());
        }
        if self.topics.len() > 100 {
            return Err("单个连接最多保存 100 个订阅主题".into());
        }
        let mut filters = std::collections::HashSet::new();
        for t in self.topics.iter().filter(|t| t.enabled) {
            validate_topic(&t.topic, true)?;
            qos(t.qos)?;
            if !filters.insert(&t.topic) {
                return Err("启用的订阅主题重复".into());
            }
        }
        let host = url
            .host_str()
            .unwrap()
            .trim_start_matches('[')
            .trim_end_matches(']');
        let mut options = MqttOptions::new(
            self.client_id.clone(),
            host,
            url.port()
                .unwrap_or(if url.scheme() == "mqtts" { 8883 } else { 1883 }),
        );
        options.set_keep_alive(Duration::from_secs(self.keep_alive.into()));
        // 客户端不持久化 QoS 交付状态，因此始终使用全新会话，避免重启后双方状态不一致。
        options
            .set_clean_start(true)
            .set_session_expiry_interval(Some(0));
        options
            .set_connection_timeout(self.timeout)
            .set_max_packet_size(Some(1024 * 1024))
            .set_outgoing_inflight_upper_limit(16);
        if url.scheme() == "mqtts" {
            options.set_transport(Transport::tls_with_default_config());
        }
        if self.auth {
            mqtt_string(&self.username, "用户名")?;
            if self.password.len() > 65535 {
                return Err("密码超过 65535 字节".into());
            }
            options.set_credentials(&self.username, &self.password);
        }
        let props = self
            .properties
            .iter()
            .filter(|r| r.enabled && !r.key.is_empty())
            .map(|r| {
                mqtt_string(&r.key, "属性名称")?;
                mqtt_string(&r.value, "属性值")?;
                Ok((r.key.clone(), r.value.clone()))
            })
            .collect::<Result<Vec<_>, String>>()?;
        options.set_user_properties(props);
        if self.will_enabled {
            validate_topic(&self.will_topic, false)?;
            if self.will_payload.len() > 256 * 1024 {
                return Err("遗嘱消息不能超过 256 KiB".into());
            }
            options.set_last_will(LastWill::new(
                &self.will_topic,
                self.will_payload.as_bytes().to_vec(),
                qos(self.will_qos)?,
                self.will_retain,
                None,
            ));
        }
        Ok(options)
    }
    /// 校验发布主题、正文、QoS 和属性后尝试加入客户端队列；成功仅表示入队，尚未获得服务器确认。
    fn publish(&self, client: &AsyncClient) -> Result<(), String> {
        validate_topic(&self.topic, false)?;
        if self.payload.len() > 256 * 1024 {
            return Err("发布消息不能超过 256 KiB".into());
        }
        if self.format == "json" {
            serde_json::from_str::<Value>(&self.payload)
                .map_err(|e| format!("JSON 格式错误：第 {} 行，第 {} 列", e.line(), e.column()))?;
        }
        if !self.response_topic.is_empty() {
            validate_topic(&self.response_topic, false)?
        }
        mqtt_string(&self.content_type, "内容类型")?;
        let properties = PublishProperties {
            payload_format_indicator: Some(1),
            content_type: (!self.content_type.is_empty()).then(|| self.content_type.clone()),
            response_topic: (!self.response_topic.is_empty()).then(|| self.response_topic.clone()),
            message_expiry_interval: (self.expiry > 0).then_some(self.expiry),
            ..Default::default()
        };
        client
            .try_publish_with_properties(
                &self.topic,
                qos(self.qos)?,
                self.retain,
                self.payload.as_bytes().to_vec(),
                properties,
            )
            .map_err(|_| "发送队列已满或连接已关闭，请稍后重试".into())
    }
}
/// MQTT 会话的连接生命周期状态，独立于持久化配置。
#[derive(Clone, PartialEq)]
pub enum State {
    /// 正在建立连接并等待 CONNACK。
    Connecting,
    /// 已收到连接确认，可提交发布与订阅命令。
    Connected,
    /// 已请求关闭，正在尝试发送 DISCONNECT 并清理连接。
    Disconnecting,
    /// 连接已结束或失败，须由用户重新发起连接。
    Disconnected,
}
impl State {
    /// 返回连接状态对应的中文界面文案。
    pub fn label(&self) -> &'static str {
        match self {
            Self::Connecting => "正在连接",
            Self::Connected => "已连接",
            Self::Disconnecting => "正在断开",
            Self::Disconnected => "已断开",
        }
    }
}
/// 单条内存消息或生命周期事件，正文仅保留有界预览。
#[derive(Clone)]
pub struct Record {
    /// 由会话修订号生成的记录标识，用于列表选择和详情定位。
    pub id: u64,
    /// 记录产生时的本地日期时间，格式为 YYYY-MM-DD HH:MM:SS，不包含时区偏移。
    pub time: String,
    /// 记录类别，如连接、排队、发送、接收、确认、错误或断开。
    pub kind: String,
    /// 相关 MQTT 主题；不关联主题的生命周期事件为空字符串。
    pub topic: String,
    /// 展示用原始详情，通常含元数据和正文；超出预览容量时带截断提示。
    pub detail: String,
    /// 详情是否因超过预览容量被截断；为 true 时不再尝试 JSON 格式化。
    pub truncated: bool,
}
impl Record {
    /// 仅对未截断的发送或接收记录格式化完整 JSON 正文，不改写保存的原始详情。
    pub fn display_detail(&self) -> String {
        if self.truncated || !matches!(self.kind.as_str(), "发送" | "接收") {
            return self.detail.clone();
        }
        let Some((metadata, payload)) = self.detail.split_once('\n') else {
            return self.detail.clone();
        };
        let Ok(value) = serde_json::from_str::<Value>(payload) else {
            return self.detail.clone();
        };
        match serde_json::to_string_pretty(&value) {
            Ok(formatted) => format!("{metadata}\n{formatted}"),
            Err(_) => self.detail.clone(),
        }
    }
}
/// 连接任务与界面共享的内存状态，通过修订号触发界面刷新。
#[derive(Clone)]
pub struct Live {
    /// 当前连接生命周期状态。
    pub state: State,
    /// 最近最多 500 条事件记录，最新项位于队首。
    pub records: VecDeque<Record>,
    /// 订阅行标识到实际订阅标记及中文状态说明的映射，由确认报文更新。
    pub subscriptions: HashMap<String, (bool, String)>,
    /// 界面可观察的递增修订号，记录和连接状态变更时更新。
    pub revision: u64,
}
impl Live {
    /// 追加事件并递增修订号，按 UTF-8 边界限制详情为 8 KiB 预览，仅保留最近 500 条。
    fn log(&mut self, kind: &str, topic: &str, detail: &str) {
        self.revision += 1;
        let truncated = detail.len() > 8192;
        let detail = if truncated {
            let mut end = 8192;
            while !detail.is_char_boundary(end) {
                end -= 1
            }
            format!("{}\n…预览已截断（最多 8 KiB）", &detail[..end])
        } else {
            detail.to_owned()
        };
        self.records.push_front(Record {
            id: self.revision,
            time: format_message_time(&Local::now()),
            kind: kind.into(),
            topic: topic.into(),
            detail,
            truncated,
        });
        self.records.truncate(500);
    }
}
/// 使用传入日期时间自身的时区格式化到秒，保留跨日后的完整日期。
fn format_message_time<Tz: TimeZone>(time: &DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    time.format("%Y-%m-%d %H:%M:%S").to_string()
}
/// 界面提交给连接任务的操作，连接任务再交由协议客户端发送。
enum Command {
    /// 发布一条已按当前操作解析变量的配置快照。
    Publish(Config),
    /// 订阅操作，依次携带主题行和启用标记；false 表示取消订阅。
    Subscribe(Topic, bool),
}
/// 单个标签拥有的临时 MQTT 会话，释放时取消连接任务，不持久化消息或网络连接。
pub struct Session {
    /// 互斥保护的连接状态、订阅确认和有界消息记录。
    pub live: Arc<Mutex<Live>>,
    /// 提交发布及订阅操作的有界通道，避免界面等待网络处理。
    commands: tokio::sync::mpsc::Sender<Command>,
    /// 通知连接任务终止并尽力完成正常断开的取消信号。
    cancel: CancellationToken,
}
impl Drop for Session {
    /// 释放会话时触发取消，使关闭标签后后台连接能够退出。
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
impl Session {
    /// 克隆当前连接状态供界面一致读取。
    ///
    /// # Panics
    /// 共享状态互斥锁因其他线程恐慌而中毒时触发恐慌。
    pub fn snapshot(&self) -> Live {
        self.live.lock().unwrap().clone()
    }
    /// 判断连接是否尚未完全断开，包含连接中和断开中状态。
    ///
    /// # Panics
    /// 共享状态互斥锁中毒时触发恐慌。
    pub fn active(&self) -> bool {
        self.live.lock().unwrap().state != State::Disconnected
    }
    /// 标记正在断开并触发取消信号；已断开时不再修改状态。
    ///
    /// # Panics
    /// 共享状态互斥锁中毒时触发恐慌。
    pub fn disconnect(&self) {
        let mut s = self.live.lock().unwrap();
        if s.state != State::Disconnected {
            s.state = State::Disconnecting;
            s.revision += 1;
        }
        self.cancel.cancel();
    }
    /// 仅在已连接时提交发布配置快照，队列满或关闭时返回错误；消息校验和回执由后台处理。
    ///
    /// # Panics
    /// 共享状态互斥锁中毒时触发恐慌。
    pub fn publish(&self, c: Config) -> Result<(), String> {
        if self.live.lock().unwrap().state != State::Connected {
            return Err("请先连接 MQTT 服务器".into());
        }
        self.commands
            .try_send(Command::Publish(c))
            .map_err(|_| "操作队列已满或连接已断开".into())
    }
    /// 仅在已连接时提交主题操作；enabled 为 false 表示取消订阅，实际状态等待服务器确认。
    ///
    /// # Panics
    /// 共享状态互斥锁中毒时触发恐慌。
    pub fn subscribe(&self, t: Topic, enabled: bool) -> Result<(), String> {
        if self.live.lock().unwrap().state != State::Connected {
            return Err("请等待 MQTT 连接完成".into());
        }
        self.commands
            .try_send(Command::Subscribe(t, enabled))
            .map_err(|_| "操作队列已满或连接已断开".into())
    }
    /// 校验配置后启动有界连接任务并立即返回会话；连接超时或失败记录在 live 中，不自动重连。
    pub fn start(c: Config, runtime: &tokio::runtime::Runtime) -> Result<Self, String> {
        let options = c.options()?;
        let (client, mut events) = AsyncClient::new(options, 128);
        let live = Arc::new(Mutex::new(Live {
            state: State::Connecting,
            records: VecDeque::new(),
            subscriptions: HashMap::new(),
            revision: 0,
        }));
        live.lock()
            .unwrap()
            .log("连接", "", "正在建立 MQTT V5 连接…");
        let (commands, mut rx) = tokio::sync::mpsc::channel(64);
        let cancel = CancellationToken::new();
        let state = live.clone();
        let stop = cancel.clone();
        runtime.spawn(async move {
            let mut connected=false;
            let deadline=tokio::time::sleep(Duration::from_secs(c.timeout));tokio::pin!(deadline);
            let mut queued_sub:VecDeque<(Topic,bool)>=VecDeque::new();
            let mut pending_sub:HashMap<u16,(Topic,bool)>=HashMap::new();
            let mut queued_pub:VecDeque<Config>=VecDeque::new();
            let mut pending_pub:HashMap<u16,String>=HashMap::new();
            let mut acknowledged:HashMap<String,Topic>=HashMap::new();
            loop {
                tokio::select! {
                    biased;
                    _=stop.cancelled()=>{
                        if connected && client.try_disconnect().is_ok() {
                            let _=tokio::time::timeout(Duration::from_secs(2),async {
                                loop {match events.poll().await {Ok(Event::Outgoing(Outgoing::Disconnect))|Err(_)=>break,_=>{}}}
                            }).await;
                        }
                        state.lock().unwrap().log("断开","","连接已由客户端关闭");break;
                    }
                    _=&mut deadline,if !connected=>{state.lock().unwrap().log("错误","","连接超时，请检查地址、网络及服务器状态");break;}
                    command=rx.recv(),if connected=>{
                        match command {
                            Some(Command::Publish(p))=>match if queued_pub.len()+pending_pub.len()>=32 {Err("待确认消息过多，请等待服务器确认后重试".into())}else{p.publish(&client)}{Ok(())=>{state.lock().unwrap().log("排队",&p.topic,"消息已进入发送队列，等待网络发送");queued_pub.push_back(p)},Err(e)=>state.lock().unwrap().log("错误","",&e)},
                            Some(Command::Subscribe(mut t,on))=>{
                                if queued_sub.iter().chain(pending_sub.values()).any(|(p,_)|p.id==t.id || p.topic==t.topic){state.lock().unwrap().log("提示","","此主题操作尚未确认，请稍后重试");continue;}
                                if on && acknowledged.values().any(|a|a.topic==t.topic && a.id!=t.id){state.lock().unwrap().log("错误",&t.topic,"此主题已由另一行订阅");continue;}
                                if !on {if let Some(old)=acknowledged.get(&t.id){t=old.clone();}}
                                let result=validate_topic(&t.topic,true).and_then(|_|qos(t.qos)).and_then(|q|{
                                    if on{client.try_subscribe(&t.topic,q)}else{client.try_unsubscribe(&t.topic)}.map_err(|_|"订阅操作队列已满".to_owned())
                                });
                                match result {Ok(())=>{state.lock().unwrap().subscriptions.insert(t.id.clone(),(!on,if on{"正在订阅"}else{"正在取消订阅"}.into()));state.lock().unwrap().log("订阅",&t.topic,"已提交，等待服务器确认");queued_sub.push_back((t,on));},Err(e)=>state.lock().unwrap().log("错误",&t.topic,&e)}
                            },None=>break,
                        }
                    }
                    event=events.poll()=>{
                        match event {
                            Ok(Event::Incoming(Packet::ConnAck(_)))=>{
                                connected=true;state.lock().unwrap().state=State::Connected;
                                state.lock().unwrap().log("连接","","MQTT V5 连接成功；使用全新会话，断开后不保留服务器会话");
                                for t in c.topics.iter().filter(|t|t.enabled){
                                    if client.try_subscribe(&t.topic,qos(t.qos).unwrap()).is_ok(){queued_sub.push_back((t.clone(),true));state.lock().unwrap().subscriptions.insert(t.id.clone(),(false,"正在订阅".into()));}
                                }
                            }
                            Ok(Event::Outgoing(Outgoing::Subscribe(id)|Outgoing::Unsubscribe(id)))=>{if let Some(t)=queued_sub.pop_front(){pending_sub.insert(id,t);}}
                            Ok(Event::Incoming(Packet::SubAck(ack)))=>{if let Some((t,_))=pending_sub.remove(&ack.pkid){
                                let ok=ack.return_codes.iter().all(|v|matches!(v,SubscribeReasonCode::Success(_)));
                                let msg=if ok{"已订阅"}else{"订阅被服务器拒绝"};
                                state.lock().unwrap().subscriptions.insert(t.id.clone(),(ok,msg.into()));
                                if ok{acknowledged.insert(t.id.clone(),t.clone());}
                                state.lock().unwrap().log(if ok{"订阅"}else{"错误"},&t.topic,msg);
                            }}
                            Ok(Event::Incoming(Packet::UnsubAck(ack)))=>{if let Some((t,_))=pending_sub.remove(&ack.pkid){
                                let ok=ack.reasons.iter().all(|r|matches!(r,UnsubAckReason::Success | UnsubAckReason::NoSubscriptionExisted));
                                let msg=if ok{"已取消订阅"}else{"取消订阅被服务器拒绝"};
                                state.lock().unwrap().subscriptions.insert(t.id.clone(),(!ok,msg.into()));
                                if ok{acknowledged.remove(&t.id);}
                                state.lock().unwrap().log(if ok{"订阅"}else{"错误"},&t.topic,msg);
                            }}
                            Ok(Event::Outgoing(Outgoing::Publish(id)))=>{if let Some(p)=queued_pub.pop_front(){
                                state.lock().unwrap().log("发送",&p.topic,&format!("QoS {} · 保留 {} · {} 字节\n{}",p.qos,if p.retain{"是"}else{"否"},p.payload.len(),p.payload));
                                if id>0{pending_pub.insert(id,p.topic);}else{state.lock().unwrap().log("确认",&p.topic,"QoS 0 已写入连接，不要求服务器确认");}
                            }}
                            Ok(Event::Incoming(Packet::PubAck(ack)))=>{if let Some(topic)=pending_pub.remove(&ack.pkid){let ok=matches!(ack.reason,PubAckReason::Success|PubAckReason::NoMatchingSubscribers);state.lock().unwrap().log(if ok{"确认"}else{"错误"},&topic,if ok{"QoS 1 服务器已确认接收"}else{"服务器拒绝发布消息"});}}
                            Ok(Event::Incoming(Packet::PubRec(ack)))=>{if !matches!(ack.reason,PubRecReason::Success|PubRecReason::NoMatchingSubscribers){if let Some(topic)=pending_pub.remove(&ack.pkid){state.lock().unwrap().log("错误",&topic,"服务器拒绝 QoS 2 发布消息");}}}
                            Ok(Event::Incoming(Packet::PubComp(ack)))=>{if let Some(topic)=pending_pub.remove(&ack.pkid){let ok=ack.reason==PubCompReason::Success;state.lock().unwrap().log(if ok{"确认"}else{"错误"},&topic,if ok{"QoS 2 交付流程已完成"}else{"QoS 2 交付失败"});}}
                            Ok(Event::Incoming(Packet::Publish(p)))=>{
                                let topic=String::from_utf8_lossy(&p.topic);
                                let payload=match std::str::from_utf8(&p.payload){Ok(t)=>t.to_owned(),Err(_)=>{use base64::Engine;format!("二进制消息（Base64）：\n{}",base64::engine::general_purpose::STANDARD.encode(&p.payload[..p.payload.len().min(4096)]))}};
                                state.lock().unwrap().log("接收",&topic,&format!("QoS {} · 保留 {} · {} 字节\n{}",p.qos as u8,if p.retain{"是"}else{"否"},p.payload.len(),payload));
                            }
                            Ok(Event::Incoming(Packet::Disconnect(_)))=>{state.lock().unwrap().log("断开","","服务器主动断开连接，请检查认证、权限及客户端标识是否重复");break;}
                            Err(e)=>{state.lock().unwrap().log("错误","",connection_error(&e));break;}
                            _=>{}
                        }
                    }
                }
            }
            let mut s=state.lock().unwrap();s.state=State::Disconnected;s.subscriptions.clear();s.revision+=1;
        });
        Ok(Self {
            live,
            commands,
            cancel,
        })
    }
}

/// 使用本机 MQTT 报文服务验证会话生命周期、QoS、配置与持久化边界。
#[cfg(test)]
mod tests;

/// 将连接错误转换为不暴露认证凭据的中文诊断，覆盖认证、TLS、超时及协议失败。
fn connection_error(error: &rumqttc::v5::ConnectionError) -> &'static str {
    use rumqttc::v5::{ConnectionError as E, mqttbytes::v5::ConnectReturnCode as C};
    match error {
        E::ConnectionRefused(C::BadUserNamePassword) => {
            "认证失败：用户名或密码错误。请在认证页检查后重新连接。"
        }
        E::ConnectionRefused(C::NotAuthorized | C::Banned) => {
            "连接被拒绝：当前客户端没有访问权限或已被服务器禁止。"
        }
        E::ConnectionRefused(C::RefusedProtocolVersion | C::UnsupportedProtocolVersion) => {
            "服务器不支持 MQTT V5，当前客户端仅支持 V5。"
        }
        E::ConnectionRefused(C::BadClientId | C::ClientIdentifierNotValid) => {
            "连接被拒绝：客户端标识无效。"
        }
        E::ConnectionRefused(_) => "服务器拒绝连接，请检查服务器状态、配额与连接参数。",
        E::Tls(_) => "TLS 连接失败，请检查证书信任、有效期及服务器主机名。",
        E::Timeout(_) => "连接超时，请检查服务器地址及网络。",
        E::Io(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
            "服务器拒绝 TCP 连接，请检查主机、端口及服务是否启动。"
        }
        E::Io(_) => "网络连接中断或无法建立连接，请检查地址和网络后手动重连。",
        _ => "MQTT 协议连接中断，请检查服务器是否支持 V5、报文大小与保活设置后手动重连。",
    }
}
