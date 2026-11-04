# ReqSucc 本地兼容补丁

上游版本：`gpui-pre-macos` 0.3.11，来自 Zed 的 `gpui_macos` crate，快照提交为 `f816e1f9263e3a8ef63c4baaa02e1441ce1c6038`。

上游来源：[Zed](https://github.com/zed-industries/zed)。本目录保存上游源码副本及以下本地补丁，通过根目录 `Cargo.toml` 的 `[patch.crates-io]` 引用。

许可证：Apache-2.0，保留原始 `LICENSE-APACHE` 及上游署名。本地修改维护者为 Andersen，不将整个上游依赖标记为项目原创。

## 1. stable Rust 兼容

- 文件：`src/window.rs`。
- 首次修改：2026-11-03。
- 将三处 `Option::map_or_default` 替换为等价的 `map_or_else(Default::default, ...)`，使其兼容本项目的 stable Rust 工具链。
- 此补丁仅调整不稳定 API 的写法，不改变窗口尺寸的回退语义。

## 2. 开发应用 Dock 图标刷新

- 文件：`src/platform.rs` 的 `did_finish_launching` 回调。
- 相同包标识的开发应用原地重打包后，Launch Services 可能继续使用旧图标；本地补丁在启动时读取当前应用包的 `CFBundleIconFile`，加载图像并显式设置应用图标。
- 图标配置、资源路径或图像不可用时跳过刷新，不影响原有启动流程；设置后释放本地持有的图像引用。
- 本项说明记录工作区已有的功能补丁，本次注释整理未新增图标刷新行为。

## 维护边界

保留上游原始代码与注释，避免为了格式或语言统一扩大补丁差异。本项目新增或修改的补丁须遵循根目录 `AGENTS.md` 的中文注释要求，说明修改原因、行为边界及必要的资源所有权；后续更新上游时逐项核对补丁是否仍有必要。
