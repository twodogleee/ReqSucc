//! WebSocket 握手、消息收发与关闭，支持 ws/wss、自定义请求头和子协议。
//!
//! 作者：Andersen

use crate::{
    http,
    messaging::{self, Command, Outgoing, Session, Shared},
    storage::Snapshot,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{
        self, Message,
        client::IntoClientRequest,
        http::{HeaderName, HeaderValue, Request},
        protocol::WebSocketConfig,
    },
};
use tokio_util::sync::CancellationToken;

/// 本机握手、帧收发、超时与取消回归检查。
#[cfg(test)]
mod tests;

/// 可持久化的 WebSocket 配置，不包含网络会话或收到的消息。
#[derive(Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    /// ws:// 或 wss:// 地址模板，支持路径和查询参数。
    pub address: String,
    /// 握手请求头，启用项发送，支持变量及敏感值遮蔽。
    pub headers: Vec<http::Row>,
    /// 逗号分隔的候选子协议，握手后显示服务器选中的协议。
    pub subprotocols: String,
    /// 连接和发送操作超时，单位秒，范围 1–60。
    pub timeout: u64,
    /// 消息编辑格式，text、json 或 base64。
    pub format: String,
    /// 尚未展开变量的消息正文。
    pub payload: String,
}
impl Default for Config {
    /// 创建尚未填写地址的文本消息草稿。
    fn default() -> Self {
        Self {
            address: String::new(),
            headers: vec![],
            subprotocols: String::new(),
            timeout: 10,
            format: "text".into(),
            payload: String::new(),
        }
    }
}

/// 校验保存结构但允许未填写的地址和未完成的 JSON 草稿。
pub fn validate_saved(value: &Value) -> Result<(), String> {
    let c: Config = serde_json::from_value(value.clone()).map_err(|_| "WebSocket 配置格式无效")?;
    if c.headers.len() > 100 || !["text", "json", "base64"].contains(&c.format.as_str()) {
        return Err("WebSocket 消息格式无效或请求头超过 100 项".into());
    }
    Ok(())
}

/// 仅解析连接字段，发送正文中的未定义变量不影响握手。
pub fn resolve_connection(doc: &Snapshot) -> Result<Config, String> {
    serde_json::from_value(messaging::resolve(
        doc,
        "websocket",
        &["address", "headers", "subprotocols"],
    )?)
    .map_err(|_| "WebSocket 配置格式无效".into())
}

/// 解析当前消息并转换成独立发送快照，Base64 用二进制帧，其他格式用文本帧。
pub fn outgoing(doc: &Snapshot) -> Result<Outgoing, String> {
    let value = messaging::resolve(doc, "websocket", &["payload"])?;
    let c: Config = serde_json::from_value(value).map_err(|_| "WebSocket 配置格式无效")?;
    Ok(Outgoing {
        topic: String::new(),
        key: None,
        payload: messaging::encode(&c.payload, &c.format)?,
        binary: c.format == "base64",
        partition: None,
        headers: vec![],
    })
}

/// 构造握手请求并保护协议必需头，认证通过普通 Authorization 请求头配置。
fn handshake(c: &Config) -> Result<Request<()>, String> {
    if !(1..=60).contains(&c.timeout) {
        return Err("WebSocket 超时须为 1–60 秒".into());
    }
    let url = url::Url::parse(&c.address).map_err(|_| "WebSocket 地址格式不正确")?;
    if !["ws", "wss"].contains(&url.scheme())
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err("地址须使用 ws:// 或 wss://，认证请使用请求头，地址不能包含片段".into());
    }
    let mut request = c
        .address
        .as_str()
        .into_client_request()
        .map_err(|_| "无法构造 WebSocket 握手请求")?;
    if c.headers.len() > 100 {
        return Err("请求头最多 100 项".into());
    }
    let mut total = 0;
    for row in c.headers.iter().filter(|row| row.enabled) {
        if row.key.trim().is_empty() {
            continue;
        }
        let name = HeaderName::from_bytes(row.key.as_bytes()).map_err(|_| "请求头名称无效")?;
        if [
            "host",
            "connection",
            "upgrade",
            "sec-websocket-key",
            "sec-websocket-version",
            "sec-websocket-protocol",
            "sec-websocket-extensions",
            "content-length",
            "transfer-encoding",
        ]
        .contains(&name.as_str())
        {
            return Err("握手协议头由客户端维护；子协议请在专用字段中设置".into());
        }
        let value = HeaderValue::from_str(&row.value).map_err(|_| "请求头值包含非法字符")?;
        total += row.key.len() + row.value.len();
        if total > 32 * 1024 {
            return Err("请求头总长度超过 32 KiB".into());
        }
        request.headers_mut().append(name, value);
    }
    if !c.subprotocols.trim().is_empty() {
        let protocols: Vec<_> = c.subprotocols.split(',').map(str::trim).collect();
        if protocols
            .iter()
            .any(|p| p.is_empty() || HeaderName::from_bytes(p.as_bytes()).is_err())
        {
            return Err("子协议须为逗号分隔的合法协议名称".into());
        }
        let value = HeaderValue::from_str(&protocols.join(", ")).map_err(|_| "子协议格式无效")?;
        request
            .headers_mut()
            .insert("sec-websocket-protocol", value);
    }
    Ok(request)
}

/// 返回不含请求地址、认证头或服务器响应正文的中文错误说明。
fn error(error: tungstenite::Error) -> String {
    match error {
        tungstenite::Error::Http(response) => {
            format!("WebSocket 握手被拒绝（HTTP {}）", response.status())
        }
        tungstenite::Error::Tls(_) => "WebSocket TLS 握手失败，请检查证书与系统信任设置".into(),
        tungstenite::Error::Capacity(_) => "WebSocket 消息超过 1 MiB 接收限制或缓冲区已满".into(),
        tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed => {
            "WebSocket 连接已关闭".into()
        }
        tungstenite::Error::Io(error) => format!("WebSocket 网络错误（{:?}）", error.kind()),
        _ => "WebSocket 协议或握手校验失败，请检查服务器协议配置".into(),
    }
}

/// 校验连接后启动异步会话，取消、关闭标签或超时均会释放网络连接。
pub fn start(c: Config, runtime: &tokio::runtime::Runtime) -> Result<Session, String> {
    let request = handshake(&c)?;
    let (session, rx, cancel) = Session::channel();
    let state = session.live.clone();
    runtime.spawn(async move {
        let result = run(c, request, state.clone(), rx, cancel).await;
        messaging::with(&state, |live| live.finish(result));
    });
    Ok(session)
}

/// 驱动握手、接收和有界发送，关闭握手最多等待两秒，不自动重连。
async fn run(
    c: Config,
    request: Request<()>,
    state: Shared,
    mut rx: mpsc::Receiver<Command>,
    cancel: CancellationToken,
) -> Result<(), String> {
    let timeout = Duration::from_secs(c.timeout);
    let config = WebSocketConfig::default()
        .max_message_size(Some(messaging::MAX_RECEIVE))
        .max_frame_size(Some(messaging::MAX_RECEIVE));
    let connected = tokio::select! {
        _ = cancel.cancelled() => return Ok(()),
        result = tokio::time::timeout(timeout, connect_async_with_config(request, Some(config), false)) => result.map_err(|_| "WebSocket 连接超时")?.map_err(error)?,
    };
    let (mut socket, response) = connected;
    let protocol = response
        .headers()
        .get("sec-websocket-protocol")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("未协商子协议");
    messaging::with(&state, |live| {
        live.connected();
        live.log(
            "连接",
            "WebSocket 握手成功",
            &format!("HTTP {} · {protocol}", response.status()),
        );
    });
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                let _ = tokio::time::timeout(Duration::from_secs(2), socket.close(None)).await;
                return Ok(());
            }
            incoming = socket.next() => {
                match incoming {
                    Some(Ok(Message::Text(value))) => messaging::with(&state, |live| live.log_message("接收", "文本帧", "", value.as_bytes(), false)),
                    Some(Ok(Message::Binary(value))) => messaging::with(&state, |live| live.log_message("接收", "二进制帧", "", &value, true)),
                    Some(Ok(Message::Ping(value))) => {
                        messaging::with(&state, |live| live.log("控制", "收到 Ping，自动回复 Pong", &messaging::payload_preview(&value, true)));
                        tokio::select! { _ = cancel.cancelled() => return Ok(()), result = tokio::time::timeout(timeout, socket.flush()) => result.map_err(|_| "Pong 发送超时")?.map_err(error)? }
                    }
                    Some(Ok(Message::Pong(_))) => messaging::with(&state, |live| live.log("控制", "收到 Pong", "服务器已响应心跳")),
                    Some(Ok(Message::Close(frame))) => {
                        messaging::with(&state, |live| live.log("连接", "服务器关闭连接", &frame.map(|frame| format!("{} {}", frame.code, frame.reason)).unwrap_or_default()));
                        let _ = tokio::time::timeout(Duration::from_secs(2), socket.flush()).await;
                        return Ok(());
                    }
                    Some(Ok(Message::Frame(_))) => {}
                    Some(Err(failure)) => return Err(error(failure)),
                    None => return Ok(()),
                }
            }
            command = rx.recv() => {
                let Some(command) = command else { return Ok(()); };
                let (message, summary) = match command {
                    Command::Send(outgoing) => {
                        let message = if outgoing.binary { Message::Binary(outgoing.payload.into()) } else { Message::Text(String::from_utf8(outgoing.payload).map_err(|_| "文本消息不是 UTF-8")?.into()) };
                        (message, "消息已写入连接")
                    }
                    Command::Ping => (Message::Ping(Vec::new().into()), "已发送 Ping"),
                    _ => { messaging::with(&state, |live| live.complete(Err("此操作不适用于 WebSocket".into()))); continue; }
                };
                let result = tokio::select! { _ = cancel.cancelled() => return Ok(()), result = tokio::time::timeout(timeout, socket.send(message.clone())) => result.map_err(|_| "WebSocket 发送超时".to_owned()).and_then(|result| result.map_err(error)) };
                // 写入成功不代表业务服务端已处理；接收的响应独立记录。
                if result.is_ok() {
                    messaging::with(&state, |live| match &message {
                        Message::Text(value) => live.log_message("发送", summary, "", value.as_bytes(), false),
                        Message::Binary(value) => live.log_message("发送", summary, "", value, true),
                        _ => live.log("发送", summary, ""),
                    });
                }
                result?;
                messaging::with(&state, |live| live.complete(Ok(())));
            }
        }
    }
}
