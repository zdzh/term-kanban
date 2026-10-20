use chrono::{DateTime, Local};

/// 分组：卡片的横向容器。归档标记只打在自己身上，不传染组内卡片。
#[derive(Debug, Clone)]
pub struct Group {
    pub id: i64,
    pub name: String,
    pub position: i64,
    pub archived_at: Option<DateTime<Local>>,
}

/// 卡片：看板上的最小可摆放单位。
#[derive(Debug, Clone)]
pub struct Card {
    pub id: i64,
    pub group_id: i64,
    pub title: String,
    pub body: String,
    pub done: bool,
    pub completed_at: Option<DateTime<Local>>,
    /// 只在未完成段里有意义；已完成段按 completed_at 倒序。
    pub position: i64,
    pub archived_at: Option<DateTime<Local>>,
}

impl Card {
    pub fn is_archived(&self) -> bool {
        self.archived_at.is_some()
    }
}

/// 子任务：轻量检查项，不能挂正文/动作，也不能嵌套。
#[derive(Debug, Clone)]
pub struct Subtask {
    pub id: i64,
    pub card_id: i64,
    pub text: String,
    pub done: bool,
    pub completed_at: Option<DateTime<Local>>,
    pub position: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    /// 在本机执行 shell 命令。
    Command,
    /// 用系统默认浏览器打开。
    Link,
}

impl ActionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ActionKind::Command => "command",
            ActionKind::Link => "link",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "command" => Some(ActionKind::Command),
            "link" => Some(ActionKind::Link),
            _ => None,
        }
    }
}

/// 动作：挂在卡片上、点击即触发的一个操作。
#[derive(Debug, Clone)]
pub struct Action {
    pub id: i64,
    pub card_id: i64,
    pub kind: ActionKind,
    pub label: String,
    /// 命令字符串或 URL。
    pub target: String,
    /// 为真时 TUI 把整个终端让给它（ssh、vim 这类交互程序）。
    pub needs_terminal: bool,
    pub position: i64,
}

impl Action {
    /// 卡片正面上显示的图标：命令用 `$`，链接用 `↗`。
    pub fn glyph(&self) -> &'static str {
        match self.kind {
            ActionKind::Command => "$",
            ActionKind::Link => "↗",
        }
    }
}

/// 一次动作执行的记账（见 docs/adr/0006-action-runs-and-log-files.md）。
#[derive(Debug, Clone)]
pub struct Run {
    pub id: i64,
    pub action_id: i64,
    pub started_at: DateTime<Local>,
    pub ended_at: Option<DateTime<Local>>,
    pub exit_code: Option<i32>,
    pub log_path: String,
}
