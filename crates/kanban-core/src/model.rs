use chrono::{DateTime, Local};

/// 分组：卡片的横向容器。归档标记只打在自己身上，不传染组内卡片。
///
/// 三个配置字段让分组从"纯容器"升级成"带环境的一列"：
/// 组内所有卡片共享同一个仓库与同一套按钮。
#[derive(Debug, Clone)]
pub struct Group {
    pub id: i64,
    pub name: String,
    pub position: i64,
    pub archived_at: Option<DateTime<Local>>,
    /// 关联的本地 git 仓库（绝对路径，`~` 已展开）。
    pub repo_path: Option<String>,
    /// worktree 根目录模板，默认 `{repo}/../{repo_name}.worktrees`。
    pub worktree_root: Option<String>,
    /// 分支名模板，默认 `task/{card_id}-{slug}`。
    pub branch_template: Option<String>,
}

impl Group {
    /// 是否配好了"能开工作区"的最小条件。
    pub fn has_repo(&self) -> bool {
        self.repo_path
            .as_deref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
    }
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
    /// 卡片动作触发时才有；分组按钮与钩子触发的执行没有 action。
    pub action_id: Option<i64>,
    /// `action` / `button` / `hook`
    pub source: String,
    pub command: String,
    pub started_at: DateTime<Local>,
    pub ended_at: Option<DateTime<Local>>,
    pub exit_code: Option<i32>,
    pub log_path: String,
}

/// 分组按钮：组级预设，组内所有卡片自动获得，不复制到卡片上。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    /// 内置语义：确保 worktree 存在，再用 `open` 模板打开它。
    Workspace,
    /// 直接执行一条命令。
    Command,
    /// 用系统浏览器打开。
    Link,
}

impl ButtonKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ButtonKind::Workspace => "workspace",
            ButtonKind::Command => "command",
            ButtonKind::Link => "link",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "workspace" => Some(ButtonKind::Workspace),
            "command" => Some(ButtonKind::Command),
            "link" => Some(ButtonKind::Link),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct GroupButton {
    pub id: i64,
    pub group_id: i64,
    pub label: String,
    pub kind: ButtonKind,
    /// `kind = workspace` 时用它打开工作区，例如 `cmux {worktree}`。
    pub open_template: String,
    /// `kind = command/link` 时的命令或 URL。
    pub target: String,
    pub needs_terminal: bool,
    pub position: i64,
}

impl GroupButton {
    /// 真正要执行的东西：工作区按钮用 `open_template`，其余用 `target`。
    pub fn command(&self) -> &str {
        match self.kind {
            ButtonKind::Workspace => &self.open_template,
            _ => &self.target,
        }
    }

    pub fn glyph(&self) -> &'static str {
        match self.kind {
            ButtonKind::Workspace => "⎇",
            ButtonKind::Command => "$",
            ButtonKind::Link => "↗",
        }
    }
}

/// 生命周期钩子。目前只有"卡片被勾成完成"这一个事件。
#[derive(Debug, Clone)]
pub struct GroupHook {
    pub id: i64,
    pub group_id: i64,
    pub event: String,
    pub run: String,
    /// 执行前是否要人确认（默认要）。
    pub confirm: bool,
    pub position: i64,
}

pub const HOOK_ON_COMPLETE: &str = "on_complete";

/// 卡片之间的关系。目前只有父子。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationKind {
    /// from 是父卡，to 是子卡。
    ParentChild,
}

impl RelationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RelationKind::ParentChild => "parent_child",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "parent_child" => Some(RelationKind::ParentChild),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            RelationKind::ParentChild => "父子",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Relation {
    pub id: i64,
    pub from_card: i64,
    pub to_card: i64,
    pub kind: RelationKind,
    pub created_at: DateTime<Local>,
}

/// 一张卡对应的工作区（git worktree）。
#[derive(Debug, Clone)]
pub struct CardWorkspace {
    pub card_id: i64,
    pub repo_path: String,
    pub path: String,
    pub branch: String,
    pub created_at: DateTime<Local>,
    pub last_opened_at: Option<DateTime<Local>>,
}
