//! 界面与后台任务的事件桥接；存储命令在独立线程按入队顺序执行。
//!
//! 作者：Andersen

use crate::{
    http,
    storage::{Loaded, Snapshot, StoragePlan, StorageService},
};
use std::{path::PathBuf, sync::mpsc};

/// 后台任务回传界面的结果，携带定位请求或保存批次所需的标识。
pub enum Event {
    /// 草稿保存回执，依次携带界面变更代数及数据库新修订号或错误。
    Saved(u64, Result<i64, String>),
    /// 数据目录加载或切换结果，成功时包含目录路径及完整快照。
    Loaded(Result<Loaded, String>),
    /// 目录操作预检结果，依次携带操作模式及计划或错误。
    Planned(String, Result<StoragePlan, String>),
    /// HTTP 执行结果，依次携带标签标识、运行标识及响应或失败信息。
    Response(String, String, Result<http::Output, http::Failure>),
    /// 响应文件导出完成回执；错误用于提示保存失败。
    Export(Result<(), String>),
}
/// 存储线程按顺序消费的命令，避免并发写入和目录切换越过保存队列。
enum Command {
    /// 异步保存草稿，依次携带界面变更代数及待保存快照。
    Save(u64, Snapshot),
    /// 同步刷盘，依次携带最新快照及返回数据库修订号或错误的确认通道。
    Flush(Snapshot, mpsc::SyncSender<Result<i64, String>>),
    /// 切换数据目录，依次携带目标路径及 create、open 或 migrate 模式。
    Switch(PathBuf, String),
    /// 预检目录操作，依次携带目标路径及操作模式，不执行实际切换。
    Plan(PathBuf, String),
    /// 加载当前配置指向的工作空间。
    Load,
}
/// 存储线程的可克隆命令入口，界面通过事件通道接收异步结果。
#[derive(Clone)]
pub struct Backend {
    /// 向唯一存储工作线程发送命令的通道。
    tx: mpsc::Sender<Command>,
}
impl Backend {
    /// 创建串行存储线程并立即排队加载数据；config 为配置目录，default 为默认数据目录。
    pub fn new(config: PathBuf, default: PathBuf, events: mpsc::Sender<Event>) -> Self {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut service = StorageService::new(config, default);
            let mut revision = None;
            let mut save_failed = false;
            for command in rx {
                match command {
                    Command::Load => {
                        let result = service.load();
                        if let Ok(v) = &result {
                            revision = Some(v.snapshot.revision)
                        }
                        let _ = events.send(Event::Loaded(result));
                    }
                    Command::Save(generation, mut snapshot) => {
                        if let Some(r) = revision {
                            snapshot.revision = r
                        }
                        let result = service.save(snapshot);
                        if let Ok(r) = &result {
                            revision = Some(*r)
                        }
                        save_failed = result.is_err();
                        let _ = events.send(Event::Saved(generation, result));
                    }
                    Command::Flush(mut snapshot, ack) => {
                        if let Some(r) = revision {
                            snapshot.revision = r
                        }
                        let result = service.save(snapshot);
                        if let Ok(r) = &result {
                            revision = Some(*r)
                        }
                        save_failed = result.is_err();
                        let _ = ack.send(result);
                    }
                    Command::Plan(path, mode) => {
                        let _ =
                            events.send(Event::Planned(mode.clone(), service.plan(&path, &mode)));
                    }
                    Command::Switch(path, mode) => {
                        let result = if save_failed {
                            Err("当前草稿未能写入磁盘，已停止目录切换。请先重试保存。".into())
                        } else {
                            service.switch(&path, &mode)
                        };
                        if let Ok(v) = &result {
                            revision = Some(v.snapshot.revision)
                        }
                        let _ = events.send(Event::Loaded(result));
                    }
                }
            }
        });
        let b = Self { tx };
        b.load();
        b
    }
    /// 将加载当前工作空间的命令入队，结果通过 Loaded 事件返回。
    pub fn load(&self) {
        let _ = self.tx.send(Command::Load);
    }
    /// 按界面变更代数排队保存快照，工作线程以最新数据库修订号串行写入。
    pub fn save(&self, generation: u64, snapshot: Snapshot) {
        let _ = self.tx.send(Command::Save(generation, snapshot));
    }
    /// 在已有命令之后写入最新快照并阻塞等待结果；线程退出、回执丢失或保存失败时返回错误。
    pub fn flush(&self, snapshot: Snapshot) -> Result<i64, String> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.tx
            .send(Command::Flush(snapshot, tx))
            .map_err(|_| "存储线程已退出")?;
        rx.recv().map_err(|_| "未收到存储结果".to_string())?
    }
    /// 排队检查目标目录与操作模式，供界面展示迁移或切换计划。
    pub fn plan(&self, path: PathBuf, mode: String) {
        let _ = self.tx.send(Command::Plan(path, mode));
    }
    /// 排队执行目录切换；最近一次草稿保存失败时由工作线程阻止切换。
    pub fn switch(&self, path: PathBuf, mode: String) {
        let _ = self.tx.send(Command::Switch(path, mode));
    }
}
/// 优先使用 REQSUCC_HOME，否则返回平台配置目录下的应用目录。
///
/// # Panics
/// 未指定覆盖目录且系统无法提供平台数据或配置目录时终止调用。
pub fn data_home() -> PathBuf {
    if let Some(path) = std::env::var_os("REQSUCC_HOME") {
        return PathBuf::from(path);
    }
    #[cfg(target_os = "macos")]
    return dirs::data_dir()
        .expect("无法定位应用数据目录")
        .join("com.reqsucc.desktop");
    #[cfg(not(target_os = "macos"))]
    return dirs::config_dir()
        .expect("无法定位应用数据目录")
        .join("com.reqsucc.desktop");
}

/// 验证存储队列刷盘顺序和保存失败时的目录切换保护。
#[cfg(test)]
mod tests {
    use super::*;
    use crate::model;
    use serde_json::json;
    use std::time::Duration;

    /// 验证排队保存后同步刷盘，再迁移目录仍保留最新草稿。
    #[test]
    fn queued_saves_flush_and_migration_preserve_latest_draft() {
        let root = tempfile::tempdir().unwrap();
        let (events, rx) = mpsc::channel();
        let backend = Backend::new(root.path().join("config"), root.path().join("data"), events);
        let Event::Loaded(Ok(loaded)) = rx.recv_timeout(Duration::from_secs(3)).unwrap() else {
            panic!("加载失败")
        };
        let mut snapshot = loaded.snapshot;
        model::normalize(&mut snapshot);
        let folder = snapshot.folders[0].id.clone();
        model::open(&mut snapshot, model::request(&folder));
        model::active_mut(&mut snapshot).unwrap()["body"] = json!("第一份草稿");
        backend.save(1, snapshot.clone());
        model::active_mut(&mut snapshot).unwrap()["body"] = json!("最终中文草稿");
        let revision = backend.flush(snapshot.clone()).unwrap();
        assert!(revision >= 2);
        let target = root.path().join("moved");
        std::fs::create_dir(&target).unwrap();
        backend.switch(target.clone(), "migrate".into());
        loop {
            match rx.recv_timeout(Duration::from_secs(3)).unwrap() {
                Event::Loaded(result) => {
                    let loaded = result.unwrap();
                    assert_eq!(
                        model::active(&loaded.snapshot).unwrap()["body"],
                        "最终中文草稿"
                    );
                    assert_eq!(
                        loaded.path,
                        target.canonicalize().unwrap().to_string_lossy()
                    );
                    break;
                }
                _ => {}
            }
        }
    }

    /// 验证草稿写入失败会阻止后续目录切换，避免丢失内存编辑。
    #[test]
    fn failed_draft_save_blocks_directory_switch() {
        let root = tempfile::tempdir().unwrap();
        let (events, rx) = mpsc::channel();
        let backend = Backend::new(root.path().join("config"), root.path().join("data"), events);
        let Event::Loaded(Ok(loaded)) = rx.recv_timeout(Duration::from_secs(3)).unwrap() else {
            panic!("加载失败")
        };
        let mut snapshot = loaded.snapshot;
        model::normalize(&mut snapshot);
        model::open(&mut snapshot, model::request(""));
        model::active_mut(&mut snapshot).unwrap()["method"] = json!("");
        assert!(backend.flush(snapshot).is_err());
        let target = root.path().join("other");
        std::fs::create_dir(&target).unwrap();
        backend.switch(target.clone(), "create".into());
        let Event::Loaded(Err(error)) = rx.recv_timeout(Duration::from_secs(3)).unwrap() else {
            panic!("切换未被阻止")
        };
        assert!(error.contains("停止目录切换"));
        assert!(!target.join("manifest.json").exists());
    }
}
