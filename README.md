# ReqSucc

ReqSucc，意为“请求成功”。全中文、本地存储的桌面接口调试工具。使用 Rust、GPUI 和 GPUI Kit 0.9.0，界面由原生 GPU 渲染。当前版本 0.4.0。

## 运行与打包

当前已在 macOS 15 / Apple Silicon 上验证。需要 Rust stable 1.95 或更高版本、Xcode Command Line Tools；打包脚本需要 Python 3。首次构建需要联网下载 Cargo 依赖，日常使用不需要账户或云服务。

```sh
cargo run --locked
```

生成可双击打开的 macOS 应用：

```sh
python3 scripts/package_macos.py
# target/debug/ReqSucc.app

python3 scripts/package_macos.py --release
# target/release/ReqSucc.app
```

应用包使用本机临时签名；尚未配置 Developer ID 分发签名和公证。Windows、Linux 尚未完成构建与界面验收。

## 功能

- 统一原生标题栏、深色 / 浅色主题，中文菜单、导航、提示与快捷键；侧栏、请求区和响应区可调整尺寸。
- 统一界面规范：普通图标 16、导航图标 20、常规控件高度 28 逻辑像素；参数表头与行共用列宽，接口列表左对齐并用更多菜单收纳操作，选择菜单统一右侧勾选、滚动与弹层配色。
- 分类设置页：显示设置、数据存储、请求设置、关于。界面缩放 90% / 100% / 110% / 125%，编辑器字号 12–20 像素，行号、自动换行、增强边框对比度，可恢复默认；偏好自动保存并在重启后恢复。
- 集合与多级文件夹管理，接口保存、复制、移动、排序；多标签编辑、自动保存草稿、重启恢复。
- 紧凑工作台：控件字号与间距统一收紧，标签根据可用空间在 120–168 逻辑像素间调整宽度，长名称省略并支持完整提示；提供横向滚动、全部标签菜单及当前标签自动滚入视野。环境选择位于顶部工具栏。
- HTTP 方法、地址、查询参数、请求头，JSON / 文本 / XML / 表单编码请求体。
- 无认证、Bearer Token、Basic Auth、API Key，以及集合认证继承。
- 全局、集合、环境变量解析，环境管理与敏感变量显示控制。
- 异步发送与取消，超时、重定向、TLS 校验配置；响应正文、响应头、Cookie 与实际请求信息。
- JSON 语法高亮、格式化、原始文本、复制预览、完整响应文件导出。
- SQLite 本地持久化，可选择数据目录、打开已有数据、迁移或建立新数据目录。

不包含账号、云同步、团队协作、脚本、断言或集合自动运行。历史列表尚未开放；当前响应在内存和临时缓存中，重启后不保留。

## 本地数据

macOS 配置目录：`~/Library/Application Support/com.reqsucc.desktop/`。默认数据位于其 `data/` 子目录，已选定的数据位置由配置文件记录。通过设置中的存储位置操作切换目录，切换前会保存当前草稿。

保留原有 SQLite 格式与迁移逻辑，现有集合、接口、草稿和环境配置可直接读取。保存失败会阻止目录切换，迁移会校验目标并保留源数据；单实例锁和数据目录锁防止多进程写入。

变量、认证与请求内容保存在本地数据库中；“敏感”表示界面遮蔽，并非数据库加密。请按需要限制数据目录的访问权限。

使用独立数据目录进行开发验收：

```sh
REQSUCC_HOME=/tmp/reqsucc-development cargo run --locked
```

## 代码结构

- `src/main.rs`：原生应用入口、菜单、快捷键和单实例检查。
- `src/ui.rs`：工作台、集合、环境、请求与响应、设置界面。
- `src/theme.rs`、`src/assets.rs`：主题、JSON 配色与图标资源。
- `src/model.rs`：请求、集合、标签与会话状态操作。
- `src/backend.rs`：串行存储队列和界面事件桥接。
- `src/http.rs`：Rust HTTP 执行、取消、认证、变量与响应缓存。
- `src/storage.rs`、`migrations/`：SQLite、目录迁移和数据兼容。
- `scripts/package_macos.py`：macOS 应用包构建。

HTTP 与 SQLite 核心复用既有实现；请求运行在 Tokio，SQLite 写入运行在独立串行工作线程，GPUI 主线程负责界面。

## 验证

```sh
cargo fmt --check
cargo test --locked
cargo build --locked
```

34 项自动化检查覆盖 HTTP、认证与变量、超时 / 取消、响应缓存、SQLite 恢复与迁移、草稿与保存版本分离，以及迁移前队列刷盘与失败阻断。它们是开发检查，不是产品中的脚本或断言功能。

## 品牌图标

客户端图标以请求箭头与成功勾号组合，使用薄荷绿与石墨色。图标源文件为 `assets/reqsucc-icon.png`，macOS 图标为 `assets/ReqSucc.icns`，顶部和关于页使用相同品牌图形。生成提示词保存在 `assets/icon-prompt.txt`。

重新生成多分辨率应用图标：

```sh
python3 scripts/build_icons.py
```

## 上游兼容补丁

GPUI Kit 固定为 0.9.0，依赖版本由 Cargo.lock 锁定。`vendor/gpui-pre-macos` 基于上游 0.3.11，保留 Apache 许可证。该依赖有三处 `Option::map_or_default` 调用尚不能在本项目 stable 工具链上使用，替换为等价的 `map_or_else(Default::default, ...)`；通过 Cargo patch 引用，不要求 nightly 或 RUSTC_BOOTSTRAP。后续上游提供稳定兼容版本时可移除本地补丁。

## 开源协议

Copyright 2026 Andersen。项目采用 [Apache License 2.0](LICENSE)，归属声明见 [NOTICE](NOTICE)。

`vendor/` 中的第三方代码保留其原始许可证与署名，具体修改记录见 [gpui-pre-macos 补丁说明](vendor/gpui-pre-macos/PATCHES.md)。
