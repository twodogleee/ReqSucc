# ReqSucc 本地兼容补丁

上游版本：`core-text` 21.0.0，来自 [servo/core-foundation-rs](https://github.com/servo/core-foundation-rs)，快照提交 `b10f1efc48343fc5590127ec6d890ffbb8b5bd02`。保留上游 MIT 与 Apache-2.0 许可证及署名；通过根目录 `Cargo.toml` 的 `[patch.crates-io]` 使用。

## macOS 13.0 字体 trait 类型兼容

- 文件：`src/font.rs`。
- macOS 13.0 的 CoreText 对部分字体的零值 trait 返回 `kCFBooleanFalse`，而 GPUI 和 font-kit 期望 `CFNumber`，会拒绝所有候选字体或在读取时触发 panic。Chromium 已记录该系统问题：[crbug.com/1372420](https://crbug.com/1372420)。
- `CTFont::all_traits` 的兼容处理仅检查四个数值 trait；其中值为 `kCFBooleanFalse` 时，在独立副本中替换为数值零。正常数值、布尔 true、其他键和缺失字段保持不变，不修改 CoreText 原始字典。
- 补丁覆盖 GPUI 的候选字体校验、font-kit 的字体属性匹配及 OpenType 回退配置；后续升级上游或移除 macOS 13.0 支持时重新评估。
