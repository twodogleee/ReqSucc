#!/usr/bin/env python3
"""在一次性本机 Kafka 容器中验证主题管理、发布消费及 PLAIN/SCRAM/TLS。

只映射回环端口，使用随机容器名与临时证书，不接触应用或用户集群数据。
依赖 Docker、OpenSSL 和 Cargo；退出时删除本脚本创建的容器及临时文件。
作者：Andersen
"""
import json
import os
import pathlib
import socket
import subprocess
import tempfile
import time
import uuid

# 锁定官方镜像版本，避免重复验收时隐式升级服务器。
IMAGE = "apache/kafka:4.1.1"
# 从脚本位置定位 Cargo 工作空间。
ROOT = pathlib.Path(__file__).resolve().parent.parent


def run(args, **kwargs):
    """执行结构化参数列表，失败立即中止并由外层 finally 清理容器。"""
    return subprocess.run(args, check=True, **kwargs)


def port():
    """选择空闲回环端口；若启动前被抢占，Docker 报错而不复用其他服务。"""
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def main():
    """启动隔离 KRaft 集群，逐项运行真实客户端回归并始终清理资源。"""
    run(["cargo", "test", "--locked", "--no-run"], cwd=ROOT)
    name = "reqsucc-kafka-test-" + uuid.uuid4().hex[:12]
    with tempfile.TemporaryDirectory(prefix="reqsucc-kafka-") as directory:
        root = pathlib.Path(directory)
        root.chmod(0o755)
        # 证书仅用于临时回环服务器，SAN 同时覆盖 IPv4 地址与主机名。
        run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", str(root / "key.pem"), "-out", str(root / "ca.pem"), "-days", "1", "-subj", "/CN=localhost", "-addext", "subjectAltName=DNS:localhost,IP:127.0.0.1"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        run(["openssl", "pkcs8", "-topk8", "-nocrypt", "-in", str(root / "key.pem"), "-out", str(root / "pkcs8.pem")])
        (root / "server.pem").write_text((root / "pkcs8.pem").read_text() + (root / "ca.pem").read_text())
        ports = {listener: port() for listener in ["HOST", "AUTH", "TLS", "SECURE"]}
        properties = {
            "process.roles": "broker,controller", "node.id": "1", "controller.quorum.voters": "1@localhost:29093",
            "controller.listener.names": "CONTROLLER", "inter.broker.listener.name": "INTERNAL",
            "listeners": "INTERNAL://:29092,CONTROLLER://:29093," + ",".join(f"{name}://:{port}" for name, port in ports.items()),
            "advertised.listeners": "INTERNAL://localhost:29092," + ",".join(f"{name}://localhost:{port}" for name, port in ports.items()),
            "listener.security.protocol.map": "INTERNAL:PLAINTEXT,CONTROLLER:PLAINTEXT,HOST:PLAINTEXT,AUTH:SASL_PLAINTEXT,TLS:SSL,SECURE:SASL_SSL",
            "log.dirs": "/tmp/reqsucc-kafka-data", "offsets.topic.replication.factor": "1",
            "transaction.state.log.replication.factor": "1", "transaction.state.log.min.isr": "1",
            "group.initial.rebalance.delay.ms": "0", "auto.create.topics.enable": "false", "delete.topic.enable": "true",
            "ssl.keystore.type": "PEM", "ssl.keystore.location": "/fixture/server.pem",
            "ssl.truststore.type": "PEM", "ssl.truststore.location": "/fixture/ca.pem", "ssl.client.auth": "required",
            "sasl.enabled.mechanisms": "PLAIN,SCRAM-SHA-256,SCRAM-SHA-512",
        }
        for listener in ["auth", "secure"]:
            properties[f"listener.name.{listener}.plain.sasl.jaas.config"] = 'org.apache.kafka.common.security.plain.PlainLoginModule required user_test="fixturepass";'
            for mechanism in ["scram-sha-256", "scram-sha-512"]:
                properties[f"listener.name.{listener}.{mechanism}.sasl.jaas.config"] = "org.apache.kafka.common.security.scram.ScramLoginModule required;"
        (root / "server.properties").write_text("\n".join(f"{key}={value}" for key, value in properties.items()) + "\n")
        for path in root.iterdir():
            path.chmod(0o644)
        command = ["docker", "run", "--rm", "-d", "--name", name, "--memory", "1g", "-e", "KAFKA_HEAP_OPTS=-Xms256m -Xmx512m", "-v", f"{root}:/fixture:ro"]
        for host_port in ports.values():
            command.extend(["-p", f"127.0.0.1:{host_port}:{host_port}"])
        # 固定脚本文本不包含用户输入；SCRAM 凭据只在一次性测试集群中有效。
        startup = "/opt/kafka/bin/kafka-storage.sh format -t MkU3OEVBNTcwNTJENDM2Qk -c /fixture/server.properties --add-scram 'SCRAM-SHA-256=[name=test,password=fixturepass]' --add-scram 'SCRAM-SHA-512=[name=test,password=fixturepass]' && exec /opt/kafka/bin/kafka-server-start.sh /fixture/server.properties"
        command.extend(["--entrypoint", "/bin/bash", IMAGE, "-c", startup])
        try:
            run(command, stdout=subprocess.DEVNULL)
            deadline = time.monotonic() + 60
            while True:
                try:
                    with socket.create_connection(("127.0.0.1", ports["HOST"]), timeout=1):
                        break
                except OSError:
                    if time.monotonic() >= deadline:
                        run(["docker", "logs", "--tail", "40", name])
                        raise RuntimeError("隔离 Kafka 启动超时")
                    time.sleep(0.5)
            for security, listener in [("PLAINTEXT", "HOST"), ("SSL", "TLS"), ("SASL_PLAINTEXT", "AUTH"), ("SASL_SSL", "SECURE")]:
                for mechanism in (["PLAIN", "SCRAM-SHA-256", "SCRAM-SHA-512"] if security.startswith("SASL") else ["PLAIN"]):
                    config = {"brokers": f"localhost:{ports[listener]}", "security": security, "mechanism": mechanism, "username": "test", "password": "fixturepass", "timeout": 10, "caFile": str(root / "ca.pem"), "certFile": str(root / "ca.pem"), "keyFile": str(root / "pkcs8.pem")}
                    path = root / "client.json"
                    path.write_text(json.dumps(config))
                    print(f"验证 {security} / {mechanism}", flush=True)
                    env = dict(os.environ, REQSUCC_KAFKA_TEST_CONFIG=str(path))
                    run(["cargo", "test", "--locked", "kafka::tests::isolated_broker_lifecycle", "--", "--ignored", "--exact", "--nocapture"], cwd=ROOT, env=env, timeout=150)
        finally:
            subprocess.run(["docker", "rm", "-f", name], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False)


if __name__ == "__main__":
    main()
