# ReqSucc

**请求成功，调试从容。**

ReqSucc 是全中文、本地存储的原生桌面接口调试工具，支持 **HTTP、MQTT V5、WebSocket 和 Kafka**，可导入 Postman 集合与环境。使用 Rust、GPUI / GPUI Kit 构建，无需账号，工作数据保存在本机。

当前版本：**0.4.0** · [下载与版本记录](https://github.com/twodogleee/ReqSucc/releases) · [构建状态](https://github.com/twodogleee/ReqSucc/actions) · [问题反馈](https://github.com/twodogleee/ReqSucc/issues) · [作者主页](https://36knight.com)

## 平台与下载

| 平台 | 分发包 | 当前状态 |
| --- | --- | --- |
| macOS 15 及以上 / Apple Silicon | `ReqSucc-v<版本>-macos-arm64.zip` | 已完成本地构建与原生界面验收 |
| Windows 10/11 / x64 | `ReqSucc-v<版本>-windows-x64.zip` | 已接入兼容代码和构建流程，首次远程构建与实机验收待完成 |
| Intel Mac、Windows ARM64、Linux | — | 暂未提供构建与分发支持 |

从 [Releases](https://github.com/twodogleee/ReqSucc/releases) 下载对应平台的 **Assets 附件**；`Source code` 是源码归档，不是可运行应用。如果尚无版本附件，可按下文从源码构建，或由维护者生成 Actions 验收包。

- **macOS**：解压，将 `ReqSucc.app` 拖入“应用程序”后打开。
- **Windows**：完整解压，在解压目录运行 `ReqSucc.exe`。保留同目录 DLL，它们是应用所需的运行库。
- 当前使用 ZIP 分发，没有安装向导或自动更新。升级时退出旧版本、替换应用文件，工作数据保留在独立的数据目录中。
- macOS 目前仅使用临时签名，尚未配置 Developer ID 签名及公证；Windows 尚未配置 Authenticode 签名。系统仍可能显示来源或信任提示，GitHub 托管下载不等同于系统签名认证。

## 主要功能

| 功能 | 支持范围 |
| --- | --- |
| HTTP | 查询参数、请求头、JSON / 文本 / XML / URL 编码表单，认证、超时、取消、重定向、TLS 校验，响应预览与文件导出 |
| MQTT V5 | TCP / TLS、用户名密码、QoS 0/1/2、保留消息、遗嘱、主题订阅与取消、多组发布用例 |
| WebSocket | WS / WSS、自定义握手请求头与子协议、文本 / JSON / Base64 二进制消息、Ping/Pong |
| Kafka | 主题查看 / 创建 / 删除，消息发布与订阅消费，消息键 / 分区 / 消息头，SASL / SSL 常用配置 |
| 请求管理 | 集合与多级目录、复制 / 移动 / 排序、多标签、标签拖动与右键批量关闭、草稿自动保存与恢复 |
| 环境与认证 | 全局 / 集合 / 环境变量，无认证、Bearer、Basic、API Key，集合认证继承 |
| 消息展示 | 完整日期、毫秒和时区，收发方向与状态图标，JSON 格式化，复制原始详情 |
| Postman 导入 | Collection v2.0 / v2.1、环境 JSON，多文件预览、同名追加、事务保存 |
| 本地存储 | SQLite，迁移当前数据、打开已有目录、创建新目录 |
| 界面偏好 | 深浅主题、90%–125% 缩放、编辑器字号、行号与换行，应用快捷键设置 |

当前不包含账号、云同步、团队协作、脚本、断言或集合自动运行。历史列表暂不可用；响应与消息记录不会在重启后恢复。

## 开始使用

1. 新建请求，通过请求名称左侧的协议图标选择 HTTP、MQTT、WebSocket 或 Kafka。
2. 填写请求地址或连接配置。HTTP 点击“发送”；消息协议先连接，再发布消息或订阅主题。
3. 点击“保存”将请求加入集合。标题栏的“查找请求”按名称或地址查找集合中已保存的请求，不搜索未保存标签或消息内容。
4. 在“环境”中管理变量，通过 `{{变量名}}` 引用；顶部下拉框选择当前环境。
5. 在“设置”中管理外观、快捷键、数据目录和 Postman 导入。

标签右键菜单支持复制当前草稿，以及关闭当前、其他、左侧、右侧或全部标签。关闭有修改的标签会丢弃未保存内容；确认后本次启动不再提醒。退出应用会保留仍打开的标签草稿，下次启动恢复。

窄窗口自动使用可收起的集合面板和设置分类下拉框；矮窗口可上下滚动查看请求与响应。大窗口保留拖动分栏。错误提示使用浮层，不挤动主布局。

### 默认快捷键

| 操作 | macOS | Windows |
| --- | --- | --- |
| 查找请求 | `⌘ K` | `Ctrl K` |
| 新建请求 | `⌘ N` | `Ctrl N` |
| 保存请求 | `⌘ S` | `Ctrl S` |
| 发送 / 取消 HTTP，连接 / 断开消息协议 | `⌘ Enter` | `Ctrl Enter` |
| 关闭当前标签 | `⌘ W` | `Ctrl W` |
| 打开设置 | `⌘ ,` | `Ctrl ,` |
| 退出应用 | `⌘ Q` | `Ctrl Q` |

在“设置 → 快捷键”录入、清除或恢复默认。两平台分别保存配置，标准复制、粘贴、撤销、全选等编辑快捷键不受影响。旧版 macOS 自定义组合键继续保留；首次在 Windows 打开同一份数据时使用 Ctrl 默认值，并保留旧版显式清除的状态。Windows 不接受 Win 和已识别的系统保留组合键。

## MQTT 使用

新建请求后点击名称左侧的协议图标，选择 **MQTT · V5**。填写 `mqtt://主机:1883` 或 `mqtts://主机:8883`，设置客户端标识与认证。主题订阅页可添加主题并打开连接后订阅开关；连接成功后，在消息页填写发布主题和内容，点击“发布消息”。`⌘ / Ctrl + Enter` 用于连接或断开。

消息页的“发布用例”可保存多组主题和消息，包含内容格式、QoS、保留标记及发布属性；使用“新建”和旁边的操作菜单可以添加、复制、重命名或删除用例。切换用例不会断开连接或自动发送。点击上方“保存”会将所有用例与连接配置一起保存，未保存的编辑也会作为草稿恢复；旧版的单条发布配置会成为“默认消息”。每个连接最多保存 100 个发布用例。

连接期间可以发布消息、订阅或取消订阅；修改连接参数和主题定义前需要断开。订阅状态以服务器确认结果为准，QoS 0 的写入记录不表示服务器已确认。连接失败或中断不会自动重连。关闭标签会关闭其连接；切换数据目录前须先断开所有 MQTT 连接。

使用 MQTT V5 和全新会话（Clean Start，Session Expiry = 0），重启只恢复配置，不恢复连接或消息记录。不支持 MQTT 3.1.1、MQTT over WebSocket、自定义 CA、双向证书、增强认证或持久会话。TLS 使用系统可信根证书校验。

单连接最多保存 100 个订阅主题，发布消息和遗嘱上限 256 KiB，接收报文上限 1 MiB；最近 500 条消息记录保存在内存，单条详情预览最多 8 KiB。当前不提供完整 MQTT 消息导出。

## Postman 数据导入

在“设置 → 数据存储 → Postman 数据导入”中选择本地导出的 JSON，核对预览数量和兼容性提示后点击“确认导入”。支持 Collection v2.0 / v2.1 和带有环境标记的 Environment JSON；可同时选择最多 20 个文件，总大小不超过 20 MiB，单次最多处理 10000 个接口、2000 个集合及目录。

- 保留集合层级、HTTP 方法与地址、路径变量、重复及禁用查询参数、请求头、描述、原始文本 / JSON / XML 请求体、URL 编码表单，以及集合和环境变量。
- 支持无认证、Bearer、Basic 和 API Key。集合及目录的继承认证按导入时的有效值写入每条接口；导入后修改集合认证不会自动改写这些接口。
- 全部内容作为新增数据写入，同名根集合和环境自动添加名称后缀；已有接口、未保存标签草稿和当前环境选择保持不变。确认后通过同一 SQLite 事务保存，失败时原数据保持不变。
- 脚本不会执行或导入，示例响应和 Postman 专用协议行为不导入；文件夹局部变量会提示手动补充。使用不支持认证、multipart / 文件上传 / GraphQL 请求体或非 HTTP 协议的接口会跳过并在预览中统计。
- 暂不支持 ZIP、全局变量导出或其他集合版本。变量占位符按原文保留，敏感值标记仅用于界面遮蔽，本地数据库不加密。

导出步骤参考 [Postman 官方导出说明](https://learning.postman.com/docs/getting-started/importing-and-exporting/exporting-data/)。

## WebSocket 与 Kafka 使用

新建请求后从名称左侧的协议菜单选择 WebSocket 或 Kafka，填写地址并连接。发送快捷键用于连接 / 断开；发送正文使用消息页中的“发送消息”或“发布消息”。连接配置、请求头、消息草稿及主题设置随请求保存，重启后不会自动连接或恢复消息记录。

WebSocket 地址使用 `ws://` 或 `wss://`，在“请求头”中添加 Authorization 等自定义字段，“连接设置”可填写逗号分隔的子协议。协议必需握手头由客户端维护。WSS 使用系统可信根证书并验证服务器身份；暂不提供自定义 CA、客户端证书或关闭验证的选项。文本和 JSON 发送文本帧，Base64 解码后发送二进制帧；Ping 可检查心跳。写入成功与业务响应分别记录，不自动重连。

Kafka 地址使用 `主机:端口`，多个 bootstrap 地址以逗号分隔。“连接与认证”配置客户端标识、消费组、起始位置和安全协议；SASL 支持 PLAIN、SCRAM-SHA-256 与 SCRAM-SHA-512，SSL 支持 CA PEM、客户端证书与私钥 PEM、私钥口令，始终验证证书与主机名。客户端证书和私钥须配对填写。暂不支持 Kerberos / GSSAPI、OAuth、Schema Registry、Avro、事务发布或位移手动管理。

Kafka 连接后可在“主题管理”查看主题及分区数量、显式创建主题、确认删除主题。“订阅消费”使用逗号分隔的精确主题名，界面分别显示订阅意图和实际分配的分区数；不支持通配符。默认每个新配置使用独立消费组，不自动提交消费位移；最早 / 最新仅在没有已提交位移时生效。发布可设置主题、消息键、分区（-1 自动）及消息头；成功记录以服务器确认的分区与位移为准。连接期间客户端可恢复瞬时网络故障，但不会在重启或用户断开后自动新建会话。

两种协议发送消息上限为 256 KiB（Kafka 包含键和消息头），每个会话最多保留最近 500 条记录，详情预览上限为 8 KiB，二进制以 Base64 展示；当前不提供完整消息导出。WebSocket 接收消息上限为 1 MiB；Kafka 限制拉取批次和客户端接收缓冲，展示仍使用有界预览。Kafka 最多显示 1000 个主题，单次最多订阅 100 个精确主题。网络及管理操作超时为 1–60 秒；取消不能撤回已提交服务器的发布或主题操作，超时后应刷新确认实际结果。

连接期间握手与认证配置锁定，正文与显式主题操作可编辑。关闭标签会取消其会话，切换数据目录前须断开所有协议连接。敏感字段仅作界面遮蔽，本地数据库不加密。

## 从源码构建

CI 固定使用 **Rust 1.95.0** 和 `Cargo.lock`。Python 建议使用 3.12；macOS 系统 Python 3.9 也可运行打包脚本。首次构建需要联网下载依赖，Kafka、OpenSSL 和 zlib 的原生库编译耗时较长。

```sh
git clone https://github.com/twodogleee/ReqSucc.git
cd ReqSucc
rustup toolchain install 1.95.0 --profile minimal --component rustfmt
```

下列命令均在仓库根目录执行。本地 `cargo` 使用当前默认工具链；需要与 CI 严格一致时使用 `cargo +1.95.0`，或先运行 `rustup override set 1.95.0`。

### macOS

需要 Apple Silicon Mac、macOS 15 及以上和 Xcode Command Line Tools（`xcode-select --install`）。

```sh
# 使用隔离目录运行，避免影响日常数据
REQSUCC_HOME=/tmp/reqsucc-development cargo run --locked

# 构建并打包调试版
python3 scripts/package_macos.py

# 构建正式版并生成分发 ZIP
cargo build --release --locked
python3 scripts/release.py package --platform macos-arm64
```

输出位置：

- 调试应用：`target/debug/ReqSucc.app`
- 正式应用：`target/release/ReqSucc.app`
- 分发包：`target/dist/ReqSucc-v0.4.0-macos-arm64.zip`

只生成正式 `.app` 时可运行 `python3 scripts/package_macos.py --release`。已有最新二进制时可加 `--skip-build` 跳过编译；该选项不会检查源码是否比二进制更新。

### Windows

需要 Visual Studio 2022 C++ 构建工具与 Windows SDK、CMake、Strawberry Perl、NASM 和 Python。请在 **x64 Native Tools Command Prompt for VS 2022** 中运行，确认 `cmake`、`perl`、`nasm`、`python` 和 `cargo` 可用。

```bat
rustup toolchain install 1.95.0 --profile minimal --component rustfmt --target x86_64-pc-windows-msvc
set OPENSSL_SRC_PERL=C:\Strawberry\perl\bin\perl.exe
cargo +1.95.0 test --locked --target x86_64-pc-windows-msvc
cargo +1.95.0 build --release --locked --target x86_64-pc-windows-msvc
python scripts/release.py package --platform windows-x64 --binary-dir target/x86_64-pc-windows-msvc/release
```

如果 Perl 安装路径不同，请调整 `OPENSSL_SRC_PERL`。打包依赖开发者命令行提供的 `VCToolsRedistDir`，用于收集 MSVC x64 运行库；缺失时脚本会停止，避免生成不能独立运行的包。

Windows 编译请使用上述开发者命令行或已加载 MSVC 环境的 PowerShell。若日志中的链接器为 `Git\usr\bin\link.exe`，说明误用了 Git Bash 的同名工具；应切换终端并检查 `where.exe link.exe` 的首项来自 Visual Studio。CI 使用 PowerShell 构建，显式固定 MSVC 链接器，并在依赖编译前验证宿主程序能完成链接和启动。

输出为 `target/dist/ReqSucc-v0.4.0-windows-x64.zip`，包含 `ReqSucc.exe`、运行库、README 和许可文件。正式程序内嵌图标、版本与多显示器 DPI 清单，不显示控制台；启动失败和重复启动使用中文原生消息框。

## GitHub 构建与分发

工作流：[`.github/workflows/build.yml`](.github/workflows/build.yml)。Windows 使用 `windows-2022`，macOS 使用 `macos-15` ARM64；两个平台独立运行测试与构建。

| 触发方式 | 构建结果 | 是否发布 Release |
| --- | --- | --- |
| 推送 `main` / 提交 PR | 格式与脚本检查、Rust 测试、开发构建 | 否，不上传分发 ZIP |
| 手动 `Run workflow` | 测试、Release 构建、双平台 ZIP 和校验清单 | 否，仅上传 Artifacts |
| 推送与应用版本一致的 `v*` 标签 | 测试、Release 构建、打包、上传 Release 附件 | 是，两个平台全部成功后发布 |

### 首次接入与验收

1. 将源码、锁文件、Windows 资源、打包脚本和工作流提交并推送到 `main`。不要提交 `target/` 或真实用户数据。
2. 在 [Actions](https://github.com/twodogleee/ReqSucc/actions) 确认自动构建通过。
3. 选择“构建与发布 → Run workflow → main”，手动生成验收包。工作流需要先进入仓库默认分支，手动触发入口才可用。
4. 运行成功后，在 Artifacts 下载 `windows-x64`、`macos-arm64` 和 `SHA256SUMS`。平台验收产物保存 14 天；GitHub 下载的外层归档内包含实际分发 ZIP。
5. 在两平台验证启动、中文输入、快捷键、主题和缩放、文件选择、数据保存与迁移，以及实际使用的协议和认证配置。

Artifacts 下载需要登录 GitHub 并有仓库读取权限，适合开发验收；用户下载入口使用 Releases。Windows 首次远程构建及实机验收尚未完成，不能用 macOS 测试结果代替。参考 [手动运行工作流](https://docs.github.com/en/actions/how-tos/manage-workflow-runs/manually-run-a-workflow) 与 [下载构建产物](https://docs.github.com/en/actions/how-tos/manage-workflow-runs/download-workflow-artifacts)。

### 正式发布

确认工作区干净、待发布提交已经验收，并检查 `Cargo.toml`、`Cargo.lock` 与 README 版本一致。以下以当前 `0.4.0` 为例，最后一步会触发公开发布：

```sh
python3 scripts/release.py metadata --tag v0.4.0
git push origin main
git tag -a v0.4.0 -m "ReqSucc 0.4.0"
git push origin v0.4.0
```

工作流自动生成版本说明，并向 [Releases](https://github.com/twodogleee/ReqSucc/releases) 上传：

```text
ReqSucc-v0.4.0-windows-x64.zip
ReqSucc-v0.4.0-macos-arm64.zip
SHA256SUMS.txt
```

- 标签必须为 `v` 加上 `Cargo.toml` 完整版本。发布 `v0.4.1-rc.1` 时，应用版本也必须为 `0.4.1-rc.1`，工作流将其标为预发布。
- 已存在的 Release 不自动覆盖。下次更新使用新版本和新标签，不移动已发布标签。
- 发布使用仓库自带的 `GITHUB_TOKEN`，不需要个人访问令牌；仓库或组织策略须允许 Actions 及工作流所声明的 `contents: write` 权限。
- 两个平台 ZIP 齐全后才生成校验清单。手动运行 `python3 scripts/release.py checksums` 时，`target/dist` 必须只包含当前版本这两个 ZIP。

## 本地数据与备份

| 平台 | 默认配置目录 |
| --- | --- |
| macOS | `~/Library/Application Support/com.reqsucc.desktop/` |
| Windows | `%APPDATA%\com.reqsucc.desktop\` |

默认工作数据位于配置目录的 `data/` 子目录，已选定的数据位置由配置文件记录。`REQSUCC_HOME` 可指定独立配置目录；其下同样默认使用 `data/` 保存工作数据。

“设置 → 数据存储”支持迁移当前数据、打开已有目录或创建新目录。切换前保存草稿，保存失败会阻止切换；创建和迁移的目标目录必须为空，迁移后保留源目录。切换前需结束在途请求并断开消息连接。

备份时先退出应用，再复制完整配置目录和所选数据目录，避免遗漏 SQLite 附属文件。接口、集合、环境、认证和草稿保存在本地；敏感值遮蔽不等于磁盘加密。单实例锁和数据目录锁用于防止多个进程同时写入。

## 验证

```sh
cargo fmt --check
cargo test --locked
cargo build --locked
python3 -m unittest discover -s scripts -p 'test_*.py'
git diff --check
```

macOS 当前回归结果为 88 项通过、1 项真实 Kafka 集群测试默认忽略；Windows 另有目录只读属性专用测试，结果以对应 Actions 运行记录为准。3 项 Python 测试覆盖标签版本、Windows 归档内容及双平台校验清单。

Rust 自动化检查覆盖 HTTP、认证与变量、超时 / 取消、响应缓存、SQLite 恢复与迁移、草稿与保存版本分离，以及迁移前队列刷盘与失败阻断；MQTT 检查使用本机 TCP 模拟服务器，覆盖 V5 握手、QoS 0/1/2、拒绝回执、取消订阅、超时 / 取消、变量和所有 MQTT 页签的草稿保存，并检查发布用例、JSON 模板格式化、消息时间、标签重排、草稿复制、批量关闭、快捷键默认 / 清除 / 冲突规则及重启和迁移恢复；Postman 检查覆盖目录与认证映射、参数与变量、批量文件校验、同名追加、事务失败保护和重启恢复。另覆盖平台快捷键配置隔离、旧配置兼容、Windows 保留组合键及工作区窗口尺寸和缩放断点。它们是开发检查，不是产品中的脚本或断言功能。

WebSocket 检查使用本机握手及回显服务，验证自定义头、子协议、文本 / 二进制帧、心跳、取消与超时；Kafka 默认检查使用进程内模拟集群验证元数据、消息键 / 头、发布确认、消费及取消订阅。另有一项默认忽略的真实集群回归，使用以下脚本启动临时容器，覆盖主题创建 / 删除与明文、PLAIN、SCRAM、双向 TLS 共 8 组配置及错误密码拒绝：

```sh
python3 scripts/test_kafka.py
```

脚本需要 Docker、OpenSSL 和 Cargo，仅使用随机临时目录、回环端口及一次性 `apache/kafka:4.1.1` 容器，退出后清理容器与证书。它不连接工作空间中的 Kafka 地址。

## 代码结构

- `src/main.rs`、`src/desktop.rs`：应用启动、单实例检查、平台提示与窗口布局。
- `src/shortcuts.rs`、`src/ui/shortcuts_view.rs`：平台快捷键配置、冲突校验与设置界面。
- `src/ui.rs`：工作台、集合、环境、请求与响应、设置界面。
- `src/theme.rs`、`src/assets.rs`：主题、JSON 配色与图标资源。
- `src/model.rs`：请求、集合、标签与会话状态操作。
- `src/backend.rs`：串行存储队列和界面事件桥接。
- `src/mqtt.rs`、`src/mqtt/tests.rs`：MQTT V5 会话、发布 / 订阅与本机报文集成检查。
- `src/ui/mqtt_view.rs`：协议入口、MQTT 编辑器与消息面板。
- `src/messaging.rs`、`src/websocket.rs`、`src/kafka.rs`：有界临时会话、WebSocket 帧收发、Kafka 客户端与主题管理。
- `src/ui/messaging_view.rs`：WebSocket 与 Kafka 连接、认证、发布消费和消息详情。
- `src/ui/controls.rs`：标准组件组合、选择器状态、统一表格列和表单开关。
- `src/http.rs`：Rust HTTP 执行、取消、认证、变量与响应缓存。
- `src/storage.rs`、`migrations/`：SQLite、目录迁移和数据兼容。
- `src/postman.rs`、`src/ui/postman_view.rs`：Postman 解析、导入预览与事务保存。
- `build.rs`、`assets/windows/`：Windows 图标、版本和 DPI 资源。
- `scripts/package_macos.py`、`scripts/release.py`：应用打包、版本校验与分发清单。
- `.github/workflows/build.yml`：双平台构建、验收产物与标签发布。

HTTP 与 SQLite 核心复用既有实现；请求运行在 Tokio，SQLite 写入运行在独立串行工作线程，GPUI 主线程负责界面。

Kafka 使用独立工作线程隔离客户端创建、元数据查询和资源释放，WebSocket 使用 Tokio。`rdkafka` 的原生库、OpenSSL 与 zlib 随源码构建并静态链接，首次构建耗时较长，不依赖运行机器预装 Kafka 客户端库。

## 字体与文字层级

界面使用统一的 `TextRole`，由 `src/theme.rs` 定义；100% 缩放下，页面标题 18、分组标题 14、正文 13、控件 12.25、辅助说明 12、状态与计数 11 逻辑像素。仅标题使用中等或半粗字重，正文和说明使用常规字重；说明使用主题次要文字色和 1.6 倍行高，支持自然换行。

普通文字基于 GPUI Kit `Label`，表单帮助使用 `Field.description_fn`，空状态使用 `EmptyTitle` / `EmptyDescription`，风险提示使用 `Alert`。各页面调用统一文字角色，避免直接附加长段字符串继承正文样式。字号采用相对于主题字号的 rem，跟随界面缩放一次；代码编辑器仍使用独立字号设置。

## 品牌图标

客户端图标采用黑色圆角底与白色飞升猫，沿用简洁的黑白配色。桌面图标由 `assets/reqsucc-icon.png` 生成，输出为 macOS 的 `assets/ReqSucc.icns` 和 Windows 的 `assets/ReqSucc.ico`；应用顶部和关于页使用透明背景的 `assets/flying-cat.svg`，颜色随界面主题变化。生成提示词保存在 `assets/icon-prompt.txt`。

重新生成多分辨率应用图标：

```sh
python3 scripts/build_icons.py
```

## 上游兼容补丁

GPUI Kit 固定为 0.9.0，依赖版本由 Cargo.lock 锁定。`vendor/gpui-pre-macos` 基于上游 0.3.11，保留 Apache 许可证。该依赖有三处 `Option::map_or_default` 调用尚不能在本项目 stable 工具链上使用，替换为等价的 `map_or_else(Default::default, ...)`；通过 Cargo patch 引用，不要求 nightly 或 RUSTC_BOOTSTRAP。另有启动时显式加载应用包图标的本地补丁，用于处理开发应用重新打包后 Dock 沿用旧图标的问题。后续更新上游时分别核对这些补丁是否仍有必要。

## 开源协议

Copyright 2026 Andersen。项目采用 [Apache License 2.0](LICENSE)，归属声明见 [NOTICE](NOTICE)。

- 作者：Andersen
- 作者网站：[36knight.com](https://36knight.com)
- 源码地址：[twodogleee/ReqSucc](https://github.com/twodogleee/ReqSucc)

`vendor/` 中的第三方代码保留其原始许可证与署名，具体修改记录见 [gpui-pre-macos 补丁说明](vendor/gpui-pre-macos/PATCHES.md)。
