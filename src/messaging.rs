//! WebSocket 与 Kafka 共用的临时会话、操作队列和有界消息记录，不保存连接或消费记录。
//!
//! 作者：Andersen

use crate::{http, model, storage::Snapshot};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::Value;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// 会话生命周期、变量解析及存储兼容回归。
#[cfg(test)]
mod tests;

/// 单条发送消息的字节上限，避免一次操作占用过多内存。
pub const MAX_PAYLOAD: usize = 256 * 1024;
/// 单条接收消息允许的字节上限。
pub const MAX_RECEIVE: usize = 1024 * 1024;
/// 消息详情最多保留的 UTF-8 字节数，超出部分明确标记为截断。
pub const MAX_PREVIEW: usize = 8192;
/// 每个会话最多保留的记录数，旧记录按顺序淘汰。
pub const MAX_RECORDS: usize = 500;

/// 临时连接生命周期，连接失败也最终进入断开状态并保留错误记录。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// 正在建立连接或验证 Kafka 集群元数据。
    Connecting,
    /// 可以提交消息和主题操作。
    Connected,
    /// 已取消任务，等待有限时长的网络操作结束。
    Disconnecting,
    /// 后台连接已释放。
    Disconnected,
}
impl Phase {
    /// 返回界面展示的连接状态名称。
    pub fn label(self) -> &'static str {
        match self {
            Self::Connecting => "正在连接",
            Self::Connected => "已连接",
            Self::Disconnecting => "正在断开",
            Self::Disconnected => "已断开",
        }
    }
}

/// 消息记录，正文和附加信息均限制预览长度；标识在清空列表后仍不重复。
#[derive(Clone)]
pub struct Record {
    /// 会话内递增的记录标识。
    pub id: u64,
    /// 本机接收或发送确认时的毫秒时间戳。
    pub timestamp: i64,
    /// 记录类别，例如接收、发送成功、错误或主题管理。
    pub kind: String,
    /// 主题、帧类型或操作摘要。
    pub summary: String,
    /// 有界详情，二进制消息使用 Base64 展示。
    pub detail: String,
    /// 完整文本正文在详情中的 UTF-8 字节起点；二进制、截断或非消息记录不设置。
    payload_start: Option<usize>,
}

impl Record {
    /// 仅排版完整 JSON 正文，保留元数据、重复键和数值写法，复制仍使用原始详情。
    pub fn display_detail(&self) -> String {
        let Some(start) = self.payload_start else {
            return self.detail.clone();
        };
        let payload = &self.detail[start..];
        if serde_json::from_str::<serde::de::IgnoredAny>(payload).is_err() {
            return self.detail.clone();
        }
        crate::mqtt::format_json_template(payload)
            .map(|formatted| preview(&format!("{}{formatted}", &self.detail[..start])))
            .unwrap_or_else(|_| self.detail.clone())
    }
}

/// 将毫秒时间戳转换为本机完整日期、时间、毫秒及 UTC 偏移；非法值不伪装成当前时间。
pub fn format_timestamp(timestamp: i64) -> String {
    chrono::DateTime::from_timestamp_millis(timestamp)
        .map(|time| format_time(&time.with_timezone(&chrono::Local)))
        .unwrap_or_else(|| "无效时间戳".into())
}

/// 保留传入时区及跨日后的日期，使列表、详情和复制内容采用相同时间格式。
fn format_time<Tz: chrono::TimeZone>(time: &chrono::DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    time.format("%Y-%m-%d %H:%M:%S%.3f %:z").to_string()
}

/// Kafka 主题的只读元数据，不包含消息内容。
#[derive(Clone)]
pub struct Topic {
    /// 集群返回的完整主题名。
    pub name: String,
    /// 元数据中的分区数量。
    pub partitions: usize,
    /// 主题级错误，没有错误时为空。
    pub error: String,
}

/// 共享运行状态；仅后台更新网络结果，界面按修订号刷新。
#[derive(Clone)]
pub struct Live {
    /// 当前连接阶段。
    pub phase: Phase,
    /// 是否有尚未完成的显式操作，避免重复发送和管理操作堆积。
    pub busy: bool,
    /// 最新错误提示，不包含连接密码或请求头值。
    pub error: String,
    /// 配置或日志变化后的递增修订号。
    pub revision: u64,
    /// 按时间排列的最近记录，Arc 避免每次渲染复制正文。
    pub records: VecDeque<Arc<Record>>,
    /// Kafka 最近成功加载的主题列表，最多 1000 项。
    pub topics: Vec<Topic>,
    /// Kafka 当前已提交的主题订阅列表，实际分配情况另行展示。
    pub subscriptions: Vec<String>,
    /// Kafka 消费组实际分配的分区数；0 表示尚未分配或没有订阅。
    pub assigned: usize,
    /// 下一条记录的递增序号。
    sequence: u64,
}
impl Live {
    /// 创建连接中的空状态。
    fn new() -> Self {
        Self {
            phase: Phase::Connecting,
            busy: false,
            error: String::new(),
            revision: 0,
            records: VecDeque::new(),
            topics: vec![],
            subscriptions: vec![],
            assigned: 0,
            sequence: 0,
        }
    }
    /// 添加有界记录并通知界面，最多保留最近 500 条。
    pub fn log(&mut self, kind: &str, summary: &str, detail: &str) {
        self.append(kind, summary, detail, None);
    }
    /// 分开记录元数据和消息正文，仅完整 UTF-8 文本允许在详情中自动格式化 JSON。
    pub fn log_message(
        &mut self,
        kind: &str,
        summary: &str,
        metadata: &str,
        payload: &[u8],
        binary: bool,
    ) {
        let complete_text = !binary
            && metadata.len() + payload.len() <= MAX_PREVIEW
            && std::str::from_utf8(payload).is_ok();
        let detail = format!("{metadata}{}", payload_preview(payload, binary));
        self.append(
            kind,
            summary,
            &detail,
            complete_text.then_some(metadata.len()),
        );
    }
    /// 统一写入有界记录，正文边界仅由已校验的消息入口提供。
    fn append(&mut self, kind: &str, summary: &str, detail: &str, payload_start: Option<usize>) {
        self.sequence += 1;
        self.records.push_back(Arc::new(Record {
            id: self.sequence,
            timestamp: chrono::Local::now().timestamp_millis(),
            kind: kind.into(),
            summary: preview(summary),
            detail: preview(detail),
            payload_start,
        }));
        while self.records.len() > MAX_RECORDS {
            self.records.pop_front();
        }
        self.revision += 1;
    }
    /// 更新阶段与修订号；已开始断开时不允许迟到的连接结果恢复为已连接。
    pub fn connected(&mut self) {
        if self.phase == Phase::Connecting {
            self.phase = Phase::Connected;
            self.revision += 1;
        }
    }
    /// 结束显式操作并记录错误；只更新错误状态，不将失败当作成功。
    pub fn complete(&mut self, result: Result<(), String>) {
        self.busy = false;
        self.error = result.err().unwrap_or_default();
        if !self.error.is_empty() {
            let error = self.error.clone();
            self.log("错误", "操作失败", &error);
        }
        self.revision += 1;
    }
    /// 结束后台会话并保留最后的错误和消息，清除实际订阅分配状态。
    pub fn finish(&mut self, result: Result<(), String>) {
        self.complete(result);
        self.phase = Phase::Disconnected;
        self.subscriptions.clear();
        self.assigned = 0;
        self.log(
            "连接",
            "连接已关闭",
            "网络资源已释放；重启应用不会自动连接。",
        );
    }
}

/// 共享状态句柄，仅在短临界区中修改，禁止持锁等待网络。
pub type Shared = Arc<Mutex<Live>>;

/// 访问共享状态；后台异常导致锁中毒时仍允许界面读取并释放连接。
pub fn with<T>(state: &Shared, f: impl FnOnce(&mut Live) -> T) -> T {
    f(&mut state.lock().unwrap_or_else(|error| error.into_inner()))
}

/// 界面提交的消息快照，连接后修改编辑器不会改变已经排队的内容。
pub struct Outgoing {
    /// Kafka 主题；WebSocket 使用空字符串。
    pub topic: String,
    /// Kafka 消息键，None 表示未设置。
    pub key: Option<Vec<u8>>,
    /// 待发送的原始字节，最多 256 KiB。
    pub payload: Vec<u8>,
    /// WebSocket 是否发送二进制帧。
    pub binary: bool,
    /// Kafka 指定分区，None 表示交由客户端选择。
    pub partition: Option<i32>,
    /// Kafka 消息头，名称与值按原顺序发送。
    pub headers: Vec<(String, Vec<u8>)>,
}

/// 连接任务支持的操作；只有所属协议处理相应命令。
pub enum Command {
    /// 发送一份已校验的消息快照。
    Send(Outgoing),
    /// WebSocket 发送 Ping 帧。
    Ping,
    /// 替换 Kafka 订阅列表，空列表表示取消全部订阅。
    Subscribe(Vec<String>),
    /// 重新拉取 Kafka 主题及分区元数据。
    Refresh,
    /// 创建 Kafka 主题，参数依次为名称、分区数量和副本数。
    Create(String, i32, i32),
    /// 删除指定 Kafka 主题，必须在界面完成目标确认。
    Delete(String),
}

/// 每个请求标签独占的临时会话，丢弃时取消后台任务。
pub struct Session {
    /// 每次连接的唯一标识，用于阻止旧确认窗口操作新连接。
    pub id: String,
    /// 会话状态和记录。
    pub live: Shared,
    /// 有界命令通道，界面发送不会阻塞。
    commands: mpsc::Sender<Command>,
    /// 连接取消信号。
    cancel: CancellationToken,
}
impl Session {
    /// 创建会话与后台接收端；调用方须启动任务并最终调用 Live::finish。
    pub fn channel() -> (Self, mpsc::Receiver<Command>, CancellationToken) {
        let (commands, rx) = mpsc::channel(8);
        let cancel = CancellationToken::new();
        (
            Self {
                id: model::id(),
                live: Arc::new(Mutex::new(Live::new())),
                commands,
                cancel: cancel.clone(),
            },
            rx,
            cancel,
        )
    }
    /// 返回一致的运行状态快照。
    pub fn snapshot(&self) -> Live {
        with(&self.live, |live| live.clone())
    }
    /// 判断会话是否还占有网络资源。
    pub fn active(&self) -> bool {
        with(&self.live, |live| live.phase != Phase::Disconnected)
    }
    /// 仅在已连接且无待处理操作时提交命令，排队失败恢复 busy 状态。
    pub fn submit(&self, command: Command) -> Result<(), String> {
        with(&self.live, |live| {
            if live.phase != Phase::Connected {
                return Err("请先连接服务器".into());
            }
            if live.busy {
                return Err("上一个操作尚未完成，请稍候".into());
            }
            self.commands
                .try_send(command)
                .map_err(|_| "操作队列已关闭或已满".to_owned())?;
            live.busy = true;
            live.revision += 1;
            Ok(())
        })
    }
    /// 请求后台断开，界面立即显示断开中，后台操作受配置超时约束。
    pub fn disconnect(&self) {
        with(&self.live, |live| {
            if live.phase != Phase::Disconnected {
                live.phase = Phase::Disconnecting;
                live.revision += 1;
            }
        });
        self.cancel.cancel();
    }
    /// 清空显示记录但保留连接、主题元数据与递增标识。
    pub fn clear(&self) {
        with(&self.live, |live| {
            live.records.clear();
            live.revision += 1;
        });
    }
}
impl Drop for Session {
    /// 关闭标签或退出时取消后台任务，网络清理由所属任务完成。
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// 截断 UTF-8 详情并保留可见的截断标记，不拆分字符。
pub fn preview(text: &str) -> String {
    if text.len() <= MAX_PREVIEW {
        return text.into();
    }
    let mut end = MAX_PREVIEW;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n…（内容已截断，原始长度 {} 字节）",
        &text[..end],
        text.len()
    )
}

/// 将消息转换为有界展示文本；非 UTF-8 或明确二进制时标记 Base64。
pub fn payload_preview(bytes: &[u8], binary: bool) -> String {
    if !binary {
        if let Ok(text) = std::str::from_utf8(bytes) {
            return preview(text);
        }
    }
    let content = STANDARD.encode(&bytes[..bytes.len().min(MAX_PREVIEW / 2)]);
    format!(
        "Base64（{} 字节{}）：\n{content}",
        bytes.len(),
        if bytes.len() > MAX_PREVIEW / 2 {
            "，预览已截断"
        } else {
            ""
        }
    )
}

/// 按字段用途展开变量，未选择的认证或消息字段不会阻止连接；不修改原草稿。
pub fn resolve(doc: &Snapshot, protocol: &str, fields: &[&str]) -> Result<Value, String> {
    let input = model::input(doc)?;
    let (variables, _) = http::variables(&input).map_err(|error| error.message)?;
    let mut value = model::active(doc).ok_or("请先新建请求")?[protocol].clone();
    let mut missing = Vec::new();
    for field in fields {
        if let Some(text) = value[*field].as_str() {
            value[*field] = Value::String(http::expand(
                text,
                "连接或消息配置",
                &variables,
                &mut missing,
            ));
        } else if let Some(rows) = value[*field].as_array_mut() {
            for row in rows
                .iter_mut()
                .filter(|row| row["enabled"].as_bool().unwrap_or(true))
            {
                for key in ["key", "value"] {
                    if let Some(text) = row[key].as_str() {
                        row[key] = Value::String(http::expand(
                            text,
                            "消息头配置",
                            &variables,
                            &mut missing,
                        ));
                    }
                }
            }
        }
    }
    if !missing.is_empty() {
        return Err(format!("未定义变量：{}", missing.join("、")));
    }
    Ok(value)
}

/// 按 text、json 或 base64 编码消息，校验 JSON 并限制实际发送字节数。
pub fn encode(payload: &str, format: &str) -> Result<Vec<u8>, String> {
    if payload.len() > MAX_PAYLOAD * 2 {
        return Err("消息超过 256 KiB 限制".into());
    }
    let bytes = match format {
        "text" => payload.as_bytes().to_vec(),
        "json" => {
            serde_json::from_str::<Value>(payload).map_err(|_| "消息不是有效 JSON")?;
            payload.as_bytes().to_vec()
        }
        "base64" => STANDARD
            .decode(payload)
            .map_err(|_| "二进制消息需要有效 Base64 编码")?,
        _ => return Err("不支持的消息格式".into()),
    };
    if bytes.len() > MAX_PAYLOAD {
        return Err("消息超过 256 KiB 限制".into());
    }
    Ok(bytes)
}
