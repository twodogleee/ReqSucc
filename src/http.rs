//! HTTP 请求执行器，负责变量与认证解析、取消和超时、敏感信息遮蔽及临时响应缓存。
//!
//! 作者：Andersen

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

/// 请求展示文本和响应预览的容量上限，单位为字节，当前为 2 MiB。
const PREVIEW: usize = 2 * 1024 * 1024;
/// 参数、请求头、表单或变量列表中的一行配置，保留启用及敏感标记。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    /// 行的稳定标识，用于编辑器定位；缺失时按空字符串反序列化。
    #[serde(default)]
    pub id: String,
    /// 是否参与当前请求或变量解析；旧数据缺失此字段时默认启用。
    #[serde(default = "yes")]
    pub enabled: bool,
    /// 参数名、请求头名或变量名，具体约束由所在配置决定。
    pub key: String,
    /// 原始配置值，可包含待发送时解析的变量占位符。
    pub value: String,
    /// 供用户维护的说明文字，不作为网络参数发送。
    #[serde(default)]
    pub description: String,
    /// 是否将变量值作为敏感内容遮蔽；不表示本地存储已加密。
    #[serde(default)]
    pub secret: bool,
}
/// 为旧版配置缺失的 enabled 字段提供启用默认值。
fn yes() -> bool {
    true
}
/// 请求认证配置，执行前由模型层完成集合认证继承。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Auth {
    /// 认证方式：none、bearer、basic 或 apiKey。
    pub kind: String,
    /// Bearer 令牌模板，发送时解析并加入敏感值列表。
    pub token: String,
    /// Basic 认证用户名模板，解析后不得包含冒号。
    pub username: String,
    /// Basic 认证密码模板，解析后的值用于展示遮蔽。
    pub password: String,
    /// API Key 写入的请求头或查询参数名称。
    pub api_key_name: String,
    /// API Key 值模板，发送时解析并遮蔽展示。
    pub api_key_value: String,
    /// API Key 传递位置，支持 header 和 query。
    pub api_key_in: String,
}
impl Default for Auth {
    /// 创建无认证配置；API Key 的默认位置为请求头。
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
/// 单次 HTTP 请求的超时、重定向、TLS 和响应接收限制。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Options {
    /// 从执行到响应接收的总超时，单位毫秒，允许 1–3600000，默认 30000。
    pub timeout_ms: u64,
    /// 建立连接的超时，单位毫秒，允许 1–3600000，默认 10000。
    pub connect_timeout_ms: u64,
    /// 是否按执行器的安全规则跟随重定向，默认启用。
    pub follow_redirects: bool,
    /// 是否校验 TLS 证书，默认启用；关闭仅作用于本次请求。
    pub verify_tls: bool,
    /// 响应缓存的接收上限，单位 MiB，允许 1–1024，默认 100。
    pub max_response_mib: u64,
}
impl Default for Options {
    /// 提供 30 秒总超时、10 秒连接超时、TLS 校验及 100 MiB 接收上限。
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
/// 发送时冻结的请求与变量配置快照，避免执行过程中受界面编辑影响。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Input {
    /// 参数表是否已经反映 URL 查询串；为 true 时以参数表重建查询，避免重复发送。
    #[serde(default)]
    pub query_synced: bool,
    /// 待校验的 HTTP 方法名称。
    pub method: String,
    /// 原始 HTTP 或 HTTPS 地址，可包含变量占位符。
    pub url: String,
    /// 有序查询参数行，允许重复键并保留空值。
    pub params: Vec<Row>,
    /// 用户配置的请求头行，发送时过滤禁用项并校验托管头冲突。
    pub headers: Vec<Row>,
    /// JSON、文本或 XML 请求体的原始模板。
    pub body: String,
    /// 请求体类型：none、json、text、xml 或 form。
    pub body_type: String,
    /// 表单编码请求体的键值行，仅 form 类型使用。
    #[serde(default)]
    pub form: Vec<Row>,
    /// 已完成集合继承、尚未展开变量的认证配置。
    pub auth: Auth,
    /// 全局默认设置与请求覆盖设置合并后的执行限制。
    pub options: Options,
    /// 最低优先级的全局变量配置。
    pub globals: Vec<Row>,
    /// 所属集合变量，覆盖同名全局变量。
    pub collection_variables: Vec<Row>,
    /// 当前环境变量，覆盖集合和全局的同名变量。
    pub environment_variables: Vec<Row>,
    /// 执行时环境名称，仅用于实际请求信息展示。
    pub environment_name: String,
}
/// 本地校验、网络或缓存失败；HTTP 非成功状态码仍作为正常响应返回。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Failure {
    /// 失败分类，供界面区分校验、取消、超时和网络等原因。
    pub kind: String,
    /// 面向用户的中文错误说明，不包含原始认证凭据。
    pub message: String,
    /// 失败前已耗时的毫秒数；发送前校验失败时为 0。
    pub elapsed_ms: u64,
}
/// 构建尚未计时的分类错误，执行结束时再补充实际耗时。
fn fail(kind: &str, message: impl Into<String>) -> Failure {
    Failure {
        kind: kind.into(),
        message: message.into(),
        elapsed_ms: 0,
    }
}
/// 响应头、Cookie 或展示用请求头的名称与值，不折叠重复项。
#[derive(Clone, Debug, Serialize)]
pub struct Pair {
    /// 头字段名称；Cookie 列表中保持 Set-Cookie 头名称。
    pub name: String,
    /// 对应头字段值，是否遮蔽由生成或展示该列表的路径决定。
    pub value: String,
}
/// 解析和认证组装后的请求展示快照，敏感内容经过遮蔽。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Effective {
    /// 实际准备发送的 HTTP 方法。
    pub method: String,
    /// 完成查询参数组装且经过敏感值遮蔽的地址。
    pub url: String,
    /// 加入认证和默认头后用于展示的脱敏请求头列表。
    pub headers: Vec<Pair>,
    /// 经脱敏及容量限制处理的请求体展示文本。
    pub body: String,
    /// 本次执行所选环境的显示名称。
    pub environment: String,
}
/// HTTP 响应元数据与有限正文预览，完整已接收字节另存临时文件。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Output {
    /// 执行标识，用于关联响应临时文件与标签运行状态。
    pub run_id: String,
    /// 服务器返回的 HTTP 数值状态码。
    pub status: u16,
    /// 状态码对应的中文说明。
    pub status_text: String,
    /// 执行至响应缓存写入完成的耗时，单位毫秒。
    pub elapsed_ms: u64,
    /// 实际缓存的响应正文大小，单位字节，受接收上限约束。
    pub received_bytes: usize,
    /// 重定向结束后的脱敏地址。
    pub final_url: String,
    /// 本次执行实际跟随的重定向次数。
    pub redirects: usize,
    /// 服务器响应头列表，保留重复头及原始值。
    pub headers: Vec<Pair>,
    /// 从响应头提取的 Set-Cookie 项，展示时另行遮蔽。
    pub cookies: Vec<Pair>,
    /// 解码后的有限文本预览；二进制响应为空字符串。
    pub body: String,
    /// 是否应作为二进制处理，界面据此禁止文本预览。
    pub binary: bool,
    /// 正文解码采用的字符编码名称。
    pub encoding: String,
    /// 接收字节数是否超过预览容量，不等同于响应接收被截断。
    pub preview_limited: bool,
    /// 是否因达到接收上限而截断正文；为 true 时缓存也不完整。
    pub incomplete: bool,
    /// 本次请求的脱敏展示快照。
    pub effective: Effective,
}
/// 发送前已校验并展开变量的请求，包含真实网络数据与独立展示快照。
struct Prepared {
    /// 已校验的 HTTP 方法。
    method: Method,
    /// 包含最终查询参数的真实请求地址，可能含敏感值。
    url: Url,
    /// 准备发送的请求头，包含认证信息，不可直接输出日志。
    headers: HeaderMap,
    /// 编码后的请求体字节；None 表示无请求体。
    body: Option<Vec<u8>>,
    /// 本次执行使用的网络与容量限制。
    options: Options,
    /// 与真实网络数据分离的脱敏展示内容。
    effective: Effective,
    /// 需要遮蔽的非空敏感值，按长度降序排列以避免部分替换泄露。
    secrets: Vec<String>,
    /// 是否使用认证或敏感请求头，用于阻止携带私有数据的跨源重定向。
    has_sensitive: bool,
}
/// 单次已登记请求的取消信号及尚未取走的执行配置。
struct Run {
    /// 通知执行任务提前停止的取消信号。
    token: CancellationToken,
    /// 仅能消费一次的请求配置，取走后禁止重复执行同一运行标识。
    prepared: Option<Prepared>,
}
/// 管理最多五个在途 HTTP 请求及最多五十份响应临时缓存。
#[derive(Default)]
pub struct HttpService {
    /// 运行标识到待执行或执行中请求的映射，受互斥锁保护。
    runs: Mutex<HashMap<String, Run>>,
    /// 运行标识到响应临时文件的共享引用，导出期间可延长文件生命周期。
    files: Mutex<HashMap<String, Arc<tempfile::NamedTempFile>>>,
}
/// 根据名称识别令牌、密码及认证或 Cookie 等敏感字段。
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
/// 遮蔽文本中的敏感原值及表单编码值，空敏感值不参与替换。
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
/// 遮蔽 URL 中的敏感查询项与已知敏感值，生成展示用地址。
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
/// 按全局、集合、环境顺序合并启用变量并收集敏感值；同作用域重名返回错误。
pub(crate) fn variables(input: &Input) -> Result<(HashMap<String, String>, Vec<String>), Failure> {
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
/// 单层展开变量占位符，将未定义项及字段标签写入 missing；不递归展开替换结果。
pub(crate) fn expand(
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
/// 按请求体类型遮蔽敏感内容并生成有限预览，截断时保持 UTF-8 字符边界。
fn preview_body(body: &Option<Vec<u8>>, kind: &str, secrets: &[String]) -> String {
    /// 递归遮蔽 JSON 对象中名称敏感的字段，数组元素继续递归处理。
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
/// 校验请求限制、地址、认证与头冲突，展开变量并组装真实请求及脱敏预览；不发起网络通信。
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
    /// 校验并登记请求，返回运行标识；配置错误或并发数达到五个时拒绝登记。
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
    /// 触发指定运行标识的取消信号；标识已不存在时视为无需取消。
    pub fn cancel(&self, id: &str) -> Result<(), String> {
        if let Some(run) = self.runs.lock().map_err(|_| "请求服务不可用")?.get(id) {
            run.token.cancel();
        }
        Ok(())
    }
    /// 移除指定响应缓存的服务引用；已持有的导出引用仍可继续读取。
    pub fn release(&self, id: &str) {
        if let Ok(mut files) = self.files.lock() {
            files.remove(id);
        }
    }
    /// 取消全部在途请求并释放服务持有的响应缓存，用于关闭或切换工作空间。
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
    /// 获取响应临时文件的共享引用；缓存已释放或锁不可用时返回错误。
    pub fn file(&self, id: &str) -> Result<Arc<tempfile::NamedTempFile>, String> {
        self.files
            .lock()
            .map_err(|_| "响应缓存不可用")?
            .get(id)
            .cloned()
            .ok_or("响应文件已释放，请重新发送请求".into())
    }
    /// 消费已登记请求并执行，优先响应取消信号；结束后释放并发名额，失败时补充耗时。
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
    /// 执行受控重定向并流式缓存响应；接收超限返回不完整标记，网络或缓存失败返回错误。
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
/// 将网络库错误归类为中文诊断信息，避免直接向界面暴露含地址或凭据的原始错误。
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
/// 将常见 HTTP 状态码转换为中文说明，其余按状态码区间提供说明。
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

/// 使用本机 HTTP 服务验证请求语义、失败边界与响应缓存。
#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };
    /// 构造一条启用且非敏感的测试键值行。
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
    /// 为指定地址构造默认 GET 请求，供各测试仅覆盖关注的配置。
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
    /// 启动本机模拟 HTTP 服务并记录收到的请求，返回访问地址与请求记录通道。
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
    /// 验证变量作用域覆盖、空值、禁用项以及单层展开语义。
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
    /// 验证未定义变量与手写认证头冲突在发送前被拒绝。
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
    /// 验证解析后的认证、重复查询参数和 JSON 请求体，并保留 500 状态响应。
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
    /// 验证 Basic 认证、表单编码及已同步查询参数只发送一次。
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
    /// 验证请求头和查询参数两种 API Key 位置及展示遮蔽。
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
    /// 验证取消可中止响应等待并释放请求并发名额。
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
    /// 验证执行前已取消的请求不会发起网络发送。
    #[tokio::test]
    async fn cancel_before_execute_never_sends() {
        let s = HttpService::default();
        let id = s.start(input("http://127.0.0.1:9".into())).unwrap();
        s.cancel(&id).unwrap();
        assert_eq!(s.execute(id).await.unwrap_err().kind, "cancelled");
    }
    /// 验证超时与连接失败返回对应的中文错误分类。
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
    /// 验证预览截断仍保留完整缓存，而接收超限会标记缓存不完整。
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
    /// 验证二进制和非 UTF-8 响应的展示判断及原始字节缓存。
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
    /// 验证携带敏感内容的跨源重定向被阻止。
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
    /// 验证启用时跟随重定向，禁用时保留原始重定向响应。
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
    /// 验证五个并发请求上限，以及不支持的 URL 协议在发送前被拒绝。
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
