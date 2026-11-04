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
- 基于 GPUI Kit 标准组件统一界面：TabBar 标签栏、Sidebar 导航、Table 参数与订阅表、Select 选择器、Form / Field 表单、Switch 开关、ListItem 列表、Tag 协议标记、Alert 提示、Empty 空状态和 StatusBar 状态栏。常规工具栏控件高度 28 逻辑像素，表头与行共用列宽，深浅主题共用语义配色。
- 字段选择器提供当前值和键盘选择，选项超过 6 项时自动支持搜索；复制、移动、删除等操作继续使用统一动作菜单。
- 分类设置页：显示设置、数据存储、请求设置、关于。界面缩放 90% / 100% / 110% / 125%，编辑器字号 12–20 像素，行号、自动换行、增强边框对比度，可恢复默认；偏好自动保存并在重启后恢复。
- 集合与多级文件夹管理，接口保存、复制、移动、排序；多标签编辑、拖动调整标签顺序、自动保存草稿、重启恢复。标签右键菜单支持复制当前草稿、关闭当前 / 其他 / 左侧所有 / 右侧所有 / 全部标签；副本插入原标签右侧，批量关闭统一确认未保存修改并释放对应 HTTP / MQTT 资源。
- 紧凑工作台：控件字号与间距统一收紧，标签根据可用空间在 120–168 逻辑像素间调整宽度，长名称省略，可通过全部标签菜单查看完整名称；提供横向滚动、全部标签菜单及当前标签自动滚入视野。环境选择位于顶部工具栏。
- HTTP 方法、地址、查询参数、请求头，JSON / 文本 / XML / 表单编码请求体。
- 无认证、Bearer Token、Basic Auth、API Key，以及集合认证继承。
- 全局、集合、环境变量解析，环境管理与敏感变量显示控制。
- 异步发送与取消，超时、重定向、TLS 校验配置；响应正文、响应头、Cookie 与实际请求信息。
- JSON 语法高亮、格式化、原始文本、复制预览、完整响应文件导出。
- 点击请求名称左侧的协议图标切换 HTTP / MQTT V5；两种协议配置分别保留。Kafka、WebSocket 预留为待支持项。
- MQTT V5：TCP / TLS 连接与取消，用户名密码，文本 / JSON 发布，QoS 0/1/2、保留消息，通配符与共享主题订阅、取消订阅。
- MQTT 连接用户属性、发布内容类型 / 响应主题 / 有效期、遗嘱消息；全局、集合、环境变量在连接、发布、订阅时分别解析。
- MQTT 消息区使用方向与状态图标区分发送、接收、排队和确认，详情支持滚动查看；完整 JSON 消息自动格式化显示，复制详情保留原始内容。连接配置与订阅列表随接口、草稿保存。
- SQLite 本地持久化，可选择数据目录、打开已有数据、迁移或建立新数据目录。

不包含账号、云同步、团队协作、脚本、断言或集合自动运行。历史列表尚未开放；当前响应在内存和临时缓存中，重启后不保留。

## MQTT 使用

新建请求后点击名称左侧的地球图标，选择 **MQTT · V5**。填写 `mqtt://主机:1883` 或 `mqtts://主机:8883`，设置客户端标识与认证。主题订阅页可添加主题并打开连接后订阅开关；连接成功后，在消息页填写发布主题和内容，点击“发布消息”。`⌘ / Ctrl + Enter` 用于连接或断开。

消息页的“发布用例”可保存多组主题和消息，包含内容格式、QoS、保留标记及发布属性；使用“新建”和旁边的操作菜单可以添加、复制、重命名或删除用例。切换用例不会断开连接或自动发送。点击上方“保存”会将所有用例与连接配置一起保存，未保存的编辑也会作为草稿恢复；旧版的单条发布配置会成为“默认消息”。每个连接最多保存 100 个发布用例。

连接期间可以发布消息、订阅或取消订阅；修改连接参数和主题定义前需要断开。订阅状态以服务器确认结果为准，QoS 0 的写入记录不表示服务器已确认。连接失败或中断不会自动重连。关闭标签会关闭其连接；切换数据目录前须先断开所有 MQTT 连接。

首版固定 MQTT V5 和全新会话（Clean Start，Session Expiry = 0），重启只恢复配置，不恢复连接或消息记录。不支持 MQTT 3.1.1、MQTT over WebSocket、自定义 CA、双向证书、增强认证或持久会话。TLS 使用系统可信根证书校验。

单连接最多保存 100 个订阅主题，发布消息和遗嘱上限 256 KiB，接收报文上限 1 MiB；最近 500 条消息记录保存在内存，单条详情预览最多 8 KiB。当前不提供完整 MQTT 消息导出。

## 字体与文字层级

界面使用统一的 `TextRole`，由 `src/theme.rs` 定义；100% 缩放下，页面标题 18、分组标题 14、正文 13、控件 12.25、辅助说明 12、状态与计数 11 逻辑像素。仅标题使用中等或半粗字重，正文和说明使用常规字重；说明使用主题次要文字色和 1.6 倍行高，支持自然换行。

普通文字基于 GPUI Kit `Label`，表单帮助使用 `Field.description_fn`，空状态使用 `EmptyTitle` / `EmptyDescription`，风险提示使用 `Alert`。各页面调用统一文字角色，避免直接附加长段字符串继承正文样式。字号采用相对于主题字号的 rem，跟随界面缩放一次；代码编辑器仍使用独立字号设置。

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
- `src/mqtt.rs`、`src/mqtt/tests.rs`：MQTT V5 会话、发布 / 订阅与本机报文集成检查。
- `src/ui/mqtt_view.rs`：协议入口、MQTT 编辑器与消息面板。
- `src/ui/controls.rs`：标准组件组合、选择器状态、统一表格列和表单开关。
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

60 项自动化检查覆盖 HTTP、认证与变量、超时 / 取消、响应缓存、SQLite 恢复与迁移、草稿与保存版本分离，以及迁移前队列刷盘与失败阻断；MQTT 检查使用本机 TCP 模拟服务器，覆盖 V5 握手、QoS 0/1/2、拒绝回执、取消订阅、超时 / 取消、变量和所有 MQTT 页签的草稿保存，并检查发布用例、JSON 模板格式化、消息时间、标签重排、草稿复制及批量关闭。它们是开发检查，不是产品中的脚本或断言功能。

## 品牌图标

客户端图标采用黑色圆角底与白色飞升猫，沿用简洁的黑白配色。macOS 图标由 `assets/reqsucc-icon.png` 生成，输出为 `assets/ReqSucc.icns`；应用顶部和关于页使用透明背景的 `assets/flying-cat.svg`，颜色随界面主题变化。生成提示词保存在 `assets/icon-prompt.txt`。

重新生成多分辨率应用图标：

```sh
python3 scripts/build_icons.py
```

## 上游兼容补丁

GPUI Kit 固定为 0.9.0，依赖版本由 Cargo.lock 锁定。`vendor/gpui-pre-macos` 基于上游 0.3.11，保留 Apache 许可证。该依赖有三处 `Option::map_or_default` 调用尚不能在本项目 stable 工具链上使用，替换为等价的 `map_or_else(Default::default, ...)`；通过 Cargo patch 引用，不要求 nightly 或 RUSTC_BOOTSTRAP。另有启动时显式加载应用包图标的本地补丁，用于处理开发应用重新打包后 Dock 沿用旧图标的问题。后续更新上游时分别核对这些补丁是否仍有必要。

## 开源协议

Copyright 2026 Andersen。项目采用 [Apache License 2.0](LICENSE)，归属声明见 [NOTICE](NOTICE)。

`vendor/` 中的第三方代码保留其原始许可证与署名，具体修改记录见 [gpui-pre-macos 补丁说明](vendor/gpui-pre-macos/PATCHES.md)。
