//! GPUI 原生工作台，编排请求编辑、后台事件、草稿保存、集合环境管理和设置界面。
//!
//! 作者：Andersen

/// 基于 GPUI Kit 的统一文字、表格、选择器和表单控件组合。
mod controls;
use crate::theme::{TextRole, TypographyExt};
use controls::*;
/// MQTT 协议切换、连接配置、发布用例和消息详情界面。
mod mqtt_view;
use crate::mqtt;
use crate::{
    backend::{self, Backend, Event},
    http,
    model::{self, text},
    storage::{Snapshot, StoragePlan},
};
use gpui_kit::assets::IconName as Glyph;
use gpui_kit::component::{
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    input::{Editor, EditorState, Input, InputEvent, InputState},
    menu::{DropdownMenu, PopupMenu, PopupMenuItem},
    resizable::{h_resizable, resizable_panel, v_resizable},
    *,
};
use gpui_kit::{prelude::FluentBuilder, *};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

/// MQTT JSON 模板格式化失败时的固定提示，也用于成功后识别并清除该错误。
const MQTT_FORMAT_ERROR: &str = "MQTT 消息 JSON 格式错误，或变量占位符位置无效。原文未修改。";

/// 请求标签拖动时携带的身份与浮动预览内容。
#[derive(Clone)]
struct DraggedRequestTab {
    /// 被拖动的请求标签标识，用于重排而非复制标签。
    id: String,
    /// 拖动浮层中展示的请求名称。
    label: String,
}

impl Render for DraggedRequestTab {
    /// 绘制跟随拖动的标签名称浮层，使用当前主题的边框与背景。
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_3()
            .py_1()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .opacity(0.9)
            .child(self.label.clone())
    }
}

actions!(
    reqsucc,
    [
        SearchRequests,
        NewRequest,
        SaveRequest,
        SendRequest,
        CloseTab,
        Quit,
        Settings
    ]
);
/// 输入控件与模型字段的绑定位置，区分活动请求、固定发布用例和会话配置。
#[derive(Clone)]
enum Binding {
    /// 活动请求内的 JSON Pointer 路径；缺失的请求设置可回退到全局默认值。
    Request(String),
    /// 使用标签和用例稳定标识定位字段，防止切换用例后旧控件写入其他消息。
    MqttCase {
        /// 发布用例所属的请求标签标识。
        tab: String,
        /// 请求内的发布用例稳定标识。
        case: String,
        /// 发布用例对象中的直接字段名。
        field: String,
    },
    /// 会话对象内的 JSON Pointer 路径，用于环境、变量和显示偏好。
    Session(String),
    /// 按目录标识绑定名称编辑，不把目录名当作身份。
    Folder(String),
    /// 绑定界面内存中的接口搜索文本，不触发草稿保存。
    Search,
}
impl Binding {
    /// 生成控件缓存键；发布用例绑定包含标签和用例标识以隔离编辑状态。
    fn key(&self) -> String {
        match self {
            Self::Request(p) => format!("r{p}"),
            Self::MqttCase { tab, case, field } => format!("mqtt-case:{tab}:{case}:{field}"),
            Self::Session(p) => format!("s{p}"),
            Self::Folder(p) => format!("f{p}"),
            Self::Search => "search".into(),
        }
    }
}
/// 工作台统一交互命令，由入口集中检查状态并分派模型和后台操作。
#[derive(Clone)]
enum Action {
    /// 切回请求页、展开侧栏并将焦点移到搜索框。
    SearchFocus,
    /// 切换侧栏展开状态并保存显示偏好。
    ToggleSidebar,
    /// 切换指定目录标识的折叠状态。
    Expand(String),
    /// 在当前目录创建空白请求标签。
    New,
    /// 将活动草稿更新为已保存接口，并触发后台持久化。
    Save,
    /// 请求关闭指定标签，必要时先确认丢弃未保存修改。
    Close(String),
    /// 确认关闭指定标签，并记住本次运行已接受未保存标签的丢弃提示。
    ConfirmClose(String),
    /// 激活指定请求标签，重建编辑控件。
    Select(String),
    /// 打开传入的请求 JSON，已打开时保留其现有草稿。
    Open(Value),
    /// 切换活动请求的编辑分区，参数为分区标识。
    Panel(String),
    /// 切换工作台主页面，参数为页面标识。
    Screen(String),
    /// 切换当前协议的运行状态：发送或取消 HTTP，连接或断开 MQTT。
    Send,
    /// 切换活动请求协议，参数仅允许 http 或 mqtt。
    Protocol(String),
    /// 提交当前选中的 MQTT 发布用例。
    MqttPublish,
    /// 调整订阅行，参数依次为下标和启用标记；false 表示取消订阅。
    MqttTopic(usize, bool),
    /// 连接未活动时增加一条空白 MQTT 订阅行。
    MqttAddTopic,
    /// 清空当前连接的内存消息记录和详情选择。
    MqttClear,
    /// 按记录标识选择当前连接的消息详情。
    MqttRecord(u64),
    /// 复制当前 MQTT 记录的原始详情，不采用展示用 JSON 格式化结果。
    MqttCopy,
    /// 按用例标识切换当前请求的消息编辑目标。
    MqttCaseSelect(String),
    /// 创建发布用例；true 表示复制当前用例，false 表示新建空白用例。
    MqttCaseNew(bool),
    /// 打开指定发布用例的重命名对话框。
    MqttCaseRenameDialog(String),
    /// 确认删除指定发布用例，至少保留一条。
    MqttCaseConfirmDelete(String),
    /// 执行用例删除，参数依次为确认时的标签标识和用例标识，防止误删切换后的请求。
    MqttCaseDelete(String, String),
    /// 将 JSON 值写入指定绑定，参数依次为绑定位置和值。
    Set(Binding, Value),
    /// 向绑定的键值列表追加默认空白行。
    AddRow(Binding),
    /// 删除绑定列表中的指定下标，参数依次为绑定位置和下标。
    RemoveRow(Binding, usize),
    /// 校验并格式化当前 HTTP JSON 请求体，失败保留原文。
    Format,
    /// 格式化当前 MQTT JSON 消息模板，保留变量占位符和原始值。
    MqttFormat,
    /// 切换 HTTP 响应分区，参数为正文、响应头、Cookie 或实际请求的分区标识。
    Response(String),
    /// 切换响应正文的原始文本和 JSON 格式化预览。
    Raw,
    /// 复制当前 HTTP 响应分区的预览文本。
    Copy,
    /// 选择目标路径并异步导出已缓存的响应字节。
    Export,
    /// 创建目录；参数为父目录标识，None 表示创建根集合。
    AddFolder(Option<String>),
    /// 选中指定目录并打开目录配置页面。
    SelectFolder(String),
    /// 执行指定目录及后代接口删除，并释放关联运行资源。
    DeleteFolder(String),
    /// 展示指定目录及其子内容的删除确认。
    ConfirmDeleteFolder(String),
    /// 复制指定已保存请求为新的未保存标签。
    DuplicateRequest(String),
    /// 删除指定接口及草稿，关闭标签并释放运行资源。
    DeleteRequest(String),
    /// 展示指定接口及草稿的删除确认。
    ConfirmDeleteRequest(String),
    /// 移动接口，参数依次为接口标识和目标目录标识，同时更新打开的草稿归属。
    MoveRequest(String, String),
    /// 移动目录，参数依次为目录标识和新父标识，None 表示提升为根集合。
    MoveFolder(String, Option<String>),
    /// 调整同级目录顺序，参数依次为目录标识和是否上移。
    OrderFolder(String, bool),
    /// 调整同目录接口顺序，参数依次为接口标识和是否上移。
    OrderRequest(String, bool),
    /// 创建默认命名的新环境并打开其变量编辑，不改变当前执行环境。
    NewEnv,
    /// 将指定环境标识设为当前执行环境。
    SelectEnv(String),
    /// 复制指定环境及变量并生成新的环境标识。
    DuplicateEnv(String),
    /// 删除指定环境；若它是当前执行环境则清空选择，并将变量编辑切回全局作用域。
    DeleteEnv(String),
    /// 切换变量编辑作用域，参数为作用域标识。
    Scope(String),
    /// 切换深浅主题；true 表示浅色，false 表示深色。
    Theme(bool),
    /// 切换设置分类，参数为分类标识。
    SettingsPage(String),
    /// 恢复默认主题、缩放、字号和编辑器显示偏好。
    ResetDisplay,
    /// 按 create、open 或 migrate 模式选择目录并提交预检。
    ChooseStorage(String),
    /// 确认目录计划，成功刷盘且无活动网络任务后执行切换。
    SwitchStorage,
    /// 放弃当前目录操作计划，不变更活动工作空间。
    CancelPlan,
    /// 重试加载工作空间，或重新触发当前草稿保存。
    Retry,
    /// 刷盘成功后取消网络任务并退出应用，失败时保留窗口。
    Quit,
}
/// 某个请求标签最近一次 HTTP 执行的界面状态。
struct RunState {
    /// HTTP 运行标识，用于拒绝旧回执及关联响应缓存。
    id: String,
    /// 是否仍在等待执行结果，决定发送按钮的取消行为。
    pending: bool,
    /// 本次请求开始时刻，用于展示等待耗时。
    started: Instant,
    /// 尚未完成时为空；完成后保存服务器响应或本地执行失败。
    result: Option<Result<http::Output, http::Failure>>,
}
/// 主窗口的界面状态与交互编排入口，模型快照和运行资源在此关联。
pub struct Workbench {
    /// 当前工作空间内存快照，包含已保存接口、草稿和显示偏好。
    doc: Snapshot,
    /// 是否已经成功加载工作空间，决定是否允许编辑和保存。
    ready: bool,
    /// 当前数据目录的显示路径。
    path: String,
    /// 串行存储命令入口，避免界面直接写数据库。
    backend: Backend,
    /// 界面轮询后台加载、保存、请求和导出结果的接收端。
    rx: mpsc::Receiver<Event>,
    /// 供网络与导出任务回传界面事件的发送端。
    tx: mpsc::Sender<Event>,
    /// 共享 Tokio 运行时，负责网络及导出等后台任务。
    runtime: Arc<tokio::runtime::Runtime>,
    /// HTTP 运行与响应缓存服务，跨标签共用并发限制。
    http: Arc<http::HttpService>,
    /// 每次可持久化编辑递增的界面代数，用于判断待保存内容。
    generation: u64,
    /// 已收到成功保存回执的最高界面代数，不等同于数据库修订号。
    saved: u64,
    /// 最近一次已提交保存队列的界面代数，避免防抖轮询重复入队。
    queued: u64,
    /// 最后一次可持久化编辑的时刻，用于 400 毫秒自动保存防抖。
    changed: Instant,
    /// 状态栏中的操作进度或成功提示。
    status: String,
    /// 当前需要向用户展示的错误文本，空字符串表示无错误。
    error: String,
    /// 是否处于阻止普通交互的后台目录操作阶段。
    busy: bool,
    /// 本次应用运行是否已确认丢弃未保存标签，后续关闭据此省略重复提示。
    confirmed_unsaved_tab_close: bool,
    /// 绑定键到单行输入状态的缓存，避免渲染时丢失编辑状态。
    fields: HashMap<String, Entity<InputState>>,
    /// 控件标识到选择器状态及订阅的缓存。
    selects: HashMap<String, SelectSlot>,
    /// 输入和编辑器事件订阅的所有权集合，重建字段时统一释放。
    subscriptions: Vec<Subscription>,
    /// 当前请求正文或 MQTT 消息使用的编辑器状态。
    body: Option<Entity<EditorState>>,
    /// 正文编辑器对应的绑定和格式标识，变化时重建编辑器。
    body_key: String,
    /// 当前 HTTP 响应或 MQTT 详情的只读编辑器状态。
    response_editor: Option<Entity<EditorState>>,
    /// 响应编辑器内容身份，用于分区或结果变化后的重建。
    response_key: String,
    /// 请求标签标识到最近 HTTP 运行状态的映射。
    runs: HashMap<String, RunState>,
    /// 请求标签标识到临时 MQTT 会话的映射，移除会话会取消连接。
    mqtt_sessions: HashMap<String, mqtt::Session>,
    /// 各 MQTT 会话上次观察到的修订号，用于按变化刷新界面。
    mqtt_revisions: HashMap<String, u64>,
    /// 各请求标签选中的 MQTT 记录标识，记录失效时回退到最新项。
    mqtt_selected: HashMap<String, u64>,
    /// 当前主页面标识，如 requests、folder、environments 或 settings。
    screen: String,
    /// 当前设置分类标识，默认显示设置。
    settings_page: String,
    /// 当前选中目录标识，作为新请求与保存位置的默认值。
    folder: String,
    /// 环境页面正在编辑的变量作用域标识。
    scope: String,
    /// 当前 HTTP 响应分区标识。
    response_tab: String,
    /// 是否直接展示响应正文原文，关闭时尝试格式化完整 JSON 预览。
    raw: bool,
    /// 侧栏接口名称和地址的搜索文本，仅保留在当前界面。
    search: String,
    /// 待确认的目录操作模式及预检结果，取消或执行后清空。
    plan: Option<(String, StoragePlan)>,
    /// 工作台焦点句柄，用于键盘事件上下文。
    focus: FocusHandle,
    /// 请求标签条的横向滚动状态。
    tab_scroll: ScrollHandle,
    /// 最近已自动滚入视野的活动标签标识，避免每次渲染都重置滚动。
    visible_tab: String,
}
impl Workbench {
    /// 读取并限制界面缩放为 0.9–1.25 倍，配置缺失时使用 1 倍。
    fn scale(&self) -> f32 {
        self.doc.session["native"]["scale"]
            .as_f64()
            .unwrap_or(100.)
            .clamp(90., 125.) as f32
            / 100.
    }
    /// 将基准逻辑像素按当前界面缩放转换为布局尺寸。
    fn scaled(&self, value: f32) -> Pixels {
        px(value * self.scale())
    }
    /// 读取显示开关，缺失或类型不正确时默认为开启。
    fn display_flag(&self, key: &str) -> bool {
        self.doc.session["native"][key].as_bool().unwrap_or(true)
    }

    /// 将原生菜单及快捷键动作绑定到目标窗口，通过弱引用避免延长工作台生命周期。
    pub fn register(view: &Entity<Self>, target: AnyWindowHandle, cx: &mut App) {
        // 各原生动作统一延迟到目标窗口上下文执行，弱引用失效时忽略回调。
        macro_rules! bind {
            ($ty:ty,$action:expr) => {{
                let view = view.downgrade();
                cx.on_action(move |_: &$ty, cx| {
                    let view = view.clone();
                    cx.defer(move |cx| {
                        let _ = target.update(cx, |_, window, cx| {
                            let _ = view.update(cx, |this, cx| {
                                let action: Action = $action(this);
                                this.act(action, window, cx);
                            });
                        });
                    });
                });
            }};
        }
        bind!(SearchRequests, |_: &mut Self| Action::SearchFocus);
        bind!(NewRequest, |_: &mut Self| Action::New);
        bind!(SaveRequest, |_: &mut Self| Action::Save);
        bind!(SendRequest, |_: &mut Self| Action::Send);
        bind!(CloseTab, |this: &mut Self| Action::Close(text(
            &this.doc.session,
            "activeId"
        )));
        bind!(Settings, |_: &mut Self| Action::Screen("settings".into()));
        bind!(Quit, |_: &mut Self| Action::Quit);
    }
    /// 构造统一尺寸、提示和无障碍标签的图标按钮，并按动作检查 MQTT 编辑锁。
    fn icon_button(
        &self,
        id: impl Into<ElementId>,
        label: &str,
        icon: impl Into<Icon>,
        action: Action,
        cx: &Context<Self>,
    ) -> Button {
        let destructive = matches!(&action, Action::RemoveRow(_, _));
        Button::new(id)
            .icon(
                icon.into()
                    .size(self.scaled(crate::theme::ICON_SIZE))
                    .when(destructive, |icon| icon.text_color(cx.theme().danger)),
            )
            .accessibility_label(label.to_owned())
            .tooltip(label.to_owned())
            .ghost()
            .small()
            .w(self.scaled(28.))
            .h(self.scaled(crate::theme::CONTROL_HEIGHT))
            .p_0()
            .disabled(self.locked_action(&action))
            .on_click(cx.listener(move |this, _, w, cx| this.act(action.clone(), w, cx)))
    }
    /// 按深浅主题返回 HTTP 方法或 MQTT 协议标记颜色。
    fn method_color(&self, method: &str) -> Hsla {
        if self.doc.session["native"]["light"]
            .as_bool()
            .unwrap_or(false)
        {
            return rgb(match method {
                "GET" => 0x18734a,
                "POST" => 0x805c1b,
                "PUT" => 0x315cb4,
                "PATCH" | "MQTT" => 0x724ca0,
                "DELETE" => 0xb13640,
                _ => 0x526274,
            })
            .into();
        }
        rgb(match method {
            "GET" => 0x79d3ad,
            "POST" => 0xe2bd73,
            "PUT" => 0x8aacf2,
            "PATCH" | "MQTT" => 0xb99ae9,
            "DELETE" => 0xee9296,
            _ => 0xaeb5bc,
        })
        .into()
    }

    /// 创建工作台、排队加载数据并启动事件轮询；注册关闭刷盘和退出清理回调。
    pub fn new(
        runtime: Arc<tokio::runtime::Runtime>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let home = backend::data_home();
        let backend = Backend::new(home.clone(), home.join("data"), tx.clone());
        let mut doc = Snapshot {
            workspace_id: String::new(),
            revision: 0,
            folders: vec![],
            requests: vec![],
            session: json!({}),
        };
        model::normalize(&mut doc);
        let task = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                if this.update(cx, |this, cx| this.tick(cx)).is_err() {
                    break;
                }
            }
        });
        task.detach();
        cx.on_app_quit(|this, _| {
            if this.ready {
                let _ = this.backend.flush(this.doc.clone());
            }
            this.http.clear();
            this.mqtt_sessions.clear();
            async {}
        })
        .detach();
        let entity = cx.entity().downgrade();
        window.on_window_should_close(cx, move |_, cx| {
            entity.update(cx, |this, cx| this.flush(cx)).unwrap_or(true)
        });
        Self {
            doc,
            ready: false,
            path: String::new(),
            backend,
            rx,
            tx,
            runtime,
            http: Arc::new(http::HttpService::default()),
            generation: 0,
            saved: 0,
            queued: 0,
            changed: Instant::now(),
            status: "正在打开本地数据…".into(),
            error: String::new(),
            busy: false,
            confirmed_unsaved_tab_close: false,
            fields: HashMap::new(),
            selects: HashMap::new(),
            subscriptions: vec![],
            body: None,
            body_key: String::new(),
            response_editor: None,
            response_key: String::new(),
            runs: HashMap::new(),
            mqtt_sessions: HashMap::new(),
            mqtt_revisions: HashMap::new(),
            mqtt_selected: HashMap::new(),
            settings_page: "display".into(),
            screen: "requests".into(),
            folder: String::new(),
            scope: "globals".into(),
            response_tab: "body".into(),
            raw: false,
            search: String::new(),
            plan: None,
            tab_scroll: ScrollHandle::new(),
            visible_tab: String::new(),
            focus: {
                let focus = cx.focus_handle();
                focus.focus(window, cx);
                focus
            },
        }
    }
    /// 同步保存最新草稿并更新修订号；失败展示错误且返回 false，供关闭或切换流程中止。
    fn flush(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.ready {
            return true;
        }
        match self.backend.flush(self.doc.clone()) {
            Ok(r) => {
                self.doc.revision = r;
                self.saved = self.generation;
                true
            }
            Err(e) => {
                self.error = e;
                cx.notify();
                false
            }
        }
    }
    /// 清除输入、选择器和相关订阅，失效正文与响应缓存键，供切换上下文后重建。
    fn reset_fields(&mut self) {
        self.fields.clear();
        self.selects.clear();
        self.subscriptions.clear();
        self.body = None;
        self.body_key.clear();
        self.response_key.clear();
    }
    /// 递增界面编辑代数并记录防抖起点，提示正在保存并请求重绘。
    fn changed(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        self.changed = Instant::now();
        self.status = "正在保存草稿…".into();
        cx.notify();
    }
    /// 处理后台回执和自动保存防抖，按运行标识丢弃旧响应，并观察 MQTT 修订号触发刷新。
    fn tick(&mut self, cx: &mut Context<Self>) {
        let mut notify = false;
        while let Ok(event) = self.rx.try_recv() {
            notify = true;
            match event {
                Event::Loaded(result) => {
                    self.busy = false;
                    match result {
                        Ok(loaded) => {
                            self.doc = loaded.snapshot;
                            model::normalize(&mut self.doc);
                            for tab in self.doc.session["tabs"].as_array_mut().unwrap() {
                                let r = &mut tab["request"];
                                if !r["settings"].is_object() {
                                    r["settings"] = json!({});
                                }
                                if !r["form"].is_array() {
                                    r["form"] = json!([]);
                                }
                            }
                            self.path = loaded.path;
                            self.ready = true;
                            self.generation = 0;
                            self.saved = 0;
                            self.queued = 0;
                            self.folder = self
                                .doc
                                .folders
                                .first()
                                .map(|f| f.id.clone())
                                .unwrap_or_default();
                            self.error.clear();
                            self.status = "草稿已保存".into();
                            self.plan = None;
                            self.http.clear();
                            self.runs.clear();
                            self.mqtt_sessions.clear();
                            self.mqtt_selected.clear();
                            self.mqtt_revisions.clear();
                            self.reset_fields();
                            let light = self.doc.session["native"]["light"]
                                .as_bool()
                                .unwrap_or(false);
                            crate::theme::apply(light, &self.doc.session["native"], cx);
                        }
                        Err(e) => {
                            self.error = e;
                            self.status = "数据目录未能打开".into();
                            self.screen = "settings".into();
                        }
                    }
                }
                Event::Saved(generation, result) => match result {
                    Ok(rev) => {
                        self.doc.revision = rev;
                        self.saved = self.saved.max(generation);
                        if self.saved == self.generation {
                            self.status = "草稿已保存".into();
                        }
                    }
                    Err(e) => {
                        self.error = e;
                        self.status = "保存失败 · 请重试".into();
                    }
                },
                Event::Planned(mode, result) => {
                    self.busy = false;
                    match result {
                        Ok(plan) => self.plan = Some((mode, plan)),
                        Err(e) => self.error = e,
                    }
                }
                Event::Response(tab, id, result) => {
                    if let Some(run) = self.runs.get_mut(&tab) {
                        if run.id == id {
                            run.pending = false;
                            run.result = Some(result);
                            self.status = if self.saved == self.generation {
                                "草稿已保存".into()
                            } else {
                                "正在保存草稿…".into()
                            };
                            self.response_key.clear();
                        } else {
                            self.http.release(&id)
                        }
                    } else {
                        self.http.release(&id)
                    }
                }
                Event::Export(result) => match result {
                    Ok(()) => self.status = "响应文件已保存".into(),
                    Err(e) => self.error = e,
                },
            }
        }
        if self.ready
            && !self.busy
            && self.generation > self.queued
            && self.changed.elapsed() > Duration::from_millis(400)
        {
            self.queued = self.generation;
            self.backend.save(self.generation, self.doc.clone());
        }
        for (id, session) in &self.mqtt_sessions {
            let rev = session.live.lock().unwrap().revision;
            if self.mqtt_revisions.insert(id.clone(), rev) != Some(rev) {
                notify = true;
            }
        }
        if notify {
            cx.notify();
        }
    }
    /// 读取绑定位置的 JSON 值；请求级设置缺失时继承全局默认，其余缺失返回 Null。
    fn read(&self, b: &Binding) -> Value {
        match b {
            Binding::Request(p) => model::active(&self.doc)
                .and_then(|r| r.pointer(p))
                .cloned()
                .unwrap_or_else(|| {
                    if p.starts_with("/settings/") {
                        self.doc.session["network"]["defaults"][p.trim_start_matches("/settings/")]
                            .clone()
                    } else {
                        Value::Null
                    }
                }),
            Binding::MqttCase { tab, case, field } => model::tabs(&self.doc)
                .iter()
                .find(|t| text(t, "id") == *tab)
                .and_then(|t| t["request"]["mqtt"]["publishCases"].as_array())
                .and_then(|cases| cases.iter().find(|v| text(v, "id") == *case))
                .and_then(|v| v.get(field))
                .cloned()
                .unwrap_or(Value::Null),
            Binding::Session(p) => self.doc.session.pointer(p).cloned().unwrap_or(Value::Null),
            Binding::Folder(id) => self
                .doc
                .folders
                .iter()
                .find(|f| &f.id == id)
                .map(|f| json!(f.name))
                .unwrap_or(Value::Null),
            Binding::Search => json!(self.search),
        }
    }
    /// 判断活动 MQTT 会话是否锁定连接配置；消息发布字段仍可编辑。
    fn mqtt_binding_locked(&self, b: &Binding) -> bool {
        matches!(b, Binding::Request(p) if p.starts_with("/mqtt/") && !["/mqtt/topic", "/mqtt/payload", "/mqtt/format", "/mqtt/qos", "/mqtt/retain", "/mqtt/contentType", "/mqtt/responseTopic", "/mqtt/expiry"].contains(&p.as_str()))
            && self
                .mqtt_sessions
                .get(&text(&self.doc.session, "activeId"))
                .is_some_and(|s| s.active())
    }
    /// 检查字段写入和列表增删动作是否受到 MQTT 连接期间的编辑锁限制。
    fn locked_action(&self, action: &Action) -> bool {
        match action {
            Action::Set(b, _) | Action::AddRow(b) | Action::RemoveRow(b, _) => {
                self.mqtt_binding_locked(b)
            }
            _ => false,
        }
    }
    /// 写入绑定字段并同步查询参数、兼容字段或主题；锁定连接配置时拒绝修改，搜索只更新界面。
    fn write(&mut self, b: &Binding, v: Value, cx: &mut Context<Self>) {
        if let Binding::Search = b {
            self.search = v.as_str().unwrap_or("").into();
            cx.notify();
            return;
        }
        if !self.ready || self.busy {
            return;
        }
        let assign = |root: &mut Value, p: &str, v: Value| {
            if let Some((parent, key)) = p.rsplit_once('/') {
                if let Some(obj) = root.pointer_mut(parent).and_then(Value::as_object_mut) {
                    obj.insert(key.into(), v);
                } else if let Some(slot) = root.pointer_mut(p) {
                    *slot = v;
                }
            }
        };
        match b {
            Binding::Request(p) => {
                let active = text(&self.doc.session, "activeId");
                if p.starts_with("/mqtt/")
                    && ![
                        "/mqtt/topic",
                        "/mqtt/payload",
                        "/mqtt/format",
                        "/mqtt/qos",
                        "/mqtt/retain",
                        "/mqtt/contentType",
                        "/mqtt/responseTopic",
                        "/mqtt/expiry",
                    ]
                    .contains(&p.as_str())
                    && self.mqtt_sessions.get(&active).is_some_and(|s| s.active())
                {
                    self.error = "请先断开 MQTT 连接，再修改连接配置或主题列表".into();
                    cx.notify();
                    return;
                }
                if let Some(r) = model::active_mut(&mut self.doc) {
                    assign(r, p, v);
                    if p == "/url" {
                        model::url_changed(r)
                    } else if p == "/params" || p.starts_with("/params/") {
                        model::params_changed(r)
                    }
                }
            }
            Binding::MqttCase { tab, case, field } => {
                let Some(tab) = self.doc.session["tabs"]
                    .as_array_mut()
                    .and_then(|tabs| tabs.iter_mut().find(|t| text(t, "id") == *tab))
                else {
                    return;
                };
                let Some(cases) = tab["request"]["mqtt"]["publishCases"].as_array_mut() else {
                    return;
                };
                let Some(target) = cases.iter_mut().find(|v| text(v, "id") == *case) else {
                    return;
                };
                target[field] = v;
                model::mqtt_sync_legacy_fields(&mut tab["request"]);
            }
            Binding::Session(p) => assign(&mut self.doc.session, p, v),
            Binding::Folder(id) => {
                if let Some(f) = self.doc.folders.iter_mut().find(|f| &f.id == id) {
                    f.name = v.as_str().unwrap_or("").into()
                }
            }
            Binding::Search => {}
        }
        if matches!(b, Binding::Session(p) if p.starts_with("/native/")) {
            let light = self.doc.session["native"]["light"]
                .as_bool()
                .unwrap_or(false);
            crate::theme::apply(light, &self.doc.session["native"], cx);
            self.body_key.clear();
            self.response_key.clear();
        }
        self.changed(cx);
    }
    /// 按绑定复用输入状态并订阅修改，支持敏感遮蔽、数值解析和连接期间禁用。
    fn field(
        &mut self,
        b: Binding,
        label: &str,
        secret: bool,
        numeric: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Input {
        let locked = self.mqtt_binding_locked(&b);
        let key = b.key();
        if !self.fields.contains_key(&key) {
            let value = self.read(&b);
            let initial = if value.is_number() {
                value.to_string()
            } else {
                value.as_str().unwrap_or("").to_owned()
            };
            let state = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(label.to_owned())
                    .masked(secret)
                    .default_value(initial)
            });
            let subscription = cx.subscribe_in(
                &state,
                window,
                move |this, state, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Change) {
                        let text = state.read(cx).value().to_string();
                        let value = if numeric {
                            match text.parse::<u64>() {
                                Ok(n) => json!(n),
                                Err(_) => {
                                    this.error = "请输入有效的非负整数".into();
                                    cx.notify();
                                    return;
                                }
                            }
                        } else {
                            json!(text)
                        };
                        if this.read(&b) == value {
                            return;
                        }
                        this.write(&b, value, cx);
                        if let Binding::Request(p) = &b {
                            if p == "/url" {
                                this.fields.retain(|key, _| !key.starts_with("r/params/"));
                            } else if p.starts_with("/params/") {
                                if let Some(url) = this.fields.get("r/url").cloned() {
                                    let value = model::active(&this.doc)
                                        .map(|r| model::text(r, "url"))
                                        .unwrap_or_default();
                                    if url.read(cx).value().as_ref() != value {
                                        url.update(cx, |s, cx| s.set_value(value, window, cx));
                                    }
                                }
                            }
                        }
                    }
                },
            );
            self.subscriptions.push(subscription);
            self.fields.insert(key.clone(), state);
        }
        // Input 自身的 h 仅作用于多行编辑器；使用 Styled 高度使单行输入与选择器、按钮对齐。
        Styled::h(
            Input::new(&self.fields[&key])
                .aria_label(label.to_owned())
                .small(),
            self.scaled(crate::theme::CONTROL_HEIGHT),
        )
        .typography(TextRole::Control)
        .disabled(self.busy || locked)
    }
    /// 创建带统一文字、图标和动作分派的常规按钮。
    fn button(
        &self,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        action: Action,
        cx: &Context<Self>,
    ) -> Button {
        let icon = match &action {
            Action::Save => Some(Icon::new(Glyph::Save)),
            Action::Send => Some(Icon::new(Glyph::Send)),
            Action::Format | Action::MqttFormat => Some(Icon::new(Glyph::Braces)),
            Action::Copy => Some(Icon::new(IconName::Copy)),
            Action::Export => Some(Icon::new(Glyph::Download)),
            Action::New
            | Action::AddRow(_)
            | Action::AddFolder(_)
            | Action::NewEnv
            | Action::MqttCaseNew(false) => Some(Icon::new(IconName::Plus)),
            _ => None,
        };
        Button::new(id)
            .label(label)
            .small()
            .h(self.scaled(crate::theme::CONTROL_HEIGHT))
            .px_2()
            .typography(TextRole::Control)
            .when_some(icon, |button, icon| {
                button.icon(icon.size(self.scaled(crate::theme::ICON_SIZE)))
            })
            .disabled(self.locked_action(&action))
            .on_click(cx.listener(move |this, _, window, cx| this.act(action.clone(), window, cx)))
    }
    /// 构造统一动作菜单入口，标记选中项并按当前操作状态禁用。
    fn dropdown(
        &self,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        choices: Vec<(String, Action)>,
        more: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let label: SharedString = label.into();
        let empty = label.is_empty();
        let title = if empty {
            if choices
                .iter()
                .any(|(_, a)| matches!(a, Action::MqttCaseRenameDialog(_)))
            {
                "发布用例操作".to_owned()
            } else if more {
                "接口操作".to_owned()
            } else if choices.iter().any(|(_, a)| matches!(a, Action::Select(_))) {
                format!("全部标签（{}）", choices.len())
            } else {
                "选择请求方法".to_owned()
            }
        } else {
            label.to_string()
        };
        let locked = choices.iter().any(|(_, a)| self.locked_action(a));
        let items: Vec<_> = choices
            .into_iter()
            .map(|(label, action)| {
                let checked = match &action {
                    Action::Set(b, value) => self.read(b) == *value,
                    Action::SelectEnv(id) => text(&self.doc.session, "environment") == *id,
                    Action::Select(id) => text(&self.doc.session, "activeId") == *id,
                    _ => false,
                };
                (label, action, checked)
            })
            .collect();
        let entity = cx.entity().downgrade();
        let scale = self.scale();
        Button::new(id)
            .small()
            .h(self.scaled(crate::theme::CONTROL_HEIGHT))
            .px_2()
            .typography(TextRole::Control)
            .disabled(self.busy || locked)
            .accessibility_label(title.clone())
            .tooltip(title)
            .when(empty, |button| button.ghost().w(self.scaled(28.)).p_0())
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .gap_2()
                    .when(empty, |row| row.justify_center())
                    .when(!empty, |row| {
                        row.child(div().flex_1().min_w_0().truncate().child(label))
                    })
                    .child(
                        Icon::new(if more {
                            Icon::new(Glyph::Ellipsis)
                        } else {
                            Icon::new(IconName::ChevronDown)
                        })
                        .size(self.scaled(crate::theme::ICON_SIZE)),
                    ),
            )
            .dropdown_menu(move |menu, _, _| {
                Self::populate_menu(menu, &items, entity.clone(), scale)
            })
            .into_any_element()
    }
    /// 按动作列表填充菜单和勾选状态，为删除动作分隔分组，并以弱引用回调工作台。
    fn populate_menu(
        mut menu: PopupMenu,
        items: &[(String, Action, bool)],
        entity: WeakEntity<Self>,
        scale: f32,
    ) -> PopupMenu {
        menu = menu
            .min_w(px(180. * scale))
            .max_w(px(420. * scale))
            .max_h(px(360. * scale))
            .scrollable(true)
            .check_side(Side::Right);
        for (label, action, checked) in items {
            if matches!(
                action,
                Action::ConfirmDeleteRequest(_)
                    | Action::ConfirmDeleteFolder(_)
                    | Action::MqttCaseConfirmDelete(_)
            ) {
                menu = menu.separator();
            }
            let entity = entity.clone();
            let action = action.clone();
            menu = menu.item(
                PopupMenuItem::new(label.clone())
                    .checked(*checked)
                    .on_click(move |_, window, cx| {
                        let _ = entity.update(cx, |this, cx| this.act(action.clone(), window, cx));
                    }),
            );
        }
        menu
    }
    /// 将字符串选项映射为字段写入动作，使用统一选择器展示当前值。
    fn choose(
        &mut self,
        key: &str,
        b: Binding,
        values: &[(&str, &str)],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let current = self.read(&b).as_str().unwrap_or("").to_owned();
        let label = values
            .iter()
            .find(|(v, _)| *v == current)
            .map(|(_, label)| (*label).to_owned())
            .unwrap_or(current);
        self.select(
            SharedString::from(key.to_owned()),
            label,
            values
                .iter()
                .map(|(v, label)| ((*label).into(), Action::Set(b.clone(), json!(v))))
                .collect(),
            window,
            cx,
        )
    }
    /// 构造绑定布尔值的复选框，敏感标记变化时使相关输入缓存失效。
    fn toggle(
        &self,
        id: impl Into<ElementId>,
        b: Binding,
        label: &str,
        cx: &Context<Self>,
    ) -> Checkbox {
        Checkbox::new(id)
            .disabled(self.mqtt_binding_locked(&b))
            .label(label.to_owned())
            .checked(self.read(&b).as_bool().unwrap_or(false))
            .on_click(cx.listener(move |this, value, _, cx| {
                this.write(&b, json!(value), cx);
                if let Binding::Session(p) = &b {
                    if p.ends_with("/secret") {
                        this.fields.remove(
                            &Binding::Session(p.trim_end_matches("/secret").to_owned() + "/value")
                                .key(),
                        );
                    }
                }
            }))
    }
    /// 展示纯文本确认对话框，确认后分派指定动作；需要弹窗互斥的调用方应先检查窗口状态。
    fn confirm(
        &self,
        title: &str,
        message: String,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = title.to_owned();
        let entity = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let entity = entity.clone();
            let action = action.clone();
            dialog
                .title(title.clone())
                .child(message.clone())
                .confirm()
                .on_ok(move |_, window, cx| {
                    let _ = entity.update(cx, |this, cx| this.act(action.clone(), window, cx));
                    true
                })
        });
    }
    /// 集中处理工作台动作、保护忙状态并协调草稿、网络资源和确认流程，最后通知重绘。
    fn act(&mut self, a: Action, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy && !matches!(a, Action::Quit) {
            return;
        }
        match a {
            Action::SearchFocus => {
                self.screen = "requests".into();
                self.doc.session["native"]["sidebarClosed"] = json!(false);
                if let Some(input) = self.fields.get("search") {
                    input.focus_handle(cx).focus(window, cx);
                }
            }
            Action::ToggleSidebar => {
                self.doc.session["native"]["sidebarClosed"] = json!(
                    !self.doc.session["native"]["sidebarClosed"]
                        .as_bool()
                        .unwrap_or(false)
                );
                self.changed(cx);
            }
            Action::Expand(id) => {
                let mut ids: Vec<String> =
                    serde_json::from_value(self.doc.session["native"]["collapsed"].clone())
                        .unwrap_or_default();
                if ids.contains(&id) {
                    ids.retain(|v| v != &id);
                } else {
                    ids.push(id);
                }
                self.doc.session["native"]["collapsed"] = json!(ids);
                self.changed(cx);
            }
            Action::New => {
                if !self.ready {
                    return;
                }
                model::new_tab(&mut self.doc, &self.folder);
                self.screen = "requests".into();
                self.reset_fields();
                self.changed(cx);
            }
            Action::Save => {
                let folder = model::active(&self.doc)
                    .map(|r| text(r, "folder"))
                    .filter(|f| self.doc.folders.iter().any(|x| &x.id == f))
                    .unwrap_or(self.folder.clone());
                match model::save_request(&mut self.doc, &folder) {
                    Ok(()) => {
                        self.error.clear();
                        self.changed(cx);
                        self.status = "接口已保存，正在写入磁盘…".into();
                    }
                    Err(e) => self.error = e,
                }
            }
            Action::Select(id) => {
                self.doc.session["activeId"] = json!(id);
                self.screen = "requests".into();
                self.response_tab = "body".into();
                self.reset_fields();
                self.changed(cx);
            }
            Action::Open(r) => {
                model::open(&mut self.doc, r);
                self.screen = "requests".into();
                self.reset_fields();
                self.changed(cx);
            }
            Action::Close(id) => {
                // 模态确认结束前不能关闭其他标签，快捷键触发的关闭也必须遵守此边界。
                if window.has_active_dialog(cx) {
                    return;
                }
                if !self.confirmed_unsaved_tab_close
                    && model::tabs(&self.doc)
                        .iter()
                        .any(|t| text(t, "id") == id && model::dirty(t))
                {
                    self.confirm(
                        "关闭未保存的请求？",
                        "关闭后将丢弃此标签的修改。本次运行期间再次关闭未保存的标签时，将直接丢弃修改，不再提醒；需要保留请先保存接口。正常退出应用会自动恢复草稿。".into(),
                        Action::ConfirmClose(id),
                        window,
                        cx,
                    );
                } else {
                    self.close_tab(&id, cx)
                }
            }
            Action::ConfirmClose(id) => {
                self.confirmed_unsaved_tab_close = true;
                self.close_tab(&id, cx);
            }
            Action::Panel(panel) => {
                if let Some(i) = model::active_index(&self.doc) {
                    self.doc.session["tabs"][i]["panel"] = json!(panel);
                    self.reset_fields();
                    self.changed(cx);
                }
            }
            Action::Screen(screen) => {
                self.screen = screen;
                self.reset_fields();
            }
            Action::Set(b, v) => {
                self.write(&b, v, cx);
                self.reset_fields();
            }
            Action::AddRow(b) => {
                let mut rows = self.read(&b).as_array().cloned().unwrap_or_default();
                rows.push(model::row());
                self.write(&b, json!(rows), cx);
                self.reset_fields();
            }
            Action::RemoveRow(b, i) => {
                let mut rows = self.read(&b).as_array().cloned().unwrap_or_default();
                if i < rows.len() {
                    rows.remove(i);
                }
                self.write(&b, json!(rows), cx);
                if let Binding::Request(p) = &b {
                    if p == "/params" {
                        if let Some(r) = model::active_mut(&mut self.doc) {
                            model::params_changed(r)
                        }
                    }
                }
                self.reset_fields();
            }
            Action::Format => {
                if let Some(r) = model::active_mut(&mut self.doc) {
                    match serde_json::from_str::<Value>(&text(r, "body")) {
                        Ok(v) => {
                            r["body"] = json!(serde_json::to_string_pretty(&v).unwrap());
                            self.body = None;
                            self.body_key.clear();
                            self.changed(cx)
                        }
                        Err(e) => {
                            self.error = format!(
                                "JSON 格式错误：第 {} 行，第 {} 列。原文未修改。",
                                e.line(),
                                e.column()
                            )
                        }
                    }
                }
            }
            Action::MqttFormat => {
                if let (Some(format), Some(payload_binding)) = (
                    self.mqtt_case_binding("format"),
                    self.mqtt_case_binding("payload"),
                ) {
                    if self.read(&format) == "json" {
                        let payload = self
                            .read(&payload_binding)
                            .as_str()
                            .unwrap_or("")
                            .to_owned();
                        if !payload.trim().is_empty() {
                            match mqtt::format_json_template(&payload) {
                                Ok(formatted) => {
                                    self.write(&payload_binding, json!(formatted), cx);
                                    self.body = None;
                                    self.body_key.clear();
                                    if self.error == MQTT_FORMAT_ERROR {
                                        self.error.clear();
                                    }
                                }
                                Err(_) => {
                                    self.error = MQTT_FORMAT_ERROR.into();
                                }
                            }
                        }
                    }
                }
            }
            Action::Send => self.send(cx),
            Action::Protocol(protocol) => self.change_protocol(&protocol, cx),
            Action::MqttPublish => self.mqtt_publish(),
            Action::MqttCaseSelect(id) => {
                if model::mqtt_select_case(&mut self.doc, &id) {
                    self.reset_fields();
                    self.changed(cx);
                }
            }
            Action::MqttCaseNew(copy) => {
                if model::mqtt_new_case(&mut self.doc, copy).is_some() {
                    self.reset_fields();
                    self.changed(cx);
                } else {
                    self.error = "发布用例最多保存 100 条".into();
                }
            }
            Action::MqttCaseRenameDialog(id) => self.mqtt_case_rename_dialog(id, window, cx),
            Action::MqttCaseConfirmDelete(id) => {
                let count = model::active(&self.doc)
                    .and_then(|r| r["mqtt"]["publishCases"].as_array())
                    .map_or(0, Vec::len);
                if count <= 1 {
                    self.error = "发布用例至少保留一条".into();
                } else {
                    self.confirm(
                        "删除发布用例？",
                        "删除后该用例的主题和消息内容将从当前连接中移除。".into(),
                        Action::MqttCaseDelete(text(&self.doc.session, "activeId"), id),
                        window,
                        cx,
                    );
                }
            }
            Action::MqttCaseDelete(tab, id) => {
                if text(&self.doc.session, "activeId") == tab
                    && model::mqtt_delete_case(&mut self.doc, &id)
                {
                    self.reset_fields();
                    self.changed(cx);
                }
            }
            Action::MqttTopic(i, on) => self.mqtt_topic(i, on, cx),
            Action::MqttAddTopic => {
                let active = text(&self.doc.session, "activeId");
                if !self.mqtt_sessions.get(&active).is_some_and(|s| s.active()) {
                    if let Some(r) = model::active_mut(&mut self.doc) {
                        if let Some(rows) = r["mqtt"]["topics"].as_array_mut() {
                            rows.push(mqtt::topic_row());
                        }
                    }
                    self.reset_fields();
                    self.changed(cx);
                }
            }
            Action::MqttClear => {
                let id = text(&self.doc.session, "activeId");
                if let Some(session) = self.mqtt_sessions.get(&id) {
                    let mut live = session.live.lock().unwrap();
                    live.records.clear();
                    live.revision += 1;
                }
                self.mqtt_selected.remove(&id);
                self.response_key.clear();
            }
            Action::MqttRecord(id) => {
                self.mqtt_selected
                    .insert(text(&self.doc.session, "activeId"), id);
                self.response_key.clear();
            }
            Action::MqttCopy => {
                cx.write_to_clipboard(ClipboardItem::new_string(self.mqtt_detail()));
                self.status = "已复制消息详情".into();
            }

            Action::Response(tab) => {
                self.response_tab = tab;
                self.response_key.clear();
            }
            Action::Raw => {
                self.raw = !self.raw;
                self.response_key.clear();
            }
            Action::Copy => {
                cx.write_to_clipboard(ClipboardItem::new_string(self.response_text()));
                self.status = "已复制响应预览".into();
            }
            Action::Export => self.export(window, cx),
            Action::AddFolder(parent) => {
                self.folder = model::add_folder(
                    &mut self.doc,
                    if parent.is_some() {
                        "新建文件夹".into()
                    } else {
                        "新建集合".into()
                    },
                    parent,
                );
                model::collection(&mut self.doc, &self.folder);
                self.screen = "folder".into();
                self.reset_fields();
                self.changed(cx);
            }
            Action::SelectFolder(id) => {
                self.folder = id;
                model::collection(&mut self.doc, &self.folder);
                self.screen = "folder".into();
                self.reset_fields();
            }
            Action::DeleteFolder(id) => {
                for rid in model::remove_folder(&mut self.doc, &id) {
                    self.release(&rid)
                }
                self.folder = self
                    .doc
                    .folders
                    .first()
                    .map(|f| f.id.clone())
                    .unwrap_or_default();
                self.screen = "requests".into();
                self.reset_fields();
                self.changed(cx);
            }
            Action::ConfirmDeleteFolder(id) => {
                if let Some(folder) = self.doc.folders.iter().find(|folder| folder.id == id) {
                    self.confirm(
                        "删除目录",
                        format!(
                            "将删除“{}”及其全部子目录和接口。此操作无法撤销。",
                            folder.name
                        ),
                        Action::DeleteFolder(id),
                        window,
                        cx,
                    );
                }
            }
            Action::DuplicateRequest(id) => {
                if let Some(mut r) = self
                    .doc
                    .requests
                    .iter()
                    .find(|r| text(r, "id") == id)
                    .cloned()
                {
                    r["id"] = json!(model::id());
                    r["name"] = json!(format!("{} 副本", text(&r, "name")));
                    model::open(&mut self.doc, r);
                    self.screen = "requests".into();
                    self.reset_fields();
                    self.changed(cx)
                }
            }
            Action::ConfirmDeleteRequest(id) => self.confirm(
                "删除接口？",
                "将删除此接口和对应草稿，此操作无法撤销。".into(),
                Action::DeleteRequest(id),
                window,
                cx,
            ),
            Action::DeleteRequest(id) => {
                model::close(&mut self.doc, &id);
                self.release(&id);
                self.doc.requests.retain(|r| text(r, "id") != id);
                self.reset_fields();
                self.changed(cx);
            }
            Action::MoveRequest(id, folder) => {
                if let Some(r) = self.doc.requests.iter_mut().find(|r| text(r, "id") == id) {
                    r["folder"] = json!(folder)
                }
                for t in self.doc.session["tabs"].as_array_mut().unwrap() {
                    if text(t, "id") == id {
                        t["request"]["folder"] = json!(folder)
                    }
                }
                self.changed(cx);
            }
            Action::MoveFolder(id, parent) => {
                if let Err(e) = model::move_folder(&mut self.doc, &id, parent) {
                    self.error = e
                } else {
                    self.changed(cx)
                }
            }
            Action::OrderFolder(id, up) => {
                if let Some(i) = self.doc.folders.iter().position(|f| f.id == id) {
                    let peers: Vec<_> = self
                        .doc
                        .folders
                        .iter()
                        .enumerate()
                        .filter(|(_, f)| f.parent == self.doc.folders[i].parent)
                        .map(|(i, _)| i)
                        .collect();
                    let p = peers.iter().position(|v| *v == i).unwrap();
                    let j = if up { p.checked_sub(1) } else { Some(p + 1) };
                    if let Some(j) = j.and_then(|j| peers.get(j)) {
                        self.doc.folders.swap(i, *j);
                        self.changed(cx)
                    }
                }
            }
            Action::OrderRequest(id, up) => {
                if let Some(i) = self.doc.requests.iter().position(|r| text(r, "id") == id) {
                    let peers: Vec<_> = self
                        .doc
                        .requests
                        .iter()
                        .enumerate()
                        .filter(|(_, r)| r["folder"] == self.doc.requests[i]["folder"])
                        .map(|(i, _)| i)
                        .collect();
                    let p = peers.iter().position(|v| *v == i).unwrap();
                    let j = if up { p.checked_sub(1) } else { Some(p + 1) };
                    if let Some(j) = j.and_then(|j| peers.get(j)) {
                        self.doc.requests.swap(i, *j);
                        self.changed(cx)
                    }
                }
            }
            Action::NewEnv => {
                let id = model::id();
                self.doc.session["network"]["environments"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"id":id,"name":"新环境","variables":[]}));
                self.scope = id;
                self.reset_fields();
                self.changed(cx);
            }
            Action::SelectEnv(id) => {
                self.doc.session["environment"] = json!(id);
                self.changed(cx);
            }
            Action::DuplicateEnv(id) => {
                if let Some(mut env) = self.doc.session["network"]["environments"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|e| text(e, "id") == id)
                    .cloned()
                {
                    env["id"] = json!(model::id());
                    env["name"] = json!(format!("{} 副本", text(&env, "name")));
                    self.scope = text(&env, "id");
                    self.doc.session["network"]["environments"]
                        .as_array_mut()
                        .unwrap()
                        .push(env);
                    self.reset_fields();
                    self.changed(cx)
                }
            }
            Action::DeleteEnv(id) => {
                self.doc.session["network"]["environments"]
                    .as_array_mut()
                    .unwrap()
                    .retain(|e| text(e, "id") != id);
                if text(&self.doc.session, "environment") == id {
                    self.doc.session["environment"] = json!("")
                }
                self.scope = "globals".into();
                self.reset_fields();
                self.changed(cx);
            }
            Action::Scope(scope) => {
                self.scope = scope;
                self.reset_fields();
            }
            Action::Theme(light) => {
                self.doc.session["native"]["light"] = json!(light);
                crate::theme::apply(light, &self.doc.session["native"], cx);
                self.changed(cx);
            }
            Action::SettingsPage(page) => {
                self.settings_page = page;
                self.reset_fields();
            }
            Action::ResetDisplay => {
                for (key, value) in [
                    ("light", json!(false)),
                    ("scale", json!(100)),
                    ("editorFontSize", json!(14)),
                    ("lineNumbers", json!(true)),
                    ("softWrap", json!(true)),
                    ("strongBorders", json!(false)),
                ] {
                    self.doc.session["native"][key] = value;
                }
                crate::theme::apply(false, &self.doc.session["native"], cx);
                self.reset_fields();
                self.changed(cx);
            }
            Action::ChooseStorage(mode) => self.choose_storage(mode, window, cx),
            Action::CancelPlan => self.plan = None,
            Action::SwitchStorage => {
                if self.runs.values().any(|r| r.pending)
                    || self.mqtt_sessions.values().any(|s| s.active())
                {
                    self.error = "请先取消正在发送的请求，并断开 MQTT 连接".into();
                } else if (!self.ready || self.flush(cx)) && self.plan.is_some() {
                    let (mode, plan) = self.plan.take().unwrap();
                    self.busy = true;
                    self.backend.switch(PathBuf::from(plan.target), mode);
                }
            }
            Action::Retry => {
                self.error.clear();
                if self.ready {
                    self.queued = 0;
                    self.saved = 0;
                    self.changed(cx)
                } else {
                    self.backend.load();
                }
            }
            Action::Quit => {
                if self.flush(cx) {
                    self.http.clear();
                    self.mqtt_sessions.clear();
                    cx.quit();
                }
            }
        }
        cx.notify();
    }
    /// 移除指定标签、释放协议运行资源并触发草稿保存；调用前应已完成必要确认。
    fn close_tab(&mut self, id: &str, cx: &mut Context<Self>) {
        model::close(&mut self.doc, id);
        self.release(id);
        self.reset_fields();
        self.changed(cx);
    }
    /// 释放指定标签的 MQTT 会话、详情选择和 HTTP 运行及缓存，HTTP 在途任务同时收到取消信号。
    fn release(&mut self, id: &str) {
        self.mqtt_sessions.remove(id);
        self.mqtt_selected.remove(id);
        self.mqtt_revisions.remove(id);
        if let Some(run) = self.runs.remove(id) {
            let _ = self.http.cancel(&run.id);
            self.http.release(&run.id);
        }
    }
    /// 按当前协议发送或取消 HTTP、连接或断开 MQTT；HTTP 结果携带标签和运行标识异步回传。
    fn send(&mut self, cx: &mut Context<Self>) {
        if model::active(&self.doc).is_some_and(mqtt::is_mqtt) {
            self.mqtt_connect(cx);
            return;
        }
        if !self.ready {
            return;
        }
        let tab = text(&self.doc.session, "activeId");
        if let Some(run) = self.runs.get(&tab) {
            if run.pending {
                let _ = self.http.cancel(&run.id);
                self.status = "正在取消请求…".into();
                return;
            }
        }
        let input = match model::input(&self.doc) {
            Ok(v) => v,
            Err(e) => {
                self.error = e;
                return;
            }
        };
        self.release(&tab);
        self.error.clear();
        match self.http.start(input) {
            Ok(id) => {
                self.runs.insert(
                    tab.clone(),
                    RunState {
                        id: id.clone(),
                        pending: true,
                        started: Instant::now(),
                        result: None,
                    },
                );
                let service = self.http.clone();
                let tx = self.tx.clone();
                self.runtime.spawn(async move {
                    let result = service.execute(id.clone()).await;
                    let _ = tx.send(Event::Response(tab, id, result));
                });
            }
            Err(e) => {
                self.runs.insert(
                    tab,
                    RunState {
                        id: String::new(),
                        pending: false,
                        started: Instant::now(),
                        result: Some(Err(e)),
                    },
                );
            }
        }
        self.response_key.clear();
        cx.notify();
    }
    /// 取得活动标签最近一次成功的 HTTP 执行结果；本地失败或尚未完成时返回 None。
    fn response(&self) -> Option<&http::Output> {
        self.runs
            .get(&text(&self.doc.session, "activeId"))
            .and_then(|r| r.result.as_ref())
            .and_then(|r| r.as_ref().ok())
    }
    /// 按当前响应分区生成预览或复制文本，遮蔽 Cookie，并仅格式化未截断的 JSON 正文。
    fn response_text(&self) -> String {
        let Some(r) = self.response() else {
            return String::new();
        };
        match self.response_tab.as_str() {
            "headers" => r
                .headers
                .iter()
                .map(|p| format!("{}: {}", p.name, p.value))
                .collect::<Vec<_>>()
                .join("\n"),
            "cookies" => r
                .cookies
                .iter()
                .map(|p| format!("{}: ••••••", p.name))
                .collect::<Vec<_>>()
                .join("\n"),
            "request" => serde_json::to_string_pretty(&r.effective).unwrap_or_default(),
            _ => {
                if r.binary {
                    return "二进制内容无法预览，请保存响应文件。".into();
                }
                if !self.raw && !r.preview_limited {
                    serde_json::from_str::<Value>(&r.body)
                        .and_then(|v| serde_json::to_string_pretty(&v))
                        .unwrap_or_else(|_| r.body.clone())
                } else {
                    r.body.clone()
                }
            }
        }
    }
    /// 选择保存路径后在后台复制响应缓存，经临时文件刷盘后替换目标，并通过事件回传结果。
    fn export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.response() else { return };
        let source = match self.http.file(&r.run_id) {
            Ok(v) => v,
            Err(e) => {
                self.error = e;
                return;
            }
        };
        let rx = cx.prompt_for_new_path(
            &dirs::download_dir().unwrap_or_else(std::env::temp_dir),
            Some("响应.bin"),
        );
        let tx = self.tx.clone();
        let rt = self.runtime.clone();
        cx.spawn_in(window, async move |_, _| {
            if let Ok(Ok(Some(path))) = rx.await {
                rt.spawn_blocking(move || {
                    let result = (|| {
                        let parent = path.parent().ok_or("保存目录无效")?;
                        let mut temp =
                            tempfile::NamedTempFile::new_in(parent).map_err(|_| "无法写入目录")?;
                        std::io::copy(
                            &mut source.reopen().map_err(|_| "无法读取响应")?,
                            temp.as_file_mut(),
                        )
                        .map_err(|_| "保存失败，请检查磁盘空间")?;
                        temp.as_file().sync_all().map_err(|_| "无法写入磁盘")?;
                        temp.persist(path).map_err(|_| "无法保存响应文件")?;
                        Ok(())
                    })();
                    let _ = tx.send(Event::Export(result));
                });
            }
        })
        .detach();
    }
    /// 无活动 HTTP 或 MQTT 任务时打开目录选择器，将目标及模式交给后台预检。
    fn choose_storage(&mut self, mode: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.runs.values().any(|r| r.pending) || self.mqtt_sessions.values().any(|s| s.active())
        {
            self.error = "请先完成或取消 HTTP 请求，并断开 MQTT 连接".into();
            return;
        }
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("选择 ReqSucc 数据目录".into()),
        });
        let backend = self.backend.clone();
        cx.spawn_in(window, async move |_, _| {
            if let Ok(Ok(Some(paths))) = receiver.await {
                if let Some(path) = paths.first() {
                    backend.plan(path.clone(), mode)
                }
            }
        })
        .detach();
    }
    /// 构造需二次确认的危险操作按钮，点击后先展示说明再执行动作。
    fn danger(
        &self,
        id: impl Into<ElementId>,
        label: &str,
        message: String,
        action: Action,
        cx: &Context<Self>,
    ) -> Button {
        let label = label.to_owned();
        Button::new(id)
            .small()
            .h(self.scaled(crate::theme::CONTROL_HEIGHT))
            .px_2()
            .typography(TextRole::Control)
            .danger()
            .outline()
            .label(label.clone())
            .on_click(cx.listener(move |this, _, window, cx| {
                this.confirm(&label, message.clone(), action.clone(), window, cx)
            }))
    }
    /// 绘制可编辑键值表及行增删入口，按用途支持敏感值标记并复用统一列宽。
    fn rows(
        &mut self,
        b: Binding,
        variables: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rows = self.read(&b).as_array().cloned().unwrap_or_default();
        let root = b.key();
        let path = match &b {
            Binding::Request(p) | Binding::Session(p) => p.clone(),
            _ => String::new(),
        };
        let make = |suffix: String| match b {
            Binding::Request(_) => Binding::Request(format!("{path}{suffix}")),
            _ => Binding::Session(format!("{path}{suffix}")),
        };
        let mut header = TableRow::new()
            .child(self.table_head("启用", Some(48.)))
            .child(self.table_head("名称", None))
            .child(self.table_head("值", None))
            .child(self.table_head("说明", None));
        if variables {
            header = header.child(self.table_head("敏感", Some(48.)));
        }
        header = header.child(self.table_head("", Some(36.)));
        let mut body = TableBody::new();
        for (i, row) in rows.iter().enumerate() {
            let name = self.field(make(format!("/{i}/key")), "名称", false, false, window, cx);
            let value = self.field(
                make(format!("/{i}/value")),
                "值",
                variables && row["secret"].as_bool().unwrap_or(false),
                false,
                window,
                cx,
            );
            let desc = self.field(
                make(format!("/{i}/description")),
                "说明",
                false,
                false,
                window,
                cx,
            );
            let mut line = TableRow::new()
                .child(
                    self.table_cell(Some(48.))
                        .justify_center()
                        .child(self.toggle(
                            SharedString::from(format!("{root}-{i}-enabled")),
                            make(format!("/{i}/enabled")),
                            "",
                            cx,
                        )),
                )
                .child(self.table_cell(None).child(name.appearance(false)))
                .child(self.table_cell(None).child(value.appearance(false)))
                .child(self.table_cell(None).child(desc.appearance(false)));
            if variables {
                line = line.child(
                    self.table_cell(Some(48.))
                        .justify_center()
                        .child(self.toggle(
                            SharedString::from(format!("{root}-{i}-secret")),
                            make(format!("/{i}/secret")),
                            "",
                            cx,
                        )),
                );
            }
            line = line.child(
                self.table_cell(Some(36.))
                    .justify_center()
                    .child(self.icon_button(
                        SharedString::from(format!("{root}-{i}-delete")),
                        "移除此行",
                        Glyph::Trash,
                        Action::RemoveRow(b.clone(), i),
                        cx,
                    )),
            );
            body = body.child(line);
        }
        let count = rows.len();
        v_flex()
            .w_full()
            .gap_0()
            .border_1()
            .border_color(cx.theme().border)
            .rounded(cx.theme().radius)
            .overflow_hidden()
            .child(
                Table::new()
                    .small()
                    .accessibility_label(if variables { "变量表" } else { "参数表" })
                    .child(TableHeader::new().child(header))
                    .child(body),
            )
            .when(count == 0, |el| {
                el.child(
                    div()
                        .py_6()
                        .px_3()
                        .typography(TextRole::Secondary)
                        .text_color(cx.theme().muted_foreground)
                        .child(if variables {
                            "暂无变量，添加后可通过 {{变量名}} 引用"
                        } else {
                            "暂无参数，点击下方按钮添加"
                        }),
                )
            })
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .justify_between()
                    .child(
                        self.button(
                            SharedString::from(format!("{root}-add")),
                            "添加一行",
                            Action::AddRow(b),
                            cx,
                        )
                        .ghost()
                        .icon(IconName::Plus),
                    )
                    .child(
                        div()
                            .typography(TextRole::Caption)
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{count} 项")),
                    ),
            )
            .into_any_element()
    }
    /// 按绑定范围展示无认证、Bearer、Basic、API Key 或集合继承配置。
    fn auth_panel(
        &mut self,
        prefix: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let b = |key: &str| match &prefix {
            Some(p) => Binding::Session(format!("{p}/{key}")),
            None => Binding::Request(format!("/{key}")),
        };
        let auth = self.read(&b("auth")).as_str().unwrap_or("none").to_owned();
        let mut choices = vec![
            ("none", "无认证"),
            ("bearer", "持有者令牌（Bearer）"),
            ("basic", "基本认证（Basic）"),
            ("apiKey", "接口密钥（API Key）"),
        ];
        if prefix.is_none() {
            choices.push(("inherit", "继承所属集合"))
        }
        let mut panel = Form::vertical().small().max_w(self.scaled(620.)).child(
            self.described_field(
                "支持 {{变量名}}。认证信息仅保存在本地数据库中，请妥善保管数据目录。",
            )
            .label("认证方式")
            .child(self.choose("auth-type", b("auth"), &choices, window, cx)),
        );
        let fields: Vec<(&str, &str, bool)> = match auth.as_str() {
            "bearer" => vec![("token", "令牌", true)],
            "basic" => vec![("username", "用户名", false), ("password", "密码", true)],
            "apiKey" => vec![
                ("apiKeyName", "密钥名称", false),
                ("apiKeyValue", "密钥值", true),
            ],
            _ => vec![],
        };
        for (key, label, secret) in fields {
            let field = self.field(b(key), label, secret, false, window, cx);
            panel = panel.child(Field::new().label(label).child(field))
        }
        if auth == "apiKey" {
            panel = panel.child(Field::new().label("密钥位置").child(self.choose(
                "api-key-in",
                b("apiKeyIn"),
                &[("header", "放入请求头"), ("query", "放入查询参数")],
                window,
                cx,
            )))
        }
        if auth == "inherit" {
            panel =
                panel.child(self.described_field(
                    "使用当前接口所在根集合的认证。临时请求未选择集合时不附加认证。",
                ));
        }
        panel.into_any_element()
    }
    /// 绘制全局或请求级网络设置，统一处理超时、重定向、TLS 和接收容量。
    fn options(&mut self, global: bool, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if !global {
            if let Some(r) = model::active_mut(&mut self.doc) {
                if !r["settings"].is_object() {
                    r["settings"] = json!({})
                }
            }
        }
        let b = |key: &str| {
            if global {
                Binding::Session(format!("/network/defaults/{key}"))
            } else {
                Binding::Request(format!("/settings/{key}"))
            }
        };
        let mut panel = Form::horizontal()
            .small()
            .label_width(self.scaled(260.))
            .max_w(self.scaled(620.))
            .when(!global, |form| {
                form.child(
                    self.described_field("未修改的选项继承全局默认设置。")
                        .label_indent(false),
                )
            });
        for (key, label) in [
            ("timeoutMs", "总超时（毫秒，1–3600000）"),
            ("connectTimeoutMs", "连接超时（毫秒，1–3600000）"),
            ("maxResponseMib", "响应接收上限（MiB，1–1024）"),
        ] {
            let field = self.field(b(key), label, false, true, window, cx);
            panel = panel.child(Field::new().label(label).child(field));
        }
        panel = panel
            .child(Field::new().label("重定向").child(self.setting_switch(
                "follow-redirects",
                b("followRedirects"),
                "跟随重定向（最多 10 次）",
                cx,
            )))
            .child(Field::new().label("安全连接").child(self.setting_switch(
                "verify-tls",
                b("verifyTls"),
                "校验 TLS 证书",
                cx,
            )));
        if !global {
            panel = panel.child(Field::new().child(self.button(
                "reset-options",
                "恢复全局默认",
                Action::Set(Binding::Request("/settings".into()), json!({})),
                cx,
            )))
        }
        panel.into_any_element()
    }
    /// 按 HTTP 请求体类型展示文本编辑器、JSON 格式化入口或表单键值表。
    fn request_body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(r) = model::active(&self.doc).cloned() else {
            return div().into_any_element();
        };
        let kind = text(&r, "bodyType");
        let choices = [
            ("none", "无"),
            ("json", "JSON"),
            ("text", "文本"),
            ("xml", "XML"),
            ("form", "表单编码"),
        ];
        let kinds = TabBar::new("body-kinds")
            .segmented()
            .small()
            .selected_index(choices.iter().position(|(v, _)| *v == kind).unwrap_or(0))
            .children(choices.iter().map(|(_, label)| Tab::new().label(*label)))
            .on_click(cx.listener(move |this, index: &usize, w, cx| {
                this.act(
                    Action::Set(
                        Binding::Request("/bodyType".into()),
                        json!(choices[*index].0),
                    ),
                    w,
                    cx,
                );
            }));
        let mut panel =
            v_flex()
                .size_full()
                .gap_3()
                .child(
                    h_flex()
                        .justify_between()
                        .child(kinds)
                        .when(kind == "json", |row| {
                            row.child(
                                self.button("format-json", "格式化", Action::Format, cx)
                                    .ghost(),
                            )
                        }),
                );
        if kind == "form" {
            if model::active(&self.doc).unwrap()["form"].is_null() {
                model::active_mut(&mut self.doc).unwrap()["form"] = json!([])
            }
            panel = panel.child(
                div()
                    .id("form-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .child(self.rows(Binding::Request("/form".into()), false, window, cx)),
            );
        } else if kind != "none" {
            let key = format!("{}-{kind}", text(&r, "id"));
            if self.body.is_none() || self.body_key != key {
                let content = text(&r, "body");
                let editor = cx.new(|cx| {
                    EditorState::new(window, cx)
                        .language(if kind == "json" { "json" } else { "plain" })
                        .default_value(content)
                        .line_number(self.display_flag("lineNumbers"))
                        .soft_wrap(self.display_flag("softWrap"))
                });
                let sub = cx.subscribe_in(
                    &editor,
                    window,
                    |this, editor, event: &InputEvent, _, cx| {
                        if matches!(event, InputEvent::Change) {
                            let value = editor.read(cx).value().to_string();
                            if model::active(&this.doc).is_some_and(|r| text(r, "body") != value) {
                                this.write(&Binding::Request("/body".into()), json!(value), cx)
                            }
                        }
                    },
                );
                self.subscriptions.push(sub);
                self.body = Some(editor);
                self.body_key = key;
            }
            panel = panel.child(
                div().flex_1().min_h_0().child(
                    Editor::new(self.body.as_ref().unwrap())
                        .h(relative(1.))
                        .aria_label("请求体编辑器"),
                ),
            );
        } else {
            panel = panel.child(div().p_4().child(self.help("此请求没有请求体", cx)))
        }
        panel.into_any_element()
    }
    /// 绘制活动请求的名称、协议、方法、地址和编辑分区；MQTT 请求交给专用视图。
    fn request_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(r) = model::active(&self.doc).cloned() else {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    self.kit_empty(
                        "从一个请求开始",
                        "创建请求或从左侧集合打开接口，所有数据保存在本地。",
                        Glyph::Send,
                    )
                    .content(
                        EmptyContent::new().child(
                            self.button("empty-new", "新建请求", Action::New, cx)
                                .primary(),
                        ),
                    ),
                )
                .into_any_element();
        };
        if mqtt::is_mqtt(&r) {
            return self.mqtt_request_panel(window, cx);
        }
        let tab_index = model::active_index(&self.doc).unwrap();
        let panel = text(&self.doc.session["tabs"][tab_index], "panel");
        let pending = self.runs.get(&text(&r, "id")).is_some_and(|r| r.pending);
        let name = self.field(
            Binding::Request("/name".into()),
            "接口名称",
            false,
            false,
            window,
            cx,
        );
        let url = self.field(
            Binding::Request("/url".into()),
            "输入请求地址，支持 {{变量名}}",
            false,
            false,
            window,
            cx,
        );
        let mut folders = vec![];
        for f in &self.doc.folders {
            folders.push((
                f.name.clone(),
                Action::Set(Binding::Request("/folder".into()), json!(f.id)),
            ))
        }
        let folder_name = self
            .doc
            .folders
            .iter()
            .find(|f| f.id == text(&r, "folder"))
            .map(|f| f.name.clone())
            .unwrap_or("选择保存目录".into());
        let method = self.field(
            Binding::Request("/method".into()),
            "请求方法",
            false,
            false,
            window,
            cx,
        );
        let nav = self.section_tabs(
            "http-panels",
            &panel,
            &[
                ("params", "参数"),
                ("auth", "认证"),
                ("headers", "请求头"),
                ("body", "请求体"),
                ("description", "说明"),
                ("settings", "请求设置"),
            ],
            false,
            cx,
        );
        let content = match panel.as_str() {
            "auth" => self.auth_panel(None, window, cx),
            "body" => self.request_body(window, cx),
            "headers" => self.rows(Binding::Request("/headers".into()), false, window, cx),
            "settings" => self.options(false, window, cx),
            "description" => {
                let input = self.field(
                    Binding::Request("/description".into()),
                    "接口说明",
                    false,
                    false,
                    window,
                    cx,
                );
                v_flex()
                    .gap_2()
                    .child(self.copy("接口说明", TextRole::Body, cx))
                    .child(input)
                    .into_any_element()
            }
            _ => self.rows(Binding::Request("/params".into()), false, window, cx),
        };
        let tls = model::input(&self.doc)
            .map(|input| input.options.verify_tls)
            .unwrap_or(true);
        v_flex()
            .size_full()
            .p_3()
            .gap_2()
            .child(
                h_flex()
                    .h(self.scaled(crate::theme::CONTROL_HEIGHT))
                    .gap_2()
                    .child(self.protocol_picker(cx))
                    .child(
                        div()
                            .typography(TextRole::Secondary)
                            .text_color(cx.theme().muted_foreground)
                            .child(if self.doc.requests.iter().any(|v| v["id"] == r["id"]) {
                                folder_name.clone()
                            } else {
                                "临时请求".into()
                            }),
                    )
                    .child(
                        Icon::new(IconName::ChevronRight)
                            .size(self.scaled(crate::theme::ICON_SIZE))
                            .text_color(cx.theme().muted_foreground),
                    )
                    .child(div().flex_1().child(name.appearance(false)))
                    .child(self.select("save-folder", folder_name, folders, window, cx))
                    .child(
                        self.button("save-request", "保存", Action::Save, cx)
                            .ghost(),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .flex_1()
                            .h(self.scaled(34.))
                            .border_1()
                            .border_color(cx.theme().input)
                            .rounded(self.scaled(7.))
                            .px_2()
                            .child(
                                div()
                                    .w(self.scaled(85.))
                                    .text_color(self.method_color(&text(&r, "method")))
                                    .child(method.appearance(false)),
                            )
                            .child(
                                self.dropdown(
                                    "method-menu",
                                    "",
                                    ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"]
                                        .iter()
                                        .map(|v| {
                                            (
                                                (*v).into(),
                                                Action::Set(
                                                    Binding::Request("/method".into()),
                                                    json!(v),
                                                ),
                                            )
                                        })
                                        .collect(),
                                    false,
                                    cx,
                                ),
                            )
                            .child(
                                div()
                                    .w(self.scaled(1.))
                                    .h(self.scaled(24.))
                                    .mx_2()
                                    .bg(cx.theme().border),
                            )
                            .child(div().flex_1().child(url.appearance(false))),
                    )
                    .child(
                        self.button(
                            "send",
                            if pending { "取消" } else { "发送" },
                            Action::Send,
                            cx,
                        )
                        .icon(
                            Icon::new(if pending {
                                Glyph::CircleStop
                            } else {
                                Glyph::Send
                            })
                            .size(self.scaled(crate::theme::ICON_SIZE)),
                        )
                        .primary()
                        .h(self.scaled(34.))
                        .w(self.scaled(86.))
                        .disabled(self.busy),
                    ),
            )
            .when(!tls, |el| {
                el.child(
                    Alert::warning(
                        "tls-warning",
                        "TLS 证书校验已关闭，仅建议用于可信的本地调试服务。",
                    )
                    .small(),
                )
            })
            .child(nav)
            .child(
                div()
                    .id("request-content")
                    .flex_1()
                    .min_h_0()
                    .when(panel != "body", |el| el.overflow_y_scroll())
                    .child(content),
            )
            .into_any_element()
    }
    /// 绘制 HTTP 等待、失败或响应结果，提供分区切换、正文预览、复制和导出。
    fn response_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if model::active(&self.doc).is_some_and(mqtt::is_mqtt) {
            return self.mqtt_messages(window, cx);
        }
        let active = text(&self.doc.session, "activeId");
        let run = self.runs.get(&active);
        let summary = if let Some(run) = run {
            if run.pending {
                format!(
                    "正在发送 · 可随时取消（已运行 {} 秒）",
                    run.started.elapsed().as_secs()
                )
            } else {
                match run.result.as_ref() {
                    Some(Ok(r)) => format!(
                        "{} {}   ·   {} 毫秒   ·   {} 字节",
                        r.status, r.status_text, r.elapsed_ms, r.received_bytes
                    ),
                    Some(Err(e)) => format!(
                        "{} · {} 毫秒",
                        if e.kind == "cancelled" {
                            "已取消"
                        } else {
                            "请求未完成"
                        },
                        e.elapsed_ms
                    ),
                    None => "等待响应".into(),
                }
            }
        } else {
            "尚未发送".into()
        };
        let mut panel = v_flex().size_full().p_3().gap_2().child(
            h_flex().justify_between().child(
                h_flex()
                    .gap_4()
                    .child(self.section_title("响应", cx))
                    .child(
                        div()
                            .typography(TextRole::Secondary)
                            .text_color(cx.theme().muted_foreground)
                            .child(summary),
                    ),
            ),
        );
        if let Some(run) = run {
            if let Some(Err(e)) = &run.result {
                return panel
                    .child(
                        div()
                            .p_3()
                            .text_color(cx.theme().muted_foreground)
                            .child(e.message.clone()),
                    )
                    .into_any_element();
            }
        }
        if self.response().is_none() {
            return panel
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(self.kit_empty(
                        "等待响应",
                        "发送请求后，在这里查看服务器返回的内容。⌘ / Ctrl + Enter 发送当前请求。",
                        Glyph::Send,
                    )),
                )
                .into_any_element();
        }
        let r = self.response().unwrap().clone();
        let nav = self.section_tabs(
            "response-panels",
            &self.response_tab,
            &[
                ("body", "响应体"),
                ("headers", "响应头"),
                ("cookies", "Cookie"),
                ("request", "本次请求"),
            ],
            true,
            cx,
        );
        panel = panel
            .child(
                h_flex().justify_between().child(nav).child(
                    h_flex()
                        .gap_2()
                        .child(
                            self.button("response-copy", "复制预览", Action::Copy, cx)
                                .ghost(),
                        )
                        .child(self.button(
                            "response-export",
                            if r.incomplete {
                                "保存已接收部分"
                            } else {
                                "保存响应文件"
                            },
                            Action::Export,
                            cx,
                        )),
                ),
            )
            .child(
                div()
                    .typography(TextRole::Secondary)
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "{} · 重定向 {} 次 · {}",
                        r.final_url, r.redirects, r.encoding
                    )),
            );
        if r.preview_limited || r.incomplete {
            panel = panel.child(
                Alert::warning(
                    "response-limit",
                    if r.incomplete {
                        "已达到接收上限，响应不完整。"
                    } else {
                        "预览仅显示前 2 MiB，完整内容可保存为文件。"
                    },
                )
                .small(),
            );
        }
        if self.response_tab == "cookies" {
            panel =
                panel.child(self.help("Cookie 值默认隐藏，本阶段仅展示，不自动保存或回传。", cx));
        }
        if self.response_tab == "request" {
            panel = panel.child(self.help(
                "有效请求预览已隐藏认证和敏感变量；重定向后的实际请求可能不同。",
                cx,
            ));
        }
        if self.response_tab == "body" && !r.binary {
            panel = panel.child(
                self.button(
                    "raw",
                    if self.raw {
                        "格式化视图"
                    } else {
                        "原始文本"
                    },
                    Action::Raw,
                    cx,
                )
                .ghost(),
            );
        }
        let key = format!("{}-{}-{}", r.run_id, self.response_tab, self.raw);
        if self.response_key != key || self.response_editor.is_none() {
            let text = self.response_text();
            let json = serde_json::from_str::<Value>(&text).is_ok();
            self.response_editor = Some(cx.new(|cx| {
                EditorState::new(window, cx)
                    .language(if json { "json" } else { "plain" })
                    .default_value(text)
                    .line_number(self.display_flag("lineNumbers"))
                    .soft_wrap(self.display_flag("softWrap"))
            }));
            self.response_key = key;
        }
        panel
            .child(
                div().flex_1().min_h_0().child(
                    Editor::new(self.response_editor.as_ref().unwrap())
                        .h(relative(1.))
                        .readonly(true)
                        .aria_label("响应内容"),
                ),
            )
            .into_any_element()
    }
    /// 绘制选中目录的名称、层级和集合变量及认证设置。
    fn folder_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(folder) = self
            .doc
            .folders
            .iter()
            .find(|f| f.id == self.folder)
            .cloned()
        else {
            return div()
                .p_4()
                .child(self.help("从左侧选择集合或新建集合", cx))
                .into_any_element();
        };
        let name = self.field(
            Binding::Folder(folder.id.clone()),
            "集合或文件夹名称",
            false,
            false,
            window,
            cx,
        );
        let mut moves = vec![(
            "设为根集合".into(),
            Action::MoveFolder(folder.id.clone(), None),
        )];
        moves.extend(
            self.doc
                .folders
                .iter()
                .filter(|f| f.id != folder.id)
                .map(|f| {
                    (
                        f.name.clone(),
                        Action::MoveFolder(folder.id.clone(), Some(f.id.clone())),
                    )
                }),
        );
        let root = model::collection(&mut self.doc, &folder.id);
        let root_name = self
            .doc
            .folders
            .iter()
            .find(|f| f.id == root)
            .map(|f| f.name.clone())
            .unwrap_or_default();
        let menu_moves: Vec<_> = moves
            .into_iter()
            .map(|(name, action)| (name, action, false))
            .collect();
        let menu_actions = vec![
            (
                "上移目录".into(),
                Action::OrderFolder(folder.id.clone(), true),
                false,
            ),
            (
                "下移目录".into(),
                Action::OrderFolder(folder.id.clone(), false),
                false,
            ),
            (
                "删除目录…".into(),
                Action::ConfirmDeleteFolder(folder.id.clone()),
                false,
            ),
        ];
        let entity = cx.entity().downgrade();
        let scale = self.scale();
        let folder_menu = Button::new("folder-actions")
            .small()
            .h(self.scaled(crate::theme::CONTROL_HEIGHT))
            .px_2()
            .typography(TextRole::Control)
            .label("更多操作")
            .icon(Icon::new(IconName::ChevronDown).size(self.scaled(crate::theme::ICON_SIZE)))
            .accessibility_label("目录更多操作")
            .disabled(self.busy)
            .dropdown_menu(move |menu, window, cx| {
                let entity_moves = entity.clone();
                let moves = menu_moves.clone();
                let menu = menu.submenu("移动到…", window, cx, move |menu, _, _| {
                    Self::populate_menu(menu, &moves, entity_moves.clone(), scale)
                });
                Self::populate_menu(menu, &menu_actions, entity.clone(), scale)
            });
        let vars = self.rows(
            Binding::Session(format!("/network/collections/{root}/variables")),
            true,
            window,
            cx,
        );
        let auth = self.auth_panel(
            Some(format!("/network/collections/{root}/auth")),
            window,
            cx,
        );
        v_flex()
            .id("folder-settings")
            .size_full()
            .overflow_y_scroll()
            .p_3()
            .gap_2()
            .child(self.page_title(if folder.parent.is_none() { "集合" } else { "文件夹" }, cx))
            .child(name)
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        self.button("folder-new-request", "新建接口", Action::New, cx)
                            .primary(),
                    )
                    .child(self.button(
                        "folder-new-child",
                        "新建子目录",
                        Action::AddFolder(Some(folder.id.clone())),
                        cx,
                    ))
                    .child(folder_menu),
            )
            .child(self.section_title("集合变量", cx))
            .child(div().typography(TextRole::Secondary).text_color(cx.theme().muted_foreground).child(
                format!("变量与认证属于根集合“{}”，其下的文件夹共享这些配置。优先级：环境 ＞ 集合 ＞ 全局。", root_name),
            ))
            .child(vars)
            .child(self.section_title("集合认证", cx))
            .child(auth)
            .into_any_element()
    }
    /// 绘制全局与环境变量管理，提供环境选择、复制、删除及键值编辑。
    fn environments(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let envs = self.doc.session["network"]["environments"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut scopes = vec![("globals".to_owned(), "全局变量".to_owned())];
        scopes.extend(envs.iter().map(|e| (text(e, "id"), text(e, "name"))));
        let nav = h_flex()
            .gap_2()
            .child(
                TabBar::new("environment-scopes")
                    .underline()
                    .small()
                    .flex_1()
                    .min_w_0()
                    .selected_index(
                        scopes
                            .iter()
                            .position(|(id, _)| *id == self.scope)
                            .unwrap_or(0),
                    )
                    .children(
                        scopes
                            .iter()
                            .map(|(_, name)| Tab::new().label(name.clone())),
                    )
                    .on_click(cx.listener(move |this, i: &usize, w, cx| {
                        this.act(Action::Scope(scopes[*i].0.clone()), w, cx)
                    })),
            )
            .child(self.button("new-env", "新建环境", Action::NewEnv, cx));
        let i = envs.iter().position(|e| text(e, "id") == self.scope);
        let mut panel = v_flex()
            .id("environment-settings")
            .size_full()
            .overflow_y_scroll()
            .p_3()
            .gap_2()
            .child(self.page_title("环境与变量", cx))
            .child(nav)
            .child(self.help(
                "通过 {{变量名}} 引用变量；仅启用的变量参与解析，敏感值默认隐藏。",
                cx,
            ));
        let path = if let Some(i) = i {
            let input = self.field(
                Binding::Session(format!("/network/environments/{i}/name")),
                "环境名称",
                false,
                false,
                window,
                cx,
            );
            panel = panel.child(input).child(
                h_flex()
                    .gap_2()
                    .child(self.button(
                        "use-env",
                        "启用此环境",
                        Action::SelectEnv(self.scope.clone()),
                        cx,
                    ))
                    .child(self.button(
                        "duplicate-env",
                        "复制环境",
                        Action::DuplicateEnv(self.scope.clone()),
                        cx,
                    ))
                    .child(self.danger(
                        "delete-env",
                        "删除环境",
                        "删除此环境及其变量，此操作无法撤销。".into(),
                        Action::DeleteEnv(self.scope.clone()),
                        cx,
                    )),
            );
            format!("/network/environments/{i}/variables")
        } else {
            "/network/globals".into()
        };
        panel
            .child(self.rows(Binding::Session(path), true, window, cx))
            .into_any_element()
    }
    /// 创建具有统一标题、说明和边框的设置分组容器。
    fn setting_card(&self, title: &str, description: &str, cx: &Context<Self>) -> Div {
        v_flex()
            .w_full()
            .border_1()
            .border_color(cx.theme().border)
            .rounded(self.scaled(8.))
            .bg(cx.theme().sidebar)
            .overflow_hidden()
            .child(
                v_flex()
                    .p_3()
                    .gap_1()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(self.section_title(title, cx))
                    .child(self.help(description, cx)),
            )
    }
    /// 绘制设置说明与控件；compact 为 true 时纵向排列，适应高缩放或窄窗口。
    fn setting_row(
        &self,
        title: &str,
        description: &str,
        control: impl IntoElement,
        compact: bool,
        cx: &Context<Self>,
    ) -> Div {
        // 控件保持左边缘对齐；高缩放或窄窗口时移到说明下方，避免挤压文本。
        div()
            .flex()
            .w_full()
            .min_w_0()
            .gap_3()
            .px_4()
            .py_3()
            .when(compact, |row| row.flex_col().items_start())
            .when(!compact, |row| row.flex_row().items_center())
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                v_flex()
                    .min_w_0()
                    .gap_1()
                    .when(!compact, |label| label.flex_1())
                    .child(self.copy(title, TextRole::Body, cx))
                    .child(self.help(description, cx)),
            )
            .child(
                h_flex()
                    .w(self.scaled(180.))
                    .flex_shrink_0()
                    .justify_start()
                    .child(control),
            )
    }
    /// 创建显示偏好开关，将变更写入 native 配置并触发主题或编辑器更新。
    fn display_switch(
        &self,
        id: &'static str,
        key: &'static str,
        checked: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        gpui_kit::component::switch::Switch::new(id)
            .checked(checked)
            .accessibility_label(match key {
                "strongBorders" => "增强边框对比度",
                "lineNumbers" => "显示行号",
                _ => "自动换行",
            })
            .on_click(cx.listener(move |this, value, _, cx| {
                this.write(
                    &Binding::Session(format!("/native/{key}")),
                    json!(value),
                    cx,
                );
            }))
    }
    /// 绘制显示、存储、请求及关于设置页，按可用内容宽度调整布局并展示目录操作计划。
    fn settings(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let page = self.settings_page.clone();
        let mut menu = SidebarMenu::new();
        for (id, name, icon) in [
            ("display", "显示设置", IconName::Settings2),
            ("storage", "数据存储", IconName::HardDrive),
            ("network", "请求设置", IconName::Globe),
            ("about", "关于 ReqSucc", IconName::Info),
        ] {
            menu = menu.child(
                SidebarMenuItem::new(name)
                    .icon(icon)
                    .active(page == id)
                    .on_click(cx.listener(move |this, _, w, cx| {
                        this.act(Action::SettingsPage(id.into()), w, cx)
                    })),
            );
        }
        let nav = Sidebar::new("settings-navigation")
            .w(self.scaled(176.))
            .header(SidebarHeader::new().child(self.section_title("偏好设置", cx)))
            .child(SidebarGroup::new("通用").child(menu));
        let heading = match page.as_str() {
            "storage" => "数据存储",
            "network" => "请求设置",
            "about" => "关于 ReqSucc",
            _ => "显示设置",
        };
        // 活动栏采用组件库固定 48 逻辑像素的折叠侧栏，设置导航与内边距跟随主题缩放。
        // 布局断点依据剩余内容宽度，避免将活动栏宽度重复计入缩放。
        let available = (f32::from(window.viewport_size().width) - 48.) / self.scale() - 176. - 24.;
        let compact = available < 660.;
        let mut content = v_flex()
            .w_full()
            .min_w_0()
            .max_w(self.scaled(720.))
            .gap_3()
            .child(
                h_flex().child(
                    self.button(
                        "settings-back",
                        "返回工作台",
                        Action::Screen("requests".into()),
                        cx,
                    )
                    .icon(
                        Icon::new(IconName::ChevronLeft).size(self.scaled(crate::theme::ICON_SIZE)),
                    ),
                ),
            )
            .child(
                v_flex()
                    .gap_1()
                    .pb_1()
                    .child(self.page_title(heading, cx))
                    .child(self.help("修改后自动保存，下次打开继续使用。", cx)),
            );
        match page.as_str() {
            "storage" => {
                content =
                    content.child(
                        self.setting_card("存储位置", "接口、集合、环境和草稿统一保存在本地。", cx)
                            .child(
                                v_flex()
                                    .p_3()
                                    .gap_4()
                                    .child(
                                        div()
                                            .p_3()
                                            .rounded(self.scaled(6.))
                                            .border_1()
                                            .border_color(cx.theme().border)
                                            .bg(cx.theme().background)
                                            .typography(TextRole::Control)
                                            .child(if self.path.is_empty() {
                                                "尚未打开数据目录".into()
                                            } else {
                                                self.path.clone()
                                            }),
                                    )
                                    .child(
                                        h_flex()
                                            .flex_wrap()
                                            .gap_2()
                                            .child(
                                                self.button(
                                                    "storage-migrate",
                                                    "迁移当前数据",
                                                    Action::ChooseStorage("migrate".into()),
                                                    cx,
                                                )
                                                .icon(
                                                    Icon::new(IconName::Folder)
                                                        .size(self.scaled(crate::theme::ICON_SIZE)),
                                                )
                                                .primary()
                                                .disabled(!self.ready),
                                            )
                                            .child(
                                                self.button(
                                                    "storage-open",
                                                    "打开已有目录",
                                                    Action::ChooseStorage("open".into()),
                                                    cx,
                                                )
                                                .icon(
                                                    Icon::new(IconName::FolderOpen)
                                                        .size(self.scaled(crate::theme::ICON_SIZE)),
                                                ),
                                            )
                                            .child(
                                                self.button(
                                                    "storage-create",
                                                    "创建新目录",
                                                    Action::ChooseStorage("create".into()),
                                                    cx,
                                                )
                                                .icon(
                                                    Icon::new(IconName::Plus)
                                                        .size(self.scaled(crate::theme::ICON_SIZE)),
                                                ),
                                            ),
                                    )
                                    .child(self.help(
                                        "迁移时保留源目录。创建和迁移的目标目录必须为空。",
                                        cx,
                                    )),
                            ),
                    );
                if let Some((mode, plan)) = &self.plan {
                    content = content.child(
                        self.setting_card("确认目录操作", "请核对目标位置后继续。", cx)
                            .child(
                                v_flex()
                                    .p_3()
                                    .gap_4()
                                    .child(format!(
                                        "{}：{}",
                                        match mode.as_str() {
                                            "open" => "打开已有目录",
                                            "migrate" => "迁移当前数据",
                                            _ => "创建数据目录",
                                        },
                                        plan.target
                                    ))
                                    .child(format!(
                                        "需要复制 {} 字节，目标可用空间 {} MiB",
                                        plan.bytes,
                                        plan.available_bytes / 1024 / 1024
                                    ))
                                    .child(
                                        h_flex()
                                            .flex_wrap()
                                            .gap_2()
                                            .child(
                                                self.button(
                                                    "confirm-storage",
                                                    "确认并继续",
                                                    Action::SwitchStorage,
                                                    cx,
                                                )
                                                .primary(),
                                            )
                                            .child(self.button(
                                                "cancel-storage",
                                                "取消",
                                                Action::CancelPlan,
                                                cx,
                                            )),
                                    ),
                            ),
                    );
                }
            }
            "network" => {
                let options = self.options(true, window, cx);
                content = content.child(
                    self.setting_card(
                        "请求默认设置",
                        "作为新请求的默认配置，可在单个请求中覆盖。",
                        cx,
                    )
                    .child(div().p_3().child(options)),
                );
            }
            "about" => {
                content =
                    content.child(
                        self.setting_card("ReqSucc 0.4.0", "全中文本地接口工作台", cx)
                            .child(
                                v_flex()
                                    .p_3()
                                    .gap_4()
                                    .child(
                                        h_flex()
                                            .flex_wrap()
                                            .gap_2()
                                            .child(
                                                svg()
                                                    .path("brand/flying-cat.svg")
                                                    .size(self.scaled(48.))
                                                    .text_color(cx.theme().foreground),
                                            )
                                            .child("请求成功，调试从容。"),
                                    )
                                    .child(self.help(
                                        "数据保存在本机，支持手动 HTTP 与 MQTT V5 调试。",
                                        cx,
                                    ))
                                    .child(self.help("当前不提供脚本、断言测试或云同步。", cx)),
                            ),
                    );
            }
            _ => {
                let light = self.doc.session["native"]["light"]
                    .as_bool()
                    .unwrap_or(false);
                let themes = TabBar::new("theme-mode")
                    .segmented()
                    .small()
                    .selected_index(usize::from(light))
                    .child(Tab::new().label("深色"))
                    .child(Tab::new().label("浅色"))
                    .on_click(cx.listener(|this, index: &usize, w, cx| {
                        this.act(Action::Theme(*index == 1), w, cx)
                    }));
                let scale = self.select(
                    "display-scale",
                    format!("{}%", (self.scale() * 100.).round() as i32),
                    [90, 100, 110, 125]
                        .into_iter()
                        .map(|n| {
                            (
                                format!("{n}%"),
                                Action::Set(Binding::Session("/native/scale".into()), json!(n)),
                            )
                        })
                        .collect(),
                    window,
                    cx,
                );
                let borders = self.display_switch(
                    "display-borders",
                    "strongBorders",
                    self.doc.session["native"]["strongBorders"]
                        .as_bool()
                        .unwrap_or(false),
                    cx,
                );
                content = content.child(
                    self.setting_card("外观与布局", "统一调整工作台的颜色、尺寸和分区边界。", cx)
                        .child(self.setting_row(
                            "界面主题",
                            "选择适合当前光线的配色。",
                            themes,
                            compact,
                            cx,
                        ))
                        .child(self.setting_row(
                            "界面缩放",
                            "调整文字、按钮和面板间距，默认 100%。",
                            scale,
                            compact,
                            cx,
                        ))
                        .child(self.setting_row(
                            "增强边框对比度",
                            "让输入框和面板分隔线更清晰。",
                            borders,
                            compact,
                            cx,
                        )),
                );
                let font = self.doc.session["native"]["editorFontSize"]
                    .as_u64()
                    .unwrap_or(14);
                let sizes = self.select(
                    "editor-font-size",
                    format!("{font} 像素"),
                    [12, 14, 16, 18, 20]
                        .into_iter()
                        .map(|n| {
                            (
                                format!("{n} 像素"),
                                Action::Set(
                                    Binding::Session("/native/editorFontSize".into()),
                                    json!(n),
                                ),
                            )
                        })
                        .collect(),
                    window,
                    cx,
                );
                let lines = self.display_switch(
                    "editor-line-numbers",
                    "lineNumbers",
                    self.display_flag("lineNumbers"),
                    cx,
                );
                let wrap = self.display_switch(
                    "editor-soft-wrap",
                    "softWrap",
                    self.display_flag("softWrap"),
                    cx,
                );
                content = content
                    .child(
                        self.setting_card("编辑器", "同时应用于请求体与响应内容。", cx)
                            .child(self.setting_row(
                                "编辑器字号",
                                "单独调整代码文字大小。",
                                sizes,
                                compact,
                                cx,
                            ))
                            .child(self.setting_row(
                                "显示行号",
                                "在编辑区左侧显示代码行号。",
                                lines,
                                compact,
                                cx,
                            ))
                            .child(self.setting_row(
                                "自动换行",
                                "长行按编辑区宽度折行，不改变实际内容。",
                                wrap,
                                compact,
                                cx,
                            )),
                    )
                    .child(
                        v_flex()
                            .w_full()
                            .min_w_0()
                            .gap_2()
                            .p_3()
                            .border_1()
                            .border_color(cx.theme().border)
                            .rounded(self.scaled(8.))
                            .child(self.help(
                                "恢复主题、界面缩放与编辑器显示选项，不影响已保存的接口和环境。",
                                cx,
                            ))
                            .child(h_flex().flex_wrap().gap_2().child(self.button(
                                "reset-display",
                                "恢复默认显示设置",
                                Action::ResetDisplay,
                                cx,
                            ))),
                    );
            }
        }
        h_flex()
            .size_full()
            .child(nav)
            .child(
                div()
                    .id("settings-scroll")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_y_scroll()
                    .p_3()
                    .child(content),
            )
            .into_any_element()
    }
    /// 绘制可搜索的集合树与接口列表，保留折叠层级及复制、移动、排序和删除入口。
    fn sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let search = self
            .field(
                Binding::Search,
                "搜索接口名称或地址",
                false,
                false,
                window,
                cx,
            )
            .prefix(
                Icon::new(IconName::Search)
                    .size(self.scaled(crate::theme::ICON_SIZE))
                    .text_color(cx.theme().muted_foreground),
            );
        let mut list = v_flex()
            .id("collection-tree")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .gap_1();
        let collapsed: Vec<String> =
            serde_json::from_value(self.doc.session["native"]["collapsed"].clone())
                .unwrap_or_default();
        let mut ordered = vec![];
        /// 按目录顺序递归生成可见节点及缩进深度；跳过折叠子树并限制异常关系的递归深度。
        fn visit(
            parent: Option<&str>,
            depth: usize,
            s: &Snapshot,
            collapsed: &[String],
            out: &mut Vec<(String, usize)>,
        ) {
            if depth > s.folders.len() {
                return;
            }
            for f in s.folders.iter().filter(|f| f.parent.as_deref() == parent) {
                out.push((f.id.clone(), depth));
                if !collapsed.contains(&f.id) {
                    visit(Some(&f.id), depth + 1, s, collapsed, out)
                }
            }
        }
        visit(
            None,
            0,
            &self.doc,
            &if self.search.is_empty() {
                collapsed.clone()
            } else {
                vec![]
            },
            &mut ordered,
        );
        let query = self.search.to_lowercase();
        for (id, depth) in ordered {
            let folder = self.doc.folders.iter().find(|f| f.id == id).unwrap();
            let expanded = !collapsed.contains(&id) || !query.is_empty();
            list = list.child(
                h_flex()
                    .pl(self.scaled(depth as f32 * 12.))
                    .h(self.scaled(32.))
                    .gap_1()
                    .child(
                        self.icon_button(
                            SharedString::from(format!("expand-{id}")),
                            if expanded {
                                "收起目录"
                            } else {
                                "展开目录"
                            },
                            Icon::new(if expanded {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .size(self.scaled(12.)),
                            Action::Expand(id.clone()),
                            cx,
                        )
                        .w(self.scaled(24.)),
                    )
                    .child(
                        ListItem::new(SharedString::from(format!("folder-{id}")))
                            .role(Role::Button)
                            .tab_index(0)
                            .accessibility_label(folder.name.clone())
                            .typography(TextRole::Control)
                            .h(self.scaled(crate::theme::CONTROL_HEIGHT))
                            .px_1()
                            .flex_1()
                            .min_w_0()
                            .selected(self.screen == "folder" && self.folder == id)
                            .child(
                                h_flex()
                                    .w_full()
                                    .gap_2()
                                    .child(
                                        Icon::new(if expanded {
                                            IconName::FolderOpen
                                        } else {
                                            IconName::Folder
                                        })
                                        .size(self.scaled(crate::theme::ICON_SIZE))
                                        .text_color(cx.theme().muted_foreground),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .typography(TextRole::Control)
                                            .truncate()
                                            .child(folder.name.clone()),
                                    ),
                            )
                            .on_key_down(cx.listener({
                                let id = id.clone();
                                move |this, event: &KeyDownEvent, w, cx| {
                                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                        this.act(Action::SelectFolder(id.clone()), w, cx);
                                    }
                                }
                            }))
                            .on_click(cx.listener({
                                let id = id.clone();
                                move |this, _, w, cx| {
                                    this.act(Action::SelectFolder(id.clone()), w, cx)
                                }
                            })),
                    ),
            );
            if !expanded {
                continue;
            }
            for r in self
                .doc
                .requests
                .iter()
                .filter(|r| text(r, "folder") == id)
                .filter(|r| {
                    query.is_empty()
                        || format!("{} {}", text(r, "name"), mqtt::address(r))
                            .to_lowercase()
                            .contains(&query)
                })
            {
                let rid = text(r, "id");

                let method = mqtt::badge(r);
                let mut choices = vec![
                    ("复制接口".into(), Action::DuplicateRequest(rid.clone())),
                    ("上移".into(), Action::OrderRequest(rid.clone(), true)),
                    ("下移".into(), Action::OrderRequest(rid.clone(), false)),
                ];
                choices.extend(self.doc.folders.iter().map(|f| {
                    (
                        format!("移动到 {}", f.name),
                        Action::MoveRequest(rid.clone(), f.id.clone()),
                    )
                }));
                choices.push((
                    "删除接口…".into(),
                    Action::ConfirmDeleteRequest(rid.clone()),
                ));
                list = list.child(
                    h_flex()
                        .pl(self.scaled((depth + 1) as f32 * 12.))
                        .h(self.scaled(crate::theme::CONTROL_HEIGHT))
                        .child(
                            ListItem::new(SharedString::from(format!("request-{rid}")))
                                .role(Role::Button)
                                .tab_index(0)
                                .accessibility_label(format!("{} {}", method, text(r, "name")))
                                .typography(TextRole::Control)
                                .h(self.scaled(crate::theme::CONTROL_HEIGHT))
                                .px_1()
                                .justify_start()
                                .flex_1()
                                .min_w_0()
                                .selected(text(&self.doc.session, "activeId") == rid)
                                .child(
                                    h_flex()
                                        .w_full()
                                        .min_w_0()
                                        .gap_1()
                                        .child(
                                            div()
                                                .flex_shrink_0()
                                                .text_left()
                                                .typography(TextRole::Caption)
                                                .text_color(self.method_color(&method))
                                                .child(method),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .typography(TextRole::Control)
                                                .truncate()
                                                .child(text(r, "name")),
                                        ),
                                )
                                .on_key_down(cx.listener({
                                    let r = r.clone();
                                    move |this, event: &KeyDownEvent, w, cx| {
                                        if matches!(event.keystroke.key.as_str(), "enter" | "space")
                                        {
                                            this.act(Action::Open(r.clone()), w, cx);
                                        }
                                    }
                                }))
                                .on_click(cx.listener({
                                    let r = r.clone();
                                    move |this, _, w, cx| this.act(Action::Open(r.clone()), w, cx)
                                })),
                        )
                        .child(self.dropdown(
                            SharedString::from(format!("request-menu-{rid}")),
                            "",
                            choices,
                            true,
                            cx,
                        )),
                );
            }
        }
        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .p_3()
            .gap_3()
            .child(
                h_flex()
                    .justify_between()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(self.section_title("集合", cx))
                            .child(
                                div()
                                    .typography(TextRole::Caption)
                                    .text_color(cx.theme().muted_foreground)
                                    .child(self.doc.requests.len().to_string()),
                            ),
                    )
                    .child(
                        self.icon_button(
                            "new-collection",
                            "新建集合",
                            IconName::Plus,
                            Action::AddFolder(None),
                            cx,
                        )
                        .disabled(!self.ready),
                    ),
            )
            .child(search)
            .child(list)
            .child(
                self.button("sidebar-new", "新建请求", Action::New, cx)
                    .ghost()
                    .justify_start()
                    .disabled(!self.ready),
            )
            .into_any_element()
    }
    /// 绘制工作台活动导航栏，连接请求、环境、设置和主题切换入口。
    fn rail(&self, cx: &Context<Self>) -> AnyElement {
        let mut menu = SidebarMenu::new();
        for (label, icon, screen) in [
            ("集合", Glyph::Layers, "requests"),
            ("环境", Glyph::SlidersHorizontal, "environments"),
            ("历史（即将开放）", Glyph::Clock, "history"),
        ] {
            let selected =
                self.screen == screen || (screen == "requests" && self.screen == "folder");
            menu = menu.child(
                SidebarMenuItem::new(label)
                    .icon(icon)
                    .accessibility_label(label)
                    .active(selected)
                    .disable(screen == "history" || (!self.ready && screen == "environments"))
                    .on_click(cx.listener(move |this, _, w, cx| {
                        this.act(Action::Screen(screen.into()), w, cx)
                    })),
            );
        }
        Sidebar::new("activity-navigation")
            .collapsed(true)
            .h_full()
            .flex_shrink_0()
            .border_r_1()
            .border_color(cx.theme().border)
            .child(SidebarGroup::new("工作台").child(menu))
            .footer(
                v_flex()
                    .gap_2()
                    .items_center()
                    .child(self.icon_button(
                        "about",
                        "设置",
                        Glyph::Settings,
                        Action::Screen("settings".into()),
                        cx,
                    ))
                    .child(
                        self.icon_button(
                            "toggle-sidebar",
                            "展开或收起侧栏",
                            if self.doc.session["native"]["sidebarClosed"]
                                .as_bool()
                                .unwrap_or(false)
                            {
                                IconName::PanelLeftOpen
                            } else {
                                IconName::PanelLeftClose
                            },
                            Action::ToggleSidebar,
                            cx,
                        ),
                    ),
            )
            .into_any_element()
    }
}
impl Render for Workbench {
    /// 组装原生标题栏、侧栏、可拖动标签及可调整尺寸的主面板，并显示保存和运行状态。
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut environments = vec![("未选择环境".into(), Action::SelectEnv(String::new()))];
        let mut environment_name = "未选择环境".to_owned();
        if let Some(envs) = self.doc.session["network"]["environments"].as_array() {
            for e in envs {
                let name = text(e, "name");
                if e["id"] == self.doc.session["environment"] {
                    environment_name = name.clone()
                }
                environments.push((name, Action::SelectEnv(text(e, "id"))));
            }
        }
        let top = h_flex()
            .w_full()
            .h_full()
            .px_4()
            .gap_3()
            .flex_shrink_0()
            .bg(if cx.theme().mode.is_dark() {
                rgb(0x24272a).into()
            } else {
                cx.theme().background
            })
            .child(
                h_flex()
                    .w(self.scaled(150.))
                    .gap_2()
                    .child(
                        svg()
                            .path("brand/flying-cat.svg")
                            .size(self.scaled(28.))
                            .text_color(cx.theme().foreground),
                    )
                    .child(
                        div()
                            .text_size(self.scaled(16.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("ReqSucc"),
                    ),
            )
            .child(
                h_flex().flex_1().justify_center().child(
                    Button::new("global-search")
                        .ghost()
                        .w(self.scaled(270.))
                        .h(self.scaled(crate::theme::CONTROL_HEIGHT))
                        .border_1()
                        .border_color(cx.theme().border)
                        .bg(cx.theme().background)
                        .text_color(cx.theme().muted_foreground)
                        .accessibility_label("搜索接口")
                        .child(
                            h_flex()
                                .w_full()
                                .gap_2()
                                .child(
                                    Icon::new(IconName::Search)
                                        .size(self.scaled(crate::theme::ICON_SIZE)),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .typography(TextRole::Control)
                                        .child("搜索接口"),
                                )
                                .child(div().typography(TextRole::Caption).child("⌘ K")),
                        )
                        .on_click(
                            cx.listener(|this, _, w, cx| this.act(Action::SearchFocus, w, cx)),
                        ),
                ),
            )
            .child(
                div()
                    .max_w(self.scaled(180.))
                    .overflow_hidden()
                    .child(self.select("environment", environment_name, environments, window, cx)),
            );
        let sidebar = self.sidebar(window, cx);
        let rail = self.rail(cx);
        let content = match self.screen.as_str() {
            "settings" => self.settings(window, cx),
            "environments" => self.environments(window, cx),
            "folder" => self.folder_panel(window, cx),
            _ => {
                let count = model::tabs(&self.doc).len();
                let sidebar_width = if self.doc.session["native"]["sidebarClosed"]
                    .as_bool()
                    .unwrap_or(false)
                {
                    0.
                } else {
                    self.doc.session["native"]["sidebarWidth"]
                        .as_f64()
                        .unwrap_or(272.) as f32
                };
                let available = f32::from(window.viewport_size().width) / self.scale()
                    - 48.
                    - sidebar_width
                    - 64.;
                let tab_width = (available / count.max(1) as f32).clamp(120., 168.);
                let active = text(&self.doc.session, "activeId");
                if self.visible_tab != active {
                    if let Some(index) = model::active_index(&self.doc) {
                        self.tab_scroll.scroll_to_item(index);
                    }
                    self.visible_tab = active;
                }
                let mut tabs = TabBar::new("request-tabs")
                    .folder()
                    .w_full()
                    .max_width(self.scaled(tab_width))
                    .track_scroll(&self.tab_scroll)
                    .selected_index(model::active_index(&self.doc).unwrap_or(0));
                if let Some(last) = model::tabs(&self.doc).last() {
                    let last_id = text(last, "id");
                    tabs = tabs.last_empty_space(
                        div()
                            .id("request-tabs-end")
                            .h_full()
                            .flex_grow_1()
                            .min_w_3()
                            .drag_over::<DraggedRequestTab>(|this, _, _, cx| {
                                this.border_l_2().border_color(cx.theme().primary)
                            })
                            .on_drop(cx.listener(move |this, drag: &DraggedRequestTab, _, cx| {
                                if model::reorder_tab(&mut this.doc, &drag.id, &last_id, true) {
                                    this.changed(cx);
                                }
                            })),
                    );
                }
                let mut open_tabs = Vec::new();
                let tab_order: Vec<String> = model::tabs(&self.doc)
                    .iter()
                    .map(|tab| text(tab, "id"))
                    .collect();
                for tab in model::tabs(&self.doc) {
                    let id = text(tab, "id");
                    let r = &tab["request"];
                    let method = mqtt::badge(r);
                    let name = text(r, "name");
                    open_tabs.push((format!("{method} {name}"), Action::Select(id.clone())));
                    let badge = self.protocol_tag(&method);
                    let close = self
                        .icon_button(
                            SharedString::from(format!("close-{id}")),
                            "关闭请求标签",
                            IconName::Close,
                            Action::Close(id.clone()),
                            cx,
                        )
                        .xsmall()
                        .w_5()
                        .h_5()
                        .on_click(cx.listener({
                            let id = id.clone();
                            move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.act(Action::Close(id.clone()), window, cx);
                            }
                        }));
                    let drag = DraggedRequestTab {
                        id: id.clone(),
                        label: format!("{method} {name}"),
                    };
                    let drop_id = id.clone();
                    let drag_order = tab_order.clone();
                    tabs = tabs.child(
                        Tab::new()
                            .label(name.clone())
                            .aria_label(format!("{method} {name}"))
                            .prefix(badge)
                            .suffix(
                                h_flex()
                                    .gap_1()
                                    .when(model::dirty(tab), |el| {
                                        el.child(
                                            div().size_1().rounded_full().bg(cx.theme().warning),
                                        )
                                    })
                                    .child(close),
                            )
                            .on_click(cx.listener(move |this, _, w, cx| {
                                this.act(Action::Select(id.clone()), w, cx)
                            }))
                            .on_drag(drag, |drag, _, _, cx| {
                                cx.stop_propagation();
                                cx.new(|_| drag.clone())
                            })
                            .drag_over::<DraggedRequestTab>(move |this, drag, _, cx| {
                                let source = drag_order.iter().position(|id| id == &drag.id);
                                let target = drag_order.iter().position(|id| id == &drop_id);
                                match (source, target) {
                                    (Some(a), Some(b)) if a < b => {
                                        this.border_r_2().border_color(cx.theme().primary)
                                    }
                                    (Some(a), Some(b)) if a > b => {
                                        this.border_l_2().border_color(cx.theme().primary)
                                    }
                                    _ => this,
                                }
                            })
                            .on_drop(cx.listener({
                                let drop_id = text(tab, "id");
                                move |this, drag: &DraggedRequestTab, _, cx| {
                                    let source = model::tabs(&this.doc)
                                        .iter()
                                        .position(|tab| text(tab, "id") == drag.id);
                                    let target = model::tabs(&this.doc)
                                        .iter()
                                        .position(|tab| text(tab, "id") == drop_id);
                                    let after =
                                        matches!((source, target), (Some(a), Some(b)) if a < b);
                                    if model::reorder_tab(&mut this.doc, &drag.id, &drop_id, after)
                                    {
                                        this.changed(cx);
                                    }
                                }
                            })),
                    );
                }
                let all_tabs = if open_tabs.is_empty() {
                    div().into_any_element()
                } else {
                    self.dropdown("all-tabs", "", open_tabs, false, cx)
                };
                // GPUI Kit 文件夹式标签占 32 逻辑像素并带 4 像素顶部留白。
                // 操作按钮需在同一标签栏总高度内居中，缩放后仍保持对齐。
                let tabs = tabs.h_full().items_end().suffix(
                    h_flex()
                        .h_full()
                        .items_center()
                        .child(
                            self.icon_button(
                                "new-tab",
                                "新建请求标签",
                                IconName::Plus,
                                Action::New,
                                cx,
                            )
                            .w(self.scaled(28.))
                            .h(self.scaled(crate::theme::CONTROL_HEIGHT))
                            .disabled(!self.ready),
                        )
                        .child(all_tabs),
                );
                let request = self.request_panel(window, cx);
                let response = self.response_panel(window, cx);
                v_flex()
                    .size_full()
                    .child(
                        h_flex()
                            .h(px(36. * self.scale().max(1.)))
                            .flex_shrink_0()
                            .bg(if cx.theme().mode.is_dark() {
                                rgb(0x1b1e21).into()
                            } else {
                                cx.theme().muted
                            })
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .child(div().flex_1().min_w_0().h_full().child(tabs)),
                    )
                    .child(
                        div().flex_1().min_h_0().child(
                            v_resizable("request-response")
                                .on_resize(cx.listener(
                                    |this,
                                     state: &Entity<
                                        gpui_kit::component::resizable::ResizableState,
                                    >,
                                     _,
                                     cx| {
                                        if let Some(size) = state.read(cx).sizes().first() {
                                            this.doc.session["native"]["requestHeight"] =
                                                json!(f32::from(*size) / this.scale());
                                            this.changed(cx)
                                        }
                                    },
                                ))
                                .child(
                                    resizable_panel()
                                        .size(
                                            self.scaled(
                                                self.doc.session["native"]["requestHeight"]
                                                    .as_f64()
                                                    .unwrap_or(390.)
                                                    as f32,
                                            ),
                                        )
                                        .size_range(self.scaled(230.)..self.scaled(1200.))
                                        .child(request),
                                )
                                .child(
                                    resizable_panel()
                                        .size(self.scaled(350.))
                                        .size_range(self.scaled(190.)..self.scaled(1200.))
                                        .child(response),
                                ),
                        ),
                    )
                    .into_any_element()
            }
        };
        let work = if self.screen == "settings"
            || self.doc.session["native"]["sidebarClosed"]
                .as_bool()
                .unwrap_or(false)
        {
            div().size_full().child(content).into_any_element()
        } else {
            h_resizable("main-panels")
                .on_resize(cx.listener(
                    |this,
                     state: &Entity<gpui_kit::component::resizable::ResizableState>,
                     _,
                     cx| {
                        if let Some(size) = state.read(cx).sizes().first() {
                            this.doc.session["native"]["sidebarWidth"] =
                                json!(f32::from(*size) / this.scale());
                            this.changed(cx)
                        }
                    },
                ))
                .child(
                    resizable_panel()
                        .size(
                            self.scaled(
                                self.doc.session["native"]["sidebarWidth"]
                                    .as_f64()
                                    .unwrap_or(272.) as f32,
                            ),
                        )
                        .size_range(self.scaled(220.)..self.scaled(420.))
                        .child(sidebar),
                )
                .child(resizable_panel().child(content))
                .into_any_element()
        };
        let error = self.error.clone();
        let show_retry = error != MQTT_FORMAT_ERROR && !error.starts_with("发布用例");
        v_flex()
            .id("reqsucc-workbench")
            .track_focus(&self.focus)
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .font_family(cx.theme().font_family.clone())
            .typography(TextRole::Body)
            .child(
                TitleBar::new()
                    .h(self.scaled(42.))
                    .bg(cx.theme().title_bar)
                    .border_color(cx.theme().border)
                    .child(top),
            )
            .when(!error.is_empty(), |el| {
                el.child(
                    h_flex()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .child(
                            Alert::error("workbench-error", error)
                                .small()
                                .banner()
                                .flex_1()
                                .on_close(cx.listener(|this, _, _, cx| {
                                    this.error.clear();
                                    cx.notify();
                                })),
                        )
                        .when(show_retry, |el| {
                            el.child(self.button("retry", "重试保存 / 打开", Action::Retry, cx))
                        }),
                )
            })
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(rail)
                    .child(div().flex_1().min_w_0().h_full().child(work)),
            )
            .child(
                StatusBar::new()
                    .h_7()
                    .flex_shrink_0()
                    .left(
                        h_flex()
                            .gap_2()
                            .child(Icon::new(IconName::HardDrive).small())
                            .child("本地模式"),
                    )
                    .left(if self.busy {
                        "正在处理数据目录，请稍候…".to_owned()
                    } else {
                        self.status.clone()
                    })
                    .right(
                        div()
                            .max_w(self.scaled(520.))
                            .truncate()
                            .child(self.path.clone()),
                    )
                    .right("v0.4.0"),
            )
    }
}
