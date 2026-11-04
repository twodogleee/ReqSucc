//! ReqSucc 原生应用入口，负责单实例锁、运行时、菜单快捷键与主窗口生命周期。
//!
//! 作者：Andersen

/// 品牌与组件图标的统一资源入口。
mod assets;
/// 存储命令队列和界面事件桥接。
mod backend;
/// HTTP 请求执行、认证与响应缓存。
mod http;
/// Kafka 认证、发布消费与主题管理。
mod kafka;
/// WebSocket 与 Kafka 的临时会话和有界消息记录。
mod messaging;
/// 请求、标签、集合和会话状态操作。
mod model;
/// MQTT V5 连接、发布、订阅与临时消息记录。
mod mqtt;
/// Postman 本地集合和环境导入转换。
mod postman;
/// 应用快捷键配置、冲突校验与动态注册。
mod shortcuts;
/// SQLite 持久化、数据目录切换与迁移。
mod storage;
/// 全局主题、语义配色和文字层级。
mod theme;
/// 原生工作台界面与交互编排。
mod ui;
/// WebSocket 握手、自定义请求头和双向消息。
mod websocket;
use gpui_kit::{
    component::{self},
    *,
};

/// 获取单实例锁后启动网络运行时和原生工作台，注册中文菜单与快捷键。
///
/// # Panics
/// 无法创建配置目录、运行锁、网络运行时或主窗口时终止启动。
fn main() {
    use fs2::FileExt;
    let home = backend::data_home();
    std::fs::create_dir_all(&home).expect("无法创建应用配置目录");
    let instance = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(home.join(".desktop-instance.lock"))
        .expect("无法创建应用运行锁");
    if instance.try_lock_exclusive().is_err() {
        eprintln!("ReqSucc 已在运行，请切换到已打开的窗口。");
        return;
    }

    // Tokio 驱动网络 I/O，GPUI 驱动原生事件循环；网络等待不占用界面线程。
    let runtime = std::sync::Arc::new(tokio::runtime::Runtime::new().expect("无法启动网络运行时"));
    gpui_kit::application()
        .with_assets(assets::AppAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            component::set_locale("zh-CN");
            theme::apply(false, &serde_json::json!({}), cx);

            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::centered(size(px(1440.), px(940.)), cx)),
                    window_min_size: Some(size(px(1100.), px(720.))),
                    ..component::TitleBar::window_options()
                },
                cx,
                move |window, cx| {
                    window.set_window_title("ReqSucc · 接口工作台");
                    let view = cx.new(|cx| ui::Workbench::new(runtime, window, cx));
                    ui::Workbench::register(&view, window.window_handle(), cx);
                    view
                },
            )
            .expect("无法打开 ReqSucc 窗口");
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
        });
}
