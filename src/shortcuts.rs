//! 应用操作快捷键的默认值、校验、持久化配置与动态注册；保留组件的标准编辑绑定。
//!
//! 作者：Andersen

use gpui_kit::{Action, App, AsKeystroke, KeyBinding, Keystroke, Menu, MenuItem, OperatingSystem};
use serde_json::{Value, json};

/// 可由用户配置的应用操作，不包含输入框和编辑器的标准编辑操作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shortcut {
    /// 将焦点移至接口搜索框。
    Search,
    /// 新建空白请求标签。
    New,
    /// 保存当前接口。
    Save,
    /// 发送或取消 HTTP 请求，连接或断开消息协议。
    Send,
    /// 关闭当前请求标签。
    Close,
    /// 打开应用设置。
    Settings,
    /// 刷盘并退出应用。
    Quit,
}

/// 设置页展示及动态注册的固定操作顺序。
pub const ALL: [Shortcut; 7] = [
    Shortcut::Search,
    Shortcut::New,
    Shortcut::Save,
    Shortcut::Send,
    Shortcut::Close,
    Shortcut::Settings,
    Shortcut::Quit,
];

impl Shortcut {
    /// 返回稳定配置键，避免显示文案变化影响已保存的绑定。
    pub fn id(self) -> &'static str {
        match self {
            Self::Search => "search",
            Self::New => "new",
            Self::Save => "save",
            Self::Send => "send",
            Self::Close => "close",
            Self::Settings => "settings",
            Self::Quit => "quit",
        }
    }

    /// 返回设置页和冲突提示使用的中文名称。
    pub fn label(self) -> &'static str {
        match self {
            Self::Search => "搜索接口",
            Self::New => "新建请求",
            Self::Save => "保存接口",
            Self::Send => "发送 / 取消 / 连接",
            Self::Close => "关闭当前标签",
            Self::Settings => "打开设置",
            Self::Quit => "退出应用",
        }
    }

    /// 根据当前平台生成默认组合键，macOS 使用 Command，其他平台使用 Control。
    pub fn default_key(self, os: OperatingSystem) -> String {
        let modifier = if os == OperatingSystem::Mac {
            "cmd"
        } else {
            "ctrl"
        };
        let key = match self {
            Self::Search => "k",
            Self::New => "n",
            Self::Save => "s",
            Self::Send => "enter",
            Self::Close => "w",
            Self::Settings => ",",
            Self::Quit => "q",
        };
        format!("{modifier}-{key}")
    }

    /// 构造工作台已有动作，保证菜单、按钮和快捷键共用同一业务处理路径。
    fn action(self) -> Box<dyn Action> {
        use crate::ui;
        match self {
            Self::Search => Box::new(ui::SearchRequests),
            Self::New => Box::new(ui::NewRequest),
            Self::Save => Box::new(ui::SaveRequest),
            Self::Send => Box::new(ui::SendRequest),
            Self::Close => Box::new(ui::CloseTab),
            Self::Settings => Box::new(ui::Settings),
            Self::Quit => Box::new(ui::Quit),
        }
    }
}

/// 读取当前绑定；缺失或非字符串值继承默认，空字符串表示用户明确清除绑定。
pub fn key(session: &Value, shortcut: Shortcut, os: OperatingSystem) -> String {
    session["native"]["shortcuts"][shortcut.id()]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| shortcut.default_key(os))
}

/// 返回界面组合键文本；清除后为空，调用方可按场景显示“未设置”或隐藏提示。
pub fn display(session: &Value, shortcut: Shortcut, os: OperatingSystem) -> String {
    let raw = key(session, shortcut, os);
    if raw.is_empty() {
        return raw;
    }
    if validate(session, shortcut, &raw, &[], os).is_err() {
        return "无效配置".into();
    }
    Keystroke::parse_with_os(&raw, os)
        .map(|stroke| gpui_kit::component::kbd::Kbd::format(&stroke))
        .unwrap_or_else(|_| "无效配置".into())
}

/// 写入单项覆盖值；空字符串保留为显式禁用，不删除该键以免重启恢复默认。
pub fn set(session: &mut Value, shortcut: Shortcut, value: &str) {
    if !session["native"].is_object() {
        session["native"] = json!({});
    }
    if !session["native"]["shortcuts"].is_object() {
        session["native"]["shortcuts"] = json!({});
    }
    session["native"]["shortcuts"][shortcut.id()] = json!(value);
}

/// 校验并规范化一次组合键，拒绝普通打字、重复绑定及组件已占用的编辑和导航快捷键。
pub fn validate(
    session: &Value,
    shortcut: Shortcut,
    raw: &str,
    standard: &[KeyBinding],
    os: OperatingSystem,
) -> Result<String, String> {
    if raw.is_empty() {
        return Ok(String::new());
    }
    if raw.split_whitespace().count() != 1 {
        return Err("请设置单个组合键，不支持连续按键序列。".into());
    }
    let stroke = Keystroke::parse_with_os(raw, os)
        .map_err(|_| "无法识别此组合键，请重新录入。".to_owned())?;
    if matches!(
        stroke.key.as_str(),
        "shift" | "control" | "alt" | "platform" | "function"
    ) {
        return Err("请同时按下修饰键和一个普通键。".into());
    }
    let function_key = stroke
        .key
        .strip_prefix('f')
        .and_then(|n| n.parse::<u8>().ok())
        .is_some_and(|n| (1..=20).contains(&n));
    if !stroke.modifiers.control && !stroke.modifiers.platform && !function_key {
        return Err("请使用包含 Command / Ctrl 的组合键或 F1–F20，避免影响正常输入。".into());
    }
    let normalized = stroke.unparse(os);
    for other in ALL {
        if other != shortcut
            && Keystroke::parse_with_os(&key(session, other, os), os)
                .is_ok_and(|other| other.modifiers == stroke.modifiers && other.key == stroke.key)
        {
            return Err(format!(
                "此快捷键已用于“{}”，请先清除该绑定或选择其他组合键。",
                other.label()
            ));
        }
    }
    // 默认应用组合键沿用既有优先级；其他组合键不可覆盖组件的编辑、选择或导航行为。
    let app_default = ALL.iter().any(|item| item.default_key(os) == normalized);
    if !app_default
        && standard.iter().any(|binding| {
            binding.keystrokes().first().is_some_and(|key| {
                let key = key.as_keystroke();
                key.modifiers == stroke.modifiers && key.key == stroke.key
            })
        })
    {
        return Err("此组合键已用于标准编辑或界面导航，请选择其他组合键。".into());
    }
    Ok(normalized)
}

/// 重建应用绑定并恢复完整组件绑定，移除旧组合键；无效的外部配置停用并返回中文提示。
pub fn apply(session: &Value, standard: &[KeyBinding], cx: &mut App) -> Vec<String> {
    let os = cx.platform_info().os;
    let mut bindings = standard.to_vec();
    let mut errors = Vec::new();
    for shortcut in ALL {
        let raw = key(session, shortcut, os);
        match validate(session, shortcut, &raw, standard, os) {
            Ok(key) if key.is_empty() => {}
            Ok(key) => match KeyBinding::load(&key, shortcut.action(), None, false, None, cx) {
                Ok(binding) => bindings.push(binding),
                Err(_) => errors.push(format!("{}：无法加载组合键", shortcut.label())),
            },
            Err(error) => errors.push(format!("{}：{error}", shortcut.label())),
        }
    }
    cx.clear_key_bindings();
    cx.bind_keys(bindings);
    refresh_menus(cx);
    errors
}

/// 在组合键变化后重建原生菜单，使菜单提示和系统菜单快捷键同步更新。
fn refresh_menus(cx: &App) {
    use crate::ui;
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
}

/// 应用快捷键配置和冲突规则的回归检查，不启动原生窗口。
#[cfg(test)]
mod tests {
    use super::*;

    /// 验证旧会话继承各平台默认值，清除单项后其他操作和显示偏好保持原值。
    #[test]
    fn defaults_and_explicit_clear_are_distinct() {
        let mut session = json!({"native":{"scale":110}});
        assert_eq!(key(&session, Shortcut::New, OperatingSystem::Mac), "cmd-n");
        assert_eq!(
            key(&session, Shortcut::New, OperatingSystem::Linux),
            "ctrl-n"
        );
        set(&mut session, Shortcut::New, "");
        assert_eq!(key(&session, Shortcut::New, OperatingSystem::Mac), "");
        assert_eq!(key(&session, Shortcut::Save, OperatingSystem::Mac), "cmd-s");
        assert_eq!(session["native"]["scale"], 110);
        session["native"]["shortcuts"] = json!({});
        assert_eq!(key(&session, Shortcut::New, OperatingSystem::Mac), "cmd-n");
    }

    /// 验证重复快捷键被拒绝，清除原占用项后可重新分配，修饰键顺序统一规范化。
    #[test]
    fn conflicts_are_rejected_until_previous_binding_is_cleared() {
        let mut session = json!({});
        let os = OperatingSystem::Mac;
        assert!(
            validate(&session, Shortcut::New, "cmd-s", &[], os)
                .unwrap_err()
                .contains("保存接口")
        );
        set(&mut session, Shortcut::Save, "");
        assert_eq!(
            validate(&session, Shortcut::New, "cmd-s", &[], os).unwrap(),
            "cmd-s"
        );
        let normalized = validate(&session, Shortcut::New, "shift-cmd-n", &[], os).unwrap();
        set(&mut session, Shortcut::New, &normalized);
        assert!(validate(&session, Shortcut::Search, "cmd-shift-n", &[], os).is_err());
        assert!(validate(&session, Shortcut::New, &normalized, &[], os).is_ok());
    }

    /// 验证普通输入、单修饰键和多段组合被拒绝，功能键与明确清除操作可用。
    #[test]
    fn recording_rejects_typing_and_accepts_function_keys() {
        for raw in [
            "n",
            "shift-n",
            "alt-n",
            "cmd",
            "cmd-shift",
            "cmd-k cmd-n",
            "escape",
            "f21",
        ] {
            assert!(
                validate(&json!({}), Shortcut::New, raw, &[], OperatingSystem::Mac).is_err(),
                "{raw}"
            );
        }
        for raw in ["f5", "cmd-shift-n", "ctrl-alt-n", ""] {
            assert!(
                validate(&json!({}), Shortcut::New, raw, &[], OperatingSystem::Mac).is_ok(),
                "{raw}"
            );
        }
    }

    /// 验证组件编辑和导航快捷键不能被覆盖，已有应用默认组合键保持兼容。
    #[test]
    fn standard_editor_bindings_are_reserved() {
        use gpui_kit::component::input::Copy;
        let standard = vec![
            KeyBinding::new("cmd-c", Copy, Some("Input")),
            KeyBinding::new("cmd-enter", Copy, Some("List")),
        ];
        let session = json!({});
        assert!(
            validate(
                &session,
                Shortcut::New,
                "cmd-c",
                &standard,
                OperatingSystem::Mac
            )
            .unwrap_err()
            .contains("标准编辑")
        );
        assert!(
            validate(
                &session,
                Shortcut::Send,
                "cmd-enter",
                &standard,
                OperatingSystem::Mac
            )
            .is_ok()
        );
        assert_eq!(standard.len(), 2);
    }

    /// 验证配置展示处理无效值，并且全部清除经过 JSON 往返后不会恢复默认快捷键。
    #[test]
    fn cleared_bindings_survive_json_roundtrip_and_hide_hints() {
        let mut session = json!({"native":{"shortcuts":{"search":false}}});
        assert_eq!(
            key(&session, Shortcut::Search, OperatingSystem::Mac),
            "cmd-k"
        );
        set(&mut session, Shortcut::Search, "cmd");
        assert_eq!(
            display(&session, Shortcut::Search, OperatingSystem::Mac),
            "无效配置"
        );
        for shortcut in ALL {
            set(&mut session, shortcut, "");
        }
        let restored = serde_json::from_str(&session.to_string()).unwrap();
        for shortcut in ALL {
            assert_eq!(key(&restored, shortcut, OperatingSystem::Mac), "");
            assert_eq!(display(&restored, shortcut, OperatingSystem::Mac), "");
        }
    }
}
