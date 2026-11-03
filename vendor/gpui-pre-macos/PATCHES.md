# Local compatibility patch

Upstream: `gpui-pre-macos` 0.3.11, a snapshot of the Zed `gpui_macos` crate at commit `f816e1f9263e3a8ef63c4baaa02e1441ce1c6038`.

Source: https://github.com/zed-industries/zed

License: Apache-2.0; the original `LICENSE-APACHE` is preserved.

Modified by Andersen for ReqSucc on 2026-11-03: three calls to `Option::map_or_default` in `src/window.rs` are replaced with equivalent `map_or_else(Default::default, ...)` calls so the crate builds on the project's stable Rust toolchain. No other behavioral changes are intended. The project uses this copy through `[patch.crates-io]`.
