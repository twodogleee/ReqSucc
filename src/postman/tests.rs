//! Postman 导入转换、数据隔离与 SQLite 往返回归，使用临时文件和测试凭证。

use super::*;

/// 构造锁定版本的最小集合导出。
fn collection(items: Value) -> Value {
    json!({"info":{"name":"导入测试","schema":"https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},"item":items})
}
/// 验证嵌套目录、继承认证、参数启用状态、变量及 JSON 原文映射。
#[test]
fn imports_nested_requests_auth_queries_and_variables() {
    let mut source = collection(
        json!([{"name":"子目录","auth":{"type":"basic","basic":[{"key":"username","value":"test"},{"key":"password","value":"test-only"}]},"item":[{"name":"请求","request":{"method":"POST","url":{"raw":"{{host}}/users/:id?a=old","variable":[{"key":"id","value":"{{user}}"}],"query":[{"key":"a","value":"1"},{"key":"a","value":"2"},{"key":"disabled","value":"secret","disabled":true}]},"header":[{"key":"X-Test","value":"{{token}}","disabled":true}],"body":{"mode":"raw","raw":"{\"x\":1e+02}","options":{"raw":{"language":"json"}}}}}]}]),
    );
    source["variable"] = json!([{"key":"host","value":"http://localhost"},{"key":"zero","value":0},{"key":"off","value":false}]);
    let mut plan = Plan::default();
    plan.document(&source).unwrap();
    assert_eq!(plan.folders.len(), 2);
    let r = &plan.requests[0];
    assert_eq!(r["url"], "{{host}}/users/{{user}}?a=1&a=2");
    assert_eq!(r["params"][2]["enabled"], false);
    assert_eq!(r["headers"][0]["enabled"], false);
    assert_eq!(r["auth"], "basic");
    assert_eq!(r["username"], "test");
    assert_eq!(r["body"], "{\"x\":1e+02}");
    assert_eq!(r["bodyType"], "json");
    let vars = &plan.collections.values().next().unwrap()["variables"];
    assert_eq!(vars[1]["value"], "0");
    assert_eq!(vars[2]["value"], "false");
}
/// 验证 v2.0 对象认证、显式无认证和表单支持，不支持功能明确统计且不导入脚本。
#[test]
fn imports_v20_forms_and_reports_unsupported_requests() {
    let mut source = collection(json!([
        {"name":"表单","request":{"method":"POST","url":"http://localhost","auth":{"type":"noauth"},"body":{"mode":"urlencoded","urlencoded":[{"key":"x","value":"a b"}]}}},
        {"name":"继承","request":"http://localhost"},
        {"name":"上传","request":{"url":"http://localhost","body":{"mode":"formdata","formdata":[]}}},
        {"name":"认证","request":{"url":"http://localhost","auth":{"type":"oauth2"}}}
    ]));
    source["info"]["schema"] =
        json!("https://schema.getpostman.com/json/collection/v2.0.0/collection.json");
    source["auth"] = json!({"type":"bearer","bearer":{"token":"test-only"}});
    source["event"] = json!([{"listen":"prerequest","script":{"exec":["throw 'must not run'"]}}]);
    let mut plan = Plan::default();
    plan.document(&source).unwrap();
    assert_eq!(plan.requests.len(), 2);
    assert_eq!(plan.skipped, 2);
    assert_eq!(plan.requests[0]["auth"], "none");
    assert_eq!(plan.requests[0]["form"][0]["value"], "a b");
    assert_eq!(plan.requests[1]["token"], "test-only");
    assert!(plan.warnings.iter().any(|w| w.contains("脚本")));
}
/// 验证新增导入的同名处理、旧草稿保护和重启恢复，不切换当前环境。
#[test]
fn append_preserves_existing_data_and_survives_restart() {
    let temp = tempfile::tempdir().unwrap();
    let mut service =
        crate::storage::StorageService::new(temp.path().join("config"), temp.path().join("data"));
    let mut original = service.load().unwrap().snapshot;
    model::normalize(&mut original);
    model::add_folder(&mut original, "导入测试".into(), None);
    let folder = original.folders[0].id.clone();
    model::open(&mut original, model::request(&folder));
    model::active_mut(&mut original).unwrap()["body"] = json!("未保存草稿");
    let mut plan = Plan::default();
    plan.document(&collection(
        json!([{"name":"测试","request":"http://localhost"}]),
    ))
    .unwrap();
    plan.document(&json!({"_postman_variable_scope":"environment","name":"本地环境","values":[{"key":"secret","value":"test-only","type":"secret","enabled":false}]})).unwrap();
    let merged = plan.merge(&original).unwrap();
    assert_eq!(merged.session["tabs"], original.session["tabs"]);
    assert_eq!(
        merged.session["environment"],
        original.session["environment"]
    );
    assert_eq!(merged.folders.last().unwrap().name, "导入测试（导入 2）");
    assert_eq!(
        merged.session["network"]["environments"][1]["name"],
        "本地环境（导入 2）"
    );
    assert_eq!(
        merged.session["network"]["environments"][1]["variables"][0]["secret"],
        true
    );
    service.save(merged.clone()).unwrap();
    drop(service);
    let mut service =
        crate::storage::StorageService::new(temp.path().join("config"), temp.path().join("data"));
    let restored = service.load().unwrap().snapshot;
    assert_eq!(restored.requests, merged.requests);
    assert_eq!(restored.session, merged.session);
}
/// 验证读取错误、批量混入无效文件、BOM 和容量限制都在提交前处理。
#[test]
fn reads_files_with_bom_and_rejects_invalid_batches_and_limits() {
    let temp = tempfile::tempdir().unwrap();
    let valid = temp.path().join("collection.json");
    std::fs::write(&valid, format!("\u{feff}{}", collection(json!([])))).unwrap();
    assert_eq!(read_files(&[valid.clone()]).unwrap().collections.len(), 1);
    let invalid = temp.path().join("invalid.json");
    std::fs::write(&invalid, b"{broken").unwrap();
    assert!(read_files(&[valid, invalid]).is_err());
    assert!(read_files(&[]).is_err());
    let large = temp.path().join("large.json");
    let file = std::fs::File::create(&large).unwrap();
    file.set_len(MAX_BYTES as u64 + 1).unwrap();
    assert!(read_files(&[large]).err().unwrap().contains("20 MiB"));
    let mut plan = Plan::default();
    assert!(plan.document(&json!({"item":[]})).is_err());
}

/// 验证串行导入写入及退出等待不会被旧快照覆盖，失败事务不改变已保存接口。
#[test]
fn backend_import_is_atomic_and_exit_barrier_preserves_result() {
    use crate::backend::{Backend, Event};
    use std::{sync::mpsc, time::Duration};
    let temp = tempfile::tempdir().unwrap();
    let (tx, rx) = mpsc::channel();
    let backend = Backend::new(temp.path().join("config"), temp.path().join("data"), tx);
    let Event::Loaded(Ok(loaded)) = rx.recv_timeout(Duration::from_secs(3)).unwrap() else {
        panic!("加载失败")
    };
    let mut original = loaded.snapshot;
    model::normalize(&mut original);
    let folder = original.folders[0].id.clone();
    model::open(&mut original, model::request(&folder));
    model::active_mut(&mut original).unwrap()["body"] = json!("导入前未保存草稿");
    backend.save(1, original.clone());
    let mut plan = Plan::default();
    plan.document(&collection(
        json!([{"name":"保存的导入接口","request":"http://localhost"}]),
    ))
    .unwrap();
    backend
        .import_postman(original.clone(), plan.clone())
        .unwrap();
    backend.wait_pending();
    let mut imported = None;
    while let Ok(event) = rx.try_recv() {
        if let Event::PostmanImported(result) = event {
            imported = Some(result.unwrap());
        }
    }
    let imported = imported.unwrap();
    assert_eq!(imported.requests.len(), original.requests.len() + 1);
    assert_eq!(imported.session["tabs"], original.session["tabs"]);
    plan.requests[0]["method"] = json!("");
    backend.import_postman(imported.clone(), plan).unwrap();
    let Event::PostmanImported(result) = rx.recv_timeout(Duration::from_secs(3)).unwrap() else {
        panic!("导入未返回")
    };
    assert!(result.is_err());
    backend.load();
    let Event::Loaded(Ok(loaded)) = rx.recv_timeout(Duration::from_secs(3)).unwrap() else {
        panic!("加载失败")
    };
    assert_eq!(loaded.snapshot.requests, imported.requests);
    assert_eq!(loaded.snapshot.session, imported.session);
}
