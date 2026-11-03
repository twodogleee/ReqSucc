use base64::{Engine, engine::general_purpose::STANDARD};
use reqwest::{
    Method, Url,
    header::{HeaderMap, HeaderName, HeaderValue},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const PREVIEW: usize = 2 * 1024 * 1024;
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    #[serde(default)]
    pub id: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    pub key: String,
    pub value: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub secret: bool,
}
fn yes() -> bool {
    true
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Auth {
    pub kind: String,
    pub token: String,
    pub username: String,
    pub password: String,
    pub api_key_name: String,
    pub api_key_value: String,
    pub api_key_in: String,
}
impl Default for Auth {
    fn default() -> Self {
        Self {
            kind: "none".into(),
            token: String::new(),
            username: String::new(),
            password: String::new(),
            api_key_name: String::new(),
            api_key_value: String::new(),
            api_key_in: "header".into(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Options {
    pub timeout_ms: u64,
    pub connect_timeout_ms: u64,
    pub follow_redirects: bool,
    pub verify_tls: bool,
    pub max_response_mib: u64,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            timeout_ms: 30000,
            connect_timeout_ms: 10000,
            follow_redirects: true,
            verify_tls: true,
            max_response_mib: 100,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Input {
    #[serde(default)]
    pub query_synced: bool,
    pub method: String,
    pub url: String,
    pub params: Vec<Row>,
    pub headers: Vec<Row>,
    pub body: String,
    pub body_type: String,
    #[serde(default)]
    pub form: Vec<Row>,
    pub auth: Auth,
    pub options: Options,
    pub globals: Vec<Row>,
    pub collection_variables: Vec<Row>,
    pub environment_variables: Vec<Row>,
    pub environment_name: String,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Failure {
    pub kind: String,
    pub message: String,
    pub elapsed_ms: u64,
}
fn fail(kind: &str, message: impl Into<String>) -> Failure {
    Failure {
        kind: kind.into(),
        message: message.into(),
        elapsed_ms: 0,
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct Pair {
    pub name: String,
    pub value: String,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Effective {
    pub method: String,
    pub url: String,
    pub headers: Vec<Pair>,
    pub body: String,
    pub environment: String,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Output {
    pub run_id: String,
    pub status: u16,
    pub status_text: String,
    pub elapsed_ms: u64,
    pub received_bytes: usize,
    pub final_url: String,
    pub redirects: usize,
    pub headers: Vec<Pair>,
    pub cookies: Vec<Pair>,
    pub body: String,
    pub binary: bool,
    pub encoding: String,
    pub preview_limited: bool,
    pub incomplete: bool,
    pub effective: Effective,
}
struct Prepared {
    method: Method,
    url: Url,
    headers: HeaderMap,
    body: Option<Vec<u8>>,
    options: Options,
    effective: Effective,
    secrets: Vec<String>,
    has_sensitive: bool,
}
struct Run {
    token: CancellationToken,
    prepared: Option<Prepared>,
}
#[derive(Default)]
pub struct HttpService {
    runs: Mutex<HashMap<String, Run>>,
    files: Mutex<HashMap<String, Arc<tempfile::NamedTempFile>>>,
}
fn sensitive(key: &str) -> bool {
    let s = key.to_lowercase();
    s.contains("token")
        || s.contains("secret")
        || s.contains("password")
        || s.contains("api-key")
        || s.contains("api_key")
        || s == "authorization"
        || s == "proxy-authorization"
        || s == "cookie"
        || s == "set-cookie"
}
fn mask(value: &str, secrets: &[String]) -> String {
    let mut s = value.to_owned();
    for secret in secrets {
        if !secret.is_empty() {
            s = s.replace(secret, "••••••");
            let encoded: String = url::form_urlencoded::byte_serialize(secret.as_bytes()).collect();
            s = s.replace(&encoded, "••••••");
        }
    }
    s
}
fn safe_url(url: &Url, secrets: &[String]) -> String {
    let mut u = url.clone();
    if url.query().is_some() {
        let pairs: Vec<_> = url
            .query_pairs()
            .map(|(k, v)| {
                let value = if sensitive(&k) {
                    "••••••".into()
                } else {
                    mask(&v, secrets)
                };
                (k.into_owned(), value)
            })
            .collect();
        u.set_query(None);
        u.query_pairs_mut().extend_pairs(pairs);
    }
    mask(u.as_str(), secrets)
}
fn variables(input: &Input) -> Result<(HashMap<String, String>, Vec<String>), Failure> {
    let mut map = HashMap::new();
    let mut secrets = Vec::new();
    for scope in [
        &input.globals,
        &input.collection_variables,
        &input.environment_variables,
    ] {
        let mut keys = std::collections::HashSet::new();
        for row in scope.iter().filter(|r| r.enabled) {
            let key = row.key.trim();
            if key.is_empty() {
                continue;
            }
            if !keys.insert(key) {
                return Err(fail("validation", format!("同一作用域内的变量重复：{key}")));
            }
            if row.secret || sensitive(key) {
                secrets.push(row.value.clone());
            }
            map.insert(key.to_owned(), row.value.clone());
        }
    }
    Ok((map, secrets))
}
fn expand(
    value: &str,
    label: &str,
    map: &HashMap<String, String>,
    missing: &mut Vec<String>,
) -> String {
    let mut rest = value;
    let mut result = String::new();
    while let Some(start) = rest.find("{{") {
        result.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        if let Some(end) = tail.find("}}") {
            let key = tail[..end].trim();
            if let Some(v) = map.get(key) {
                result.push_str(v);
            } else {
                missing.push(format!("{label}：{{{{{key}}}}}"));
            }
            rest = &tail[end + 2..];
        } else {
            result.push_str(&rest[start..]);
            rest = "";
            break;
        }
    }
    result.push_str(rest);
    result
}
fn preview_body(body: &Option<Vec<u8>>, kind: &str, secrets: &[String]) -> String {
    fn redact(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(map) => {
                for (k, v) in map {
                    if sensitive(k) {
                        *v = serde_json::Value::String("••••••".into());
                    } else {
                        redact(v);
                    }
                }
            }
            serde_json::Value::Array(list) => {
                for v in list {
                    redact(v);
                }
            }
            _ => {}
        }
    }
    let text = String::from_utf8_lossy(body.as_deref().unwrap_or_default());
    let masked = if kind == "json" {
        if let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&text) {
            redact(&mut value);
            value.to_string()
        } else {
            text.into_owned()
        }
    } else if kind == "form" {
        let pairs: Vec<_> = url::form_urlencoded::parse(text.as_bytes())
            .map(|(k, v)| {
                let v = if sensitive(&k) {
                    "••••••".into()
                } else {
                    v.into_owned()
                };
                (k.into_owned(), v)
            })
            .collect();
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        serializer.extend_pairs(pairs);
        serializer.finish()
    } else {
        text.into_owned()
    };
    let mut preview = mask(&masked, secrets);
    if preview.len() > PREVIEW {
        let mut end = PREVIEW;
        while !preview.is_char_boundary(end) {
            end -= 1;
        }
        preview.truncate(end);
        preview.push_str("\n…（请求预览已截断）");
    }
    preview
}
fn prepare(input: Input) -> Result<Prepared, Failure> {
    let o = &input.options;
    if !(1..=3600000).contains(&o.timeout_ms)
        || !(1..=3600000).contains(&o.connect_timeout_ms)
        || !(1..=1024).contains(&o.max_response_mib)
    {
        return Err(fail(
            "validation",
            "超时须为 1–3600000 毫秒，响应上限须为 1–1024 MiB",
        ));
    }
    let (vars, mut secrets) = variables(&input)?;
    let mut missing = Vec::new();
    let url_template = if input.query_synced {
        input
            .url
            .split_once('?')
            .map(|(base, _)| base)
            .unwrap_or(&input.url)
    } else {
        &input.url
    };
    let raw_url = expand(url_template.trim(), "请求地址", &vars, &mut missing);
    let pairs: Vec<_> = input
        .params
        .iter()
        .filter(|r| r.enabled)
        .map(|r| {
            (
                expand(&r.key, "参数名称", &vars, &mut missing),
                expand(&r.value, &format!("参数 {}", r.key), &vars, &mut missing),
            )
        })
        .collect();
    let mut headers: Vec<_> = input
        .headers
        .iter()
        .filter(|r| r.enabled && !r.key.is_empty())
        .map(|r| {
            (
                expand(&r.key, "请求头名称", &vars, &mut missing),
                expand(&r.value, &format!("请求头 {}", r.key), &vars, &mut missing),
            )
        })
        .collect();
    let mut auth = input.auth.clone();
    match auth.kind.as_str() {
        "bearer" => auth.token = expand(&auth.token, "认证令牌", &vars, &mut missing),
        "basic" => {
            auth.username = expand(&auth.username, "认证用户名", &vars, &mut missing);
            auth.password = expand(&auth.password, "认证密码", &vars, &mut missing);
        }
        "apiKey" => {
            auth.api_key_name = expand(&auth.api_key_name, "API Key 名称", &vars, &mut missing);
            auth.api_key_value = expand(&auth.api_key_value, "API Key 值", &vars, &mut missing);
        }
        "none" => {}
        _ => return Err(fail("validation", "认证类型无效，请检查集合认证设置")),
    }
    let body = match input.body_type.as_str() {
        "none" => None,
        "json" | "text" | "xml" => {
            Some(expand(&input.body, "请求体", &vars, &mut missing).into_bytes())
        }
        "form" => {
            let form: Vec<_> = input
                .form
                .iter()
                .filter(|r| r.enabled)
                .map(|r| {
                    (
                        expand(&r.key, "表单名称", &vars, &mut missing),
                        expand(&r.value, &format!("表单 {}", r.key), &vars, &mut missing),
                    )
                })
                .collect();
            let mut serializer = url::form_urlencoded::Serializer::new(String::new());
            serializer.extend_pairs(form);
            Some(serializer.finish().into_bytes())
        }
        _ => return Err(fail("validation", "此请求体类型尚不支持")),
    };
    if !missing.is_empty() {
        missing.sort();
        missing.dedup();
        return Err(fail(
            "variables",
            format!("存在未定义变量，请先填写：\n{}", missing.join("\n")),
        ));
    }
    let mut url = Url::parse(&raw_url).map_err(|_| {
        fail(
            "validation",
            "请求地址无效，请输入完整的 http:// 或 https:// 地址",
        )
    })?;
    if !["http", "https"].contains(&url.scheme()) || url.host_str().is_none() {
        return Err(fail("validation", "仅支持 HTTP 和 HTTPS 请求"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(fail(
            "validation",
            "请在认证面板填写凭据，不支持 URL 内嵌用户名或密码",
        ));
    }
    url.set_fragment(None);
    if !pairs.is_empty() {
        url.query_pairs_mut().extend_pairs(pairs);
    }
    let method = Method::from_bytes(input.method.as_bytes())
        .map_err(|_| fail("validation", "HTTP 方法格式无效"))?;
    let auth_header = match auth.kind.as_str() {
        "bearer" => {
            secrets.push(auth.token.clone());
            Some(("Authorization".to_owned(), format!("Bearer {}", auth.token)))
        }
        "basic" => {
            if auth.username.contains(':') {
                return Err(fail("validation", "Basic 认证用户名不能包含冒号"));
            }
            secrets.push(auth.password.clone());
            Some((
                "Authorization".to_owned(),
                format!(
                    "Basic {}",
                    STANDARD.encode(format!("{}:{}", auth.username, auth.password))
                ),
            ))
        }
        "apiKey" => {
            if auth.api_key_name.trim().is_empty() {
                return Err(fail("validation", "API Key 名称不能为空"));
            }
            secrets.push(auth.api_key_value.clone());
            if auth.api_key_in == "query" {
                if url.query_pairs().any(|(k, _)| k == auth.api_key_name) {
                    return Err(fail("validation", "API Key 与查询参数同名，请删除重复配置"));
                }
                url.query_pairs_mut()
                    .append_pair(&auth.api_key_name, &auth.api_key_value);
                None
            } else if auth.api_key_in == "header" {
                Some((auth.api_key_name.clone(), auth.api_key_value.clone()))
            } else {
                return Err(fail("validation", "API Key 位置无效"));
            }
        }
        _ => None,
    };
    if let Some((key, value)) = auth_header {
        if headers.iter().any(|(k, _)| k.eq_ignore_ascii_case(&key)) {
            return Err(fail(
                "validation",
                format!("认证生成的 {key} 与手工请求头冲突，请禁用或删除其中一项"),
            ));
        }
        headers.push((key, value));
    }
    if body.is_some()
        && !headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("content-type"))
    {
        headers.push((
            "Content-Type".into(),
            match input.body_type.as_str() {
                "json" => "application/json",
                "xml" => "application/xml",
                "form" => "application/x-www-form-urlencoded",
                _ => "text/plain; charset=utf-8",
            }
            .into(),
        ));
    }
    if !headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("user-agent"))
    {
        headers.push((
            "User-Agent".into(),
            format!("ReqSucc/{}", env!("CARGO_PKG_VERSION")),
        ));
    }
    let mut h = HeaderMap::new();
    let mut display = Vec::new();
    let mut has_sensitive = auth.kind != "none";
    for (key, value) in headers {
        if [
            "host",
            "content-length",
            "transfer-encoding",
            "connection",
            "proxy-authorization",
        ]
        .iter()
        .any(|k| key.eq_ignore_ascii_case(k))
        {
            return Err(fail(
                "validation",
                format!("{key} 由请求执行器管理，请移除此请求头"),
            ));
        }
        let name = HeaderName::from_bytes(key.as_bytes())
            .map_err(|_| fail("validation", format!("请求头名称无效：{key}")))?;
        let val = HeaderValue::from_str(&value)
            .map_err(|_| fail("validation", format!("请求头 {key} 包含非法字符")))?;
        let hidden = sensitive(&key)
            || (auth.kind == "apiKey" && key.eq_ignore_ascii_case(&auth.api_key_name));
        if hidden {
            has_sensitive = true;
            secrets.push(value.clone());
        }
        display.push(Pair {
            name: key,
            value: if hidden {
                "••••••".into()
            } else {
                mask(&value, &secrets)
            },
        });
        h.append(name, val);
    }
    secrets.retain(|s| !s.is_empty());
    secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
    secrets.dedup();
    for header in &mut display {
        header.value = mask(&header.value, &secrets);
    }
    let effective = Effective {
        method: method.to_string(),
        url: safe_url(&url, &secrets),
        headers: display,
        body: preview_body(&body, &input.body_type, &secrets),
        environment: input.environment_name,
    };
    Ok(Prepared {
        method,
        url,
        headers: h,
        body,
        options: input.options,
        effective,
        secrets,
        has_sensitive,
    })
}
impl HttpService {
    pub fn start(&self, input: Input) -> Result<String, Failure> {
        let prepared = prepare(input)?;
        let mut runs = self
            .runs
            .lock()
            .map_err(|_| fail("internal", "请求服务不可用"))?;
        if runs.len() >= 5 {
            return Err(fail("limit", "最多同时发送 5 个请求，请等待或取消其他请求"));
        }
        let id = Uuid::new_v4().to_string();
        runs.insert(
            id.clone(),
            Run {
                token: CancellationToken::new(),
                prepared: Some(prepared),
            },
        );
        Ok(id)
    }
    pub fn cancel(&self, id: &str) -> Result<(), String> {
        if let Some(run) = self.runs.lock().map_err(|_| "请求服务不可用")?.get(id) {
            run.token.cancel();
        }
        Ok(())
    }
    pub fn release(&self, id: &str) {
        if let Ok(mut files) = self.files.lock() {
            files.remove(id);
        }
    }
    pub fn clear(&self) {
        if let Ok(runs) = self.runs.lock() {
            for run in runs.values() {
                run.token.cancel();
            }
        }
        if let Ok(mut files) = self.files.lock() {
            files.clear();
        }
    }
    pub fn file(&self, id: &str) -> Result<Arc<tempfile::NamedTempFile>, String> {
        self.files
            .lock()
            .map_err(|_| "响应缓存不可用")?
            .get(id)
            .cloned()
            .ok_or("响应文件已释放，请重新发送请求".into())
    }
    pub async fn execute(&self, id: String) -> Result<Output, Failure> {
        let (p, token) = {
            let mut runs = self
                .runs
                .lock()
                .map_err(|_| fail("internal", "请求服务不可用"))?;
            let run = runs
                .get_mut(&id)
                .ok_or_else(|| fail("validation", "请求已失效"))?;
            (
                run.prepared
                    .take()
                    .ok_or_else(|| fail("validation", "请求已经发送，请勿重复执行"))?,
                run.token.clone(),
            )
        };
        let start = Instant::now();
        let timeout = p.options.timeout_ms;
        let result = tokio::select! {biased; _=token.cancelled()=>Err(fail("cancelled","请求已取消；服务器可能仍在处理本次操作")),r=tokio::time::timeout(Duration::from_millis(timeout),self.perform(&id,p))=>r.unwrap_or_else(|_|Err(fail("timeout","请求已超时，请检查服务或调整超时时间")))};
        if let Ok(mut runs) = self.runs.lock() {
            runs.remove(&id);
        }
        result.map_err(|mut e| {
            e.elapsed_ms = start.elapsed().as_millis() as u64;
            e
        })
    }
    async fn perform(&self, id: &str, p: Prepared) -> Result<Output, Failure> {
        let started = Instant::now();
        let mut url = p.url.clone();
        let mut method = p.method.clone();
        let mut body = p.body.clone();
        let mut headers = p.headers.clone();
        let mut redirects = 0;
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .referer(false)
            .tls_danger_accept_invalid_certs(!p.options.verify_tls)
            .connect_timeout(Duration::from_millis(p.options.connect_timeout_ms))
            .build()
            .map_err(network_error)?;
        let mut response = loop {
            let mut builder = client
                .request(method.clone(), url.clone())
                .headers(headers.clone());
            if let Some(b) = &body {
                builder = builder.body(b.clone());
            }
            let response = builder.send().await.map_err(network_error)?;
            if p.options.follow_redirects
                && [301, 302, 303, 307, 308].contains(&response.status().as_u16())
            {
                if let Some(location) = response.headers().get("location") {
                    if redirects >= 10 {
                        return Err(fail("redirect", "重定向超过 10 次，请检查服务配置"));
                    }
                    let next = url
                        .join(
                            location
                                .to_str()
                                .map_err(|_| fail("redirect", "重定向地址格式无效"))?,
                        )
                        .map_err(|_| fail("redirect", "重定向地址格式无效"))?;
                    if !["http", "https"].contains(&next.scheme())
                        || !next.username().is_empty()
                        || next.password().is_some()
                    {
                        return Err(fail("redirect", "已阻止无效协议或内嵌凭据的重定向"));
                    }
                    if url.origin() != next.origin() {
                        // 携带凭据、敏感变量或请求体时不跨源跟随，避免转发私有数据。
                        if p.has_sensitive
                            || !p.secrets.is_empty()
                            || body.is_some()
                            || url.scheme() == "https" && next.scheme() == "http"
                        {
                            return Err(fail(
                                "redirect",
                                "已阻止携带认证、敏感变量或请求体的跨源重定向；请确认目标后直接发送",
                            ));
                        }
                        for key in ["authorization", "cookie"] {
                            headers.remove(key);
                        }
                    }
                    if response.status() == 303 && method != Method::HEAD
                        || [301, 302].contains(&response.status().as_u16())
                            && method == Method::POST
                    {
                        method = Method::GET;
                        body = None;
                        headers.remove("content-type");
                    }
                    url = next;
                    redirects += 1;
                    continue;
                }
            }
            break response;
        };
        let status = response.status().as_u16();
        let response_headers: Vec<Pair> = response
            .headers()
            .iter()
            .map(|(k, v)| Pair {
                name: k.to_string(),
                value: String::from_utf8_lossy(v.as_bytes()).into_owned(),
            })
            .collect();
        let cookies = response_headers
            .iter()
            .filter(|p| p.name.eq_ignore_ascii_case("set-cookie"))
            .cloned()
            .collect();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("")
            .to_owned();
        let file =
            tempfile::NamedTempFile::new().map_err(|_| fail("storage", "无法创建响应临时文件"))?;
        let mut writer = tokio::fs::File::from_std(
            file.reopen()
                .map_err(|_| fail("storage", "无法打开响应缓存"))?,
        );
        let limit = p.options.max_response_mib as usize * 1024 * 1024;
        let mut received = 0;
        let mut preview = Vec::new();
        let mut incomplete = false;
        while let Some(chunk) = response.chunk().await.map_err(network_error)? {
            let count = chunk.len().min(limit - received);
            writer
                .write_all(&chunk[..count])
                .await
                .map_err(|_| fail("storage", "响应写入磁盘失败，请检查剩余空间"))?;
            let n = count.min(PREVIEW.saturating_sub(preview.len()));
            preview.extend_from_slice(&chunk[..n]);
            received += count;
            if count < chunk.len() {
                incomplete = true;
                break;
            }
        }
        writer
            .flush()
            .await
            .map_err(|_| fail("storage", "响应缓存写入失败"))?;
        drop(writer);
        let mime = content_type
            .split(';')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let text_mime = mime.starts_with("text/")
            || mime.contains("json")
            || mime.contains("xml")
            || mime.contains("javascript")
            || mime.contains("x-www-form-urlencoded");
        let binary = if mime.is_empty() {
            preview.contains(&0) || std::str::from_utf8(&preview).is_err()
        } else {
            !text_mime
        };
        let charset = content_type
            .split(';')
            .skip(1)
            .find_map(|s| {
                s.trim()
                    .split_once('=')
                    .filter(|(k, _)| k.trim().eq_ignore_ascii_case("charset"))
                    .map(|(_, v)| v.trim().trim_matches('"'))
            })
            .unwrap_or("utf-8");
        let encoding =
            encoding_rs::Encoding::for_label(charset.as_bytes()).unwrap_or(encoding_rs::UTF_8);
        let (text, _, bad) = encoding.decode(&preview);
        let binary = binary || (bad && received <= PREVIEW && !incomplete);
        let output = Output {
            run_id: id.into(),
            status,
            status_text: status_label(status).into(),
            elapsed_ms: started.elapsed().as_millis() as u64,
            received_bytes: received,
            final_url: safe_url(&url, &p.secrets),
            redirects,
            headers: response_headers,
            cookies,
            body: if binary {
                String::new()
            } else {
                text.into_owned()
            },
            binary,
            encoding: encoding.name().into(),
            preview_limited: received > PREVIEW,
            incomplete,
            effective: p.effective,
        };
        let mut files = self
            .files
            .lock()
            .map_err(|_| fail("storage", "响应缓存不可用"))?;
        if files.len() >= 50 {
            return Err(fail(
                "limit",
                "响应缓存已达 50 份，请关闭部分请求标签后重试",
            ));
        }
        files.insert(id.into(), Arc::new(file));
        Ok(output)
    }
}
fn network_error(e: reqwest::Error) -> Failure {
    let detail = format!("{e:?}").to_lowercase();
    if e.is_timeout() {
        fail("timeout", "连接或读取响应超时，请检查服务或调整超时")
    } else if detail.contains("dns") || detail.contains("resolve") {
        fail("dns", "域名解析失败，请检查地址和网络配置")
    } else if detail.contains("certificate") || detail.contains("tls") || detail.contains("cert") {
        fail("tls", "TLS 连接或证书校验失败，请检查证书与系统时间")
    } else if e.is_connect() {
        fail("connect", "无法连接服务，请检查主机、端口和网络")
    } else {
        fail(
            "network",
            "网络传输失败或响应连接意外中断，请检查服务后重试",
        )
    }
}
fn status_label(code: u16) -> &'static str {
    match code {
        200 => "成功",
        201 => "已创建",
        202 => "已接受",
        204 => "无内容",
        301 => "永久重定向",
        302 => "临时重定向",
        304 => "未修改",
        400 => "请求错误",
        401 => "未认证",
        403 => "禁止访问",
        404 => "未找到",
        405 => "方法不允许",
        409 => "请求冲突",
        422 => "无法处理内容",
        429 => "请求过于频繁",
        500 => "服务器内部错误",
        502 => "网关错误",
        503 => "服务不可用",
        504 => "网关超时",
        100..=199 => "信息响应",
        200..=299 => "请求成功",
        300..=399 => "重定向",
        400..=499 => "客户端错误",
        _ => "服务端错误",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };
    fn row(key: &str, value: &str) -> Row {
        Row {
            id: Uuid::new_v4().to_string(),
            enabled: true,
            key: key.into(),
            value: value.into(),
            description: String::new(),
            secret: false,
        }
    }
    fn input(url: String) -> Input {
        Input {
            query_synced: false,
            method: "GET".into(),
            url,
            params: vec![],
            headers: vec![],
            body: String::new(),
            body_type: "none".into(),
            form: vec![],
            auth: Auth::default(),
            options: Options::default(),
            globals: vec![],
            collection_variables: vec![],
            environment_variables: vec![],
            environment_name: "本地".into(),
        }
    }
    async fn server(
        body: Vec<u8>,
        status: &str,
        extra: &str,
        delay: u64,
    ) -> (String, tokio::sync::oneshot::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let status = status.to_owned();
        let extra = extra.to_owned();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut data = Vec::new();
            let mut buf = [0; 4096];
            loop {
                let n = socket.read(&mut buf).await.unwrap_or(0);
                if n == 0 {
                    break;
                }
                data.extend_from_slice(&buf[..n]);
                if let Some(end) = data.windows(4).position(|x| x == b"\r\n\r\n") {
                    let header = String::from_utf8_lossy(&data[..end]);
                    let len = header
                        .lines()
                        .find_map(|l| {
                            l.to_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|s| s.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if data.len() >= end + 4 + len {
                        break;
                    }
                }
            }
            let _ = tx.send(String::from_utf8_lossy(&data).into_owned());
            tokio::time::sleep(Duration::from_millis(delay)).await;
            let h = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n",
                body.len()
            );
            let _ = socket.write_all(h.as_bytes()).await;
            let _ = socket.write_all(&body).await;
        });
        (url, rx)
    }
    #[test]
    fn variables_precedence_empty_disabled_and_single_level() {
        let mut i = input("http://localhost/{{path}}".into());
        i.globals = vec![row("path", "global"), row("empty", "")];
        i.collection_variables = vec![row("path", "collection")];
        i.environment_variables = vec![
            row("path", "environment"),
            row("nested", "{{not-expanded}}"),
        ];
        i.params = vec![row("x", "{{empty}}"), row("n", "{{nested}}")];
        let mut disabled = row("ignored", "{{missing}}");
        disabled.enabled = false;
        i.params.push(disabled);
        let p = prepare(i).unwrap();
        assert_eq!(p.url.path(), "/environment");
        assert_eq!(
            p.url.query_pairs().collect::<Vec<_>>(),
            vec![
                ("x".into(), "".into()),
                ("n".into(), "{{not-expanded}}".into())
            ]
        );
    }
    #[test]
    fn missing_variables_and_auth_conflict_block_before_network() {
        let mut i = input("http://localhost/{{missing}}".into());
        assert_eq!(prepare(i.clone()).err().unwrap().kind, "variables");
        i.url = "http://localhost".into();
        i.auth.kind = "bearer".into();
        i.auth.token = "test".into();
        i.headers.push(row("authorization", "manual"));
        assert!(prepare(i).err().unwrap().message.contains("冲突"));
    }
    #[tokio::test]
    async fn sends_resolved_auth_repeated_query_and_json_and_keeps_500_response() {
        let(url,rx)=server(b"{\"error\":\"bad\"}".to_vec(),"500 Internal Server Error","Content-Type: application/json\r\nX-Repeat: first\r\nX-Repeat: second\r\nSet-Cookie: a=1\r\nSet-Cookie: b=2\r\n",0).await;
        let mut i = input(format!("{url}/echo?q=raw"));
        i.method = "POST".into();
        i.params = vec![row("q", "中文 &"), row("q", "")];
        i.auth = Auth {
            kind: "bearer".into(),
            token: "{{token}}".into(),
            ..Auth::default()
        };
        i.environment_variables = vec![row("token", "secret-value")];
        i.body_type = "json".into();
        i.body = "{\"value\":\"{{token}}\"}".into();
        let s = HttpService::default();
        let id = s.start(i).unwrap();
        let r = s.execute(id.clone()).await.unwrap();
        let wire = rx.await.unwrap();
        assert!(wire.contains("authorization: Bearer secret-value"));
        assert!(wire.contains("q=raw&q=%E4%B8%AD%E6%96%87+%26&q="));
        assert!(wire.contains("{\"value\":\"secret-value\"}"));
        assert_eq!(r.status, 500);
        assert!(r.body.contains("bad"));
        assert_eq!(r.headers.iter().filter(|p| p.name == "x-repeat").count(), 2);
        assert_eq!(r.cookies.len(), 2);
        assert!(
            !serde_json::to_string(&r.effective)
                .unwrap()
                .contains("secret-value")
        );
        assert!(s.file(&id).is_ok());
        s.release(&id);
        assert!(s.file(&id).is_err());
    }
    #[tokio::test]
    async fn basic_auth_form_encoding_and_synced_query_sent_once() {
        let (url, rx) = server(b"ok".to_vec(), "200 OK", "Content-Type: text/plain\r\n", 0).await;
        let mut i = input(format!("{url}/?a=one"));
        i.query_synced = true;
        i.params = vec![row("a", "one")];
        i.method = "POST".into();
        i.auth = Auth {
            kind: "basic".into(),
            username: "user".into(),
            password: "pass".into(),
            ..Auth::default()
        };
        i.body_type = "form".into();
        i.form = vec![row("q", "中文 +"), row("q", "")];
        let s = HttpService::default();
        let id = s.start(i).unwrap();
        s.execute(id).await.unwrap();
        let wire = rx.await.unwrap();
        assert!(wire.contains("authorization: Basic dXNlcjpwYXNz"));
        assert!(wire.starts_with("POST /?a=one HTTP/1.1"));
        assert!(wire.ends_with("q=%E4%B8%AD%E6%96%87+%2B&q="));
    }
    #[test]
    fn api_keys_support_header_query_and_redaction() {
        let mut i = input("http://localhost".into());
        i.auth = Auth {
            kind: "apiKey".into(),
            api_key_name: "X-Custom".into(),
            api_key_value: "top-secret".into(),
            api_key_in: "header".into(),
            ..Auth::default()
        };
        let p = prepare(i.clone()).unwrap();
        assert_eq!(p.headers["x-custom"], "top-secret");
        assert!(
            !serde_json::to_string(&p.effective)
                .unwrap()
                .contains("top-secret")
        );
        i.auth.api_key_in = "query".into();
        let p = prepare(i).unwrap();
        assert!(p.url.query().unwrap().contains("top-secret"));
        assert!(!p.effective.url.contains("top-secret"));
    }
    #[tokio::test]
    async fn cancellation_stops_wait_and_releases_concurrency_slot() {
        let (url, rx) = server(b"slow".to_vec(), "200 OK", "", 1500).await;
        let s = Arc::new(HttpService::default());
        let id = s.start(input(url)).unwrap();
        let task = {
            let s = s.clone();
            let id = id.clone();
            tokio::spawn(async move { s.execute(id).await })
        };
        rx.await.unwrap();
        s.cancel(&id).unwrap();
        let r = tokio::time::timeout(Duration::from_millis(500), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(r.kind, "cancelled");
        assert!(s.runs.lock().unwrap().is_empty());
    }
    #[tokio::test]
    async fn cancel_before_execute_never_sends() {
        let s = HttpService::default();
        let id = s.start(input("http://127.0.0.1:9".into())).unwrap();
        s.cancel(&id).unwrap();
        assert_eq!(s.execute(id).await.unwrap_err().kind, "cancelled");
    }
    #[tokio::test]
    async fn timeout_and_connection_failure_are_localized() {
        let (url, _) = server(vec![], "200 OK", "", 500).await;
        let mut i = input(url);
        i.options.timeout_ms = 50;
        let s = HttpService::default();
        let id = s.start(i).unwrap();
        assert_eq!(s.execute(id).await.unwrap_err().kind, "timeout");
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let id = s.start(input(url)).unwrap();
        assert_eq!(s.execute(id).await.unwrap_err().kind, "connect");
    }
    #[tokio::test]
    async fn response_preview_limit_keeps_raw_file_and_receive_cap_marks_incomplete() {
        let (url, _) = server(
            vec![b'a'; 3 * 1024 * 1024],
            "200 OK",
            "Content-Type: text/plain\r\n",
            0,
        )
        .await;
        let s = HttpService::default();
        let id = s.start(input(url)).unwrap();
        let r = s.execute(id.clone()).await.unwrap();
        assert!(r.preview_limited);
        assert!(!r.incomplete);
        assert_eq!(r.body.len(), PREVIEW);
        assert_eq!(
            s.file(&id).unwrap().as_file().metadata().unwrap().len(),
            3 * 1024 * 1024
        );
        let (url, _) = server(
            vec![b'a'; 2 * 1024 * 1024],
            "200 OK",
            "Content-Type: text/plain\r\n",
            0,
        )
        .await;
        let mut i = input(url);
        i.options.max_response_mib = 1;
        let id = s.start(i).unwrap();
        let r = s.execute(id).await.unwrap();
        assert!(r.incomplete);
        assert_eq!(r.received_bytes, 1024 * 1024);
    }
    #[tokio::test]
    async fn binary_and_non_utf8_are_handled_without_loss() {
        let (url, _) = server(
            vec![0, 255, 1],
            "200 OK",
            "Content-Type: application/octet-stream\r\n",
            0,
        )
        .await;
        let s = HttpService::default();
        let id = s.start(input(url)).unwrap();
        let r = s.execute(id.clone()).await.unwrap();
        assert!(r.binary);
        assert_eq!(
            std::fs::read(s.file(&id).unwrap().path()).unwrap(),
            vec![0, 255, 1]
        );
        let (bytes, _, _) = encoding_rs::GBK.encode("中文");
        let (url, _) = server(
            bytes.into_owned(),
            "200 OK",
            "Content-Type: text/plain; charset=gbk\r\n",
            0,
        )
        .await;
        let id = s.start(input(url)).unwrap();
        assert_eq!(s.execute(id).await.unwrap().body, "中文");
    }
    #[tokio::test]
    async fn cross_origin_sensitive_redirect_is_blocked() {
        let (url, _) = server(
            vec![],
            "302 Found",
            "Location: http://127.0.0.1:9/other\r\n",
            0,
        )
        .await;
        let mut i = input(url);
        i.auth = Auth {
            kind: "bearer".into(),
            token: "secret".into(),
            ..Auth::default()
        };
        let s = HttpService::default();
        let id = s.start(i).unwrap();
        assert_eq!(s.execute(id).await.unwrap_err().kind, "redirect");
    }
    #[tokio::test]
    async fn redirects_follow_when_enabled_and_remain_visible_when_disabled() {
        let (target, _) = server(
            b"final".to_vec(),
            "200 OK",
            "Content-Type: text/plain\r\n",
            0,
        )
        .await;
        let (url, _) = server(vec![], "302 Found", &format!("Location: {target}\r\n"), 0).await;
        let s = HttpService::default();
        let id = s.start(input(url)).unwrap();
        let r = s.execute(id).await.unwrap();
        assert_eq!(r.redirects, 1);
        assert_eq!(r.body, "final");
        let (url, _) = server(vec![], "302 Found", "Location: /next\r\n", 0).await;
        let mut i = input(url);
        i.options.follow_redirects = false;
        let id = s.start(i).unwrap();
        assert_eq!(s.execute(id).await.unwrap().status, 302);
    }
    #[test]
    fn max_five_and_unsupported_protocol_validation() {
        let s = HttpService::default();
        assert!(s.start(input("file:///tmp/a".into())).is_err());
        for _ in 0..5 {
            s.start(input("http://localhost".into())).unwrap();
        }
        assert_eq!(
            s.start(input("http://localhost".into())).unwrap_err().kind,
            "limit"
        );
    }
}
