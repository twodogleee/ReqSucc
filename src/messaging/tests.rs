//! 消息协议的公共边界与临时 SQLite 工作空间回归。

use super::*;
use serde_json::json;

/// 验证完整日期、毫秒和时区偏移，跨日显示不会只剩时分秒，非法时间戳明确标记。
#[test]
fn message_timestamps_preserve_date_milliseconds_and_timezone() {
    use chrono::{FixedOffset, TimeZone, Utc};
    let instant =
        Utc.with_ymd_and_hms(2026, 11, 4, 0, 5, 6).unwrap() + chrono::Duration::milliseconds(789);
    assert_eq!(
        format_time(&instant.with_timezone(&FixedOffset::east_opt(8 * 3600).unwrap())),
        "2026-11-04 08:05:06.789 +08:00"
    );
    assert_eq!(
        format_time(&instant.with_timezone(&FixedOffset::west_opt(8 * 3600).unwrap())),
        "2026-11-03 16:05:06.789 -08:00"
    );
    assert_eq!(format_timestamp(i64::MAX), "无效时间戳");
}

/// 验证记录有界、UTF-8 截断安全，清空后标识不复用且迟到连接事件不能覆盖取消状态。
#[test]
fn bounds_records_and_preserves_cancellation() {
    let (session, _rx, _) = Session::channel();
    with(&session.live, |live| {
        for _ in 0..600 {
            live.log("接收", "主题", &"中".repeat(4000));
        }
    });
    let snapshot = session.snapshot();
    assert_eq!(snapshot.records.len(), MAX_RECORDS);
    assert!(snapshot.records[0].detail.contains("截断"));
    assert!(snapshot.records[0].detail.len() < MAX_PREVIEW + 100);
    let last = snapshot.records.back().unwrap().id;
    session.clear();
    with(&session.live, |live| live.log("连接", "清空后", ""));
    assert!(session.snapshot().records[0].id > last);
    session.disconnect();
    with(&session.live, |live| live.connected());
    assert!(session.snapshot().phase == Phase::Disconnecting);
    assert!(session.submit(Command::Ping).is_err());
}

/// 验证每次只允许一个显式操作，关闭通道失败不会留下忙状态。
#[test]
fn serializes_commands_and_handles_closed_queue() {
    let (session, mut rx, _) = Session::channel();
    with(&session.live, |live| live.connected());
    session.submit(Command::Ping).unwrap();
    assert!(session.submit(Command::Ping).is_err());
    assert!(matches!(rx.try_recv().unwrap(), Command::Ping));
    with(&session.live, |live| live.complete(Ok(())));
    drop(rx);
    assert!(session.submit(Command::Ping).is_err());
    assert!(!session.snapshot().busy);
}

/// 验证 JSON 和 Base64 发送校验、实际字节上限及二进制预览。
#[test]
fn validates_payload_encoding_and_limits() {
    assert_eq!(encode("AP8H", "base64").unwrap(), [0, 255, 7]);
    assert!(encode("{broken", "json").is_err());
    assert!(encode("%%%", "base64").is_err());
    assert!(encode(&"x".repeat(MAX_PAYLOAD + 1), "text").is_err());
    assert!(encode(&"x".repeat(MAX_PAYLOAD), "text").is_ok());
    assert!(payload_preview(&[255, 0], false).contains("Base64"));
}

/// 验证两种新协议保留已保存版本和未完成草稿，并能恢复变量、请求头与消息格式。
#[test]
fn saves_protocol_drafts_and_resolves_only_operation_fields() {
    let temp = tempfile::tempdir().unwrap();
    let mut service =
        crate::storage::StorageService::new(temp.path().join("config"), temp.path().join("data"));
    let mut doc = service.load().unwrap().snapshot;
    model::normalize(&mut doc);
    let folder = model::add_folder(&mut doc, "消息协议测试".into(), None);
    for protocol in ["websocket", "kafka"] {
        let mut request = model::request(&folder);
        request["protocol"] = json!(protocol);
        request[protocol] = if protocol == "websocket" {
            json!(crate::websocket::Config::default())
        } else {
            json!(crate::kafka::Config::default())
        };
        request[protocol]["payload"] = json!("{{not_defined}}");
        request[protocol]["format"] = json!("json");
        doc.requests.push(request.clone());
        model::open(&mut doc, request);
        assert_eq!(
            doc.session["tabs"].as_array().unwrap().last().unwrap()["panel"],
            "message"
        );
        let active = model::active_mut(&mut doc).unwrap();
        active[protocol]["headers"] =
            json!([{"id":model::id(),"key":"X-Test","value":"{{value}}","enabled":true}]);
        active[protocol]["address"] = json!("ws://localhost");
        active[protocol]["brokers"] = json!("localhost:9092");
        doc.session["network"]["globals"] =
            json!([{"id":model::id(),"key":"value","value":"resolved","enabled":true}]);
        assert_eq!(
            resolve(&doc, protocol, &["headers"]).unwrap()["headers"][0]["value"],
            "resolved"
        );
        assert!(resolve(&doc, protocol, &["payload"]).is_err());
        assert_eq!(doc.requests.last().unwrap()[protocol]["headers"], json!([]));
    }
    for panel in [
        "message",
        "headers",
        "connection",
        "subscriptions",
        "topics",
        "description",
    ] {
        for tab in doc.session["tabs"].as_array_mut().unwrap() {
            tab["panel"] = json!(panel);
        }
        doc.revision = service.save(doc.clone()).unwrap();
        let persisted = service.load().unwrap().snapshot;
        assert_eq!(persisted.session["tabs"], doc.session["tabs"]);
    }
    drop(service);
    let mut reopened =
        crate::storage::StorageService::new(temp.path().join("config"), temp.path().join("data"));
    let loaded = reopened.load().unwrap().snapshot;
    assert_eq!(loaded.requests, doc.requests);
    assert_eq!(loaded.session["tabs"], doc.session["tabs"]);
}
