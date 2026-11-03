use crate::{
    http,
    storage::{Loaded, Snapshot, StoragePlan, StorageService},
};
use std::{path::PathBuf, sync::mpsc};

pub enum Event {
    Saved(u64, Result<i64, String>),
    Loaded(Result<Loaded, String>),
    Planned(String, Result<StoragePlan, String>),
    Response(String, String, Result<http::Output, http::Failure>),
    Export(Result<(), String>),
}
enum Command {
    Save(u64, Snapshot),
    Flush(Snapshot, mpsc::SyncSender<Result<i64, String>>),
    Switch(PathBuf, String),
    Plan(PathBuf, String),
    Load,
}
#[derive(Clone)]
pub struct Backend {
    tx: mpsc::Sender<Command>,
}
impl Backend {
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
    pub fn load(&self) {
        let _ = self.tx.send(Command::Load);
    }
    pub fn save(&self, generation: u64, snapshot: Snapshot) {
        let _ = self.tx.send(Command::Save(generation, snapshot));
    }
    pub fn flush(&self, snapshot: Snapshot) -> Result<i64, String> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.tx
            .send(Command::Flush(snapshot, tx))
            .map_err(|_| "存储线程已退出")?;
        rx.recv().map_err(|_| "未收到存储结果".to_string())?
    }
    pub fn plan(&self, path: PathBuf, mode: String) {
        let _ = self.tx.send(Command::Plan(path, mode));
    }
    pub fn switch(&self, path: PathBuf, mode: String) {
        let _ = self.tx.send(Command::Switch(path, mode));
    }
}
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model;
    use serde_json::json;
    use std::time::Duration;

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
