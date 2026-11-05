//! 嵌入 Windows 应用图标与版本；执行清单由 GPUI 提供，其他平台不生成资源。
//!
//! 作者：Andersen

/// 根据 Cargo 版本生成资源定义；Windows 资源编译失败时阻止产生不完整应用。
fn main() {
    println!("cargo:rerun-if-changed=assets/windows/app.rc");
    println!("cargo:rerun-if-changed=assets/ReqSucc.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let version = std::env::var("CARGO_PKG_VERSION").expect("Cargo 必须提供应用版本");
    let major = std::env::var("CARGO_PKG_VERSION_MAJOR").unwrap();
    let minor = std::env::var("CARGO_PKG_VERSION_MINOR").unwrap();
    let patch = std::env::var("CARGO_PKG_VERSION_PATCH").unwrap();
    let definitions = [
        format!("APP_VERSION=\"{version}\""),
        format!("APP_VERSION_NUM={major},{minor},{patch},0"),
    ];
    // 当前 gpui-pre 后端已嵌入含 PerMonitorV2 的清单，应用资源不得重复声明 RT_MANIFEST。
    embed_resource::compile("assets/windows/app.rc", definitions)
        .manifest_required()
        .expect("Windows 应用资源编译失败");
}
