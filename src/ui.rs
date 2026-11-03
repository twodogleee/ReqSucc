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
#[derive(Clone)]
enum Binding {
    Request(String),
    Session(String),
    Folder(String),
    Search,
}
impl Binding {
    fn key(&self) -> String {
        match self {
            Self::Request(p) => format!("r{p}"),
            Self::Session(p) => format!("s{p}"),
            Self::Folder(p) => format!("f{p}"),
            Self::Search => "search".into(),
        }
    }
}
#[derive(Clone)]
enum Action {
    SearchFocus,
    ToggleSidebar,
    Expand(String),
    New,
    Save,
    Close(String),
    Select(String),
    Open(Value),
    Panel(String),
    Screen(String),
    Send,
    Set(Binding, Value),
    AddRow(Binding),
    RemoveRow(Binding, usize),
    Format,
    Response(String),
    Raw,
    Copy,
    Export,
    AddFolder(Option<String>),
    SelectFolder(String),
    DeleteFolder(String),
    DuplicateRequest(String),
    DeleteRequest(String),
    ConfirmDeleteRequest(String),
    MoveRequest(String, String),
    MoveFolder(String, Option<String>),
    OrderFolder(String, bool),
    OrderRequest(String, bool),
    NewEnv,
    SelectEnv(String),
    DuplicateEnv(String),
    DeleteEnv(String),
    Scope(String),
    Theme(bool),
    SettingsPage(String),
    ResetDisplay,
    ChooseStorage(String),
    SwitchStorage,
    CancelPlan,
    Retry,
    Quit,
}
struct RunState {
    id: String,
    pending: bool,
    started: Instant,
    result: Option<Result<http::Output, http::Failure>>,
}
pub struct Workbench {
    doc: Snapshot,
    ready: bool,
    path: String,
    backend: Backend,
    rx: mpsc::Receiver<Event>,
    tx: mpsc::Sender<Event>,
    runtime: Arc<tokio::runtime::Runtime>,
    http: Arc<http::HttpService>,
    generation: u64,
    saved: u64,
    queued: u64,
    changed: Instant,
    status: String,
    error: String,
    busy: bool,
    fields: HashMap<String, Entity<InputState>>,
    subscriptions: Vec<Subscription>,
    body: Option<Entity<EditorState>>,
    body_key: String,
    response_editor: Option<Entity<EditorState>>,
    response_key: String,
    runs: HashMap<String, RunState>,
    screen: String,
    settings_page: String,
    folder: String,
    scope: String,
    response_tab: String,
    raw: bool,
    search: String,
    plan: Option<(String, StoragePlan)>,
    focus: FocusHandle,
    tab_scroll: ScrollHandle,
    visible_tab: String,
}
impl Workbench {
    fn scale(&self) -> f32 {
        self.doc.session["native"]["scale"]
            .as_f64()
            .unwrap_or(100.)
            .clamp(90., 125.) as f32
            / 100.
    }
    fn scaled(&self, value: f32) -> Pixels {
        px(value * self.scale())
    }
    fn display_flag(&self, key: &str) -> bool {
        self.doc.session["native"][key].as_bool().unwrap_or(true)
    }

    pub fn register(view: &Entity<Self>, target: AnyWindowHandle, cx: &mut App) {
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
    fn icon_button(
        &self,
        id: impl Into<ElementId>,
        label: &str,
        icon: impl Into<Icon>,
        action: Action,
        cx: &Context<Self>,
    ) -> Button {
        Button::new(id)
            .icon(icon.into().size(self.scaled(crate::theme::ICON_SIZE)))
            .accessibility_label(label.to_owned())
            .tooltip(label.to_owned())
            .ghost()
            .small()
            .w(self.scaled(28.))
            .h(self.scaled(crate::theme::CONTROL_HEIGHT))
            .p_0()
            .on_click(cx.listener(move |this, _, w, cx| this.act(action.clone(), w, cx)))
    }
    fn method_color(method: &str) -> Hsla {
        rgb(match method {
            "GET" => 0x79d3ad,
            "POST" => 0xe2bd73,
            "PUT" => 0x8aacf2,
            "PATCH" => 0xb99ae9,
            "DELETE" => 0xee9296,
            _ => 0xaeb5bc,
        })
        .into()
    }

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
            fields: HashMap::new(),
            subscriptions: vec![],
            body: None,
            body_key: String::new(),
            response_editor: None,
            response_key: String::new(),
            runs: HashMap::new(),
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
    fn reset_fields(&mut self) {
        self.fields.clear();
        self.subscriptions.clear();
        self.body = None;
        self.body_key.clear();
        self.response_key.clear();
    }
    fn changed(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        self.changed = Instant::now();
        self.status = "正在保存草稿…".into();
        cx.notify();
    }
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
        if notify {
            cx.notify();
        }
    }
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
                if let Some(r) = model::active_mut(&mut self.doc) {
                    assign(r, p, v);
                    if p == "/url" {
                        model::url_changed(r)
                    } else if p == "/params" || p.starts_with("/params/") {
                        model::params_changed(r)
                    }
                }
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
    fn field(
        &mut self,
        b: Binding,
        label: &str,
        secret: bool,
        numeric: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Input {
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
        Input::new(&self.fields[&key])
            .aria_label(label.to_owned())
            .small()
            .h(self.scaled(crate::theme::CONTROL_HEIGHT))
            .text_size(self.scaled(12.))
            .font_weight(FontWeight::NORMAL)
            .disabled(self.busy)
    }
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
            Action::Format => Some(Icon::new(Glyph::Braces)),
            Action::Copy => Some(Icon::new(IconName::Copy)),
            Action::Export => Some(Icon::new(Glyph::Download)),
            Action::New | Action::AddRow(_) | Action::AddFolder(_) | Action::NewEnv => {
                Some(Icon::new(IconName::Plus))
            }
            _ => None,
        };
        Button::new(id)
            .label(label)
            .small()
            .h(self.scaled(crate::theme::CONTROL_HEIGHT))
            .px_2()
            .text_size(self.scaled(12.))
            .font_weight(FontWeight::NORMAL)
            .when_some(icon, |button, icon| {
                button.icon(icon.size(self.scaled(crate::theme::ICON_SIZE)))
            })
            .on_click(cx.listener(move |this, _, window, cx| this.act(action.clone(), window, cx)))
    }
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
            if more {
                "接口操作".to_owned()
            } else if choices.iter().any(|(_, a)| matches!(a, Action::Select(_))) {
                format!("全部标签（{}）", choices.len())
            } else {
                "选择请求方法".to_owned()
            }
        } else {
            label.to_string()
        };
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
            .text_size(self.scaled(12.))
            .font_weight(FontWeight::NORMAL)
            .disabled(self.busy)
            .accessibility_label(title.clone())
            .tooltip(title)
            .when(empty, |button| button.ghost().w(self.scaled(28.)).p_0())
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .gap_2()
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
            if matches!(action, Action::ConfirmDeleteRequest(_)) {
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
    fn select(
        &self,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        choices: Vec<(String, Action)>,
        cx: &Context<Self>,
    ) -> AnyElement {
        self.dropdown(id, label, choices, false, cx)
            .into_any_element()
    }
    fn choose(
        &self,
        key: &str,
        b: Binding,
        values: &[(&str, &str)],
        cx: &Context<Self>,
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
            cx,
        )
    }
    fn toggle(
        &self,
        id: impl Into<ElementId>,
        b: Binding,
        label: &str,
        cx: &Context<Self>,
    ) -> Checkbox {
        Checkbox::new(id)
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
                let r = model::request(&self.folder);
                model::open(&mut self.doc, r);
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
                if model::tabs(&self.doc)
                    .iter()
                    .any(|t| text(t, "id") == id && model::dirty(t))
                {
                    self.confirm("关闭未保存的请求？","关闭后将丢弃此标签的修改。需要保留时请先保存接口；正常退出应用会自动恢复草稿。".into(),Action::Set(Binding::Session("/__close".into()),json!(id)),window,cx);
                } else {
                    self.close_tab(&id, cx)
                }
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
                if let Binding::Session(p) = &b {
                    if p == "/__close" {
                        self.close_tab(v.as_str().unwrap_or(""), cx);
                        return;
                    }
                }
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
            Action::Send => self.send(cx),
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
                if self.runs.values().any(|r| r.pending) {
                    self.error = "请先取消或完成正在发送的请求".into();
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
                    cx.quit();
                }
            }
        }
        cx.notify();
    }
    fn close_tab(&mut self, id: &str, cx: &mut Context<Self>) {
        model::close(&mut self.doc, id);
        self.release(id);
        self.reset_fields();
        self.changed(cx);
    }
    fn release(&mut self, id: &str) {
        if let Some(run) = self.runs.remove(id) {
            let _ = self.http.cancel(&run.id);
            self.http.release(&run.id);
        }
    }
    fn send(&mut self, cx: &mut Context<Self>) {
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
    fn response(&self) -> Option<&http::Output> {
        self.runs
            .get(&text(&self.doc.session, "activeId"))
            .and_then(|r| r.result.as_ref())
            .and_then(|r| r.as_ref().ok())
    }
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
    fn choose_storage(&mut self, mode: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.runs.values().any(|r| r.pending) {
            self.error = "有请求正在发送，请先完成或取消".into();
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
            .label(label.clone())
            .on_click(cx.listener(move |this, _, window, cx| {
                this.confirm(&label, message.clone(), action.clone(), window, cx)
            }))
    }
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
        let mut list = v_flex()
            .w_full()
            .border_1()
            .border_color(cx.theme().border)
            .rounded(self.scaled(6.))
            .overflow_hidden()
            .child(
                h_flex()
                    .h(self.scaled(30.))
                    .px_2()
                    .gap_2()
                    .bg(cx.theme().sidebar)
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .text_size(self.scaled(12.))
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        div()
                            .w(self.scaled(32.))
                            .flex_shrink_0()
                            .text_center()
                            .child("启用"),
                    )
                    .child(div().flex_1().min_w_0().px_2().child("名称"))
                    .child(div().flex_1().min_w_0().px_2().child("值"))
                    .child(div().flex_1().min_w_0().px_2().child("说明"))
                    .when(variables, |row| {
                        row.child(
                            div()
                                .w(self.scaled(44.))
                                .flex_shrink_0()
                                .text_center()
                                .child("敏感"),
                        )
                    })
                    .child(div().w(self.scaled(28.)).flex_shrink_0()),
            );
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
            list = list.child(
                h_flex()
                    .min_h(self.scaled(34.))
                    .px_2()
                    .gap_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .w(self.scaled(32.))
                            .flex_shrink_0()
                            .justify_center()
                            .child(self.toggle(
                                SharedString::from(format!("{root}-{i}-enabled")),
                                make(format!("/{i}/enabled")),
                                "",
                                cx,
                            )),
                    )
                    .child(div().flex_1().min_w_0().child(name.appearance(false)))
                    .child(div().flex_1().min_w_0().child(value.appearance(false)))
                    .child(div().flex_1().min_w_0().child(desc.appearance(false)))
                    .when(variables, |el| {
                        el.child(
                            h_flex()
                                .w(self.scaled(44.))
                                .flex_shrink_0()
                                .justify_center()
                                .child(self.toggle(
                                    SharedString::from(format!("{root}-{i}-secret")),
                                    make(format!("/{i}/secret")),
                                    "",
                                    cx,
                                )),
                        )
                    })
                    .child(self.icon_button(
                        SharedString::from(format!("{root}-{i}-delete")),
                        "移除此行",
                        Icon::new(Glyph::Trash),
                        Action::RemoveRow(b.clone(), i),
                        cx,
                    )),
            );
        }
        list.child(
            h_flex().h(self.scaled(34.)).px_2().child(
                self.button(
                    SharedString::from(format!("{root}-add")),
                    "添加一行",
                    Action::AddRow(b),
                    cx,
                )
                .ghost(),
            ),
        )
        .into_any_element()
    }
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
        let mut panel = v_flex()
            .gap_4()
            .child(h_flex().gap_4().child("认证方式").child(self.choose(
                "auth-type",
                b("auth"),
                &choices,
                cx,
            )))
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("支持 {{变量名}}。认证信息仅保存在本地数据库中，请妥善保管数据目录。"),
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
            panel = panel.child(v_flex().gap_2().child(label).child(field))
        }
        if auth == "apiKey" {
            panel = panel.child(self.choose(
                "api-key-in",
                b("apiKeyIn"),
                &[("header", "放入请求头"), ("query", "放入查询参数")],
                cx,
            ))
        }
        if auth == "inherit" {
            panel = panel.child("使用当前接口所在根集合的认证。临时请求未选择集合时不附加认证。");
        }
        panel.into_any_element()
    }
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
        let mut panel =
            v_flex()
                .gap_4()
                .child(
                    div()
                        .text_color(cx.theme().muted_foreground)
                        .child(if global {
                            "所有请求默认使用这些设置，可在单个请求中覆盖。"
                        } else {
                            "未修改的选项继承全局默认设置。"
                        }),
                );
        for (key, label) in [
            ("timeoutMs", "总超时（毫秒，1–3600000）"),
            ("connectTimeoutMs", "连接超时（毫秒，1–3600000）"),
            ("maxResponseMib", "响应接收上限（MiB，1–1024）"),
        ] {
            let field = self.field(b(key), label, false, true, window, cx);
            panel = panel.child(
                h_flex()
                    .gap_4()
                    .child(div().w(self.scaled(280.)).child(label))
                    .child(div().w(self.scaled(180.)).child(field)),
            );
        }
        panel = panel
            .child(self.toggle(
                "follow-redirects",
                b("followRedirects"),
                "跟随重定向（最多 10 次）",
                cx,
            ))
            .child(self.toggle("verify-tls", b("verifyTls"), "校验 TLS 证书", cx));
        if !global {
            panel = panel.child(self.button(
                "reset-options",
                "恢复全局默认",
                Action::Set(Binding::Request("/settings".into()), json!({})),
                cx,
            ))
        }
        panel.into_any_element()
    }
    fn request_body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(r) = model::active(&self.doc).cloned() else {
            return div().into_any_element();
        };
        let kind = text(&r, "bodyType");
        let mut kinds = h_flex().gap_4();
        for (value, label) in [
            ("none", "无"),
            ("json", "JSON"),
            ("text", "文本"),
            ("xml", "XML"),
            ("form", "表单编码"),
        ] {
            kinds = kinds.child(
                gpui_kit::component::radio::Radio::new(SharedString::from(format!(
                    "body-kind-{value}"
                )))
                .label(label)
                .small()
                .checked(kind == value)
                .on_click(cx.listener(move |this, _, w, cx| {
                    this.act(
                        Action::Set(Binding::Request("/bodyType".into()), json!(value)),
                        w,
                        cx,
                    )
                })),
            );
        }
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
            panel = panel.child(
                div()
                    .p_8()
                    .text_color(cx.theme().muted_foreground)
                    .child("此请求没有请求体"),
            )
        }
        panel.into_any_element()
    }
    fn request_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(r) = model::active(&self.doc).cloned() else {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_4()
                .child(div().text_xl().child("从一个请求开始"))
                .child("创建请求或从左侧集合打开接口，所有数据保存在本地。")
                .child(
                    self.button("empty-new", "新建请求", Action::New, cx)
                        .primary(),
                )
                .into_any_element();
        };
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
        let mut nav = h_flex()
            .gap_1()
            .border_b_1()
            .border_color(cx.theme().border);
        for (key, label) in [
            ("params", "参数"),
            ("auth", "认证"),
            ("headers", "请求头"),
            ("body", "请求体"),
            ("description", "说明"),
            ("settings", "请求设置"),
        ] {
            nav = nav.child(
                self.button(
                    SharedString::from(format!("panel-{key}")),
                    label,
                    Action::Panel(key.into()),
                    cx,
                )
                .ghost()
                .rounded(self.scaled(0.))
                .h(self.scaled(30.))
                .border_b_2()
                .border_color(if panel == key {
                    cx.theme().primary
                } else {
                    cx.theme().background
                })
                .text_color(if panel == key {
                    cx.theme().foreground
                } else {
                    cx.theme().muted_foreground
                }),
            );
        }
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
                    .child("接口说明")
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
                    .child(
                        Icon::new(IconName::Globe)
                            .size(self.scaled(crate::theme::ICON_SIZE))
                            .text_color(cx.theme().primary),
                    )
                    .child(div().text_color(cx.theme().muted_foreground).child(
                        if self.doc.requests.iter().any(|v| v["id"] == r["id"]) {
                            folder_name.clone()
                        } else {
                            "临时请求".into()
                        },
                    ))
                    .child(
                        Icon::new(IconName::ChevronRight)
                            .size(self.scaled(crate::theme::ICON_SIZE))
                            .text_color(cx.theme().muted_foreground),
                    )
                    .child(div().flex_1().child(name.appearance(false)))
                    .child(self.select("save-folder", folder_name, folders, cx))
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
                                    .text_color(Self::method_color(&text(&r, "method")))
                                    .child(method.appearance(false)),
                            )
                            .child(
                                self.select(
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
                    div()
                        .text_color(rgb(0xe2bd73))
                        .child("TLS 证书校验已关闭，仅建议用于可信的本地调试服务。"),
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
    fn response_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
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
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("响应"))
                    .child(
                        div()
                            .text_sm()
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
                    v_flex()
                        .flex_1()
                        .justify_center()
                        .items_center()
                        .gap_3()
                        .text_color(cx.theme().muted_foreground)
                        .child("发送请求后，在这里查看服务器返回的内容")
                        .child("⌘ / Ctrl + Enter 发送当前请求"),
                )
                .into_any_element();
        }
        let r = self.response().unwrap().clone();
        let mut nav = h_flex().gap_1();
        for (id, label) in [
            ("body", "响应体"),
            ("headers", "响应头"),
            ("cookies", "Cookie"),
            ("request", "本次请求"),
        ] {
            nav = nav.child(
                self.button(
                    SharedString::from(format!("response-{id}")),
                    label,
                    Action::Response(id.into()),
                    cx,
                )
                .ghost()
                .selected(self.response_tab == id),
            );
        }
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
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "{} · 重定向 {} 次 · {}",
                        r.final_url, r.redirects, r.encoding
                    )),
            );
        if r.preview_limited || r.incomplete {
            panel = panel.child(div().text_color(rgb(0xe2bd73)).child(if r.incomplete {
                "已达到接收上限，响应不完整。"
            } else {
                "预览仅显示前 2 MiB，完整内容可保存为文件。"
            }));
        }
        if self.response_tab == "cookies" {
            panel = panel.child("Cookie 值默认隐藏，本阶段仅展示，不自动保存或回传。");
        }
        if self.response_tab == "request" {
            panel = panel.child("有效请求预览已隐藏认证和敏感变量；重定向后的实际请求可能不同。");
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
                .child("从左侧选择集合或新建集合")
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
            .child(div().text_xl().child("集合与文件夹"))
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
                    .child(self.select("folder-move", "移动到…", moves, cx))
                    .child(self.button(
                        "folder-up",
                        "上移",
                        Action::OrderFolder(folder.id.clone(), true),
                        cx,
                    ))
                    .child(self.button(
                        "folder-down",
                        "下移",
                        Action::OrderFolder(folder.id.clone(), false),
                        cx,
                    ))
                    .child(self.danger(
                        "folder-delete",
                        "删除目录",
                        format!(
                            "将删除“{}”及其全部子目录和接口。此操作无法撤销。",
                            folder.name
                        ),
                        Action::DeleteFolder(folder.id.clone()),
                        cx,
                    )),
            )
            .child(div().text_lg().child("集合变量"))
            .child(div().text_color(cx.theme().muted_foreground).child(
                "变量与认证属于根集合，其下的文件夹共享这些配置。优先级：环境 ＞ 集合 ＞ 全局。",
            ))
            .child(vars)
            .child(div().text_lg().child("集合认证"))
            .child(auth)
            .into_any_element()
    }
    fn environments(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let envs = self.doc.session["network"]["environments"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut nav = h_flex().gap_2().flex_wrap().child(
            self.button("globals", "全局变量", Action::Scope("globals".into()), cx)
                .selected(self.scope == "globals"),
        );
        for e in &envs {
            nav = nav.child(
                self.button(
                    SharedString::from(format!("scope-{}", text(e, "id"))),
                    text(e, "name"),
                    Action::Scope(text(e, "id")),
                    cx,
                )
                .selected(self.scope == text(e, "id")),
            );
        }
        nav = nav.child(self.button("new-env", "新建环境", Action::NewEnv, cx));
        let i = envs.iter().position(|e| text(e, "id") == self.scope);
        let mut panel = v_flex()
            .id("environment-settings")
            .size_full()
            .overflow_y_scroll()
            .p_3()
            .gap_2()
            .child(div().text_xl().child("环境与变量"))
            .child(nav)
            .child("使用 {{变量名}} 引用变量。关闭启用开关后不参与解析，敏感值在界面中隐藏。");
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
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .child(title.to_owned()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(description.to_owned()),
                    ),
            )
    }
    fn setting_row(
        &self,
        title: &str,
        description: &str,
        control: impl IntoElement,
        cx: &Context<Self>,
    ) -> Div {
        h_flex()
            .w_full()
            .justify_between()
            .gap_3()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                v_flex().flex_1().gap_1().child(title.to_owned()).child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(description.to_owned()),
                ),
            )
            .child(div().flex_shrink_0().child(control))
    }
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
    fn settings(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let page = self.settings_page.clone();
        let mut nav = v_flex()
            .w(self.scaled(156.))
            .h_full()
            .flex_shrink_0()
            .p_3()
            .gap_2()
            .bg(cx.theme().sidebar)
            .border_r_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .px_3()
                    .py_3()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("偏好设置"),
            );
        for (id, name, icon) in [
            ("display", "显示设置", IconName::Settings2),
            ("storage", "数据存储", IconName::HardDrive),
            ("network", "请求设置", IconName::Globe),
            ("about", "关于 ReqSucc", IconName::Info),
        ] {
            nav = nav.child(
                self.button(
                    SharedString::from(format!("settings-{id}")),
                    name,
                    Action::SettingsPage(id.into()),
                    cx,
                )
                .ghost()
                .icon(Icon::new(icon).size(self.scaled(crate::theme::ICON_SIZE)))
                .selected(page == id)
                .w_full()
                .h(self.scaled(30.))
                .justify_start(),
            );
        }
        let heading = match page.as_str() {
            "storage" => "数据存储",
            "network" => "请求设置",
            "about" => "关于 ReqSucc",
            _ => "显示设置",
        };
        let mut content = v_flex().w_full().max_w(self.scaled(900.)).gap_3().child(
            h_flex()
                .justify_between()
                .child(
                    v_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_size(self.scaled(18.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(heading),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("修改后自动保存，下次打开继续使用。"),
                        ),
                )
                .child(
                    self.button(
                        "settings-back",
                        "返回工作台",
                        Action::Screen("requests".into()),
                        cx,
                    )
                    .ghost()
                    .icon(
                        Icon::new(IconName::ChevronLeft).size(self.scaled(crate::theme::ICON_SIZE)),
                    ),
                ),
        );
        match page.as_str() {
            "storage" => {
                content = content.child(
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
                                        .text_sm()
                                        .child(if self.path.is_empty() {
                                            "尚未打开数据目录".into()
                                        } else {
                                            self.path.clone()
                                        }),
                                )
                                .child(
                                    h_flex()
                                        .gap_3()
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
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("迁移时保留源目录。创建和迁移的目标目录必须为空。"),
                                ),
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
                                            .gap_3()
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
                content = content.child(
                    self.setting_card("ReqSucc 0.4.0", "全中文本地接口工作台", cx)
                        .child(
                            v_flex()
                                .p_3()
                                .gap_4()
                                .child(
                                    h_flex()
                                        .gap_3()
                                        .child(img("brand/reqsucc.png").size(self.scaled(48.)))
                                        .child("请求成功，调试从容。"),
                                )
                                .child("数据保存在本机，支持手动 HTTP 调试。")
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("当前不提供脚本、断言测试或云同步。"),
                                ),
                        ),
                );
            }
            _ => {
                let light = self.doc.session["native"]["light"]
                    .as_bool()
                    .unwrap_or(false);
                let themes = h_flex()
                    .gap_2()
                    .child(
                        self.button("theme-dark", "深色", Action::Theme(false), cx)
                            .selected(!light)
                            .h(self.scaled(crate::theme::CONTROL_HEIGHT))
                            .w(self.scaled(72.)),
                    )
                    .child(
                        self.button("theme-light", "浅色", Action::Theme(true), cx)
                            .selected(light)
                            .h(self.scaled(crate::theme::CONTROL_HEIGHT))
                            .w(self.scaled(72.)),
                    );
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
                        .child(self.setting_row("界面主题", "选择适合当前光线的配色。", themes, cx))
                        .child(self.setting_row(
                            "界面缩放",
                            "调整文字、按钮和面板间距，默认 100%。",
                            scale,
                            cx,
                        ))
                        .child(self.setting_row(
                            "增强边框对比度",
                            "让输入框和面板分隔线更清晰。",
                            borders,
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
                                cx,
                            ))
                            .child(self.setting_row(
                                "显示行号",
                                "在编辑区左侧显示代码行号。",
                                lines,
                                cx,
                            ))
                            .child(self.setting_row(
                                "自动换行",
                                "长行按编辑区宽度折行，不改变实际内容。",
                                wrap,
                                cx,
                            )),
                    )
                    .child(
                        h_flex().justify_end().child(
                            self.button(
                                "reset-display",
                                "恢复默认显示设置",
                                Action::ResetDisplay,
                                cx,
                            )
                            .ghost(),
                        ),
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
                        Button::new(SharedString::from(format!("folder-{id}")))
                            .accessibility_label(folder.name.clone())
                            .ghost()
                            .small()
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
                                            .text_size(self.scaled(12.))
                                            .font_weight(FontWeight::NORMAL)
                                            .truncate()
                                            .child(folder.name.clone()),
                                    ),
                            )
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
                        || format!("{} {}", text(r, "name"), text(r, "url"))
                            .to_lowercase()
                            .contains(&query)
                })
            {
                let rid = text(r, "id");

                let method = text(r, "method");
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
                        .gap_1()
                        .h(self.scaled(32.))
                        .child(
                            Button::new(SharedString::from(format!("request-{rid}")))
                                .accessibility_label(format!("{} {}", method, text(r, "name")))
                                .ghost()
                                .small()
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
                                        .gap_2()
                                        .child(
                                            div()
                                                .w(self.scaled(42.))
                                                .flex_shrink_0()
                                                .text_left()
                                                .text_size(self.scaled(10.))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(Self::method_color(&method))
                                                .child(method),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .text_size(self.scaled(12.))
                                                .font_weight(FontWeight::NORMAL)
                                                .truncate()
                                                .child(text(r, "name")),
                                        ),
                                )
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
                            .child(div().font_weight(FontWeight::MEDIUM).child("集合"))
                            .child(
                                div()
                                    .text_xs()
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
            .child(
                v_flex()
                    .gap_1()
                    .p_3()
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded(self.scaled(6.))
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Icon::new(IconName::HardDrive)
                                    .size(self.scaled(crate::theme::ICON_SIZE)),
                            )
                            .child("接口与草稿，保存在本地"),
                    )
                    .child("保存接口后，可随时继续调试。"),
            )
            .into_any_element()
    }
    fn rail(&self, cx: &Context<Self>) -> AnyElement {
        let mut rail = v_flex()
            .w(self.scaled(48.))
            .flex_shrink_0()
            .h_full()
            .items_center()
            .py_3()
            .gap_2()
            .bg(if cx.theme().mode.is_dark() {
                rgb(0x1b1e21).into()
            } else {
                cx.theme().muted
            })
            .border_r_1()
            .border_color(cx.theme().border);
        for (id, label, icon, screen) in [
            ("nav-collections", "集合", Glyph::Layers, "requests"),
            (
                "nav-environments",
                "环境",
                Glyph::SlidersHorizontal,
                "environments",
            ),
            ("nav-history", "历史", Glyph::Clock, "history"),
        ] {
            let selected = if screen == "requests" {
                self.screen == "requests" || self.screen == "folder"
            } else {
                self.screen == screen
            };
            rail = rail.child(
                Button::new(id)
                    .accessibility_label(label)
                    .ghost()
                    .w(self.scaled(42.))
                    .h(self.scaled(42.))
                    .p_1()
                    .selected(selected)
                    .text_color(if selected {
                        cx.theme().primary
                    } else {
                        cx.theme().muted_foreground
                    })
                    .child(
                        v_flex()
                            .items_center()
                            .gap_1()
                            .child(Icon::new(icon).size(self.scaled(crate::theme::NAV_ICON_SIZE)))
                            .child(
                                div()
                                    .text_size(self.scaled(10.))
                                    .font_weight(FontWeight::NORMAL)
                                    .child(label),
                            ),
                    )
                    .disabled(screen == "history" || (!self.ready && screen == "environments"))
                    .tooltip(if screen == "history" {
                        "历史记录将在后续开放"
                    } else {
                        label
                    })
                    .on_click(cx.listener(move |this, _, w, cx| {
                        this.act(Action::Screen(screen.into()), w, cx)
                    })),
            );
        }
        rail.child(div().flex_1())
            .child(self.icon_button(
                "about",
                "关于 ReqSucc",
                Icon::new(Glyph::CircleQuestionMark),
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
            )
            .into_any_element()
    }
}
impl Render for Workbench {
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
                    .child(img("brand/reqsucc.png").size(self.scaled(28.)))
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
                        .h(self.scaled(29.))
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
                                        .text_size(self.scaled(12.))
                                        .font_weight(FontWeight::NORMAL)
                                        .child("搜索接口"),
                                )
                                .child(div().text_xs().child("⌘ K")),
                        )
                        .on_click(
                            cx.listener(|this, _, w, cx| this.act(Action::SearchFocus, w, cx)),
                        ),
                ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .size(self.scaled(5.))
                            .rounded_full()
                            .bg(cx.theme().primary),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("本地模式"),
                    ),
            )
            .child(
                div()
                    .max_w(self.scaled(180.))
                    .overflow_hidden()
                    .child(self.select("environment", environment_name, environments, cx)),
            )
            .child(self.icon_button(
                "nav-settings",
                "打开设置",
                IconName::Settings2,
                Action::Screen("settings".into()),
                cx,
            ));
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
                let mut tabs = h_flex()
                    .id("request-tabs")
                    .h(self.scaled(32.))
                    .w_full()
                    .overflow_x_scroll()
                    .track_scroll(&self.tab_scroll)
                    .gap_0();
                let mut open_tabs = Vec::new();
                for tab in model::tabs(&self.doc) {
                    let id = text(tab, "id");
                    let r = &tab["request"];
                    let method = text(r, "method");
                    let name = text(r, "name");
                    let selected = id == text(&self.doc.session, "activeId");
                    open_tabs.push((format!("{method} {name}"), Action::Select(id.clone())));
                    tabs = tabs.child(
                        h_flex()
                            .h_full()
                            .w(self.scaled(tab_width))
                            .flex_shrink_0()
                            .border_r_1()
                            .border_color(cx.theme().border)
                            .border_t_2()
                            .when(selected, |el| {
                                el.bg(cx.theme().background)
                                    .border_color(cx.theme().primary)
                            })
                            .child(
                                Button::new(SharedString::from(format!("tab-{id}")))
                                    .accessibility_label(format!("{method} {name}"))
                                    .tooltip(format!("{method} {name}\n{}", text(r, "url")))
                                    .ghost()
                                    .flex_1()
                                    .min_w_0()
                                    .h_full()
                                    .rounded(self.scaled(0.))
                                    .px_2()
                                    .overflow_hidden()
                                    .child(
                                        h_flex()
                                            .w_full()
                                            .min_w_0()
                                            .gap_1()
                                            .child(
                                                div()
                                                    .flex_shrink_0()
                                                    .text_size(self.scaled(9.))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(Self::method_color(&method))
                                                    .child(method),
                                            )
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .truncate()
                                                    .text_size(self.scaled(12.))
                                                    .font_weight(FontWeight::NORMAL)
                                                    .child(name),
                                            )
                                            .when(model::dirty(tab), |el| {
                                                el.child(
                                                    div()
                                                        .size(self.scaled(4.))
                                                        .flex_shrink_0()
                                                        .rounded_full()
                                                        .bg(rgb(0xe2bd73)),
                                                )
                                            }),
                                    )
                                    .on_click(cx.listener({
                                        let id = id.clone();
                                        move |this, _, w, cx| {
                                            this.act(Action::Select(id.clone()), w, cx)
                                        }
                                    })),
                            )
                            .child(
                                self.icon_button(
                                    SharedString::from(format!("close-{id}")),
                                    "关闭请求标签",
                                    IconName::Close,
                                    Action::Close(id),
                                    cx,
                                )
                                .xsmall()
                                .w(self.scaled(20.))
                                .h(self.scaled(24.))
                                .flex_shrink_0()
                                .mr_1(),
                            ),
                    );
                }
                let all_tabs = self.dropdown("all-tabs", "", open_tabs, false, cx);
                let request = self.request_panel(window, cx);
                let response = self.response_panel(window, cx);
                v_flex()
                    .size_full()
                    .child(
                        h_flex()
                            .h(self.scaled(34.))
                            .flex_shrink_0()
                            .bg(if cx.theme().mode.is_dark() {
                                rgb(0x1b1e21).into()
                            } else {
                                cx.theme().muted
                            })
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .child(div().flex_1().min_w_0().child(tabs))
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
                    )
                    .child(
                        v_resizable("request-response")
                            .on_resize(cx.listener(
                                |this,
                                 state: &Entity<gpui_kit::component::resizable::ResizableState>,
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
        v_flex()
            .id("reqsucc-workbench")
            .track_focus(&self.focus)
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .font_family(cx.theme().font_family.clone())
            .font_weight(FontWeight::NORMAL)
            .text_size(self.scaled(13.))
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
                        .px_4()
                        .py_2()
                        .gap_3()
                        .bg(cx.theme().muted)
                        .child(
                            Icon::new(IconName::CircleAlert)
                                .size(self.scaled(crate::theme::ICON_SIZE)),
                        )
                        .child(div().flex_1().child(error))
                        .child(self.button("retry", "重试保存 / 打开", Action::Retry, cx)),
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
                h_flex()
                    .h(self.scaled(crate::theme::CONTROL_HEIGHT))
                    .flex_shrink_0()
                    .px_3()
                    .gap_3()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        Icon::new(IconName::HardDrive).size(self.scaled(crate::theme::ICON_SIZE)),
                    )
                    .child("本地模式")
                    .child(if self.busy {
                        "正在处理数据目录，请稍候…".to_owned()
                    } else {
                        self.status.clone()
                    })
                    .child(div().flex_1())
                    .child(self.path.clone())
                    .child("v0.4.0"),
            )
    }
}
