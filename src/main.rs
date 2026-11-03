mod assets;
mod backend;
mod http;
mod model;
mod storage;
mod theme;
mod ui;
use gpui_kit::{
    component::{self},
    *,
};

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

    // Tokio owns network I/O; GPUI owns the native event loop and UI thread.
    let runtime = std::sync::Arc::new(tokio::runtime::Runtime::new().expect("无法启动网络运行时"));
    gpui_kit::application()
        .with_assets(assets::AppAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            component::set_locale("zh-CN");
            theme::apply(false, &serde_json::json!({}), cx);

            cx.bind_keys([
                KeyBinding::new(
                    if cfg!(target_os = "macos") {
                        "cmd-k"
                    } else {
                        "ctrl-k"
                    },
                    ui::SearchRequests,
                    None,
                ),
                KeyBinding::new(
                    if cfg!(target_os = "macos") {
                        "cmd-n"
                    } else {
                        "ctrl-n"
                    },
                    ui::NewRequest,
                    None,
                ),
                KeyBinding::new(
                    if cfg!(target_os = "macos") {
                        "cmd-s"
                    } else {
                        "ctrl-s"
                    },
                    ui::SaveRequest,
                    None,
                ),
                KeyBinding::new(
                    if cfg!(target_os = "macos") {
                        "cmd-enter"
                    } else {
                        "ctrl-enter"
                    },
                    ui::SendRequest,
                    None,
                ),
                KeyBinding::new(
                    if cfg!(target_os = "macos") {
                        "cmd-w"
                    } else {
                        "ctrl-w"
                    },
                    ui::CloseTab,
                    None,
                ),
                KeyBinding::new(
                    if cfg!(target_os = "macos") {
                        "cmd-q"
                    } else {
                        "ctrl-q"
                    },
                    ui::Quit,
                    None,
                ),
                KeyBinding::new(
                    if cfg!(target_os = "macos") {
                        "cmd-,"
                    } else {
                        "ctrl-,"
                    },
                    ui::Settings,
                    None,
                ),
            ]);
            use gpui_kit::component::input::{Copy, Cut, Paste, Redo, SelectAll, Undo};
            cx.set_menus(vec![
                Menu {
                    disabled: false,
                    name: "ReqSucc".into(),
                    items: vec![
                        MenuItem::action("设置…", ui::Settings),
                        MenuItem::separator(),
                        MenuItem::action("退出 ReqSucc", ui::Quit),
                    ],
                },
                Menu {
                    disabled: false,
                    name: "文件".into(),
                    items: vec![
                        MenuItem::action("新建请求", ui::NewRequest),
                        MenuItem::action("保存接口", ui::SaveRequest),
                        MenuItem::action("发送请求", ui::SendRequest),
                        MenuItem::action("关闭标签", ui::CloseTab),
                    ],
                },
                Menu {
                    disabled: false,
                    name: "编辑".into(),
                    items: vec![
                        MenuItem::action("撤销", Undo),
                        MenuItem::action("重做", Redo),
                        MenuItem::separator(),
                        MenuItem::action("剪切", Cut),
                        MenuItem::action("复制", Copy),
                        MenuItem::action("粘贴", Paste),
                        MenuItem::action("全选", SelectAll),
                    ],
                },
            ]);
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
