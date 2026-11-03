use crate::{
    http,
    storage::{Folder, Snapshot},
};
use serde_json::{Value, json};
use uuid::Uuid;

pub fn id() -> String {
    Uuid::new_v4().to_string()
}
pub fn text(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}
pub fn row() -> Value {
    json!({"id":id(),"key":"","value":"","description":"","enabled":true,"secret":false})
}
pub fn auth() -> Value {
    json!({"auth":"none","token":"","username":"","password":"","apiKeyName":"","apiKeyValue":"","apiKeyIn":"header"})
}
pub fn network() -> Value {
    json!({"globals":[],"environments":[{"id":"local","name":"本地环境","variables":[{"id":id(),"key":"base_url","value":"http://localhost:8080","description":"接口根地址","enabled":true,"secret":false}]}],"collections":{},"defaults":http::Options::default()})
}
pub fn request(folder: &str) -> Value {
    json!({"id":id(),"name":"未命名请求","method":"GET","url":"","folder":folder,"querySynced":true,"params":[],"headers":[],"body":"","bodyType":"none","description":"","auth":"none","token":"","username":"","password":"","apiKeyName":"","apiKeyValue":"","apiKeyIn":"header","form":[],"settings":{}})
}
pub fn normalize(s: &mut Snapshot) {
    if !s.session["native"].is_object() {
        s.session["native"] = json!({});
    }
    for (key, value) in [
        ("light", json!(false)),
        ("scale", json!(100)),
        ("editorFontSize", json!(14)),
        ("lineNumbers", json!(true)),
        ("softWrap", json!(true)),
        ("strongBorders", json!(false)),
    ] {
        if s.session["native"].get(key).is_none() {
            s.session["native"][key] = value;
        }
    }

    if !s.session["network"].is_object() {
        s.session["network"] = network();
    }
    if !s.session["tabs"].is_array() {
        s.session["tabs"] = json!([]);
    }
    if !s.session["expanded"].is_array() {
        s.session["expanded"] = json!([]);
    }
    if !s.session["environment"].is_string() {
        s.session["environment"] = json!("local");
    }
    if !s.session["activeId"].is_string() {
        s.session["activeId"] = json!("");
    }
}
pub fn tabs(s: &Snapshot) -> &[Value] {
    s.session["tabs"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
}
pub fn active_index(s: &Snapshot) -> Option<usize> {
    tabs(s)
        .iter()
        .position(|t| t["id"] == s.session["activeId"])
}
pub fn active(s: &Snapshot) -> Option<&Value> {
    active_index(s).map(|i| &s.session["tabs"][i]["request"])
}
pub fn active_mut(s: &mut Snapshot) -> Option<&mut Value> {
    let i = active_index(s)?;
    Some(&mut s.session["tabs"][i]["request"])
}
pub fn open(s: &mut Snapshot, r: Value) {
    let rid = r["id"].clone();
    if !tabs(s).iter().any(|t| t["id"] == rid) {
        let baseline = if s.requests.iter().any(|v| v["id"] == rid) {
            r.to_string()
        } else {
            String::new()
        };
        s.session["tabs"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":rid,"request":r,"baseline":baseline,"panel":"params"}));
    }
    s.session["activeId"] = rid;
}
pub fn dirty(tab: &Value) -> bool {
    text(tab, "baseline") != tab["request"].to_string()
}
pub fn save_request(s: &mut Snapshot, folder: &str) -> Result<(), String> {
    if !s.folders.iter().any(|f| f.id == folder) {
        return Err("请先选择保存目录".into());
    }
    let i = active_index(s).ok_or("请先新建请求")?;
    let r = &mut s.session["tabs"][i]["request"];
    if text(r, "name").trim().is_empty() {
        return Err("接口名称不能为空".into());
    }
    r["folder"] = json!(folder);
    let r = r.clone();
    if let Some(saved) = s.requests.iter_mut().find(|v| v["id"] == r["id"]) {
        *saved = r.clone()
    } else {
        s.requests.push(r.clone())
    }
    s.session["tabs"][i]["baseline"] = json!(r.to_string());
    Ok(())
}
pub fn close(s: &mut Snapshot, rid: &str) {
    let was_active = text(&s.session, "activeId") == rid;
    let ts = s.session["tabs"].as_array_mut().unwrap();
    let pos = ts.iter().position(|t| text(t, "id") == rid).unwrap_or(0);
    ts.retain(|t| text(t, "id") != rid);
    if was_active {
        s.session["activeId"] = ts
            .get(pos.min(ts.len().saturating_sub(1)))
            .map(|t| t["id"].clone())
            .unwrap_or(json!(""));
    }
}
pub fn root(s: &Snapshot, folder: &str) -> String {
    let mut result = folder.to_owned();
    for _ in 0..s.folders.len() {
        match s
            .folders
            .iter()
            .find(|f| f.id == result)
            .and_then(|f| f.parent.clone())
        {
            Some(p) => result = p,
            None => break,
        }
    }
    result
}
pub fn collection(s: &mut Snapshot, folder: &str) -> String {
    let r = root(s, folder);
    if !s.session["network"]["collections"][&r].is_object() {
        s.session["network"]["collections"][&r] = json!({"variables":[],"auth":auth()});
    }
    r
}
pub fn input(s: &Snapshot) -> Result<http::Input, String> {
    let r = active(s).ok_or("请先新建请求")?;
    let n = &s.session["network"];
    let collection = &n["collections"][root(s, &text(r, "folder"))];
    let env = n["environments"]
        .as_array()
        .and_then(|es| es.iter().find(|e| e["id"] == s.session["environment"]));
    let empty = auth();
    let a = if text(r, "auth") == "inherit" {
        collection.get("auth").unwrap_or(&empty)
    } else {
        r
    };
    let mut options = n["defaults"].clone();
    if let Some(values) = r["settings"].as_object() {
        for (k, v) in values {
            options[k] = v.clone();
        }
    }
    serde_json::from_value(json!({
        "querySynced":r["querySynced"].as_bool().unwrap_or(false),
        "method":text(r,"method"),"url":text(r,"url"),"params":r["params"],"headers":r["headers"],"body":text(r,"body"),"bodyType":text(r,"bodyType"),"form":r.get("form").cloned().unwrap_or(json!([])),
        "auth":{"kind":text(a,"auth"),"token":text(a,"token"),"username":text(a,"username"),"password":text(a,"password"),"apiKeyName":text(a,"apiKeyName"),"apiKeyValue":text(a,"apiKeyValue"),"apiKeyIn":a.get("apiKeyIn").cloned().unwrap_or(json!("header"))},
        "options":options,"globals":n["globals"],"collectionVariables":collection.get("variables").cloned().unwrap_or(json!([])),"environmentVariables":env.map(|e|e["variables"].clone()).unwrap_or(json!([])),"environmentName":env.map(|e|text(e,"name")).unwrap_or("未选择环境".into())
    })).map_err(|_|"请求配置格式不正确，请检查参数与网络设置".into())
}
pub fn url_changed(r: &mut Value) {
    let raw = text(r, "url");
    let query = raw
        .split_once('?')
        .map(|(_, q)| q.split('#').next().unwrap_or(""))
        .unwrap_or("");
    r["params"] = Value::Array(
        url::form_urlencoded::parse(query.as_bytes())
            .map(|(k, v)| json!({"id":id(),"key":k,"value":v,"description":"","enabled":true}))
            .collect(),
    );
    r["querySynced"] = json!(true);
}
pub fn params_changed(r: &mut Value) {
    let raw = text(r, "url");
    let (url, frag) = raw
        .split_once('#')
        .map(|(u, f)| (u, format!("#{f}")))
        .unwrap_or((&raw, String::new()));
    let base = url.split('?').next().unwrap_or(url);
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    if let Some(rows) = r["params"].as_array() {
        for row in rows {
            if row["enabled"].as_bool().unwrap_or(true) && !text(row, "key").is_empty() {
                serializer.append_pair(&text(row, "key"), &text(row, "value"));
            }
        }
    }
    let query = serializer
        .finish()
        .replace("%7B%7B", "{{")
        .replace("%7D%7D", "}}");
    r["url"] = json!(format!(
        "{base}{}{frag}",
        if query.is_empty() {
            String::new()
        } else {
            format!("?{query}")
        }
    ));
    r["querySynced"] = json!(true);
}
pub fn add_folder(s: &mut Snapshot, name: String, parent: Option<String>) -> String {
    let id = id();
    s.folders.push(Folder {
        id: id.clone(),
        name,
        parent,
    });
    id
}
pub fn move_folder(s: &mut Snapshot, id: &str, parent: Option<String>) -> Result<(), String> {
    let mut cursor = parent.clone();
    while let Some(p) = cursor {
        if p == id {
            return Err("不能移入自身或子目录".into());
        }
        cursor = s
            .folders
            .iter()
            .find(|f| f.id == p)
            .and_then(|f| f.parent.clone());
    }
    s.folders
        .iter_mut()
        .find(|f| f.id == id)
        .ok_or("目录不存在")?
        .parent = parent;
    Ok(())
}
pub fn remove_folder(s: &mut Snapshot, id: &str) -> Vec<String> {
    let mut ids = vec![id.to_string()];
    loop {
        let more: Vec<_> = s
            .folders
            .iter()
            .filter(|f| f.parent.as_ref().is_some_and(|p| ids.contains(p)) && !ids.contains(&f.id))
            .map(|f| f.id.clone())
            .collect();
        if more.is_empty() {
            break;
        }
        ids.extend(more);
    }
    let requests: Vec<_> = s
        .requests
        .iter()
        .filter(|r| ids.contains(&text(r, "folder")))
        .map(|r| text(r, "id"))
        .collect();
    for rid in &requests {
        close(s, rid);
    }
    s.requests.retain(|r| !requests.contains(&text(r, "id")));
    s.folders.retain(|f| !ids.contains(&f.id));
    for id in ids {
        s.session["network"]["collections"]
            .as_object_mut()
            .unwrap()
            .remove(&id);
    }
    requests
}
#[cfg(test)]
mod tests {
    use super::*;
    fn state() -> Snapshot {
        let mut s = Snapshot {
            workspace_id: id(),
            revision: 0,
            folders: vec![],
            requests: vec![],
            session: json!({}),
        };
        normalize(&mut s);
        s
    }
    #[test]
    fn inherited_auth_and_environment_take_snapshot() {
        let mut s = state();
        let root = add_folder(&mut s, "集合".into(), None);
        let sub = add_folder(&mut s, "目录".into(), Some(root.clone()));
        let mut r = request(&sub);
        r["auth"] = json!("inherit");
        open(&mut s, r);
        collection(&mut s, &sub);
        s.session["network"]["collections"][&root]["auth"]["auth"] = json!("bearer");
        s.session["network"]["collections"][&root]["auth"]["token"] = json!("test-token");
        let input = input(&s).unwrap();
        assert_eq!(input.auth.token, "test-token");
        assert_eq!(input.environment_variables.len(), 1);
    }
    #[test]
    fn query_sync_preserves_repeated_keys() {
        let mut r = request("");
        r["url"] = json!("{{base_url}}/items?q=中文&q=2#fragment");
        url_changed(&mut r);
        assert_eq!(r["params"].as_array().unwrap().len(), 2);
        params_changed(&mut r);
        let mut s = state();
        open(&mut s, r);
        assert!(input(&s).unwrap().query_synced);
        assert!(text(active(&s).unwrap(), "url").contains("#fragment"));
    }
    #[test]
    fn save_and_close_preserve_other_drafts() {
        let mut s = state();
        let f = add_folder(&mut s, "集合".into(), None);
        open(&mut s, request(&f));
        let first = text(active(&s).unwrap(), "id");
        save_request(&mut s, &f).unwrap();
        open(&mut s, request(&f));
        close(&mut s, &first);
        assert_eq!(tabs(&s).len(), 1);
        assert_eq!(s.requests.len(), 1);
        assert!(dirty(&tabs(&s)[0]));
    }
}
