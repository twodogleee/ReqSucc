//! MQTT V5 回归检查，使用本机 TCP 模拟服务器与临时工作空间验证协议及数据边界。

use super::*;
use chrono::{FixedOffset, TimeZone, Utc};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

/// 验证 JSON 模板格式化保留重复键、原始数值、转义和变量占位符，并满足重复格式化不变。
#[test]
fn formats_json_templates_without_rewriting_tokens_or_expanding_variables() {
    let source = r#"{"z":1e+02,"z":1234567890123456789012345,"s":"中文 {{ name }} \"quoted\"", "n":{{ count }},"obj":{{object}},"empty":{},"items":[1,{{item}},[]]}"#;
    let formatted = format_json_template(source).unwrap();
    assert_eq!(
        formatted,
        "{\n  \"z\": 1e+02,\n  \"z\": 1234567890123456789012345,\n  \"s\": \"中文 {{ name }} \\\"quoted\\\"\",\n  \"n\": {{ count }},\n  \"obj\": {{object}},\n  \"empty\": {},\n  \"items\": [\n    1,\n    {{item}},\n    []\n  ]\n}"
    );
    assert_eq!(format_json_template(&formatted).unwrap(), formatted);
}

/// 验证不合法的 JSON 或变量位置返回错误，不尝试改写为其他语义。
#[test]
fn rejects_invalid_json_templates_instead_of_rewriting_them() {
    for invalid in ["", "{bad}", "{\"n\":1{{x}}}", "{\"n\":{{x}", "[1,]", "1 2"] {
        assert!(format_json_template(invalid).is_err(), "{invalid}");
    }
}

/// 从测试连接读取一个 MQTT 固定头和报文体，断言剩余长度编码有效且报文小于 1 MiB。
async fn packet(socket: &mut TcpStream) -> (u8, Vec<u8>) {
    let header = socket.read_u8().await.unwrap();
    let mut len = 0;
    let mut shift = 0;
    loop {
        let b = socket.read_u8().await.unwrap();
        len |= ((b & 127) as usize) << shift;
        if b & 128 == 0 {
            break;
        }
        shift += 7;
        assert!(shift < 28);
    }
    assert!(len < 1024 * 1024);
    let mut body = vec![0; len];
    socket.read_exact(&mut body).await.unwrap();
    (header, body)
}
/// 将测试报文头和正文编码为 MQTT 变长长度格式，并完整写入本机连接。
async fn write_packet(socket: &mut TcpStream, header: u8, body: &[u8]) {
    let mut bytes = vec![header];
    let mut len = body.len();
    loop {
        let mut b = (len % 128) as u8;
        len /= 128;
        if len > 0 {
            b |= 128
        }
        bytes.push(b);
        if len == 0 {
            break;
        }
    }
    bytes.extend_from_slice(body);
    socket.write_all(&bytes).await.unwrap();
}
/// 每 5 毫秒检查会话快照，最多等待 4 秒；超时时输出事件详情并使测试失败。
async fn wait(session: &Session, predicate: impl Fn(&Live) -> bool) {
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            if predicate(&session.snapshot()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "等待状态超时：{}",
            session
                .snapshot()
                .records
                .iter()
                .map(|r| r.detail.clone())
                .collect::<Vec<_>>()
                .join(";")
        )
    });
}
/// 创建测试专用 Tokio 运行时，使各用例独立驱动连接任务。
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Runtime::new().unwrap()
}

/// 验证消息时间使用传入时区，跨 UTC 日期边界时仍输出正确本地日期。
#[test]
fn message_time_respects_timezone_and_date_boundaries() {
    let utc = Utc.with_ymd_and_hms(2026, 11, 4, 18, 30, 40).unwrap();
    let east = FixedOffset::east_opt(8 * 3600).unwrap();
    let west = FixedOffset::west_opt(7 * 3600).unwrap();
    assert_eq!(
        format_message_time(&utc.with_timezone(&east)),
        "2026-11-05 02:30:40"
    );
    assert_eq!(
        format_message_time(&utc.with_timezone(&west)),
        "2026-11-04 11:30:40"
    );
}
/// 为本机模拟服务器端口创建默认 MQTT 配置，不依赖外部服务。
fn config_for(port: u16) -> Config {
    Config {
        address: format!("mqtt://127.0.0.1:{port}"),
        ..Default::default()
    }
}
/// 创建启用的 QoS 1 通配符订阅，供握手和回执测试复用。
fn topic() -> Topic {
    Topic {
        id: "topic-a".into(),
        topic: "验收/+".into(),
        qos: 1,
        enabled: true,
        description: "".into(),
    }
}

/// 验证 V5 连接、订阅、发布、接收、取消订阅和正常断开的完整报文流程。
#[test]
fn v5_connect_subscribe_publish_receive_unsubscribe_and_graceful_disconnect() {
    let rt = runtime();
    rt.block_on(async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let broker = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let (h, body) = packet(&mut s).await;
            assert_eq!(h, 0x10);
            assert_eq!(&body[..7], b"\x00\x04MQTT\x05");
            assert_eq!(body[7] & 0xc0, 0xc0);
            assert!(body.windows(8).any(|w| w == b"testuser"));
            write_packet(&mut s, 0x20, &[0, 0, 0]).await;
            let (h, body) = packet(&mut s).await;
            assert_eq!(h, 0x82);
            assert!(
                body.windows("验收/+".len())
                    .any(|w| w == "验收/+".as_bytes())
            );
            write_packet(&mut s, 0x90, &[body[0], body[1], 0, 1]).await;
            let (h, body) = packet(&mut s).await;
            assert_eq!(h, 0x33);
            let n = u16::from_be_bytes([body[0], body[1]]) as usize;
            let id = &body[2 + n..4 + n];
            assert!(body.ends_with("你好".as_bytes()));
            write_packet(&mut s, 0x40, &[id[0], id[1], 0, 0]).await;
            let topic = "验收/消息".as_bytes();
            let mut echo = vec![(topic.len() >> 8) as u8, topic.len() as u8];
            echo.extend(topic);
            echo.push(0);
            echo.extend("服务器消息".as_bytes());
            write_packet(&mut s, 0x30, &echo).await;
            let (h, body) = packet(&mut s).await;
            assert_eq!(h, 0xa2);
            write_packet(&mut s, 0xb0, &[body[0], body[1], 0, 0]).await;
            let (h, _) = packet(&mut s).await;
            assert_eq!(h, 0xe0);
        });
        let mut c = config_for(port);
        c.auth = true;
        c.username = "testuser".into();
        c.password = "testpass".into();
        c.topics = vec![topic()];
        let session = Session::start(c.clone(), &rt).unwrap();
        wait(&session, |s| {
            s.subscriptions.get("topic-a").is_some_and(|(v, _)| *v)
        })
        .await;
        c.topic = "验收/消息".into();
        c.payload = "你好".into();
        c.qos = 1;
        c.retain = true;
        session.publish(c).unwrap();
        wait(&session, |s| {
            s.records
                .iter()
                .any(|r| r.kind == "确认" && r.detail.contains("QoS 1"))
                && s.records
                    .iter()
                    .any(|r| r.kind == "接收" && r.detail.contains("服务器消息"))
        })
        .await;
        session.subscribe(topic(), false).unwrap();
        wait(&session, |s| {
            s.subscriptions
                .get("topic-a")
                .is_some_and(|(v, t)| !*v && t == "已取消订阅")
        })
        .await;
        session.disconnect();
        wait(&session, |s| s.state == State::Disconnected).await;
        broker.await.unwrap();
    });
}
/// 验证 QoS 2 仅在 PUBCOMP 后完成，而 QoS 0 写入不等待服务器确认。
#[test]
fn qos_two_completes_only_after_pubcomp_and_qos_zero_needs_no_ack() {
    let rt = runtime();
    rt.block_on(async {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let broker = tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            packet(&mut s).await;
            write_packet(&mut s, 0x20, &[0, 0, 0]).await;
            let (h, b) = packet(&mut s).await;
            assert_eq!(h, 0x34);
            let n = u16::from_be_bytes([b[0], b[1]]) as usize;
            let id = [b[n + 2], b[n + 3]];
            write_packet(&mut s, 0x50, &[id[0], id[1], 0, 0]).await;
            let (h, b) = packet(&mut s).await;
            assert_eq!(h, 0x62);
            assert_eq!(&b[..2], &id);
            write_packet(&mut s, 0x70, &[id[0], id[1], 0, 0]).await;
            let (h, _) = packet(&mut s).await;
            assert_eq!(h, 0x30);
            let (h, _) = packet(&mut s).await;
            assert_eq!(h, 0xe0);
        });
        let mut c = config_for(port);
        let session = Session::start(c.clone(), &rt).unwrap();
        wait(&session, |s| s.state == State::Connected).await;
        c.topic = "qa".into();
        c.qos = 2;
        c.payload = "{}".into();
        session.publish(c.clone()).unwrap();
        wait(&session, |s| {
            s.records
                .iter()
                .any(|r| r.kind == "确认" && r.detail.contains("QoS 2"))
        })
        .await;
        c.qos = 0;
        session.publish(c).unwrap();
        wait(&session, |s| {
            s.records.iter().any(|r| r.detail.contains("QoS 0 已写入"))
        })
        .await;
        session.disconnect();
        wait(&session, |s| s.state == State::Disconnected).await;
        broker.await.unwrap();
    });
}
/// 验证服务器拒绝发布或订阅时记录失败，不误报操作成功。
#[test]
fn server_rejection_is_not_reported_as_success() {
    let rt = runtime();
    rt.block_on(async {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let broker = tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            packet(&mut s).await;
            write_packet(&mut s, 0x20, &[0, 0, 0]).await;
            let (_, b) = packet(&mut s).await;
            write_packet(&mut s, 0x90, &[b[0], b[1], 0, 135]).await;
            let (_, b) = packet(&mut s).await;
            let n = u16::from_be_bytes([b[0], b[1]]) as usize;
            write_packet(&mut s, 0x40, &[b[n + 2], b[n + 3], 135, 0]).await;
            packet(&mut s).await;
        });
        let mut c = config_for(port);
        c.topics = vec![topic()];
        let session = Session::start(c.clone(), &rt).unwrap();
        wait(&session, |s| {
            s.records
                .iter()
                .any(|r| r.detail.contains("订阅被服务器拒绝"))
        })
        .await;
        assert!(!session.snapshot().subscriptions["topic-a"].0);
        c.topic = "forbidden".into();
        c.qos = 1;
        session.publish(c).unwrap();
        wait(&session, |s| {
            s.records.iter().any(|r| r.detail == "服务器拒绝发布消息")
        })
        .await;
        assert!(!session.snapshot().records.iter().any(|r| r.kind == "确认"));
        session.disconnect();
        wait(&session, |s| s.state == State::Disconnected).await;
        broker.await.unwrap();
    });
}
/// 验证连接中可以取消，未收到 CONNACK 时按配置超时结束。
#[test]
fn connecting_can_be_cancelled_and_times_out_without_connack() {
    let rt = runtime();
    rt.block_on(async {
        for cancel in [true, false] {
            let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = l.local_addr().unwrap().port();
            let broker = tokio::spawn(async move {
                let (mut s, _) = l.accept().await.unwrap();
                packet(&mut s).await;
                let mut byte = [0];
                assert_eq!(s.read(&mut byte).await.unwrap(), 0);
            });
            let mut c = config_for(port);
            c.timeout = 1;
            let session = Session::start(c, &rt).unwrap();
            tokio::time::sleep(Duration::from_millis(40)).await;
            if cancel {
                session.disconnect();
            }
            wait(&session, |s| s.state == State::Disconnected).await;
            broker.await.unwrap();
            assert!(
                !session
                    .snapshot()
                    .records
                    .iter()
                    .any(|r| r.detail.contains("连接成功"))
            );
        }
    });
}
/// 验证主题过滤器、连接地址、参数范围和遗嘱限制在使用前检查。
#[test]
fn validates_filters_addresses_limits_and_will() {
    for valid in ["a/+", "a/#", "#", "$share/team/a/+", "中文/主题"] {
        assert!(validate_topic(valid, true).is_ok());
    }
    for invalid in ["", "a/#/b", "a+", "$share//a", "a/\0"] {
        assert!(validate_topic(invalid, true).is_err());
    }
    assert!(validate_topic("a/+", false).is_err());
    for url in [
        "http://localhost",
        "mqtt://user:pass@localhost",
        "mqtt://localhost/path",
        "mqtt://localhost?x=1",
    ] {
        let mut c = Config::default();
        c.address = url.into();
        assert!(c.options().is_err());
    }
    let mut c = config_for(1883);
    c.keep_alive = 0;
    assert!(c.options().is_err());
    c.keep_alive = 60;
    c.will_enabled = true;
    c.will_topic = "bad/#".into();
    assert!(c.options().is_err());
    c.will_topic = "qa/will".into();
    c.will_payload = "offline".into();
    assert!(c.options().is_ok());
}
/// 验证消息记录数量有界，详情截断不破坏 UTF-8 字符边界。
#[test]
fn logs_are_bounded_and_truncate_utf8_safely() {
    let mut live = Live {
        state: State::Connected,
        records: VecDeque::new(),
        subscriptions: HashMap::new(),
        revision: 0,
    };
    for _ in 0..1000 {
        live.log("接收", "主题", &"你".repeat(4000));
    }
    assert_eq!(live.records.len(), 500);
    assert!(live.records[0].detail.len() < 8300);
    assert!(live.records[0].detail.ends_with("8 KiB）"));
    assert!(live.records[0].truncated);
}
/// 验证仅格式化完整 JSON 消息，截断或非 JSON 详情保留原文且不修改存储内容。
#[test]
fn message_detail_formats_only_complete_json_payloads_without_changing_raw() {
    let mut live = Live {
        state: State::Connected,
        records: VecDeque::new(),
        subscriptions: HashMap::new(),
        revision: 0,
    };
    let original = "QoS 1 · 保留 否 · 27 字节\n{\"名称\":\"设备\",\"a\":[1,2]}";
    live.log("接收", "device", original);
    let record = &live.records[0];
    assert_eq!(record.detail, original);
    assert!(record.display_detail().contains("\n  \"名称\": \"设备\""));
    assert!(record.display_detail().contains("\n  \"a\": ["));

    let invalid = "QoS 0 · 保留 否 · 3 字节\n{xx";
    live.log("发送", "device", invalid);
    assert_eq!(live.records[0].display_detail(), invalid);
    let event = "状态\n{\"ok\":true}";
    live.log("确认", "device", event);
    assert_eq!(live.records[0].display_detail(), event);

    let large = format!("QoS 0\n{{\"value\":\"{}\"}}", "a".repeat(9000));
    live.log("接收", "device", &large);
    let truncated = &live.records[0];
    assert!(truncated.truncated);
    assert_eq!(truncated.display_detail(), truncated.detail);
}
/// 验证 MQTT 已保存配置及各分区草稿在重启和迁移后分别恢复。
#[test]
fn mqtt_saved_request_and_draft_survive_restart_and_migration() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config");
    let data = temp.path().join("data");
    let mut storage = crate::storage::StorageService::new(config.clone(), data.clone());
    let mut doc = storage.load().unwrap().snapshot;
    model::normalize(&mut doc);
    let folder = doc.folders[0].id.clone();
    let mut request = model::request(&folder);
    request["protocol"] = json!("mqtt");
    request["mqtt"]["address"] = json!("mqtt://{{broker}}:1883");
    request["mqtt"]["topics"] = json!([topic()]);
    model::open(&mut doc, request);
    model::save_request(&mut doc, &folder).unwrap();
    model::active_mut(&mut doc).unwrap()["mqtt"]["publishCases"][0]["payload"] =
        json!("未保存中文草稿");
    for panel in [
        "message",
        "topics",
        "mqtt-auth",
        "properties",
        "will",
        "mqtt-settings",
        "description",
    ] {
        doc.session["tabs"][0]["panel"] = json!(panel);
        doc.revision = storage.save(doc.clone()).unwrap();
    }
    drop(storage);
    let mut storage = crate::storage::StorageService::new(config, data);
    let doc = storage.load().unwrap().snapshot;
    assert!(is_mqtt(&doc.requests[0]));
    assert_eq!(doc.requests[0]["mqtt"]["publishCases"][0]["payload"], "");
    assert_eq!(
        model::active(&doc).unwrap()["mqtt"]["publishCases"][0]["payload"],
        "未保存中文草稿"
    );
    let target = temp.path().join("moved");
    std::fs::create_dir(&target).unwrap();
    let loaded = storage.switch(&target, "migrate").unwrap();
    assert_eq!(
        model::active(&loaded.snapshot).unwrap()["mqtt"]["topics"][0]["topic"],
        "验收/+"
    );
}

/// 验证各发布用例独立解析变量，切换选择不混用执行快照，也不回写原始模板。
#[test]
fn selected_publish_cases_resolve_independently_and_keep_snapshots() {
    let temp = tempfile::tempdir().unwrap();
    let mut storage =
        crate::storage::StorageService::new(temp.path().join("config"), temp.path().join("data"));
    let mut doc = storage.load().unwrap().snapshot;
    model::normalize(&mut doc);
    let folder = doc.folders[0].id.clone();
    model::new_tab(&mut doc, &folder);
    let request = model::active_mut(&mut doc).unwrap();
    request["protocol"] = json!("mqtt");
    request["mqtt"]["address"] = json!("mqtt://localhost");
    request["mqtt"]["publishCases"][0]["topic"] = json!("first/{{device}}");
    request["mqtt"]["publishCases"][0]["payload"] = json!("{\"value\":1}");
    request["mqtt"]["publishCases"][0]["format"] = json!("json");
    let first = model::text(&request["mqtt"]["publishCases"][0], "id");
    doc.session["network"]["globals"] = json!([{"id":"v","key":"device","value":"one","enabled":true,"secret":false,"description":""}]);
    let second = model::mqtt_new_case(&mut doc, false).unwrap();
    let request = model::active_mut(&mut doc).unwrap();
    request["mqtt"]["publishCases"][1]["topic"] = json!("second/{{device}}");
    request["mqtt"]["publishCases"][1]["payload"] = json!("other");
    request["mqtt"]["publishCases"][1]["qos"] = json!(2);
    request["mqtt"]["publishCases"][1]["retain"] = json!(true);
    assert!(model::mqtt_select_case(&mut doc, &first));
    let first_snapshot = resolve(&doc, Operation::Publish).unwrap();
    assert!(model::mqtt_select_case(&mut doc, &second));
    let second_snapshot = resolve(&doc, Operation::Publish).unwrap();
    assert_eq!(first_snapshot.topic, "first/one");
    assert_eq!(first_snapshot.payload, "{\"value\":1}");
    assert_eq!(first_snapshot.qos, 0);
    assert_eq!(second_snapshot.topic, "second/one");
    assert_eq!(second_snapshot.payload, "other");
    assert_eq!(second_snapshot.qos, 2);
    assert!(second_snapshot.retain);
    assert_eq!(
        model::active(&doc).unwrap()["mqtt"]["publishCases"][0]["topic"],
        "first/{{device}}"
    );
}

/// 验证发布用例拒绝空白名称、重复用例、空列表和超过数量上限的列表。
#[test]
fn publish_case_validation_rejects_invalid_structure_and_limits() {
    let mut request = model::request("folder");
    request["protocol"] = json!("mqtt");
    assert!(validate_saved(&request).is_ok());
    let original = request["mqtt"]["publishCases"][0].clone();
    request["mqtt"]["publishCases"][0]["name"] = json!(" ");
    assert!(validate_saved(&request).is_err());
    request["mqtt"]["publishCases"] = json!([original.clone(), original.clone()]);
    assert!(validate_saved(&request).is_err());
    request["mqtt"]["publishCases"] = json!([]);
    assert!(validate_saved(&request).is_err());
    request["mqtt"]["publishCases"] = json!(
        (0..=MAX_PUBLISH_CASES)
            .map(|i| {
                let mut case = original.clone();
                case["id"] = json!(i.to_string());
                case
            })
            .collect::<Vec<_>>()
    );
    assert!(validate_saved(&request).is_err());
}
/// 验证 MQTT 复用变量作用域优先级，且只解析本次操作实际使用的字段。
#[test]
fn variables_respect_scopes_and_only_resolve_fields_used_by_operation() {
    let temp = tempfile::tempdir().unwrap();
    let mut storage =
        crate::storage::StorageService::new(temp.path().join("config"), temp.path().join("data"));
    let mut doc = storage.load().unwrap().snapshot;
    model::normalize(&mut doc);
    let folder = doc.folders[0].id.clone();
    model::collection(&mut doc, &folder);
    model::open(&mut doc, model::request(&folder));
    let row = |v: &str| json!({"id":model::id(),"key":"broker","value":v,"enabled":true,"secret":false,"description":""});
    doc.session["network"]["globals"] = json!([row("global")]);
    doc.session["network"]["collections"][&folder]["variables"] = json!([row("collection")]);
    doc.session["network"]["environments"][0]["variables"] = json!([row("environment")]);
    let r = model::active_mut(&mut doc).unwrap();
    r["mqtt"]["address"] = json!("mqtt://{{broker}}");
    r["mqtt"]["publishCases"][0]["payload"] = json!("{{missing}}");
    r["mqtt"]["password"] = json!("{{unused_password}}");
    assert_eq!(
        resolve(&doc, Operation::Connect).unwrap().address,
        "mqtt://environment"
    );
    assert!(resolve(&doc, Operation::Publish).is_err());
    doc.session["environment"] = json!("");
    assert_eq!(
        resolve(&doc, Operation::Connect).unwrap().address,
        "mqtt://collection"
    );
}
