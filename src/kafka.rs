//! Kafka 发布、消费与主题管理，使用独立工作线程隔离客户端阻塞调用，认证信息不进入日志。
//!
//! 作者：Andersen

use crate::{
    http,
    messaging::{self, Command, Outgoing, Session, Shared, Topic},
    model,
    storage::Snapshot,
};
use rdkafka::{
    ClientConfig, ClientContext,
    admin::{AdminClient, AdminOptions, NewTopic, TopicReplication},
    config::RDKafkaLogLevel,
    consumer::{BaseConsumer, Consumer, ConsumerContext},
    error::KafkaError,
    message::{Header, Headers, Message, OwnedHeaders},
    producer::{FutureProducer, FutureRecord, Producer, PurgeConfig},
    util::Timeout,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{future::Future, sync::Arc, time::Duration};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// 配置、真实协议发布消费和独立容器管理操作回归。
#[cfg(test)]
mod tests;

/// Kafka 持久化配置，消息编辑内容和连接配置分开解析变量。
#[derive(Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    /// 逗号分隔的 bootstrap 主机与端口，支持 IPv6 方括号。
    pub brokers: String,
    /// 客户端标识，便于服务器识别调试连接。
    pub client_id: String,
    /// 消费组标识，默认每份草稿独立生成，不自动提交消费位移。
    pub group_id: String,
    /// PLAINTEXT、SSL、SASL_PLAINTEXT 或 SASL_SSL。
    pub security: String,
    /// SASL 机制：PLAIN、SCRAM-SHA-256 或 SCRAM-SHA-512。
    pub mechanism: String,
    /// SASL 用户名模板，仅启用 SASL 时使用。
    pub username: String,
    /// SASL 密码模板，不输出日志。
    pub password: String,
    /// 可选 PEM CA 文件路径，留空使用客户端默认信任路径。
    pub ca_file: String,
    /// 可选客户端 PEM 证书路径，用于双向 TLS。
    pub cert_file: String,
    /// 客户端 PEM 私钥路径，须与证书一起设置。
    pub key_file: String,
    /// 加密私钥的口令，不输出日志。
    pub key_password: String,
    /// 连接及管理操作超时，单位秒，范围 1–60。
    pub timeout: u64,
    /// 没有已提交位移时从 earliest 或 latest 开始。
    pub offset: String,
    /// 逗号分隔的订阅主题草稿，连接后由用户显式启动消费。
    pub subscriptions: String,
    /// 发布消息的主题模板。
    pub topic: String,
    /// 消息键模板，空字符串表示不设置键。
    pub message_key: String,
    /// 发布分区，-1 表示自动选择。
    pub partition: i32,
    /// 消息格式，text、json 或 base64。
    pub format: String,
    /// 待发布的消息正文模板。
    pub payload: String,
    /// Kafka 消息头列表，支持重复名称和变量。
    pub headers: Vec<http::Row>,
    /// 待创建的主题名模板。
    pub new_topic: String,
    /// 创建主题的分区数量，允许 1–1024。
    pub partitions: i32,
    /// 创建主题的副本数量，允许 1–1000，实际值须不超过服务器数量。
    pub replication: i32,
}
impl Default for Config {
    /// 默认不认证、不自动消费，使用独立客户端和消费组标识。
    fn default() -> Self {
        Self {
            brokers: String::new(),
            client_id: "reqsucc".into(),
            group_id: format!("reqsucc-{}", model::id()),
            security: "PLAINTEXT".into(),
            mechanism: "PLAIN".into(),
            username: String::new(),
            password: String::new(),
            ca_file: String::new(),
            cert_file: String::new(),
            key_file: String::new(),
            key_password: String::new(),
            timeout: 10,
            offset: "latest".into(),
            subscriptions: String::new(),
            topic: String::new(),
            message_key: String::new(),
            partition: -1,
            format: "text".into(),
            payload: String::new(),
            headers: vec![],
            new_topic: String::new(),
            partitions: 1,
            replication: 1,
        }
    }
}

/// 校验配置结构但允许未完成的主题、地址和消息草稿。
pub fn validate_saved(value: &Value) -> Result<(), String> {
    let c: Config = serde_json::from_value(value.clone()).map_err(|_| "Kafka 配置格式无效")?;
    if !["PLAINTEXT", "SSL", "SASL_PLAINTEXT", "SASL_SSL"].contains(&c.security.as_str())
        || !["PLAIN", "SCRAM-SHA-256", "SCRAM-SHA-512"].contains(&c.mechanism.as_str())
        || !["earliest", "latest"].contains(&c.offset.as_str())
        || !["text", "json", "base64"].contains(&c.format.as_str())
        || c.headers.len() > 100
    {
        return Err("Kafka 安全协议、消息格式、消费位置或消息头配置无效".into());
    }
    Ok(())
}

/// 只展开连接实际使用的认证和证书字段，不因未填写消息或停用认证而阻断连接。
pub fn resolve_connection(doc: &Snapshot) -> Result<Config, String> {
    let raw = &model::active(doc).ok_or("请先新建请求")?["kafka"];
    let security = model::text(raw, "security");
    let mut fields = vec!["brokers", "clientId", "groupId"];
    if security.starts_with("SASL") {
        fields.extend(["username", "password"]);
    }
    if security.ends_with("SSL") {
        fields.extend(["caFile", "certFile", "keyFile", "keyPassword"]);
    }
    serde_json::from_value(messaging::resolve(doc, "kafka", &fields)?)
        .map_err(|_| "Kafka 配置格式无效".into())
}

/// 校验 Kafka 主题名称，拒绝空名称、保留名、非法字符及超过 249 字节的名称。
pub fn topic_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 249
        || name == "."
        || name == ".."
        || !name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
    {
        return Err("主题名称须为 1–249 个字母、数字、点、下划线或连字符，且不能为 . 或 ..".into());
    }
    Ok(())
}

/// 解析精确主题订阅列表并去重，不接受正则表达式或通配符。
pub fn subscription_names(value: &str) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    for name in value.split(',').map(str::trim) {
        topic_name(name)?;
        if !names.iter().any(|existing| existing == name) {
            names.push(name.to_owned());
        }
    }
    if names.len() > 100 {
        return Err("一次最多订阅 100 个主题".into());
    }
    Ok(names)
}

/// 解析发布消息及消息头并限制总大小，返回不会随草稿继续修改的快照。
pub fn outgoing(doc: &Snapshot) -> Result<Outgoing, String> {
    let c: Config = serde_json::from_value(messaging::resolve(
        doc,
        "kafka",
        &["topic", "messageKey", "payload", "headers"],
    )?)
    .map_err(|_| "Kafka 配置格式无效")?;
    topic_name(&c.topic)?;
    if c.partition < -1 {
        return Err("发布分区须为 -1（自动）或非负整数".into());
    }
    let payload = messaging::encode(&c.payload, &c.format)?;
    let mut headers = Vec::new();
    let mut length = payload.len() + c.message_key.len();
    for row in c
        .headers
        .iter()
        .filter(|row| row.enabled && !row.key.is_empty())
    {
        if row.key.contains('\0') {
            return Err("消息头名称不能包含空字符".into());
        }
        length += row.key.len() + row.value.len();
        headers.push((row.key.clone(), row.value.as_bytes().to_vec()));
    }
    if length > messaging::MAX_PAYLOAD || headers.len() > 100 {
        return Err("Kafka 消息、键和消息头总长度最多 256 KiB，消息头最多 100 项".into());
    }
    Ok(Outgoing {
        topic: c.topic,
        key: (!c.message_key.is_empty()).then(|| c.message_key.into_bytes()),
        payload,
        binary: c.format == "base64",
        partition: (c.partition >= 0).then_some(c.partition),
        headers,
    })
}

/// 仅展示错误码和类别，避免客户端错误字符串带出密码、证书口令或服务器认证详情。
fn error(error: KafkaError) -> String {
    error
        .rdkafka_error_code()
        .map(|code| format!("Kafka 操作失败：{code:?}"))
        .unwrap_or_else(|| "Kafka 客户端配置或操作失败，请检查地址、认证、证书及服务器权限".into())
}

/// 客户端回调上下文，隐藏原始客户端日志，仅记录去敏后的错误类别。
#[derive(Clone)]
struct Context {
    /// 当前会话的运行状态。
    state: Shared,
}
impl ClientContext for Context {
    /// 原始日志可能含认证握手详情，界面使用显式操作结果和去敏错误。
    fn log(&self, _: RDKafkaLogLevel, _: &str, _: &str) {}
    /// 对同一错误去重，避免失联重试持续刷满消息列表。
    fn error(&self, failure: KafkaError, _: &str) {
        let message = error(failure);
        messaging::with(&self.state, |live| {
            if live.error != message {
                live.error = message.clone();
                live.log("错误", "Kafka 连接异常", &message);
            }
        });
    }
}
impl ConsumerContext for Context {}

/// 构造客户端配置，保持 TLS 证书验证并禁止自动创建主题和自动提交消费位移。
fn client_config(c: &Config) -> Result<ClientConfig, String> {
    validate_saved(&serde_json::to_value(c).map_err(|_| "Kafka 配置无法序列化")?)?;
    if !(1..=60).contains(&c.timeout) {
        return Err("Kafka 超时须为 1–60 秒".into());
    }
    if c.client_id.is_empty()
        || c.group_id.is_empty()
        || c.client_id.len() > 255
        || c.group_id.len() > 255
    {
        return Err("客户端标识和消费组须为 1–255 字节".into());
    }
    let brokers: Vec<_> = c.brokers.split(',').map(str::trim).collect();
    if brokers.len() > 50 {
        return Err("最多配置 50 个 bootstrap 地址".into());
    }
    for broker in &brokers {
        let url = url::Url::parse(&format!("kafka://{broker}"))
            .map_err(|_| "Kafka 地址须为主机:端口，多个地址以逗号分隔")?;
        if url.host_str().is_none()
            || url.port().is_none_or(|port| port == 0)
            || !url.path().is_empty()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err("Kafka 地址须为主机:端口，不包含协议、认证或路径".into());
        }
    }
    let mut config = ClientConfig::new();
    config
        .set("bootstrap.servers", brokers.join(","))
        .set("client.id", &c.client_id)
        .set("security.protocol", &c.security)
        .set("socket.timeout.ms", (c.timeout * 1000).to_string())
        .set(
            "socket.connection.setup.timeout.ms",
            (c.timeout * 1000).to_string(),
        )
        .set("allow.auto.create.topics", "false")
        .set(
            "receive.message.max.bytes",
            (messaging::MAX_RECEIVE * 4).to_string(),
        );
    if c.security.starts_with("SASL") {
        if c.username.is_empty() || c.password.is_empty() {
            return Err("启用 SASL 后须填写用户名和密码".into());
        }
        config
            .set("sasl.mechanism", &c.mechanism)
            .set("sasl.username", &c.username)
            .set("sasl.password", &c.password);
    }
    if c.security.ends_with("SSL") {
        if c.cert_file.is_empty() != c.key_file.is_empty() {
            return Err("双向 TLS 须同时填写客户端证书与私钥".into());
        }
        for (field, path) in [
            ("ssl.ca.location", &c.ca_file),
            ("ssl.certificate.location", &c.cert_file),
            ("ssl.key.location", &c.key_file),
        ] {
            if !path.is_empty() {
                if !std::path::Path::new(path).is_file() {
                    return Err("TLS 证书或私钥文件不存在，请检查配置路径".into());
                }
                config.set(field, path);
            }
        }
        if !c.key_password.is_empty() {
            config.set("ssl.key.password", &c.key_password);
        }
        config
            .set("enable.ssl.certificate.verification", "true")
            .set("ssl.endpoint.identification.algorithm", "https");
    }
    Ok(config)
}

/// 在工作线程建立 Kafka 客户端，界面线程不进行元数据请求、证书读取或客户端析构。
pub fn start(c: Config, runtime: Arc<tokio::runtime::Runtime>) -> Result<Session, String> {
    let (session, rx, cancel) = Session::channel();
    let state = session.live.clone();
    std::thread::Builder::new()
        .name("reqsucc-kafka".into())
        .spawn(move || {
            let result = run(c, &runtime, state.clone(), rx, cancel);
            messaging::with(&state, |live| live.finish(result));
        })
        .map_err(|_| "无法启动 Kafka 工作线程")?;
    Ok(session)
}

/// 从服务器读取元数据，主题列表按名称排序并限制为 1000 项。
fn refresh(
    consumer: &BaseConsumer<Context>,
    state: &Shared,
    timeout: Duration,
) -> Result<(), String> {
    let metadata = consumer.fetch_metadata(None, timeout).map_err(error)?;
    if metadata.brokers().is_empty() {
        return Err("服务器未返回可用 Kafka 节点".into());
    }
    let mut topics: Vec<_> = metadata
        .topics()
        .iter()
        .take(1000)
        .map(|topic| Topic {
            name: topic.name().into(),
            partitions: topic.partitions().len(),
            error: topic
                .error()
                .map(|error| format!("{error:?}"))
                .unwrap_or_default(),
        })
        .collect();
    topics.sort_by(|a, b| a.name.cmp(&b.name));
    messaging::with(state, |live| {
        live.topics = topics;
        live.log(
            "主题",
            "主题列表已刷新",
            &format!(
                "服务器返回 {} 个主题，最多显示前 1000 个",
                metadata.topics().len()
            ),
        );
    });
    Ok(())
}

/// 在 Kafka 工作线程等待异步操作，取消不能撤回已提交给服务器的发布或管理请求。
fn wait<F, T>(
    runtime: &tokio::runtime::Runtime,
    cancel: &CancellationToken,
    timeout: Duration,
    future: F,
) -> Result<T, String>
where
    F: Future<Output = T>,
{
    runtime.block_on(async { tokio::select! {
        _ = cancel.cancelled() => Err("操作已取消；已提交服务器的消息或主题操作可能仍已执行，请刷新确认".into()),
        result = tokio::time::timeout(timeout + Duration::from_secs(1), future) => result.map_err(|_| "Kafka 操作超时；请确认服务器最终结果后重试".into()),
    } })
}

/// 串行处理显式命令并持续轮询消费事件，断开时退组、清理未完成发送并释放客户端。
fn run(
    c: Config,
    runtime: &tokio::runtime::Runtime,
    state: Shared,
    mut rx: mpsc::Receiver<Command>,
    cancel: CancellationToken,
) -> Result<(), String> {
    let config = client_config(&c)?;
    if cancel.is_cancelled() {
        return Ok(());
    }
    let context = Context {
        state: state.clone(),
    };
    let timeout = Duration::from_secs(c.timeout);
    let mut consumer_config = config.clone();
    consumer_config
        .set("group.id", &c.group_id)
        .set("auto.offset.reset", &c.offset)
        .set("enable.auto.commit", "false")
        .set("enable.auto.offset.store", "false")
        // 同时限制原生预取队列，避免界面记录有界但后台仍无限积压。
        .set("queued.max.messages.kbytes", "4096")
        .set("queued.min.messages", "1000")
        .set(
            "fetch.message.max.bytes",
            messaging::MAX_RECEIVE.to_string(),
        )
        .set("fetch.max.bytes", (messaging::MAX_RECEIVE * 2).to_string());
    let consumer: BaseConsumer<Context> = consumer_config
        .create_with_context(context.clone())
        .map_err(error)?;
    let mut producer_config = config.clone();
    producer_config
        .set("message.timeout.ms", (c.timeout * 1000).to_string())
        .set("enable.idempotence", "true")
        .set("acks", "all")
        .set(
            "message.max.bytes",
            (messaging::MAX_PAYLOAD + 8192).to_string(),
        );
    let producer: FutureProducer<Context> = producer_config
        .create_with_context(context.clone())
        .map_err(error)?;
    let admin: AdminClient<Context> = config.create_with_context(context).map_err(error)?;
    refresh(&consumer, &state, timeout)?;
    if cancel.is_cancelled() {
        return Ok(());
    }
    messaging::with(&state, |live| {
        live.connected();
        live.error.clear();
        live.log(
            "连接",
            "Kafka 集群已连接",
            "使用独立消费组配置；消费位移不会自动提交。请显式订阅主题开始消费。",
        );
    });
    while !cancel.is_cancelled() {
        if let Ok(command) = rx.try_recv() {
            let result = execute(
                command, &consumer, &producer, &admin, runtime, &cancel, &state, timeout,
            );
            messaging::with(&state, |live| live.complete(result));
        }
        if cancel.is_cancelled() {
            break;
        }
        if let Some(event) = consumer.poll(Duration::from_millis(50)) {
            match event {
                Ok(message) => {
                    let mut detail = format!(
                        "分区：{} · 位移：{}\n消息时间：{}\n",
                        message.partition(),
                        message.offset(),
                        match message.timestamp() {
                            rdkafka::message::Timestamp::NotAvailable => "服务器未提供".into(),
                            rdkafka::message::Timestamp::CreateTime(value) =>
                                format!("{}（生产时间）", messaging::format_timestamp(value)),
                            rdkafka::message::Timestamp::LogAppendTime(value) =>
                                format!("{}（服务器写入时间）", messaging::format_timestamp(value)),
                        }
                    );
                    if let Some(key) = message.key() {
                        detail
                            .push_str(&format!("键：{}\n", messaging::payload_preview(key, false)));
                    }
                    if let Some(headers) = message.headers() {
                        for header in headers.iter().take(100) {
                            if detail.len() >= messaging::MAX_PREVIEW / 2 {
                                break;
                            }
                            detail.push_str(&format!(
                                "头 {}：{}\n",
                                messaging::preview(header.key),
                                header
                                    .value
                                    .map(|value| messaging::payload_preview(value, false))
                                    .unwrap_or_else(|| "null".into())
                            ));
                        }
                    }
                    detail.push_str("\n消息：\n");
                    detail.push_str(
                        &message
                            .payload()
                            .map(|bytes| messaging::payload_preview(bytes, false))
                            .unwrap_or_else(|| "null（墓碑消息）".into()),
                    );
                    messaging::with(&state, |live| live.log("接收", message.topic(), &detail));
                }
                Err(failure) => {
                    let message = error(failure);
                    messaging::with(&state, |live| {
                        if live.error != message {
                            live.error = message.clone();
                            live.log("错误", "消费失败", &message);
                        }
                    });
                }
            }
        }
        if let Ok(assignment) = consumer.assignment() {
            messaging::with(&state, |live| {
                if live.assigned != assignment.count() {
                    live.assigned = assignment.count();
                    live.log(
                        "订阅",
                        "分区分配已更新",
                        &format!("当前分配 {} 个分区", assignment.count()),
                    );
                }
            });
        }
    }
    consumer.unsubscribe();
    producer.purge(PurgeConfig::default().queue().inflight());
    Ok(())
}

/// 执行发布、订阅或主题管理，只有服务器确认发布和管理结果后才记录成功。
fn execute(
    command: Command,
    consumer: &BaseConsumer<Context>,
    producer: &FutureProducer<Context>,
    admin: &AdminClient<Context>,
    runtime: &tokio::runtime::Runtime,
    cancel: &CancellationToken,
    state: &Shared,
    timeout: Duration,
) -> Result<(), String> {
    match command {
        Command::Send(outgoing) => {
            topic_name(&outgoing.topic)?;
            let mut headers = OwnedHeaders::new();
            for (key, value) in &outgoing.headers {
                headers = headers.insert(Header {
                    key,
                    value: Some(value.as_slice()),
                });
            }
            let mut record: FutureRecord<'_, [u8], [u8]> = FutureRecord::to(&outgoing.topic)
                .payload(outgoing.payload.as_slice())
                .headers(headers);
            if let Some(key) = &outgoing.key {
                record = record.key(key.as_slice());
            }
            if let Some(partition) = outgoing.partition {
                record = record.partition(partition);
            }
            let delivery = wait(
                runtime,
                cancel,
                timeout,
                producer.send(record, Timeout::After(timeout)),
            )?
            .map_err(|(failure, _)| error(failure))?;
            messaging::with(state, |live| {
                live.log(
                    "发送成功",
                    &outgoing.topic,
                    &format!(
                        "服务器确认：分区 {} · 位移 {}\n{}",
                        delivery.partition,
                        delivery.offset,
                        messaging::payload_preview(&outgoing.payload, outgoing.binary)
                    ),
                )
            });
        }
        Command::Subscribe(names) => {
            for name in &names {
                topic_name(name)?;
            }
            if names.is_empty() {
                consumer.unsubscribe();
            } else {
                consumer
                    .subscribe(&names.iter().map(String::as_str).collect::<Vec<_>>())
                    .map_err(error)?;
            }
            messaging::with(state, |live| {
                live.subscriptions = names.clone();
                live.assigned = 0;
                live.log(
                    "订阅",
                    if names.is_empty() {
                        "已取消订阅"
                    } else {
                        "订阅已提交，等待分区分配"
                    },
                    &names.join(", "),
                );
            });
        }
        Command::Refresh => refresh(consumer, state, timeout)?,
        Command::Create(name, partitions, replication) => {
            topic_name(&name)?;
            if !(1..=1024).contains(&partitions) || !(1..=1000).contains(&replication) {
                return Err("分区数量须为 1–1024，副本数量须为 1–1000".into());
            }
            let options = AdminOptions::new()
                .request_timeout(Some(timeout))
                .operation_timeout(Some(timeout));
            let results = wait(
                runtime,
                cancel,
                timeout,
                admin.create_topics(
                    &[NewTopic::new(
                        &name,
                        partitions,
                        TopicReplication::Fixed(replication),
                    )],
                    &options,
                ),
            )?
            .map_err(error)?;
            for result in results {
                result.map_err(|(_, code)| format!("创建主题失败：{code:?}"))?;
            }
            messaging::with(state, |live| {
                live.log("主题", "服务器已确认创建主题", &name)
            });
            refresh(consumer, state, timeout)?;
        }
        Command::Delete(name) => {
            topic_name(&name)?;
            let options = AdminOptions::new()
                .request_timeout(Some(timeout))
                .operation_timeout(Some(timeout));
            let results = wait(
                runtime,
                cancel,
                timeout,
                admin.delete_topics(&[&name], &options),
            )?
            .map_err(error)?;
            for result in results {
                result.map_err(|(_, code)| format!("删除主题失败：{code:?}"))?;
            }
            messaging::with(state, |live| {
                live.log("主题", "服务器已确认删除主题", &name)
            });
            refresh(consumer, state, timeout)?;
        }
        Command::Ping => return Err("Kafka 不支持 WebSocket Ping 操作".into()),
    }
    Ok(())
}
