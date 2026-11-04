//! Kafka 回归使用进程内模拟集群或显式指定的隔离测试集群，不连接用户工作空间。

use super::*;

/// 等待可观察的会话状态，限定等待时间并保留错误诊断。
fn until(session: &Session, predicate: impl Fn(&messaging::Live) -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(35);
    loop {
        let snapshot = session.snapshot();
        if predicate(&snapshot) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "等待 Kafka 状态超时：{}",
            snapshot
                .records
                .iter()
                .map(|r| format!("{} {}", r.summary, r.detail))
                .collect::<Vec<_>>()
                .join("\n")
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// 创建携带分区、键和消息头的二进制测试消息。
fn record(topic: &str) -> Outgoing {
    Outgoing {
        topic: topic.into(),
        key: Some(b"test-key".to_vec()),
        payload: b"reqsucc-message".to_vec(),
        binary: false,
        partition: Some(0),
        headers: vec![("trace".into(), b"test-header".to_vec())],
    }
}

/// 验证主题和 broker 校验，以及 SASL/TLS 配置映射、证书配对和 TLS 验证不能被关闭。
#[test]
fn validates_names_addresses_and_security_configuration() {
    for name in ["", ".", "..", "wild*", "a/b", "中文"] {
        assert!(topic_name(name).is_err());
    }
    assert_eq!(subscription_names("a, b,a").unwrap(), ["a", "b"]);
    let mut c = Config {
        brokers: "localhost:9092".into(),
        ..Default::default()
    };
    assert!(client_config(&c).is_ok());
    for brokers in [
        "",
        "localhost",
        "localhost:0",
        "kafka://localhost:9092",
        "user:pass@host:9092",
        "localhost:9092/path",
    ] {
        c.brokers = brokers.into();
        assert!(client_config(&c).is_err(), "{brokers}");
    }
    c.brokers = "[::1]:9092, localhost:9093".into();
    c.security = "SASL_SSL".into();
    assert!(client_config(&c).is_err());
    c.username = "test".into();
    c.password = "test-only".into();
    for mechanism in ["PLAIN", "SCRAM-SHA-256", "SCRAM-SHA-512"] {
        c.mechanism = mechanism.into();
        let config = client_config(&c).unwrap();
        assert_eq!(config.get("sasl.mechanism"), Some(mechanism));
        assert_eq!(
            config.get("enable.ssl.certificate.verification"),
            Some("true")
        );
        assert_eq!(
            config.get("ssl.endpoint.identification.algorithm"),
            Some("https")
        );
    }
    c.cert_file = "/not/a/certificate.pem".into();
    assert!(client_config(&c).is_err());
}

/// 使用进程内模拟 Kafka 验证 metadata、消息头、键、发布确认、消费和取消订阅。
#[test]
fn mock_cluster_publish_consume_and_disconnect() {
    let cluster = rdkafka::mocking::MockCluster::new(1).unwrap();
    cluster.create_topic("reqsucc-test", 1, 1).unwrap();
    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let session = start(
        Config {
            brokers: cluster.bootstrap_servers(),
            offset: "earliest".into(),
            ..Default::default()
        },
        runtime,
    )
    .unwrap();
    until(&session, |s| s.phase == messaging::Phase::Connected);
    assert!(
        session
            .snapshot()
            .topics
            .iter()
            .any(|t| t.name == "reqsucc-test")
    );
    session
        .submit(Command::Subscribe(vec!["reqsucc-test".into()]))
        .unwrap();
    until(&session, |s| s.assigned > 0 && !s.busy);
    session
        .submit(Command::Send(record("reqsucc-test")))
        .unwrap();
    until(&session, |s| {
        !s.busy && s.records.iter().any(|r| r.kind == "发送成功")
    });
    until(&session, |s| {
        s.records.iter().any(|r| {
            r.kind == "接收"
                && r.detail.contains("reqsucc-message")
                && r.detail.contains("test-key")
                && r.detail.contains("test-header")
        })
    });
    session.submit(Command::Subscribe(vec![])).unwrap();
    until(&session, |s| {
        !s.busy && s.subscriptions.is_empty() && s.assigned == 0
    });
    session.disconnect();
    until(&session, |s| s.phase == messaging::Phase::Disconnected);
}

/// 使用显式配置的临时集群验证创建、查看、发布消费、重复创建失败、删除以及认证拒绝。
/// 仅创建和删除随机 reqsucc-test 前缀主题；默认忽略，由 scripts/test_kafka.py 启动隔离集群后调用。
#[test]
#[ignore = "需要显式隔离 Kafka 容器，由 scripts/test_kafka.py 执行"]
fn isolated_broker_lifecycle() {
    let raw =
        std::fs::read(std::env::var("REQSUCC_KAFKA_TEST_CONFIG").expect("测试配置路径")).unwrap();
    let mut config: Config = serde_json::from_slice(&raw).unwrap();
    config.offset = "earliest".into();
    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let session = start(config.clone(), runtime.clone()).unwrap();
    until(&session, |s| s.phase != messaging::Phase::Connecting);
    assert!(
        session.snapshot().phase == messaging::Phase::Connected,
        "{}",
        session.snapshot().error
    );
    let topic = format!("reqsucc-test-{}", model::id());
    session
        .submit(Command::Create(topic.clone(), 1, 1))
        .unwrap();
    until(&session, |s| !s.busy);
    assert!(
        session.snapshot().error.is_empty(),
        "{}",
        session.snapshot().error
    );
    // 控制器确认后元数据可能尚未传播，显式刷新直至可见。
    for _ in 0..20 {
        if session.snapshot().topics.iter().any(|t| t.name == topic) {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
        session.submit(Command::Refresh).unwrap();
        until(&session, |s| !s.busy);
    }
    assert!(session.snapshot().topics.iter().any(|t| t.name == topic));
    session
        .submit(Command::Subscribe(vec![topic.clone()]))
        .unwrap();
    until(&session, |s| s.assigned > 0 && !s.busy);
    session.submit(Command::Send(record(&topic))).unwrap();
    until(&session, |s| {
        !s.busy && s.records.iter().any(|r| r.kind == "发送成功")
    });
    until(&session, |s| {
        s.records.iter().any(|r| {
            r.kind == "接收"
                && r.detail.contains("test-header")
                && r.detail.contains("reqsucc-message")
        })
    });
    session
        .submit(Command::Create(topic.clone(), 1, 1))
        .unwrap();
    until(&session, |s| !s.busy);
    assert!(!session.snapshot().error.is_empty());
    session.submit(Command::Subscribe(vec![])).unwrap();
    until(&session, |s| !s.busy);
    session.submit(Command::Delete(topic.clone())).unwrap();
    until(&session, |s| !s.busy);
    assert!(
        session.snapshot().error.is_empty(),
        "{}",
        session.snapshot().error
    );
    for _ in 0..20 {
        if !session.snapshot().topics.iter().any(|t| t.name == topic) {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
        session.submit(Command::Refresh).unwrap();
        until(&session, |s| !s.busy);
    }
    assert!(!session.snapshot().topics.iter().any(|t| t.name == topic));
    session.disconnect();
    until(&session, |s| s.phase == messaging::Phase::Disconnected);
    if config.security.starts_with("SASL") {
        config.password = "intentionally-wrong-test-password".into();
        config.timeout = 2;
        let rejected = start(config, runtime).unwrap();
        until(&rejected, |s| s.phase == messaging::Phase::Disconnected);
        assert!(!rejected.snapshot().error.is_empty());
        assert!(!rejected.snapshot().error.contains("intentionally-wrong"));
    }
}
