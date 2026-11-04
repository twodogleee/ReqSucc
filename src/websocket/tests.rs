//! WebSocket 本机协议回归，不访问外部服务或真实工作空间。

use super::*;
use serde_json::json;
use tokio::net::TcpListener;

/// 等待会话达到可观察状态，超时输出去敏后的记录以便定位失败。
async fn wait(session: &Session, predicate: impl Fn(&messaging::Live) -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !predicate(&session.snapshot()) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "{}",
            session
                .snapshot()
                .records
                .iter()
                .map(|r| r.detail.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        )
    });
}

/// 创建独立发送快照，用于检查文本和二进制消息的实际线路内容。
fn outgoing(payload: &[u8], binary: bool) -> Outgoing {
    Outgoing {
        payload: payload.into(),
        binary,
        topic: String::new(),
        key: None,
        partition: None,
        headers: vec![],
    }
}

/// 验证自定义认证头、子协议协商、文本与二进制双向传输、Ping/Pong 和显式断开。
#[test]
fn headers_subprotocol_frames_ping_and_disconnect() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let listener = runtime.block_on(TcpListener::bind("127.0.0.1:0")).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = runtime.spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_hdr_async(
            socket,
            |request: &Request<()>, mut response: tungstenite::handshake::server::Response| {
                assert_eq!(request.headers()["authorization"], "Bearer test-only");
                assert_eq!(request.headers()["sec-websocket-protocol"], "reqsucc.test");
                response.headers_mut().insert(
                    "sec-websocket-protocol",
                    HeaderValue::from_static("reqsucc.test"),
                );
                Ok(response)
            },
        )
        .await
        .unwrap();
        while let Some(message) = socket.next().await {
            match message.unwrap() {
                value @ (Message::Text(_) | Message::Binary(_)) => {
                    socket.send(value).await.unwrap()
                }
                Message::Ping(_) => socket.flush().await.unwrap(),
                Message::Close(_) => {
                    let _ = socket.flush().await;
                    break;
                }
                _ => {}
            }
        }
    });
    let config = Config {
        address: format!("ws://127.0.0.1:{port}/echo"),
        subprotocols: "reqsucc.test".into(),
        headers: vec![
            serde_json::from_value(json!({"key":"Authorization","value":"Bearer test-only"}))
                .unwrap(),
        ],
        ..Default::default()
    };
    let session = start(config, &runtime).unwrap();
    runtime.block_on(async {
        wait(&session, |s| s.phase == messaging::Phase::Connected).await;
        session
            .submit(Command::Send(outgoing("中文消息".as_bytes(), false)))
            .unwrap();
        wait(&session, |s| {
            !s.busy
                && s.records
                    .iter()
                    .any(|r| r.kind == "接收" && r.detail == "中文消息")
        })
        .await;
        session
            .submit(Command::Send(outgoing(&[0, 255, 7], true)))
            .unwrap();
        wait(&session, |s| {
            !s.busy
                && s.records
                    .iter()
                    .any(|r| r.kind == "接收" && r.detail.contains("AP8H"))
        })
        .await;
        session.submit(Command::Ping).unwrap();
        wait(&session, |s| {
            !s.busy && s.records.iter().any(|r| r.summary == "收到 Pong")
        })
        .await;
        session.disconnect();
        wait(&session, |s| s.phase == messaging::Phase::Disconnected).await;
        tokio::time::timeout(Duration::from_secs(3), server)
            .await
            .unwrap()
            .unwrap();
    });
}

/// 验证握手等待可取消且会按配置超时，未建立连接不能提交发送。
#[test]
fn cancels_and_times_out_stalled_handshakes() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let listener = runtime.block_on(TcpListener::bind("127.0.0.1:0")).unwrap();
    let address = format!("ws://{}", listener.local_addr().unwrap());
    let session = start(
        Config {
            address: address.clone(),
            timeout: 1,
            ..Default::default()
        },
        &runtime,
    )
    .unwrap();
    assert!(session.submit(Command::Ping).is_err());
    runtime.block_on(wait(&session, |s| {
        s.phase == messaging::Phase::Disconnected
    }));
    assert!(session.snapshot().error.contains("超时"));
    let session = start(
        Config {
            address,
            ..Default::default()
        },
        &runtime,
    )
    .unwrap();
    session.disconnect();
    runtime.block_on(wait(&session, |s| {
        s.phase == messaging::Phase::Disconnected
    }));
    assert!(!session.active());
}

/// 验证非法地址、必需协议头覆盖、换行注入及无效子协议在网络操作前被拒绝。
#[test]
fn validates_handshake_without_exposing_secrets() {
    for address in [
        "https://localhost",
        "ws://user:password@localhost",
        "ws://localhost/#fragment",
    ] {
        assert!(
            handshake(&Config {
                address: address.into(),
                ..Default::default()
            })
            .is_err()
        );
    }
    let mut config = Config {
        address: "ws://localhost:8080".into(),
        ..Default::default()
    };
    for (key, value) in [
        ("Host", "another"),
        ("Authorization", "secret\r\ninjected"),
        ("sec-websocket-protocol", "manual"),
    ] {
        config.headers = vec![serde_json::from_value(json!({"key":key,"value":value})).unwrap()];
        let failure = handshake(&config).unwrap_err();
        assert!(!failure.contains("secret"));
    }
    config.headers.clear();
    config.subprotocols = "invalid protocol".into();
    assert!(handshake(&config).is_err());
}

/// 验证服务器拒绝只展示状态码，不泄漏响应正文中的认证信息。
#[test]
fn handshake_rejection_is_sanitized() {
    let response = tungstenite::http::Response::builder()
        .status(401)
        .body(Some(b"secret-token".to_vec()))
        .unwrap();
    let message = error(tungstenite::Error::Http(Box::new(response)));
    assert!(message.contains("401"));
    assert!(!message.contains("secret-token"));
}
