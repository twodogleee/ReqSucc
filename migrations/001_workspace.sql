-- 初始化空数据库的第一版工作空间结构，仅由工作空间创建流程执行。
-- 不适用于已初始化数据库；初始集合、空间标识和会话内容由存储层随后写入。
PRAGMA foreign_keys = ON;
-- 单行元数据：id 固定为 1，workspace_id 对应目录清单，revision 用于阻止旧快照覆盖。
CREATE TABLE workspace_meta (id INTEGER PRIMARY KEY CHECK(id=1), workspace_id TEXT NOT NULL, revision INTEGER NOT NULL DEFAULT 0);
-- 集合与目录：id 为稳定标识，parent 为空表示根集合，name 为显示名，position 为排序下标。
-- 父目录外键延迟到事务提交时检查，允许整份快照按显示顺序重新写入。
CREATE TABLE folders (id TEXT PRIMARY KEY, parent TEXT REFERENCES folders(id) DEFERRABLE INITIALLY DEFERRED, name TEXT NOT NULL, position INTEGER NOT NULL);
-- 已保存接口：id 为请求标识，folder 为归属目录，payload 为完整 JSON，position 为排序下标。
CREATE TABLE requests (id TEXT PRIMARY KEY, folder TEXT NOT NULL REFERENCES folders(id) DEFERRABLE INITIALLY DEFERRED, payload TEXT NOT NULL, position INTEGER NOT NULL);
-- 单行会话：id 固定为 1，payload 单独保存标签草稿、基线和界面状态，不覆盖已保存接口。
CREATE TABLE session (id INTEGER PRIMARY KEY CHECK(id=1), payload TEXT NOT NULL);
-- 模式版本由存储层读取，用于选择后续升级步骤。
PRAGMA user_version = 1;
