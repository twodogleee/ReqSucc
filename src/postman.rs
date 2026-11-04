//! 本地 Postman Collection v2.0/v2.1 与环境 JSON 转换，不执行脚本或访问导出文件引用的资源。
//!
//! 作者：Andersen

use crate::{
    model,
    storage::{Folder, Snapshot},
};
use serde_json::{Value, json};
use std::{collections::HashSet, io::Read, path::PathBuf};

/// 导入解析与持久化边界回归。
#[cfg(test)]
mod tests;
/// 单次导入文件总字节上限，限制为 20 MiB。
const MAX_BYTES: usize = 20 * 1024 * 1024;
/// 单次最多导入的请求数量。
const MAX_REQUESTS: usize = 10_000;
/// 单次最多创建的集合及目录数量。
const MAX_FOLDERS: usize = 2_000;

/// 已解析且尚未写入工作空间的新增内容，预览不展示凭证或请求正文。
#[derive(Clone, Default)]
pub struct Plan {
    /// 新集合及嵌套目录，全部使用新标识。
    pub folders: Vec<Folder>,
    /// 导入后的 HTTP 请求。
    pub requests: Vec<Value>,
    /// 新环境及其变量。
    pub environments: Vec<Value>,
    /// 根集合标识对应的变量和认证配置。
    pub collections: serde_json::Map<String, Value>,
    /// 去重且有界的兼容性提示，不含文件中的敏感值。
    pub warnings: Vec<String>,
    /// 因不支持的认证、请求体或协议而跳过的请求数量。
    pub skipped: usize,
}
impl Plan {
    /// 添加去重提示，最多保留 30 类问题，避免预览被大文件撑满。
    fn warn(&mut self, message: &str) {
        if self.warnings.len() < 30 && !self.warnings.iter().any(|v| v == message) {
            self.warnings.push(message.into());
        }
    }
    /// 返回仅含数量的预览和完成说明。
    pub fn summary(&self) -> String {
        format!(
            "{} 个集合、{} 个子目录、{} 个接口、{} 个环境；跳过 {} 个接口",
            self.collections.len(),
            self.folders.len() - self.collections.len(),
            self.requests.len(),
            self.environments.len(),
            self.skipped
        )
    }
    /// 将新增内容合并到当前快照，原请求、标签、环境选择和同名数据均保持不变。
    pub fn merge(&self, source: &Snapshot) -> Result<Snapshot, String> {
        if source.folders.len() + self.folders.len() > 20_000
            || source.requests.len() + self.requests.len() > 50_000
        {
            return Err("导入后将超过工作空间的目录或接口数量上限".into());
        }
        let mut result = source.clone();
        model::normalize(&mut result);
        let mut names: HashSet<_> = result
            .folders
            .iter()
            .filter(|f| f.parent.is_none())
            .map(|f| f.name.clone())
            .collect();
        for mut folder in self.folders.clone() {
            if folder.parent.is_none() {
                folder.name = unique_name(&folder.name, &mut names);
            }
            result.folders.push(folder);
        }
        result.requests.extend(self.requests.clone());
        let network = &mut result.session["network"];
        let environments = network["environments"]
            .as_array_mut()
            .ok_or("当前环境配置无效，无法导入")?;
        let mut names = environments
            .iter()
            .map(|e| model::text(e, "name"))
            .collect();
        for mut environment in self.environments.clone() {
            environment["name"] =
                json!(unique_name(&model::text(&environment, "name"), &mut names));
            environments.push(environment);
        }
        network["collections"]
            .as_object_mut()
            .ok_or("当前集合配置无效，无法导入")?
            .extend(self.collections.clone());
        Ok(result)
    }
    /// 识别单份导出，集合严格限制版本，环境必须具有 Postman 环境标记。
    fn document(&mut self, value: &Value) -> Result<(), String> {
        if value["_postman_variable_scope"] == "environment" {
            let variables = self.variables(value.get("values").ok_or("环境缺少 values 列表")?)?;
            self.environments.push(
                json!({"id":model::id(),"name":name(value, "导入的环境"),"variables":variables}),
            );
            return Ok(());
        }
        let schema = model::text(&value["info"], "schema");
        if !["/v2.0.0/collection.json", "/v2.1.0/collection.json"]
            .iter()
            .any(|suffix| schema.ends_with(suffix))
        {
            return Err("请选择 Postman Collection v2.0/v2.1 或环境导出的 JSON 文件；暂不支持 ZIP、全局变量和其他版本".into());
        }
        let items = value["item"].as_array().ok_or("集合缺少有效 item 列表")?;
        let root = self.folder(name(&value["info"], "导入的集合"), None)?;
        let variables = self.variables(value.get("variable").unwrap_or(&json!([])))?;
        let auth = self.auth(&value["auth"]).unwrap_or_else(|_| model::auth());
        self.collections
            .insert(root.clone(), json!({"variables":variables,"auth":auth}));
        self.notices(value);
        self.items(items, &root, &value["auth"], 0)
    }
    /// 分配新目录标识，并在递归构造前限制目录总量。
    fn folder(&mut self, name: String, parent: Option<String>) -> Result<String, String> {
        if self.folders.len() >= MAX_FOLDERS {
            return Err("单次最多导入 2000 个集合和目录".into());
        }
        let id = model::id();
        self.folders.push(Folder {
            id: id.clone(),
            name,
            parent,
        });
        Ok(id)
    }
    /// 按目录层级递归转换请求，认证继承物化为每条请求的有效值，最大深度为 32。
    fn items(
        &mut self,
        items: &[Value],
        folder: &str,
        inherited: &Value,
        depth: usize,
    ) -> Result<(), String> {
        if depth > 32 {
            return Err("集合目录嵌套超过 32 层，未导入任何内容".into());
        }
        for item in items {
            if !item.is_object() {
                return Err("集合条目格式无效".into());
            }
            self.notices(item);
            if let Some(children) = item.get("item") {
                let children = children.as_array().ok_or("目录 item 必须是列表")?;
                let id = self.folder(name(item, "未命名目录"), Some(folder.into()))?;
                if item.get("variable").is_some() {
                    self.warn("文件夹局部变量暂不支持，未导入；发送前请补充到集合或环境变量。");
                }
                let auth = item
                    .get("auth")
                    .filter(|a| !a.is_null())
                    .unwrap_or(inherited);
                self.items(children, &id, auth, depth + 1)?;
            } else {
                if self.requests.len() + self.skipped >= MAX_REQUESTS {
                    return Err("单次最多处理 10000 个接口".into());
                }
                let request = item.get("request").ok_or("集合条目缺少 request 或 item")?;
                match self.request(item, request, folder, inherited) {
                    Ok(request) => self.requests.push(request),
                    Err(reason) => {
                        self.skipped += 1;
                        self.warn(reason);
                    }
                }
            }
        }
        Ok(())
    }
    /// 汇总不能执行或保留的 Postman 功能，仅提示类别而不输出脚本或正文。
    fn notices(&mut self, value: &Value) {
        if value["event"].as_array().is_some_and(|v| !v.is_empty()) {
            self.warn("前置脚本和测试脚本未导入，也不会执行。");
        }
        if value["response"].as_array().is_some_and(|v| !v.is_empty()) {
            self.warn("示例响应未导入。");
        }
        if value.get("protocolProfileBehavior").is_some() {
            self.warn("Postman 专用协议行为设置未导入，使用 ReqSucc 请求设置。");
        }
    }
    /// 转换环境及集合变量；重复键保留最后一项，与 HTTP 变量唯一性契约一致。
    fn variables(&mut self, value: &Value) -> Result<Vec<Value>, String> {
        let mut rows: Vec<Value> = vec![];
        for row in value.as_array().ok_or("变量列表格式无效")? {
            let converted = row_value(row)?;
            let key = model::text(&converted, "key");
            if let Some(index) = rows.iter().position(|r| r["key"] == key) {
                rows.remove(index);
                self.warn("同一作用域的重复变量名保留最后一项。");
            }
            rows.push(converted);
        }
        Ok(rows)
    }
    /// 转换常见认证，缺失或 null 表示无认证；不支持的认证必须跳过对应接口。
    fn auth(&mut self, auth: &Value) -> Result<Value, &'static str> {
        let mut result = model::auth();
        if auth.is_null() || auth["type"] == "noauth" {
            return Ok(result);
        }
        let kind = auth["type"].as_str().unwrap_or("");
        let values = &auth[kind];
        let get = |key: &str| -> String {
            if let Some(rows) = values.as_array() {
                rows.iter()
                    .rev()
                    .find(|row| row["key"] == key)
                    .map(|row| scalar(&row["value"]))
                    .unwrap_or_default()
            } else {
                scalar(&values[key])
            }
        };
        match kind {
            "bearer" => {
                result["auth"] = json!("bearer");
                result["token"] = json!(get("token"));
            }
            "basic" => {
                result["auth"] = json!("basic");
                result["username"] = json!(get("username"));
                result["password"] = json!(get("password"));
            }
            "apikey" => {
                let location = get("in");
                if !["header", "query"].contains(&location.as_str()) {
                    return Err("API Key 位置无效的接口已跳过。");
                }
                result["auth"] = json!("apiKey");
                result["apiKeyName"] = json!(get("key"));
                result["apiKeyValue"] = json!(get("value"));
                result["apiKeyIn"] = json!(location);
            }
            _ => return Err("使用不支持的认证方式（如 OAuth、Digest、AWS 签名）的接口已跳过。"),
        }
        Ok(result)
    }
    /// 转换 HTTP 请求及可用请求体，未支持的协议和文件上传不会被悄悄转为空请求。
    fn request(
        &mut self,
        item: &Value,
        source: &Value,
        folder: &str,
        inherited: &Value,
    ) -> Result<Value, &'static str> {
        if !source.is_object() && !source.is_string() {
            return Err("格式无效的接口已跳过。");
        }
        let mut result = model::request(folder);
        result["name"] = json!(name(item, "未命名接口"));
        result["description"] = json!(description(
            source.get("description").unwrap_or(&item["description"])
        ));
        result["method"] = json!(source["method"].as_str().unwrap_or("GET"));
        let url = if source.is_string() {
            source
        } else {
            &source["url"]
        };
        let mut address = address(url).ok_or("缺少有效地址的接口已跳过。")?;
        if address.contains("://")
            && !address.starts_with("http://")
            && !address.starts_with("https://")
            && !address.starts_with("{{")
        {
            return Err("非 HTTP 协议接口已跳过。");
        }
        if let Some(variables) = url["variable"].as_array() {
            for variable in variables {
                let key = model::text(variable, "key");
                if !key.is_empty() {
                    let value = scalar(&variable["value"]);
                    let replacement = if value.is_empty() {
                        format!("{{{{{key}}}}}")
                    } else {
                        value
                    };
                    let suffix_start = address.find(['?', '#']).unwrap_or(address.len());
                    let (base, suffix) = address.split_at(suffix_start);
                    let replaced = base
                        .split('/')
                        .map(|part| {
                            if part == format!(":{key}") {
                                replacement.as_str()
                            } else {
                                part
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("/");
                    address = format!("{replaced}{suffix}");
                }
            }
        }
        result["url"] = json!(address);
        model::url_changed(&mut result);
        if let Some(query) = url.get("query") {
            result["params"] = json!(rows(query).map_err(|_| "参数列表无效的接口已跳过。")?);
            model::params_changed(&mut result);
        }
        result["headers"] = json!(
            rows(source.get("header").unwrap_or(&json!([])))
                .map_err(|_| "请求头列表无效的接口已跳过。")?
        );
        let auth = self.auth(
            source
                .get("auth")
                .filter(|a| !a.is_null())
                .unwrap_or(inherited),
        )?;
        for (key, value) in auth.as_object().into_iter().flatten() {
            result[key] = value.clone();
        }
        let body = &source["body"];
        if body["disabled"] != true {
            match body["mode"].as_str().unwrap_or("") {
                "" => {}
                "raw" => {
                    result["body"] = json!(scalar(&body["raw"]));
                    let language = body["options"]["raw"]["language"]
                        .as_str()
                        .unwrap_or("text");
                    result["bodyType"] = json!(match language {
                        "json" => "json",
                        "xml" => "xml",
                        _ => "text",
                    });
                }
                "urlencoded" => {
                    result["bodyType"] = json!("form");
                    result["form"] =
                        json!(rows(&body["urlencoded"]).map_err(|_| "表单字段无效的接口已跳过。")?);
                }
                _ => {
                    return Err(
                        "使用 multipart、文件上传或 GraphQL 等暂不支持请求体的接口已跳过。",
                    );
                }
            }
        }
        Ok(result)
    }
}

/// 从本地普通文件读取并转换，限制文件数和总量，任一文件格式错误则整批不导入。
pub fn read_files(paths: &[PathBuf]) -> Result<Plan, String> {
    if paths.is_empty() || paths.len() > 20 {
        return Err("每次请选择 1–20 个 Postman JSON 文件".into());
    }
    let mut plan = Plan::default();
    let mut remaining = MAX_BYTES;
    for path in paths {
        if !std::fs::metadata(path)
            .map_err(|_| "无法读取导入文件信息")?
            .is_file()
        {
            return Err("请选择普通 JSON 文件".into());
        }
        let file = std::fs::File::open(path).map_err(|_| "无法读取所选导入文件，请检查权限")?;
        if !file
            .metadata()
            .map_err(|_| "无法读取导入文件信息")?
            .is_file()
        {
            return Err("请选择普通 JSON 文件".into());
        }
        let mut bytes = vec![];
        file.take(remaining as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "导入文件读取失败")?;
        if bytes.len() > remaining {
            return Err("单次导入文件总大小不能超过 20 MiB".into());
        }
        remaining -= bytes.len();
        let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
        let value: Value = serde_json::from_slice(bytes).map_err(|e| {
            format!(
                "导入文件不是有效 JSON（第 {} 行、第 {} 列），未导入任何内容",
                e.line(),
                e.column()
            )
        })?;
        plan.document(&value)?;
    }
    if plan.folders.is_empty() && plan.environments.is_empty() {
        return Err("文件中没有可导入的集合或环境".into());
    }
    Ok(plan)
}
/// 为同名根集合或环境分配确定的新增名称，原有名称不改变。
fn unique_name(name: &str, used: &mut HashSet<String>) -> String {
    let mut candidate = name.to_owned();
    let mut index = 2;
    while !used.insert(candidate.clone()) {
        candidate = format!("{name}（导入 {index}）");
        index += 1;
    }
    candidate
}
/// 使用非空导出名称，空名称回退为中文默认名称。
fn name(value: &Value, fallback: &str) -> String {
    value["name"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(fallback)
        .into()
}
/// 读取字符串或 Postman 描述对象中的正文。
fn description(value: &Value) -> String {
    value
        .as_str()
        .or_else(|| value["content"].as_str())
        .unwrap_or("")
        .into()
}
/// 将变量基本值转换为文本，保留 false、0 和显式空字符串。
fn scalar(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        _ => value.to_string(),
    }
}
/// 转换单条参数或变量，保留启用状态、说明和敏感标记。
fn row_value(row: &Value) -> Result<Value, String> {
    let key = row["key"].as_str().ok_or("参数或变量缺少字符串 key")?;
    Ok(
        json!({"id":model::id(),"key":key,"value":scalar(&row["value"]),"description":description(&row["description"]),"enabled":row["disabled"] != true && row["enabled"] != false,"secret":row["type"] == "secret"}),
    )
}
/// 转换有序键值列表，保留重复请求参数与请求头。
fn rows(value: &Value) -> Result<Vec<Value>, String> {
    value
        .as_array()
        .ok_or("参数列表格式无效".into())
        .and_then(|rows| rows.iter().map(row_value).collect())
}
/// 从字符串、raw 或结构化 URL 还原地址，变量模板不展开。
fn address(value: &Value) -> Option<String> {
    if let Some(raw) = value.as_str().or_else(|| value["raw"].as_str()) {
        return Some(raw.into());
    }
    let join = |value: &Value, separator: &str| -> String {
        value
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(separator)
            })
            .unwrap_or_else(|| scalar(value))
    };
    let host = join(&value["host"], ".");
    if host.is_empty() {
        return None;
    }
    let protocol = value["protocol"].as_str().unwrap_or("https");
    let port = scalar(&value["port"]);
    let path = join(&value["path"], "/");
    Some(format!(
        "{protocol}://{host}{}{path}",
        if port.is_empty() {
            "/".into()
        } else {
            format!(":{port}/")
        }
    ))
}
