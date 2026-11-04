//! 工作空间内存模型操作，维护请求、集合、标签草稿和协议配置兼容，不直接读写磁盘。
//!
//! 作者：Andersen

use crate::{
    http,
    storage::{Folder, Snapshot},
};
use serde_json::{Value, json};
use uuid::Uuid;

/// 生成用于请求、目录或配置行的随机 UUID 字符串。
pub fn id() -> String {
    Uuid::new_v4().to_string()
}
/// 读取 JSON 对象中的字符串字段；缺失或类型不匹配时返回空字符串。
pub fn text(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}
/// 创建带稳定标识、默认启用且未标记敏感的空白键值行。
pub fn row() -> Value {
    json!({"id":id(),"key":"","value":"","description":"","enabled":true,"secret":false})
}
/// 创建无认证的 JSON 配置，保留各种认证方式的空字段供编辑器切换。
pub fn auth() -> Value {
    json!({"auth":"none","token":"","username":"","password":"","apiKeyName":"","apiKeyValue":"","apiKeyIn":"header"})
}
/// 创建默认全局网络配置，包含本地环境、空集合配置和 HTTP 执行默认值。
pub fn network() -> Value {
    json!({"globals":[],"environments":[{"id":"local","name":"本地环境","variables":[{"id":id(),"key":"base_url","value":"http://localhost:8080","description":"接口根地址","enabled":true,"secret":false}]}],"collections":{},"defaults":http::Options::default()})
}
/// 创建指定目录下的空白 HTTP 请求，同时初始化独立的 MQTT 配置和发布用例。
pub fn request(folder: &str) -> Value {
    let mut r = json!({"id":id(),"name":"未命名请求","protocol":"http","mqtt":crate::mqtt::Config::default(),"method":"GET","url":"","folder":folder,"querySynced":true,"params":[],"headers":[],"body":"","bodyType":"none","description":"","auth":"none","token":"","username":"","password":"","apiKeyName":"","apiKeyValue":"","apiKeyIn":"header","form":[],"settings":{}});
    normalize_mqtt_cases(&mut r);
    r
}

/// 将旧版单条 MQTT 发布配置迁为默认用例；固定标识保证已保存版本、基线和草稿独立迁移后可比较。
pub fn normalize_mqtt_cases(request: &mut Value) {
    let Some(mqtt) = request.get_mut("mqtt").and_then(Value::as_object_mut) else {
        return;
    };
    if mqtt.contains_key("publishCases") {
        return;
    }
    let defaults = json!(crate::mqtt::Config::default());
    let mut case = json!({"id":"default", "name":"默认消息"});
    for key in crate::mqtt::PUBLISH_CASE_FIELDS {
        case[key] = mqtt
            .get(key)
            .cloned()
            .unwrap_or_else(|| defaults[key].clone());
    }
    mqtt.insert("publishCases".into(), json!([case]));
}

/// 以第一个发布用例回写旧版单消息字段，保留旧客户端读取能力；不使用当前选择，避免切换用例产生脏状态。
pub fn mqtt_sync_legacy_fields(request: &mut Value) {
    let Some(mqtt) = request.get_mut("mqtt").and_then(Value::as_object_mut) else {
        return;
    };
    let Some(first) = mqtt
        .get("publishCases")
        .and_then(Value::as_array)
        .and_then(|cases| cases.first())
        .cloned()
    else {
        return;
    };
    for key in crate::mqtt::PUBLISH_CASE_FIELDS {
        if let Some(value) = first.get(key) {
            mqtt.insert(key.into(), value.clone());
        }
    }
}
/// 补齐会话默认结构并独立迁移请求、草稿及基线，保留原有未保存状态。
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
    for request in &mut s.requests {
        normalize_mqtt_cases(request);
    }
    for tab in s.session["tabs"].as_array_mut().unwrap() {
        if let Some(request) = tab.get_mut("request") {
            normalize_mqtt_cases(request);
        }
        // 旧基线必须独立迁移；若将基线替换为当前草稿，会把未保存修改误判为已保存。
        if let Some(original) = tab.get("baseline").and_then(Value::as_str) {
            if !original.is_empty() {
                if let Ok(mut baseline) = serde_json::from_str::<Value>(original) {
                    normalize_mqtt_cases(&mut baseline);
                    tab["baseline"] = json!(baseline.to_string());
                }
            }
        }
    }
}
/// 取得打开标签的只读列表；会话缺少有效列表时返回空切片。
pub fn tabs(s: &Snapshot) -> &[Value] {
    s.session["tabs"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
}
/// 按会话中的 activeId 查找活动标签下标；没有匹配标签时返回 None。
pub fn active_index(s: &Snapshot) -> Option<usize> {
    tabs(s)
        .iter()
        .position(|t| t["id"] == s.session["activeId"])
}
/// 取得活动标签内的请求草稿，不返回集合中的已保存版本。
pub fn active(s: &Snapshot) -> Option<&Value> {
    active_index(s).map(|i| &s.session["tabs"][i]["request"])
}
/// 取得活动请求草稿的可变引用；调用方负责触发后续保存与界面更新。
pub fn active_mut(s: &mut Snapshot) -> Option<&mut Value> {
    let i = active_index(s)?;
    Some(&mut s.session["tabs"][i]["request"])
}

/// 查找活动标签选中的发布用例；选择失效时回退至第一个用例，没有用例则返回 None。
pub fn mqtt_case_index(s: &Snapshot) -> Option<usize> {
    let tab = &s.session["tabs"][active_index(s)?];
    let cases = tab["request"]["mqtt"]["publishCases"].as_array()?;
    let selected = text(tab, "mqttCaseId");
    cases
        .iter()
        .position(|case| text(case, "id") == selected)
        .or_else(|| (!cases.is_empty()).then_some(0))
}

/// 切换活动标签的发布用例选择；只修改标签状态，不改变已保存请求或比较基线，成功时返回 true。
pub fn mqtt_select_case(s: &mut Snapshot, case_id: &str) -> bool {
    let Some(tab_index) = active_index(s) else {
        return false;
    };
    let cases = &s.session["tabs"][tab_index]["request"]["mqtt"]["publishCases"];
    let Some(cases) = cases.as_array() else {
        return false;
    };
    if !cases.iter().any(|case| text(case, "id") == case_id) {
        return false;
    }
    let current = mqtt_case_index(s)
        .and_then(|i| cases.get(i))
        .map(|c| text(c, "id"));
    if current.as_deref() == Some(case_id) {
        return false;
    }
    s.session["tabs"][tab_index]["mqttCaseId"] = json!(case_id);
    true
}

/// 新建空白发布用例或复制当前用例并选中；达到数量上限或缺少有效标签时返回 None。
pub fn mqtt_new_case(s: &mut Snapshot, copy: bool) -> Option<String> {
    let tab_index = active_index(s)?;
    let current = mqtt_case_index(s)?;
    let cases = s.session["tabs"][tab_index]["request"]["mqtt"]["publishCases"].as_array_mut()?;
    if cases.len() >= crate::mqtt::MAX_PUBLISH_CASES {
        return None;
    }
    let mut case = if copy {
        cases.get(current)?.clone()
    } else {
        let defaults = json!(crate::mqtt::Config::default());
        let mut value = json!({});
        for key in crate::mqtt::PUBLISH_CASE_FIELDS {
            value[key] = defaults[key].clone();
        }
        value
    };
    let base_name = if copy {
        format!("{} 副本", text(&case, "name"))
    } else {
        "消息".into()
    };
    let mut ordinal = if copy { 1 } else { cases.len() + 1 };
    let name = loop {
        let candidate = if copy && ordinal == 1 {
            base_name.clone()
        } else {
            format!("{base_name} {ordinal}")
        };
        if !cases
            .iter()
            .any(|item| text(item, "name").trim() == candidate)
        {
            break candidate;
        }
        ordinal += 1;
    };
    let case_id = id();
    case["id"] = json!(case_id);
    case["name"] = json!(name);
    cases.push(case);
    s.session["tabs"][tab_index]["mqttCaseId"] = json!(case_id);
    Some(case_id)
}

/// 重命名活动请求中的指定发布用例；名称去除首尾空白后不能为空或与其他用例重复。
pub fn mqtt_rename_case(s: &mut Snapshot, case_id: &str, name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("发布用例名称不能为空".into());
    }
    let tab_index = active_index(s).ok_or("请先选择请求")?;
    let cases = s.session["tabs"][tab_index]["request"]["mqtt"]["publishCases"]
        .as_array_mut()
        .ok_or("发布用例格式无效")?;
    if cases
        .iter()
        .any(|item| text(item, "id") != case_id && text(item, "name").trim() == name)
    {
        return Err("发布用例名称已存在".into());
    }
    let case = cases
        .iter_mut()
        .find(|item| text(item, "id") == case_id)
        .ok_or("发布用例不存在")?;
    case["name"] = json!(name);
    Ok(())
}

/// 删除指定发布用例并调整选择及兼容字段；至少保留一条，未删除时返回 false。
pub fn mqtt_delete_case(s: &mut Snapshot, case_id: &str) -> bool {
    let Some(tab_index) = active_index(s) else {
        return false;
    };
    let current = mqtt_case_index(s);
    let Some(cases) =
        s.session["tabs"][tab_index]["request"]["mqtt"]["publishCases"].as_array_mut()
    else {
        return false;
    };
    if cases.len() <= 1 {
        return false;
    }
    let Some(index) = cases.iter().position(|item| text(item, "id") == case_id) else {
        return false;
    };
    cases.remove(index);
    if current == Some(index) {
        let new_id = text(&cases[index.min(cases.len() - 1)], "id");
        s.session["tabs"][tab_index]["mqttCaseId"] = json!(new_id);
    }
    mqtt_sync_legacy_fields(&mut s.session["tabs"][tab_index]["request"]);
    true
}
/// 创建并打开空白请求，以初始内容作为基线，使其在首次编辑前保持未修改状态。
pub fn new_tab(s: &mut Snapshot, folder: &str) {
    let r = request(folder);
    let baseline = r.to_string();
    open(s, r);
    if let Some(i) = active_index(s) {
        s.session["tabs"][i]["baseline"] = json!(baseline);
    }
}
/// 打开或激活指定请求；新标签初始化基线与协议面板，已有标签保留原草稿。
pub fn open(s: &mut Snapshot, mut r: Value) {
    normalize_mqtt_cases(&mut r);
    let rid = r["id"].clone();
    if !tabs(s).iter().any(|t| t["id"] == rid) {
        let baseline = if s.requests.iter().any(|v| v["id"] == rid) {
            r.to_string()
        } else {
            String::new()
        };
        let panel = if matches!(r["protocol"].as_str(), Some("mqtt" | "websocket" | "kafka")) {
            "message"
        } else {
            "params"
        };
        s.session["tabs"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":rid,"request":r,"baseline":baseline,"panel":panel}));
    }
    s.session["activeId"] = rid;
}
/// 标签关闭范围，以右键操作的标签和当前排列顺序为准。
#[derive(Clone, Copy)]
pub enum TabCloseScope {
    /// 仅关闭目标标签。
    Current,
    /// 关闭目标之外的所有标签。
    Others,
    /// 关闭目标左侧的标签，不包含目标。
    Left,
    /// 关闭目标右侧的标签，不包含目标。
    Right,
    /// 关闭全部标签。
    All,
}

/// 返回关闭范围内的标签标识；目标已失效时返回空列表，避免旧菜单误操作。
pub fn tab_close_ids(s: &Snapshot, anchor: &str, scope: TabCloseScope) -> Vec<String> {
    let Some(index) = tabs(s).iter().position(|tab| text(tab, "id") == anchor) else {
        return Vec::new();
    };
    tabs(s)
        .iter()
        .enumerate()
        .filter(|(i, _)| match scope {
            TabCloseScope::Current => *i == index,
            TabCloseScope::Others => *i != index,
            TabCloseScope::Left => *i < index,
            TabCloseScope::Right => *i > index,
            TabCloseScope::All => true,
        })
        .map(|(_, tab)| text(tab, "id"))
        .collect()
}

/// 复制目标的当前草稿及面板选择，在其右侧激活未保存副本；不复制运行资源或修改已保存请求。
pub fn duplicate_tab(s: &mut Snapshot, source: &str) -> Option<String> {
    let tabs = s.session.get_mut("tabs")?.as_array_mut()?;
    let index = tabs.iter().position(|tab| text(tab, "id") == source)?;
    let mut copy = tabs[index].clone();
    let new_id = id();
    copy["id"] = json!(new_id);
    copy["request"]["id"] = json!(new_id);
    copy["request"]["name"] = json!(format!("{} 副本", text(&copy["request"], "name")));
    copy["baseline"] = json!("");
    tabs.insert(index + 1, copy);
    s.session["activeId"] = json!(new_id);
    Some(new_id)
}

/// 比较草稿序列化内容与基线，判断当前标签是否存在未保存的接口修改。
pub fn dirty(tab: &Value) -> bool {
    text(tab, "baseline") != tab["request"].to_string()
}
/// 将完整标签移到目标标签之前或之后，保留内部状态；仅实际改变顺序时返回 true。
pub fn reorder_tab(s: &mut Snapshot, source: &str, target: &str, after: bool) -> bool {
    if source.is_empty() || target.is_empty() || source == target {
        return false;
    }
    let Some(tabs) = s.session.get_mut("tabs").and_then(Value::as_array_mut) else {
        return false;
    };
    let Some(from) = tabs
        .iter()
        .position(|tab| tab["id"].as_str() == Some(source))
    else {
        return false;
    };
    let Some(target_index) = tabs
        .iter()
        .position(|tab| tab["id"].as_str() == Some(target))
    else {
        return false;
    };
    // 源标签位于目标之前时，移除源标签会使目标下标左移一位。
    let destination = target_index + usize::from(after) - usize::from(from < target_index);
    if from == destination {
        return false;
    }
    let tab = tabs.remove(from);
    tabs.insert(destination, tab);
    true
}
/// 将活动草稿写入内存中的已保存请求列表并更新基线；目录或名称无效时返回错误，磁盘保存由后台另行完成。
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
    mqtt_sync_legacy_fields(r);
    let r = r.clone();
    if let Some(saved) = s.requests.iter_mut().find(|v| v["id"] == r["id"]) {
        *saved = r.clone()
    } else {
        s.requests.push(r.clone())
    }
    s.session["tabs"][i]["baseline"] = json!(r.to_string());
    Ok(())
}
/// 移除指定标签，必要时选中相邻标签；调用方负责先确认未保存内容并释放运行资源。
pub fn close(s: &mut Snapshot, rid: &str) {
    close_tabs(s, &[rid.to_owned()]);
}

/// 一次移除指定标签并返回实际移除的标识；保留存活的活动标签，否则优先选中原活动标签右侧邻居。
/// 调用方须先确认未保存修改，并为返回的每个标识释放运行资源。
pub fn close_tabs(s: &mut Snapshot, ids: &[String]) -> Vec<String> {
    let active = text(&s.session, "activeId");
    let Some(tabs) = s.session.get_mut("tabs").and_then(Value::as_array_mut) else {
        return Vec::new();
    };
    let removed: Vec<String> = tabs
        .iter()
        .map(|tab| text(tab, "id"))
        .filter(|id| ids.contains(id))
        .collect();
    let next = if removed.contains(&active) {
        let index = tabs
            .iter()
            .position(|tab| text(tab, "id") == active)
            .unwrap_or(0);
        tabs[index..]
            .iter()
            .chain(tabs[..index].iter().rev())
            .map(|tab| text(tab, "id"))
            .find(|id| !removed.contains(id))
            .unwrap_or_default()
    } else {
        active
    };
    tabs.retain(|tab| !removed.contains(&text(tab, "id")));
    s.session["activeId"] = json!(next);
    removed
}
/// 沿父目录查找所属根集合，遍历次数受目录总数限制以避免异常关系导致无限循环。
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
/// 取得所属根集合标识，并在缺失时为该集合补齐变量和认证配置。
pub fn collection(s: &mut Snapshot, folder: &str) -> String {
    let r = root(s, folder);
    if !s.session["network"]["collections"][&r].is_object() {
        s.session["network"]["collections"][&r] = json!({"variables":[],"auth":auth()});
    }
    r
}
/// 从活动草稿创建执行配置快照，合并环境变量、集合认证和请求级选项；结构无效时返回中文错误。
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
/// 将 URL 查询串同步为参数行，保留重复键并标记已同步，供发送时避免重复附加。
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
/// 按启用的参数行重建 URL 查询串，保留片段与变量占位符，并标记已同步。
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
/// 在内存中添加集合或子目录并返回标识；parent 为 None 时创建根集合。
pub fn add_folder(s: &mut Snapshot, name: String, parent: Option<String>) -> String {
    let id = id();
    s.folders.push(Folder {
        id: id.clone(),
        name,
        parent,
    });
    id
}
/// 调整目录父级，拒绝移入自身或后代目录；目标目录不存在时返回错误。
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
/// 删除目录及后代、其中已保存请求与对应标签和集合配置，返回请求标识供调用方释放运行资源。
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
/// 验证模型操作、草稿基线、变量继承与 MQTT 发布用例兼容。
#[cfg(test)]
mod tests {
    use super::*;
    /// 构造并规范化最小工作空间快照，供模型测试使用。
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
    /// 验证执行配置采用集合认证和当前环境的独立快照。
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
    /// 验证 URL 与参数表双向同步时保留重复参数名。
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
    /// 验证保存和关闭一个请求不会覆盖其他标签草稿。
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

    /// 验证空白标签初始无修改，编辑后变脏，恢复原内容及重载会话后仍可准确判断修改状态。
    #[test]
    fn new_blank_tab_is_clean_until_changed_and_survives_session_restore() {
        let mut s = state();
        new_tab(&mut s, "");
        assert!(!dirty(&tabs(&s)[0]));
        let original = active(&s).unwrap().clone();
        active_mut(&mut s).unwrap()["url"] = json!("https://example.test");
        assert!(dirty(&tabs(&s)[0]));
        *active_mut(&mut s).unwrap() = original;
        assert!(!dirty(&tabs(&s)[0]));
        let session = serde_json::to_string(&s.session).unwrap();
        s.session = serde_json::from_str(&session).unwrap();
        normalize(&mut s);
        assert!(!dirty(&tabs(&s)[0]));
        active_mut(&mut s).unwrap()["protocol"] = json!("mqtt");
        assert!(dirty(&tabs(&s)[0]));
    }

    /// 验证复制请求与旧草稿保持未保存状态，而已保存请求重新打开后保持干净。
    #[test]
    fn duplicate_and_legacy_drafts_remain_dirty_but_saved_request_reopens_clean() {
        let mut s = state();
        let folder = add_folder(&mut s, "集合".into(), None);
        new_tab(&mut s, &folder);
        save_request(&mut s, &folder).unwrap();
        let saved = active(&s).unwrap().clone();
        let saved_id = text(&saved, "id");
        close(&mut s, &saved_id);
        open(&mut s, saved.clone());
        assert!(!dirty(&tabs(&s)[0]));

        let mut copy = saved;
        copy["id"] = json!(id());
        copy["name"] = json!("未命名请求 副本");
        open(&mut s, copy);
        assert!(dirty(&tabs(&s)[1]));

        // 旧会话为未保存请求使用空基线，无法重建其编辑前状态，因此继续保留未保存标记。
        s.session["tabs"][1]["baseline"] = json!("");
        let session = serde_json::to_string(&s.session).unwrap();
        s.session = serde_json::from_str(&session).unwrap();
        normalize(&mut s);
        assert!(dirty(&tabs(&s)[1]));
    }

    /// 验证旧版 MQTT 的已保存请求、草稿和基线独立迁移，不制造或消除脏状态。
    #[test]
    fn legacy_saved_and_draft_migrate_independently_without_false_dirty() {
        let mut s = state();
        let folder = add_folder(&mut s, "集合".into(), None);
        let mut saved = request(&folder);
        saved["protocol"] = json!("mqtt");
        saved["mqtt"]
            .as_object_mut()
            .unwrap()
            .remove("publishCases");
        saved["mqtt"]["topic"] = json!("original/topic");
        saved["mqtt"]["payload"] = json!("original payload");
        let original_client_id = saved["mqtt"]["clientId"].clone();
        let mut draft = saved.clone();
        draft["mqtt"]["topic"] = json!("draft/topic");
        draft["mqtt"]["payload"] = json!("draft payload");
        let rid = text(&saved, "id");
        s.requests.push(saved.clone());
        s.session["tabs"] =
            json!([{"id": rid, "request": draft, "baseline":saved.to_string(), "panel":"message"}]);
        s.session["activeId"] = json!(rid);
        normalize(&mut s);
        assert_eq!(
            s.requests[0]["mqtt"]["publishCases"][0]["payload"],
            "original payload"
        );
        assert_eq!(
            active(&s).unwrap()["mqtt"]["publishCases"][0]["payload"],
            "draft payload"
        );
        assert_eq!(active(&s).unwrap()["mqtt"]["clientId"], original_client_id);
        assert!(dirty(&tabs(&s)[0]));

        // 原本无修改的旧标签经过同样迁移后，仍应保持无修改状态。
        let mut clean = saved.clone();
        clean["mqtt"]["payload"] = json!("clean payload");
        s.session["tabs"][0]["request"] = clean.clone();
        s.session["tabs"][0]["baseline"] = json!(clean.to_string());
        normalize(&mut s);
        assert!(!dirty(&tabs(&s)[0]));
    }

    /// 验证发布用例的选择、编辑、复制和删除遵守请求与草稿边界。
    #[test]
    fn publish_case_selection_edit_copy_and_delete_preserve_request_state() {
        let mut s = state();
        let folder = add_folder(&mut s, "集合".into(), None);
        new_tab(&mut s, &folder);
        active_mut(&mut s).unwrap()["protocol"] = json!("mqtt");
        active_mut(&mut s).unwrap()["mqtt"]["publishCases"][0]["topic"] = json!("first/topic");
        save_request(&mut s, &folder).unwrap();
        active_mut(&mut s).unwrap()["mqtt"]["publishCases"][0]["topic"] = json!("temporary/topic");
        mqtt_sync_legacy_fields(active_mut(&mut s).unwrap());
        assert!(dirty(&tabs(&s)[0]));
        active_mut(&mut s).unwrap()["mqtt"]["publishCases"][0]["topic"] = json!("first/topic");
        mqtt_sync_legacy_fields(active_mut(&mut s).unwrap());
        assert!(
            !dirty(&tabs(&s)[0]),
            "restoring the initial value must restore clean state"
        );
        let first_id = text(&active(&s).unwrap()["mqtt"]["publishCases"][0], "id");
        let second_id = mqtt_new_case(&mut s, true).unwrap();
        assert_eq!(mqtt_case_index(&s), Some(1));
        assert_eq!(
            active(&s).unwrap()["mqtt"]["publishCases"][1]["topic"],
            "first/topic"
        );
        active_mut(&mut s).unwrap()["mqtt"]["publishCases"][1]["payload"] = json!("draft second");
        mqtt_rename_case(&mut s, &second_id, "第二条").unwrap();
        assert!(mqtt_rename_case(&mut s, &second_id, "  ").is_err());
        save_request(&mut s, &folder).unwrap();
        assert!(!dirty(&tabs(&s)[0]));
        assert!(mqtt_select_case(&mut s, &first_id));
        assert!(
            !dirty(&tabs(&s)[0]),
            "selection must not change the saved request"
        );
        assert!(mqtt_select_case(&mut s, &second_id));
        assert_eq!(
            active(&s).unwrap()["mqtt"]["publishCases"][1]["payload"],
            "draft second"
        );
        assert!(mqtt_delete_case(&mut s, &second_id));
        assert_eq!(mqtt_case_index(&s), Some(0));
        assert!(!mqtt_delete_case(&mut s, &first_id));
        assert_eq!(
            active(&s).unwrap()["mqtt"]["publishCases"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    /// 验证标签重排保留活动标识及每个标签的完整状态。
    #[test]
    fn reorder_tabs_preserves_active_and_complete_tab_state() {
        let mut s = state();
        for _ in 0..4 {
            new_tab(&mut s, "");
        }
        s.session["tabs"][0]["request"]["url"] = json!("https://example.test/draft");
        s.session["tabs"][1]["request"]["protocol"] = json!("mqtt");
        s.session["tabs"][1]["request"]["mqtt"]["topic"] = json!("device/events");
        s.session["tabs"][1]["panel"] = json!("message");
        let original = tabs(&s).to_vec();
        let ids: Vec<_> = original.iter().map(|tab| text(tab, "id")).collect();
        s.session["activeId"] = json!(ids[1]);
        let session = s.session.clone();

        assert!(reorder_tab(&mut s, &ids[0], &ids[2], true));
        assert_eq!(
            tabs(&s),
            &[
                original[1].clone(),
                original[2].clone(),
                original[0].clone(),
                original[3].clone()
            ]
        );
        assert_eq!(s.session["activeId"], ids[1]);
        assert_eq!(active(&s), Some(&original[1]["request"]));
        assert!(dirty(&tabs(&s)[2]));
        assert!(reorder_tab(&mut s, &ids[3], &ids[1], false));
        assert_eq!(
            tabs(&s),
            &[
                original[3].clone(),
                original[1].clone(),
                original[2].clone(),
                original[0].clone()
            ]
        );
        assert_eq!(s.session["activeId"], session["activeId"]);
        assert_eq!(s.session["network"], session["network"]);
        assert_eq!(s.session["native"], session["native"]);
    }

    /// 验证无效标识、自身拖放和无需移动的相邻位置不会改变顺序。
    #[test]
    fn reorder_tabs_ignores_missing_self_and_adjacent_noop_targets() {
        let mut s = state();
        for _ in 0..3 {
            new_tab(&mut s, "");
        }
        let ids: Vec<_> = tabs(&s).iter().map(|tab| text(tab, "id")).collect();
        let before = s.session.clone();
        assert!(!reorder_tab(&mut s, "", &ids[0], false));
        assert!(!reorder_tab(&mut s, "missing", &ids[0], false));
        assert!(!reorder_tab(&mut s, &ids[0], "missing", true));
        assert!(!reorder_tab(&mut s, &ids[0], &ids[0], true));
        assert!(!reorder_tab(&mut s, &ids[0], &ids[1], false));
        assert!(!reorder_tab(&mut s, &ids[1], &ids[0], true));
        assert_eq!(s.session, before);
        s.session["tabs"] = Value::Null;
        assert!(!reorder_tab(&mut s, &ids[0], &ids[1], false));
    }
    /// 验证右键范围基于目标及当前顺序，边缘、单标签和失效目标不会误关其他标签。
    #[test]
    fn tab_close_scopes_follow_anchor_and_reordered_positions() {
        let mut s = state();
        for _ in 0..4 {
            new_tab(&mut s, "");
        }
        let ids: Vec<_> = tabs(&s).iter().map(|tab| text(tab, "id")).collect();
        assert_eq!(
            tab_close_ids(&s, &ids[1], TabCloseScope::Current),
            vec![ids[1].clone()]
        );
        assert_eq!(tab_close_ids(&s, &ids[1], TabCloseScope::Left), ids[..1]);
        assert_eq!(tab_close_ids(&s, &ids[1], TabCloseScope::Right), ids[2..]);
        assert_eq!(
            tab_close_ids(&s, &ids[1], TabCloseScope::Others),
            vec![ids[0].clone(), ids[2].clone(), ids[3].clone()]
        );
        assert_eq!(tab_close_ids(&s, &ids[1], TabCloseScope::All), ids);
        assert!(tab_close_ids(&s, &ids[0], TabCloseScope::Left).is_empty());
        assert!(tab_close_ids(&s, &ids[3], TabCloseScope::Right).is_empty());
        assert!(reorder_tab(&mut s, &ids[3], &ids[0], false));
        assert_eq!(
            tab_close_ids(&s, &ids[0], TabCloseScope::Left),
            vec![ids[3].clone()]
        );
        for scope in [
            TabCloseScope::Current,
            TabCloseScope::Left,
            TabCloseScope::Right,
            TabCloseScope::Others,
            TabCloseScope::All,
        ] {
            assert!(tab_close_ids(&s, "missing", scope).is_empty());
        }
        let others = tab_close_ids(&s, &ids[0], TabCloseScope::Others);
        close_tabs(&mut s, &others);
        for scope in [
            TabCloseScope::Left,
            TabCloseScope::Right,
            TabCloseScope::Others,
        ] {
            assert!(tab_close_ids(&s, &ids[0], scope).is_empty());
        }
    }

    /// 验证复制保留 HTTP 当前草稿并紧邻原标签，编辑副本不影响原稿和已保存接口。
    #[test]
    fn duplicate_tab_copies_unsaved_http_draft_independently() {
        let mut s = state();
        let folder = add_folder(&mut s, "测试集合".into(), None);
        new_tab(&mut s, &folder);
        save_request(&mut s, &folder).unwrap();
        let source = text(active(&s).unwrap(), "id");
        active_mut(&mut s).unwrap()["body"] = json!("未保存的请求体");
        s.session["tabs"][0]["panel"] = json!("body");
        let original = tabs(&s)[0].clone();
        let saved = s.requests.clone();
        new_tab(&mut s, "");
        let copy = duplicate_tab(&mut s, &source).unwrap();
        assert_ne!(copy, source);
        assert_eq!(active_index(&s), Some(1));
        assert_eq!(tabs(&s)[1]["id"], copy);
        assert_eq!(tabs(&s)[1]["panel"], "body");
        assert_eq!(active(&s).unwrap()["body"], "未保存的请求体");
        assert_eq!(active(&s).unwrap()["name"], "未命名请求 副本");
        assert!(dirty(&tabs(&s)[1]));
        active_mut(&mut s).unwrap()["body"] = json!("副本修改");
        assert_eq!(tabs(&s)[0], original);
        assert_eq!(s.requests, saved);
        let before = s.session.clone();
        assert!(duplicate_tab(&mut s, "missing").is_none());
        assert_eq!(s.session, before);
    }

    /// 验证 MQTT 副本保留连接配置、全部发布用例和当前选择，恢复会话后仍为独立草稿。
    #[test]
    fn duplicate_mqtt_tab_preserves_cases_and_selection_after_restore() {
        let mut s = state();
        new_tab(&mut s, "");
        active_mut(&mut s).unwrap()["protocol"] = json!("mqtt");
        let source = text(active(&s).unwrap(), "id");
        let case = mqtt_new_case(&mut s, false).unwrap();
        s.session["tabs"][0]["panel"] = json!("subscriptions");
        let original = tabs(&s)[0].clone();
        let copy = duplicate_tab(&mut s, &source).unwrap();
        assert_eq!(active(&s).unwrap()["mqtt"], original["request"]["mqtt"]);
        assert_eq!(tabs(&s)[1]["mqttCaseId"], case);
        assert_eq!(tabs(&s)[1]["panel"], "subscriptions");
        s.session = serde_json::from_str(&s.session.to_string()).unwrap();
        normalize(&mut s);
        assert_eq!(s.session["activeId"], copy);
        assert!(dirty(&tabs(&s)[1]));
        assert_eq!(tabs(&s)[0], original);
        assert!(s.requests.is_empty());
    }

    /// 验证批量关闭保留存活标签状态、活动选择及已保存接口，重复和过期标识不产生额外删除。
    #[test]
    fn close_tabs_preserves_surviving_active_draft_and_saved_requests() {
        let mut s = state();
        let folder = add_folder(&mut s, "测试集合".into(), None);
        for _ in 0..4 {
            new_tab(&mut s, &folder);
            save_request(&mut s, &folder).unwrap();
        }
        let ids: Vec<_> = tabs(&s).iter().map(|tab| text(tab, "id")).collect();
        s.session["activeId"] = json!(ids[1]);
        active_mut(&mut s).unwrap()["body"] = json!("保留的草稿");
        let survivor = tabs(&s)[1].clone();
        let saved = s.requests.clone();
        assert_eq!(
            close_tabs(
                &mut s,
                &[
                    ids[0].clone(),
                    ids[2].clone(),
                    ids[0].clone(),
                    "missing".into()
                ]
            ),
            vec![ids[0].clone(), ids[2].clone()]
        );
        assert_eq!(s.session["activeId"], ids[1]);
        assert_eq!(tabs(&s)[0], survivor);
        assert_eq!(s.requests, saved);
        let before = s.session.clone();
        assert!(close_tabs(&mut s, &["missing".into()]).is_empty());
        assert_eq!(s.session, before);
    }

    /// 验证活动标签被批量关闭后优先选择右侧，再回退左侧；全部关闭后活动标识为空。
    #[test]
    fn close_tabs_selects_nearest_survivor_and_handles_empty_workspace() {
        let mut s = state();
        for _ in 0..5 {
            new_tab(&mut s, "");
        }
        let ids: Vec<_> = tabs(&s).iter().map(|tab| text(tab, "id")).collect();
        s.session["activeId"] = json!(ids[1]);
        close_tabs(&mut s, &ids[1..3]);
        assert_eq!(s.session["activeId"], ids[3]);
        close_tabs(&mut s, &ids[3..]);
        assert_eq!(s.session["activeId"], ids[0]);
        close_tabs(&mut s, &ids);
        assert!(tabs(&s).is_empty());
        assert_eq!(s.session["activeId"], "");
        assert!(close_tabs(&mut s, &ids).is_empty());
    }
}
