//! SQLite 工作空间持久化、版本迁移和数据目录管理，保护已保存请求、会话草稿及源目录。
//!
//! 作者：Andersen

use fs2::FileExt;
use rusqlite::{Connection, OpenFlags, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};
use uuid::Uuid;

/// 存储层统一结果类型，失败以适合界面展示的中文说明返回。
type Result<T> = std::result::Result<T, String>;
/// 当前支持的 SQLite 与目录清单模式版本，打开更高版本时拒绝修改。
const SCHEMA: u32 = 2;
/// 工作空间根目录内的 SQLite 数据库文件名。
const DB: &str = "reqsucc.sqlite";
/// 为底层 I/O 或数据库错误添加本地存储操作上下文。
fn io(e: impl std::fmt::Display) -> String {
    format!("本地存储操作失败：{e}")
}
/// 数据目录身份清单，用于核对数据库归属及模式兼容性。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    /// 目录清单声明的模式版本，升级后与数据库版本同步。
    pub schema_version: u32,
    /// 工作空间唯一标识，必须与数据库元数据一致。
    pub workspace_id: String,
}
/// 集合或子目录的持久化描述，层级由父标识表达。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Folder {
    /// 目录稳定标识，用于请求归属和父子关系。
    pub id: String,
    /// 用户可见目录名称，保存时不能为空白。
    pub name: String,
    /// 父目录标识；None 表示根集合。
    pub parent: Option<String>,
}
/// 工作空间完整快照，分别保存接口版本、目录和包含草稿的会话数据。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    /// 快照所属工作空间标识，阻止跨空间误写。
    pub workspace_id: String,
    /// 读取时的数据库修订号，保存时必须匹配当前值以阻止旧快照覆盖。
    pub revision: i64,
    /// 按显示顺序排列的集合与子目录列表。
    pub folders: Vec<Folder>,
    /// 已明确保存的接口配置，与标签中的编辑草稿分开保存。
    pub requests: Vec<Value>,
    /// 标签草稿、比较基线、环境选择、显示偏好及网络配置等会话状态。
    pub session: Value,
}
/// 成功加载工作空间后回传界面的路径及快照。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Loaded {
    /// 当前工作空间目录的显示路径。
    pub path: String,
    /// 从该目录读取的完整工作空间快照。
    pub snapshot: Snapshot,
}
/// 配置目录内的工作空间位置记录，不保存接口业务数据。
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Bootstrap {
    /// 下次启动应打开的工作空间目录。
    active_path: PathBuf,
    /// 切换前的工作空间目录；首次定位时可为空。
    last_path: Option<PathBuf>,
}
/// 目录操作预检结果，供用户确认目标与空间需求。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoragePlan {
    /// 当前工作空间路径；尚未打开空间时为空。
    pub source: Option<String>,
    /// 经过校验和规范化的目标目录路径。
    pub target: String,
    /// 迁移源目录计入复制的文件总大小，单位字节；非迁移操作为 0。
    pub bytes: u64,
    /// 目标文件系统的可用容量，单位字节。
    pub available_bytes: u64,
}

/// 要求路径为现有绝对目录并拒绝路径本身为符号链接，返回规范化路径。
fn checked_path(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() {
        return Err("请选择绝对路径的数据目录".into());
    }
    if fs::symlink_metadata(path)
        .map_err(io)?
        .file_type()
        .is_symlink()
    {
        return Err("数据目录不能是符号链接".into());
    }
    let p = path.canonicalize().map_err(io)?;
    if !p.is_dir() {
        return Err("请选择一个有效文件夹".into());
    }
    Ok(p)
}
/// 读取目录清单，拒绝符号链接、无效 JSON 结构和不支持的模式版本；空间身份由打开流程进一步核对。
fn read_manifest(path: &Path) -> Result<Manifest> {
    let file = path.join("manifest.json");
    if fs::symlink_metadata(&file)
        .map_err(|_| "数据目录不可用或缺少 manifest.json，请重新连接磁盘或选择正确目录")?
        .file_type()
        .is_symlink()
    {
        return Err("数据清单不能是符号链接".into());
    }
    let m: Manifest = serde_json::from_slice(&fs::read(file).map_err(io)?)
        .map_err(|_| "数据清单格式不正确，未修改原目录")?;
    if m.schema_version == 0 || m.schema_version > SCHEMA {
        return Err(format!(
            "此数据目录的格式版本为 {}，当前支持版本为 {SCHEMA}。请使用匹配版本的 ReqSucc；不会清空或降级数据。",
            m.schema_version
        ));
    }
    Ok(m)
}
/// 在同目录写入临时 JSON 并刷盘后替换目标；Unix 下尽力同步父目录，避免部分配置写入。
fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().ok_or("配置路径无效")?;
    let temp = tempfile::NamedTempFile::new_in(parent).map_err(io)?;
    serde_json::to_writer(temp.as_file(), value).map_err(io)?;
    temp.as_file().sync_all().map_err(io)?;
    temp.persist(path).map_err(io)?;
    #[cfg(unix)]
    {
        let _ = File::open(parent).and_then(|f| f.sync_all());
    }
    Ok(())
}
/// 读取必需的 JSON 字符串字段，缺失或类型错误时返回字段级诊断。
fn str_field<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("接口字段 {key} 无效"))
}
/// 检查可持久化请求的字段、协议、认证方式及参数行标识；允许尚未填写完成的请求草稿。
fn validate_request(r: &Value) -> Result<()> {
    crate::mqtt::validate_saved(r)?;
    for key in [
        "id",
        "name",
        "folder",
        "method",
        "url",
        "body",
        "bodyType",
        "description",
        "auth",
        "token",
        "username",
        "password",
    ] {
        str_field(r, key)?;
    }
    if str_field(r, "id")?.is_empty() {
        return Err("接口标识不能为空".into());
    }
    if reqwest::Method::from_bytes(str_field(r, "method")?.as_bytes()).is_err()
        || !["none", "json", "text", "xml", "form"].contains(&str_field(r, "bodyType")?)
        || !["none", "bearer", "basic", "apiKey", "inherit"].contains(&str_field(r, "auth")?)
    {
        return Err("接口请求类型无效".into());
    }
    for key in ["params", "headers"] {
        let rows = r
            .get(key)
            .and_then(Value::as_array)
            .ok_or("接口参数格式不正确")?;
        let mut ids = HashSet::new();
        for row in rows {
            let id = str_field(row, "id")?;
            if id.is_empty() || !ids.insert(id) {
                return Err("参数行标识为空或重复".into());
            }
            for key in ["key", "value", "description"] {
                str_field(row, key)?;
            }
            if !row.get("enabled").is_some_and(Value::is_boolean) {
                return Err("参数启用状态无效".into());
            }
        }
    }
    Ok(())
}
/// 校验变量列表、环境标识、集合认证与默认网络限制，不展开变量或连接服务器。
fn validate_network(v: &Value) -> Result<()> {
    /// 检查变量列表是否符合 HTTP 键值行结构，避免无效类型进入持久化快照。
    fn rows(v: &Value) -> Result<()> {
        serde_json::from_value::<Vec<crate::http::Row>>(v.clone())
            .map_err(|_| "变量列表格式无效".to_string())?;
        Ok(())
    }
    rows(v.get("globals").ok_or("缺少全局变量")?)?;
    let envs = v
        .get("environments")
        .and_then(Value::as_array)
        .ok_or("环境列表格式无效")?;
    let mut ids = HashSet::new();
    for env in envs {
        if str_field(env, "id")?.is_empty() || !ids.insert(str_field(env, "id")?) {
            return Err("环境标识为空或重复".into());
        }
        if str_field(env, "name")?.trim().is_empty() {
            return Err("环境名称不能为空".into());
        }
        rows(env.get("variables").ok_or("环境缺少变量列表")?)?;
    }
    let collections = v
        .get("collections")
        .and_then(Value::as_object)
        .ok_or("集合配置无效")?;
    for c in collections.values() {
        rows(c.get("variables").ok_or("集合缺少变量列表")?)?;
        let auth = c.get("auth").ok_or("集合缺少认证配置")?;
        if !["none", "bearer", "basic", "apiKey"].contains(&str_field(auth, "auth")?) {
            return Err("集合认证类型无效".into());
        }
        for key in ["token", "username", "password"] {
            str_field(auth, key)?;
        }
    }
    let options: crate::http::Options =
        serde_json::from_value(v.get("defaults").ok_or("请求默认配置缺失")?.clone())
            .map_err(|_| "请求默认配置格式无效")?;
    if !(1..=3600000).contains(&options.timeout_ms)
        || !(1..=3600000).contains(&options.connect_timeout_ms)
        || !(1..=1024).contains(&options.max_response_mib)
    {
        return Err("超时须为 1–3600000 毫秒，响应上限须为 1–1024 MiB".into());
    }
    Ok(())
}
/// 检查快照大小、目录关系、请求归属和草稿结构，保存前阻止不一致数据覆盖数据库。
fn validate(snapshot: &Snapshot) -> Result<()> {
    if snapshot.folders.len() > 20000 || snapshot.requests.len() > 50000 {
        return Err("数据条目超过当前支持的上限".into());
    }
    let ids: HashSet<_> = snapshot.folders.iter().map(|f| f.id.as_str()).collect();
    if ids.len() != snapshot.folders.len() {
        return Err("文件夹标识重复".into());
    }
    for folder in &snapshot.folders {
        if folder.id.is_empty() || folder.name.trim().is_empty() {
            return Err("集合或文件夹名称不能为空".into());
        }
        let mut chain = HashSet::new();
        let mut next = Some(folder.id.as_str());
        while let Some(id) = next {
            if !chain.insert(id) {
                return Err("不能将目录移入自身或子目录".into());
            }
            let f = snapshot
                .folders
                .iter()
                .find(|f| f.id == id)
                .ok_or("父目录不存在")?;
            next = f.parent.as_deref();
        }
    }
    let mut request_ids = HashSet::new();
    for r in &snapshot.requests {
        validate_request(r)?;
        if !request_ids.insert(str_field(r, "id")?) {
            return Err("接口标识重复".into());
        }
        if str_field(r, "name")?.trim().is_empty() || !ids.contains(str_field(r, "folder")?) {
            return Err("接口名称为空或保存目录不存在".into());
        }
        for key in ["method", "url", "body", "bodyType", "auth"] {
            str_field(r, key)?;
        }
        for key in ["params", "headers"] {
            if !r.get(key).is_some_and(Value::is_array) {
                return Err("接口参数格式不正确".into());
            }
        }
    }
    let tabs = snapshot
        .session
        .get("tabs")
        .and_then(Value::as_array)
        .ok_or("草稿会话格式不正确")?;
    let mut tab_ids = HashSet::new();
    for tab in tabs {
        let id = str_field(tab, "id")?;
        if !tab_ids.insert(id) {
            return Err("草稿标签标识重复".into());
        }
        str_field(tab, "baseline")?;
        if ![
            "params",
            "auth",
            "headers",
            "body",
            "description",
            "settings",
            "message",
            "topics",
            "mqtt-auth",
            "properties",
            "will",
            "mqtt-settings",
            "connection",
            "subscriptions",
        ]
        .contains(&str_field(tab, "panel")?)
        {
            return Err("草稿编辑面板无效".into());
        }
        let r = tab.get("request").ok_or("草稿缺少请求")?;
        validate_request(r)?;
        if str_field(r, "id")? != id {
            return Err("草稿标识不一致".into());
        }
        for key in [
            "name", "method", "url", "folder", "body", "bodyType", "auth",
        ] {
            str_field(r, key)?;
        }
        for key in ["params", "headers"] {
            if !r.get(key).is_some_and(Value::is_array) {
                return Err("草稿参数格式不正确".into());
            }
        }
    }
    str_field(&snapshot.session, "activeId")?;
    str_field(&snapshot.session, "environment")?;
    if !snapshot
        .session
        .get("expanded")
        .and_then(Value::as_array)
        .is_some_and(|a| a.iter().all(Value::is_string))
    {
        return Err("目录展开状态无效".into());
    }
    if let Some(network) = snapshot.session.get("network") {
        validate_network(network)?;
    }
    if serde_json::to_vec(snapshot).map_err(io)?.len() > 64 * 1024 * 1024 {
        return Err("工作空间数据超过 64 MiB，未写入".into());
    }
    Ok(())
}
/// 持有单个工作空间的数据库连接和独占目录锁，负责快照读写及模式升级。
pub struct Workspace {
    /// 已规范化的工作空间目录，用于校验磁盘可用性和定位相关文件。
    path: PathBuf,
    /// 已读取并核对的目录身份与模式版本。
    manifest: Manifest,
    /// 当前工作空间 SQLite 连接，由存储线程串行使用。
    conn: Connection,
    /// 持有目录独占锁的文件句柄，随工作空间释放而解除锁定。
    _lock: File,
}
impl Workspace {
    /// 在现有空目录中创建数据库、清单和默认集合并持有独占锁；目标非空或初始化失败时返回错误。
    pub fn create(path: &Path) -> Result<Self> {
        let path = checked_path(path)?;
        if fs::read_dir(&path).map_err(io)?.next().is_some() {
            return Err("目标目录必须为空，不会覆盖已有文件".into());
        }
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path.join(".reqsucc.lock"))
            .map_err(io)?;
        FileExt::try_lock_exclusive(&lock).map_err(|_| "数据目录正由另一个 ReqSucc 实例使用")?;
        let manifest = Manifest {
            schema_version: SCHEMA,
            workspace_id: Uuid::new_v4().to_string(),
        };
        let conn = Connection::open(path.join(DB)).map_err(io)?;
        conn.execute_batch(include_str!("../migrations/001_workspace.sql"))
            .map_err(io)?;
        conn.execute_batch(include_str!("../migrations/002_network.sql"))
            .map_err(io)?;
        conn.execute(
            "INSERT INTO workspace_meta VALUES(1, ?1, 0)",
            [&manifest.workspace_id],
        )
        .map_err(io)?;
        conn.execute(
            "INSERT INTO folders VALUES('default', NULL, '我的接口', 0)",
            [],
        )
        .map_err(io)?;
        conn.execute(
            "INSERT INTO session VALUES(1, ?1)",
            [
                json!({"tabs":[],"activeId":"","environment":"local","expanded":["default"]})
                    .to_string(),
            ],
        )
        .map_err(io)?;
        for dir in ["attachments", "cache/responses", "backups", "logs"] {
            fs::create_dir_all(path.join(dir)).map_err(io)?;
        }
        atomic_json(&path.join("manifest.json"), &manifest)?;
        let w = Self {
            path,
            manifest,
            conn,
            _lock: lock,
        };
        w.configure()?;
        Ok(w)
    }
    /// 验证目录身份与数据库完整性并取得锁；旧模式先备份再升级，不创建缺失的数据库。
    pub fn open(path: &Path) -> Result<Self> {
        let path = checked_path(path)?;
        let manifest = read_manifest(&path)?;
        for name in [DB, ".reqsucc.lock"] {
            if fs::symlink_metadata(path.join(name))
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false)
            {
                return Err("数据库和锁文件不能是符号链接".into());
            }
        }
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path.join(".reqsucc.lock"))
            .map_err(io)?;
        FileExt::try_lock_exclusive(&lock)
            .map_err(|_| "此数据目录正在被其他 ReqSucc 实例使用，请先关闭其他实例")?;
        let conn = Connection::open_with_flags(path.join(DB), OpenFlags::SQLITE_OPEN_READ_WRITE)
            .map_err(io)?;
        let schema: u32 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(io)?;
        if schema == 0 || schema > SCHEMA || manifest.schema_version > schema {
            return Err("数据库版本不匹配，未执行升级或清空".into());
        }
        let id: String = conn
            .query_row(
                "SELECT workspace_id FROM workspace_meta WHERE id=1",
                [],
                |r| r.get(0),
            )
            .map_err(io)?;
        if id != manifest.workspace_id {
            return Err("数据库与数据目录清单不匹配".into());
        }
        let check: String = conn
            .pragma_query_value(None, "quick_check", |r| r.get(0))
            .map_err(io)?;
        if check != "ok" {
            return Err("数据库完整性校验失败，请从备份恢复".into());
        }
        let w = Self {
            path,
            manifest,
            conn,
            _lock: lock,
        };
        w.configure()?;
        validate(&w.snapshot()?)?;
        let mut w = w;
        if schema == 1 {
            let backup_dir = w
                .path
                .join("backups")
                .join(format!("before-v2-{}", Uuid::new_v4()));
            fs::create_dir_all(&backup_dir).map_err(io)?;
            let mut backup = Connection::open(backup_dir.join(DB)).map_err(io)?;
            {
                let job = rusqlite::backup::Backup::new(&w.conn, &mut backup).map_err(io)?;
                job.run_to_completion(128, Duration::from_millis(5), None)
                    .map_err(io)?;
            }
            drop(backup);
            File::open(backup_dir.join(DB))
                .and_then(|f| f.sync_all())
                .map_err(io)?;
            atomic_json(&backup_dir.join("manifest.json"), &w.manifest)?;
            w.conn
                .execute_batch(include_str!("../migrations/002_network.sql"))
                .map_err(io)?;
        }
        // SQLite 已升级但清单尚未替换时，可在下次打开恢复完成。
        if w.manifest.schema_version < SCHEMA {
            w.manifest.schema_version = SCHEMA;
            atomic_json(&w.path.join("manifest.json"), &w.manifest)?;
        }
        Ok(w)
    }
    /// 启用外键、WAL 和 FULL 同步，设置 3 秒数据库忙等待。
    fn configure(&self) -> Result<()> {
        self.conn.busy_timeout(Duration::from_secs(3)).map_err(io)?;
        self.conn
            .execute_batch(
                "PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;",
            )
            .map_err(io)
    }
    /// 确认原目录及数据库仍存在、身份未变且可写；磁盘断开或只读时拒绝继续操作。
    fn ensure_available(&self) -> Result<()> {
        let manifest = read_manifest(&self.path)?;
        if manifest.workspace_id != self.manifest.workspace_id || !self.path.join(DB).is_file() {
            return Err(
                "当前数据目录已变更或磁盘已断开，请恢复原目录后重试；内存编辑仍保留".into(),
            );
        }
        // Windows 的目录只读属性用于资源管理器外观，不代表 ACL 写权限；实际写入仍由系统校验。
        let directory_readonly = !cfg!(windows)
            && fs::metadata(&self.path)
                .map_err(io)?
                .permissions()
                .readonly();
        if directory_readonly
            || fs::metadata(self.path.join(DB))
                .map_err(io)?
                .permissions()
                .readonly()
        {
            return Err("当前数据目录或数据库只读，无法保存".into());
        }
        Ok(())
    }
    /// 按持久化顺序读取目录、已保存请求和会话草稿，携带当前修订号；磁盘或格式异常时返回错误。
    pub fn snapshot(&self) -> Result<Snapshot> {
        self.ensure_available()?;
        let mut stmt = self
            .conn
            .prepare("SELECT id,name,parent FROM folders ORDER BY position")
            .map_err(io)?;
        let folders = stmt
            .query_map([], |r| {
                Ok(Folder {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    parent: r.get(2)?,
                })
            })
            .map_err(io)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(io)?;
        let mut stmt = self
            .conn
            .prepare("SELECT payload FROM requests ORDER BY position")
            .map_err(io)?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(io)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(io)?;
        let requests = rows
            .iter()
            .map(|s| serde_json::from_str(s).map_err(io))
            .collect::<Result<Vec<_>>>()?;
        let session: String = self
            .conn
            .query_row("SELECT payload FROM session WHERE id=1", [], |r| r.get(0))
            .map_err(io)?;
        let revision = self
            .conn
            .query_row("SELECT revision FROM workspace_meta WHERE id=1", [], |r| {
                r.get(0)
            })
            .map_err(io)?;
        Ok(Snapshot {
            workspace_id: self.manifest.workspace_id.clone(),
            revision,
            folders,
            requests,
            session: serde_json::from_str(&session).map_err(io)?,
        })
    }
    /// 校验空间身份与修订号后在事务内保存完整快照，成功返回递增后的修订号，失败不提交事务。
    pub fn save(&mut self, snapshot: Snapshot) -> Result<i64> {
        self.ensure_available()?;
        validate(&snapshot)?;
        if snapshot.workspace_id != self.manifest.workspace_id {
            return Err("数据空间已切换，拒绝旧空间的写入".into());
        }
        let tx = self.conn.transaction().map_err(io)?;
        let revision: i64 = tx
            .query_row("SELECT revision FROM workspace_meta WHERE id=1", [], |r| {
                r.get(0)
            })
            .map_err(io)?;
        if revision != snapshot.revision {
            return Err("数据版本已变化，已停止写入以防覆盖，请重新打开数据空间".into());
        }
        tx.execute("DELETE FROM requests", []).map_err(io)?;
        tx.execute("DELETE FROM folders", []).map_err(io)?;
        for (position, f) in snapshot.folders.iter().enumerate() {
            tx.execute(
                "INSERT INTO folders VALUES(?1,?2,?3,?4)",
                params![f.id, f.parent, f.name, position as i64],
            )
            .map_err(io)?;
        }
        for (position, r) in snapshot.requests.iter().enumerate() {
            tx.execute(
                "INSERT INTO requests VALUES(?1,?2,?3,?4)",
                params![
                    str_field(r, "id")?,
                    str_field(r, "folder")?,
                    r.to_string(),
                    position as i64
                ],
            )
            .map_err(io)?;
        }
        tx.execute(
            "UPDATE session SET payload=?1 WHERE id=1",
            [snapshot.session.to_string()],
        )
        .map_err(io)?;
        tx.execute(
            "UPDATE workspace_meta SET revision=revision+1 WHERE id=1",
            [],
        )
        .map_err(io)?;
        tx.commit().map_err(io)?;
        Ok(revision + 1)
    }
    /// 组装当前目录路径和快照，供界面加载或切换后使用。
    fn loaded(&self) -> Result<Loaded> {
        Ok(Loaded {
            path: self.path.to_string_lossy().into(),
            snapshot: self.snapshot()?,
        })
    }
}

/// 管理启动配置和唯一活动工作空间，协调创建、打开、迁移及切换。
pub struct StorageService {
    /// 保存 bootstrap.json 的应用配置目录。
    config: PathBuf,
    /// 尚无启动配置时使用的默认数据目录。
    default: PathBuf,
    /// 当前持有目录锁的工作空间；加载前为 None。
    active: Option<Workspace>,
}
impl StorageService {
    /// 记录配置目录及默认目录，暂不打开或创建工作空间。
    pub fn new(config: PathBuf, default: PathBuf) -> Self {
        Self {
            config,
            default,
            active: None,
        }
    }
    /// 优先打开配置指向的目录，否则使用默认目录；已记录目录失效时返回错误，不静默创建替代数据库。
    pub fn load(&mut self) -> Result<Loaded> {
        if let Some(w) = &self.active {
            return w.loaded();
        }
        let bootstrap = self.config.join("bootstrap.json");
        let workspace = if bootstrap.exists() {
            let b: Bootstrap = serde_json::from_slice(&fs::read(&bootstrap).map_err(io)?)
                .map_err(|_| "启动配置损坏。请选择已有数据目录，不会自动创建空数据。")?;
            Workspace::open(&b.active_path)?
        } else if self.default.join("manifest.json").exists() {
            Workspace::open(&self.default)?
        } else {
            fs::create_dir_all(&self.default).map_err(io)?;
            Workspace::create(&self.default)?
        };
        let loaded = workspace.loaded()?;
        self.point_to(&workspace.path)?;
        self.active = Some(workspace);
        Ok(loaded)
    }
    /// 原子更新启动配置中的活动目录和先前目录；失败时由调用方保留原活动空间。
    fn point_to(&self, path: &Path) -> Result<()> {
        fs::create_dir_all(&self.config).map_err(io)?;
        atomic_json(
            &self.config.join("bootstrap.json"),
            &Bootstrap {
                active_path: path.into(),
                last_path: self.active.as_ref().map(|w| w.path.clone()),
            },
        )
    }
    /// 将快照交给当前活动工作空间保存；尚未加载空间时拒绝写入。
    pub fn save(&mut self, snapshot: Snapshot) -> Result<i64> {
        self.active
            .as_mut()
            .ok_or("数据空间尚未打开")?
            .save(snapshot)
    }
    /// 预检 create、open 或 migrate 操作的路径、嵌套关系、清单与磁盘空间，不切换活动空间。
    pub fn plan(&self, path: &Path, mode: &str) -> Result<StoragePlan> {
        if !["create", "open", "migrate"].contains(&mode) {
            return Err("未知的目录操作".into());
        }
        let target = checked_path(path)?;
        if let Some(w) = &self.active {
            if target.starts_with(&w.path) || w.path.starts_with(&target) {
                return Err("目标与当前目录不能相同或互为子目录".into());
            }
        }
        if mode != "open" && fs::read_dir(&target).map_err(io)?.next().is_some() {
            return Err("目标目录必须为空，不会覆盖已有内容".into());
        }
        if mode == "open" {
            read_manifest(&target)?;
        }
        let bytes = if mode == "migrate" {
            let w = self.active.as_ref().ok_or("没有可迁移的数据空间")?;
            w.ensure_available()?;
            tree_size(&w.path)?
        } else {
            0
        };
        let available = fs2::available_space(&target).map_err(io)?;
        if mode != "open" && available < bytes.saturating_add(16 * 1024 * 1024) {
            return Err("目标磁盘剩余空间不足，请选择其他目录".into());
        }
        Ok(StoragePlan {
            source: self
                .active
                .as_ref()
                .map(|w| w.path.to_string_lossy().into()),
            target: target.to_string_lossy().into(),
            bytes,
            available_bytes: available,
        })
    }
    /// 执行已校验的目录操作并更新启动配置；迁移保留源目录且核对数据库和附件，失败不替换活动空间。
    pub fn switch(&mut self, path: &Path, mode: &str) -> Result<Loaded> {
        let plan = self.plan(path, mode)?;
        let target = PathBuf::from(plan.target);
        let candidate = match mode {
            "open" => Workspace::open(&target)?,
            "create" => Workspace::create(&target)?,
            "migrate" => {
                let source = self.active.as_ref().ok_or("没有可迁移的数据")?;
                let mut dest = Workspace::create(&target)?;
                // 源连接保持打开；备份 API 包含尚未 checkpoint 的 WAL 内容。
                {
                    let backup =
                        rusqlite::backup::Backup::new(&source.conn, &mut dest.conn).map_err(io)?;
                    backup
                        .run_to_completion(128, Duration::from_millis(5), None)
                        .map_err(io)?;
                }
                dest.manifest = source.manifest.clone();
                for dir in ["attachments", "backups", "logs"] {
                    copy_tree(&source.path.join(dir), &dest.path.join(dir))?;
                }
                atomic_json(&dest.path.join("manifest.json"), &dest.manifest)?;
                let check: String = dest
                    .conn
                    .pragma_query_value(None, "integrity_check", |r| r.get(0))
                    .map_err(io)?;
                if check != "ok" {
                    return Err("目标数据库完整性校验失败，原目录仍在使用".into());
                }
                let left = serde_json::to_value(source.snapshot()?).map_err(io)?;
                let right = serde_json::to_value(dest.snapshot()?).map_err(io)?;
                if left != right {
                    return Err("迁移后数据校验不一致，未切换目录".into());
                }
                dest.conn
                    .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
                    .map_err(io)?;
                File::open(dest.path.join(DB))
                    .and_then(|f| f.sync_all())
                    .map_err(io)?;
                dest
            }
            _ => return Err("未知的目录操作".into()),
        };
        let loaded = candidate.loaded()?;
        self.point_to(&candidate.path)?; // 此步骤失败时原 active 不变。
        self.active = Some(candidate);
        Ok(loaded)
    }
}
/// 累计迁移文件大小，跳过缓存和锁文件；遇到符号链接时拒绝继续估算。
fn tree_size(path: &Path) -> Result<u64> {
    let mut size = 0;
    for entry in fs::read_dir(path).map_err(io)? {
        let entry = entry.map_err(io)?;
        let ty = entry.file_type().map_err(io)?;
        let name = entry.file_name();
        if name == "cache" || name == ".reqsucc.lock" {
            continue;
        }
        if ty.is_symlink() {
            return Err("数据目录包含符号链接，无法安全迁移".into());
        }
        size += if ty.is_dir() {
            tree_size(&entry.path())?
        } else {
            entry.metadata().map_err(io)?.len()
        };
    }
    Ok(size)
}
/// 分块计算文件的 SHA-256 摘要，供迁移后逐文件比对。
fn digest(path: &Path) -> Result<Vec<u8>> {
    let mut file = File::open(path).map_err(io)?;
    let mut hash = Sha256::new();
    let mut buf = [0; 65536];
    loop {
        let n = file.read(&mut buf).map_err(io)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(hash.finalize().to_vec())
}
/// 递归复制普通文件并刷盘、核对摘要；源目录缺失则跳过，符号链接或特殊文件返回错误。
fn copy_tree(source: &Path, target: &Path) -> Result<()> {
    if !source.exists() {
        return Ok(());
    }
    if fs::symlink_metadata(source)
        .map_err(io)?
        .file_type()
        .is_symlink()
    {
        return Err("迁移不支持符号链接附件".into());
    }
    fs::create_dir_all(target).map_err(io)?;
    for entry in fs::read_dir(source).map_err(io)? {
        let entry = entry.map_err(io)?;
        let ty = entry.file_type().map_err(io)?;
        let dest = target.join(entry.file_name());
        if ty.is_symlink() {
            return Err("迁移不支持符号链接附件".into());
        }
        if ty.is_dir() {
            copy_tree(&entry.path(), &dest)?;
        } else if ty.is_file() {
            let mut input = File::open(entry.path()).map_err(io)?;
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&dest)
                .map_err(io)?;
            std::io::copy(&mut input, &mut output).map_err(io)?;
            output.flush().map_err(io)?;
            output.sync_all().map_err(io)?;
            if digest(&entry.path())? != digest(&dest)? {
                return Err("附件校验失败，未切换数据目录".into());
            }
        } else {
            return Err("数据目录存在不支持的特殊文件".into());
        }
    }
    Ok(())
}

/// 在临时目录验证持久化一致性、锁、升级恢复和目录迁移。
#[cfg(test)]
mod tests {
    use super::*;
    /// 创建临时应用配置与默认数据路径，返回持有目录生命周期的测试环境。
    fn setup() -> (tempfile::TempDir, StorageService) {
        let t = tempfile::tempdir().unwrap();
        let s = StorageService::new(t.path().join("config"), t.path().join("data"));
        (t, s)
    }
    /// 构造满足存储校验的最小测试请求。
    fn req() -> Value {
        json!({"id":"r1","name":"接口一","folder":"default","method":"POST","url":"http://localhost","params":[{"id":"p1","enabled":true,"key":"a","value":"1","description":""},{"id":"p2","enabled":false,"key":"a","value":"2","description":""}],"headers":[],"body":"{}","bodyType":"json","auth":"none","description":"","token":"","username":"","password":""})
    }
    /// 验证重启后已保存接口与未保存草稿分别恢复。
    #[test]
    fn saved_and_draft_survive_restart_separately() {
        let (t, mut s) = setup();
        let mut x = s.load().unwrap().snapshot;
        x.requests.push(req());
        let mut draft = req();
        draft["url"] = json!("http://unsaved");
        x.session = json!({"tabs":[{"id":"r1","request":draft,"baseline":req().to_string(),"panel":"body"}],"activeId":"r1","environment":"local","expanded":["default"]});
        s.save(x).unwrap();
        drop(s);
        let mut s = StorageService::new(t.path().join("config"), t.path().join("data"));
        let x = s.load().unwrap().snapshot;
        assert_eq!(x.requests[0]["url"], "http://localhost");
        assert_eq!(x.session["tabs"][0]["request"]["url"], "http://unsaved");
        assert_eq!(x.requests[0]["params"].as_array().unwrap().len(), 2);
    }
    /// 验证空白新标签的比较基线写入并在重启后恢复。
    #[test]
    fn new_tab_baseline_survives_storage_restart() {
        use crate::model;

        let (t, mut storage) = setup();
        let mut snapshot = storage.load().unwrap().snapshot;
        model::normalize(&mut snapshot);
        model::new_tab(&mut snapshot, "");
        storage.save(snapshot).unwrap();
        drop(storage);

        let mut storage = StorageService::new(t.path().join("config"), t.path().join("data"));
        let mut restored = storage.load().unwrap().snapshot;
        model::normalize(&mut restored);
        assert!(!model::dirty(&model::tabs(&restored)[0]));
        model::active_mut(&mut restored).unwrap()["mqtt"]["clientId"] = json!("custom-client");
        assert!(model::dirty(&model::tabs(&restored)[0]));
    }
    /// 验证两个平台的自定义及已清除快捷键经重启和迁移后独立保留，新空间使用默认值。
    #[test]
    fn shortcut_overrides_survive_restart_and_migration_without_leaking_to_other_workspace() {
        use crate::shortcuts::{self, Shortcut};
        use gpui_kit::OperatingSystem;

        let (t, mut storage) = setup();
        let mut snapshot = storage.load().unwrap().snapshot;
        crate::model::normalize(&mut snapshot);
        shortcuts::set(
            &mut snapshot.session,
            Shortcut::New,
            "cmd-shift-n",
            OperatingSystem::Mac,
        );
        shortcuts::set(
            &mut snapshot.session,
            Shortcut::Quit,
            "",
            OperatingSystem::Mac,
        );
        shortcuts::set(
            &mut snapshot.session,
            Shortcut::New,
            "ctrl-shift-n",
            OperatingSystem::Windows,
        );
        storage.save(snapshot).unwrap();
        drop(storage);

        let mut storage = StorageService::new(t.path().join("config"), t.path().join("data"));
        let restored = storage.load().unwrap().snapshot;
        assert_eq!(
            shortcuts::key(&restored.session, Shortcut::New, OperatingSystem::Mac),
            "cmd-shift-n"
        );
        assert_eq!(
            shortcuts::key(&restored.session, Shortcut::Quit, OperatingSystem::Mac),
            ""
        );
        assert_eq!(
            shortcuts::key(&restored.session, Shortcut::New, OperatingSystem::Windows),
            "ctrl-shift-n"
        );
        assert_eq!(
            shortcuts::key(&restored.session, Shortcut::Quit, OperatingSystem::Windows),
            "ctrl-q"
        );
        let destination = t.path().join("shortcut-migration");
        fs::create_dir(&destination).unwrap();
        let migrated = storage.switch(&destination, "migrate").unwrap().snapshot;
        assert_eq!(
            migrated.session["native"]["shortcutsByPlatform"],
            restored.session["native"]["shortcutsByPlatform"]
        );
        let other = t.path().join("other");
        fs::create_dir(&other).unwrap();
        let fresh = storage.switch(&other, "create").unwrap().snapshot;
        assert_eq!(
            shortcuts::key(&fresh.session, Shortcut::New, OperatingSystem::Mac),
            "cmd-n"
        );
        assert_eq!(
            shortcuts::key(&fresh.session, Shortcut::Quit, OperatingSystem::Mac),
            "cmd-q"
        );
        assert_eq!(
            shortcuts::key(&fresh.session, Shortcut::New, OperatingSystem::Windows),
            "ctrl-n"
        );
    }

    /// 验证旧修订号和其他空间的快照不能覆盖当前数据库。
    #[test]
    fn stale_revision_and_workspace_cannot_overwrite() {
        let (_t, mut s) = setup();
        let x = s.load().unwrap().snapshot;
        s.save(x.clone()).unwrap();
        assert!(s.save(x.clone()).is_err());
        let mut x = s.load().unwrap().snapshot;
        x.workspace_id = "other".into();
        assert!(s.save(x).is_err());
    }
    /// 验证循环目录关系被拒绝且不会修改原数据库。
    #[test]
    fn cyclic_folders_do_not_change_database() {
        let (_t, mut s) = setup();
        let mut x = s.load().unwrap().snapshot;
        x.folders[0].parent = Some("default".into());
        assert!(s.save(x).is_err());
        assert!(s.load().unwrap().snapshot.folders[0].parent.is_none());
    }
    /// 验证迁移保留源目录，并包含 WAL 中的数据及附件。
    #[test]
    fn migrate_keeps_source_and_copies_wal_and_attachments() {
        let (t, mut s) = setup();
        let mut x = s.load().unwrap().snapshot;
        x.requests.push(req());
        s.save(x).unwrap();
        fs::write(t.path().join("data/attachments/a.bin"), [0, 1, 255]).unwrap();
        let dest = t.path().join("dest");
        fs::create_dir(&dest).unwrap();
        let result = s.switch(&dest, "migrate").unwrap();
        assert_eq!(result.snapshot.requests.len(), 1);
        assert_eq!(
            fs::read(dest.join("attachments/a.bin")).unwrap(),
            vec![0, 1, 255]
        );
        assert!(t.path().join("data/reqsucc.sqlite").exists());
        drop(s);
        let mut s = StorageService::new(t.path().join("config"), t.path().join("data"));
        assert_eq!(
            s.load().unwrap().path,
            dest.canonicalize().unwrap().to_string_lossy()
        );
        assert_eq!(
            Workspace::open(&t.path().join("data"))
                .unwrap()
                .snapshot()
                .unwrap()
                .requests
                .len(),
            1
        );
    }
    /// 验证创建或迁移拒绝非空、相同和互相嵌套的目标目录。
    #[test]
    fn reject_nonempty_same_and_nested_targets() {
        let (t, mut s) = setup();
        s.load().unwrap();
        assert!(s.switch(&t.path().join("data"), "migrate").is_err());
        let nested = t.path().join("data/nested");
        fs::create_dir(&nested).unwrap();
        assert!(s.switch(&nested, "migrate").is_err());
        let dest = t.path().join("occupied");
        fs::create_dir(&dest).unwrap();
        fs::write(dest.join("important"), "keep").unwrap();
        assert!(s.switch(&dest, "migrate").is_err());
        assert_eq!(fs::read_to_string(dest.join("important")).unwrap(), "keep");
    }
    /// 验证配置指向的目录丢失时不会静默新建空数据库。
    #[test]
    fn missing_active_directory_never_creates_empty_database() {
        let (t, mut s) = setup();
        s.load().unwrap();
        drop(s);
        fs::rename(t.path().join("data"), t.path().join("unplugged")).unwrap();
        let mut s = StorageService::new(t.path().join("config"), t.path().join("data"));
        assert!(s.load().is_err());
        assert!(!t.path().join("data").exists());
    }
    /// 验证目录独占锁阻止第二个工作空间实例写入。
    #[test]
    fn directory_lock_blocks_second_writer() {
        let (t, mut s) = setup();
        s.load().unwrap();
        assert!(Workspace::open(&t.path().join("data")).is_err());
    }
    /// 验证高于支持版本的数据库拒绝打开且保持原版本。
    #[test]
    fn newer_schema_is_never_modified() {
        let (t, mut s) = setup();
        s.load().unwrap();
        drop(s);
        let path = t.path().join("data/manifest.json");
        let mut m: Manifest = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        m.schema_version = 9;
        atomic_json(&path, &m).unwrap();
        assert!(Workspace::open(&t.path().join("data")).is_err());
        assert!(fs::read_to_string(path).unwrap().contains('9'));
    }
    /// 验证启动配置写入失败时保留原活动工作空间。
    #[test]
    fn config_failure_rolls_back_active_pointer() {
        let (t, mut s) = setup();
        s.load().unwrap();
        fs::remove_file(t.path().join("config/bootstrap.json")).unwrap();
        fs::create_dir(t.path().join("config/bootstrap.json")).unwrap();
        let dest = t.path().join("dest");
        fs::create_dir(&dest).unwrap();
        assert!(s.switch(&dest, "migrate").is_err());
        assert_eq!(
            s.load().unwrap().path,
            t.path()
                .join("data")
                .canonicalize()
                .unwrap()
                .to_string_lossy()
        );
    }
    /// 验证打开其他空间只切换快照，不合并原空间内容。
    #[test]
    fn open_other_workspace_does_not_merge() {
        let (t, mut s) = setup();
        let mut x = s.load().unwrap().snapshot;
        x.requests.push(req());
        s.save(x).unwrap();
        let dest = t.path().join("other");
        fs::create_dir(&dest).unwrap();
        drop(Workspace::create(&dest).unwrap());
        assert!(
            s.switch(&dest, "open")
                .unwrap()
                .snapshot
                .requests
                .is_empty()
        );
        assert_eq!(
            s.switch(&t.path().join("data"), "open")
                .unwrap()
                .snapshot
                .requests
                .len(),
            1
        );
    }
    /// 验证目录失联时拒绝保存，恢复原路径后可继续写入。
    #[test]
    fn disconnected_directory_rejects_write_then_recovers() {
        let (t, mut s) = setup();
        let mut x = s.load().unwrap().snapshot;
        x.requests.push(req());
        #[cfg(not(windows))]
        fs::rename(t.path().join("data"), t.path().join("unplugged")).unwrap();
        // Windows 不允许移动仍被 SQLite 打开的目录，用不可达路径模拟同一失联边界。
        #[cfg(windows)]
        let original_path = {
            let active = s.active.as_mut().unwrap();
            std::mem::replace(&mut active.path, t.path().join("unplugged"))
        };
        assert!(s.save(x.clone()).is_err());
        #[cfg(not(windows))]
        assert!(!t.path().join("data").exists());
        #[cfg(windows)]
        assert!(!t.path().join("unplugged").exists());
        #[cfg(not(windows))]
        fs::rename(t.path().join("unplugged"), t.path().join("data")).unwrap();
        #[cfg(windows)]
        {
            s.active.as_mut().unwrap().path = original_path;
        }
        s.save(x).unwrap();
        assert_eq!(s.load().unwrap().snapshot.requests.len(), 1);
    }
    /// 验证无效请求结构被拒绝且原已保存数据不丢失。
    #[test]
    fn malformed_request_is_rejected_without_losing_saved_data() {
        let (_t, mut s) = setup();
        let mut x = s.load().unwrap().snapshot;
        x.requests.push(req());
        s.save(x).unwrap();
        let mut x = s.load().unwrap().snapshot;
        x.requests[0]["headers"] = json!([42]);
        assert!(s.save(x).is_err());
        assert_eq!(s.load().unwrap().snapshot.requests[0]["headers"], json!([]));
    }
    /// 验证只读数据库保存返回失败，不报告虚假成功。
    #[test]
    fn readonly_database_does_not_report_success() {
        let (t, mut s) = setup();
        let x = s.load().unwrap().snapshot;
        let db = t.path().join("data").join(DB);
        let original = fs::metadata(&db).unwrap().permissions();
        let mut readonly = original.clone();
        readonly.set_readonly(true);
        fs::set_permissions(&db, readonly).unwrap();
        assert!(s.save(x.clone()).is_err());
        fs::set_permissions(db, original).unwrap();
        s.save(x).unwrap();
    }
    /// Windows 目录的只读外观属性不能阻止正常保存，数据库文件权限仍独立校验。
    #[cfg(windows)]
    #[test]
    fn windows_directory_readonly_attribute_allows_save() {
        let (t, mut s) = setup();
        let x = s.load().unwrap().snapshot;
        let directory = t.path().join("data");
        let original = fs::metadata(&directory).unwrap().permissions();
        let mut readonly = original.clone();
        readonly.set_readonly(true);
        fs::set_permissions(&directory, readonly).unwrap();
        let result = s.save(x);
        fs::set_permissions(directory, original).unwrap();
        assert!(result.is_ok());
    }
    /// 验证第一版模式升级保留草稿，并生成可独立重新打开的旧版备份。
    #[test]
    fn version_one_upgrades_with_reopenable_backup_and_preserves_drafts() {
        let (t, mut service) = setup();
        let mut snapshot = service.load().unwrap().snapshot;
        snapshot.requests.push(req());
        service.save(snapshot).unwrap();
        drop(service);
        let path = t.path().join("data");
        let conn = Connection::open(path.join(DB)).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        drop(conn);
        let mut manifest = read_manifest(&path).unwrap();
        manifest.schema_version = 1;
        atomic_json(&path.join("manifest.json"), &manifest).unwrap();
        let upgraded = Workspace::open(&path).unwrap();
        assert_eq!(upgraded.snapshot().unwrap().requests.len(), 1);
        assert_eq!(read_manifest(&path).unwrap().schema_version, 2);
        let backup = fs::read_dir(path.join("backups"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let copy = Connection::open(backup.join(DB)).unwrap();
        assert_eq!(
            copy.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            copy.query_row("SELECT count(*) FROM requests", [], |r| r.get::<_, u32>(0))
                .unwrap(),
            1
        );
    }
    /// 验证数据库已升级而清单未更新时，重新打开可补齐清单。
    #[test]
    fn incomplete_manifest_upgrade_finishes_on_reopen() {
        let (t, mut s) = setup();
        s.load().unwrap();
        drop(s);
        let path = t.path().join("data");
        let mut m = read_manifest(&path).unwrap();
        m.schema_version = 1;
        atomic_json(&path.join("manifest.json"), &m).unwrap();
        drop(Workspace::open(&path).unwrap());
        assert_eq!(read_manifest(&path).unwrap().schema_version, 2);
    }
    /// 验证网络配置及扩展请求字段在重启和目录迁移后保持一致。
    #[test]
    fn network_configuration_and_new_request_fields_survive_restart_and_move() {
        let (t, mut s) = setup();
        let mut snapshot = s.load().unwrap().snapshot;
        let mut request = req();
        request["auth"] = json!("apiKey");
        request["apiKeyName"] = json!("X-Key");
        request["apiKeyValue"] = json!("local-fixture");
        request["bodyType"] = json!("form");
        request["form"] = json!([]);
        snapshot.requests.push(request.clone());
        let network = json!({"globals":[{"id":"g1","key":"base_url","value":"http://localhost","enabled":true,"secret":false}],"environments":[{"id":"env1","name":"验收环境","variables":[{"id":"e1","key":"token","value":"local-only","enabled":true,"secret":true}]}],"collections":{"default":{"variables":[],"auth":{"auth":"bearer","token":"{{token}}","username":"","password":""}}},"defaults":{"timeoutMs":5000,"connectTimeoutMs":1000,"followRedirects":true,"verifyTls":true,"maxResponseMib":10}});
        snapshot.session["network"] = network.clone();
        s.save(snapshot).unwrap();
        drop(s);
        let mut s = StorageService::new(t.path().join("config"), t.path().join("data"));
        let loaded = s.load().unwrap();
        assert_eq!(loaded.snapshot.session["network"], network);
        assert_eq!(loaded.snapshot.requests[0], request);
        let dest = t.path().join("moved");
        fs::create_dir(&dest).unwrap();
        let migrated = s.switch(&dest, "migrate").unwrap();
        assert_eq!(migrated.snapshot.session["network"], network);
        assert_eq!(migrated.snapshot.requests[0]["auth"], "apiKey");
    }
}
