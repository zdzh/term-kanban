use std::collections::HashMap;
use std::time::{Duration, Instant};

use kanban_core::{
    Action, ActionKind, ButtonKind, Card, CardWorkspace, Group, GroupButton, GroupHook, Relation,
    Store, Subtask,
};
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;

/// 一栏的最小宽度；放得下几栏就按栏数平分屏幕，不留空白。
pub const MIN_COL_W: u16 = 24;
const TOAST_TTL: Duration = Duration::from_secs(4);
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Board,
    Detail,
    Archive,
    /// 分组设置：仓库、按钮、钩子
    GroupSettings,
    /// 所有活工作区总览
    Workspaces,
    /// 按钮 / 钩子的表单编辑
    EntryEditor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DraftKind {
    Button,
    Hook,
}

/// 按钮/钩子的编辑草稿。字段编辑先进草稿，`s` 才落库——避免半截配置写进数据库。
pub struct EntryDraft {
    pub kind: DraftKind,
    /// 正在改已有条目时是它的 id，新建时为 None
    pub editing_id: Option<i64>,
    pub group_id: i64,
    pub label: String,
    pub button_kind: ButtonKind,
    pub template: String,
    pub needs_terminal: bool,
    pub confirm: bool,
    pub field: usize,
    pub error: Option<String>,
}

impl EntryDraft {
    pub fn fields(&self) -> Vec<(&'static str, String)> {
        match self.kind {
            DraftKind::Button => vec![
                ("标签", self.label.clone()),
                ("类型", kind_label(self.button_kind).to_string()),
                (
                    match self.button_kind {
                        ButtonKind::Workspace => "打开命令",
                        ButtonKind::Command => "命令",
                        ButtonKind::Link => "URL",
                    },
                    self.template.clone(),
                ),
                (
                    "需要终端",
                    if self.needs_terminal { "是" } else { "否" }.to_string(),
                ),
            ],
            DraftKind::Hook => vec![
                ("命令", self.template.clone()),
                (
                    "执行前提示",
                    if self.confirm { "是" } else { "否" }.to_string(),
                ),
            ],
        }
    }

    pub fn is_text_field_public(&self, idx: usize) -> bool {
        self.is_text_field(idx)
    }

    fn is_text_field(&self, idx: usize) -> bool {
        match self.kind {
            DraftKind::Button => matches!(idx, 0 | 2),
            DraftKind::Hook => idx == 0,
        }
    }
}

fn kind_label(k: ButtonKind) -> &'static str {
    match k {
        ButtonKind::Workspace => "工作区",
        ButtonKind::Command => "命令",
        ButtonKind::Link => "链接",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Subtask,
    Action,
    Relation,
    Button,
}

pub struct DetailState {
    pub card: Card,
    pub subtasks: Vec<Subtask>,
    pub actions: Vec<Action>,
    /// 关联：关系本身 + 对方的标题 + 是不是"我指向别人"
    pub relations: Vec<(Relation, String, bool)>,
    /// 所属分组的分组按钮（组内共享）
    pub buttons: Vec<GroupButton>,
    pub focus: Focus,
    pub sel_sub: usize,
    pub sel_act: usize,
    pub sel_rel: usize,
    pub sel_btn: usize,
    pub body_scroll: u16,
    pub last_run: Option<String>,
    pub workspace: Option<CardWorkspace>,
}

/// 分组设置面板的状态。
pub struct SettingsState {
    pub group_id: i64,
    pub group_name: String,
    pub repo: String,
    pub worktree_root: String,
    pub branch: String,
    pub buttons: Vec<GroupButton>,
    pub hooks: Vec<GroupHook>,
    pub focus: SettingsFocus,
    /// 0 = 仓库路径，1 = worktree 根，2 = 分支模板
    pub field: usize,
    pub sel: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsFocus {
    Fields,
    Buttons,
    Hooks,
}

/// 工作区总览。
pub struct WorkspacesState {
    pub items: Vec<(CardWorkspace, String, String, bool)>,
    pub sel: usize,
    pub offset: usize,
}

/// 完成钩子的确认框。
pub struct Confirm {
    pub title: String,
    pub card_id: i64,
    pub commands: Vec<String>,
    pub hook_ids: Vec<i64>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    Search,
    NewCard,
    EditCardTitle,
    NewGroup,
    RenameGroup,
    NewSubtask,
    NewCommand,
    NewLink,
    GroupRepo,
    GroupWorktreeRoot,
    GroupBranch,
    EntryField,
    AddRelation,
}

pub struct Prompt {
    pub kind: PromptKind,
    pub buf: String,
}

impl Prompt {
    pub fn hint(&self) -> &'static str {
        match self.kind {
            PromptKind::Search => "搜索（标题 / 正文 / 子任务）  Esc 取消",
            PromptKind::NewCard => "新卡片标题",
            PromptKind::EditCardTitle => "改标题",
            PromptKind::NewGroup => "新分组名",
            PromptKind::RenameGroup => "改分组名",
            PromptKind::NewSubtask => "新子任务",
            PromptKind::NewCommand => "命令动作  格式：标签 | shell 命令",
            PromptKind::NewLink => "链接动作  格式：标签 | URL",
            PromptKind::GroupRepo => "关联 git 仓库的绝对路径（留空清除）",
            PromptKind::GroupWorktreeRoot => "worktree 根目录模板（留空用默认）",
            PromptKind::GroupBranch => "分支名模板（留空用默认）",
            PromptKind::EntryField => "编辑字段（Enter 确认 Esc 取消）",
            PromptKind::AddRelation => "关联到哪张卡（输入编号或标题片段）",
        }
    }
}

#[derive(Clone, Copy)]
pub enum Hit {
    Card {
        rect: Rect,
        group: usize,
        card: usize,
    },
    Check {
        rect: Rect,
        group: usize,
        card: usize,
    },
    Act {
        rect: Rect,
        action_id: i64,
    },
    SubCheck {
        rect: Rect,
        subtask_id: i64,
    },
    /// 卡片正面上的分组按钮图标
    GroupBtn {
        rect: Rect,
        card_id: i64,
        button_id: i64,
    },
    Column {
        rect: Rect,
        group: usize,
    },
}

pub struct Drag {
    pub card_id: i64,
    pub moved: bool,
}

pub enum Effect {
    RunAction(i64),
    EditBody(i64),
    /// 跑一个分组按钮（工作区按钮会先确保 worktree 存在）
    RunGroupButton {
        card_id: i64,
        button_id: i64,
    },
    /// 跑完成钩子
    RunHooks {
        card_id: i64,
        hook_ids: Vec<i64>,
    },
    /// 删掉某张卡的工作区
    RemoveWorkspace(i64),
}

pub struct App {
    pub store: Store,
    pub groups: Vec<Group>,
    pub cards: Vec<Vec<Card>>,
    pub sel_g: usize,
    pub sel_c: usize,
    pub col_offset: usize,
    pub fold_done: bool,
    pub mode: Mode,
    pub prompt: Option<Prompt>,
    pub detail: Option<DetailState>,
    pub archive_groups: Vec<Group>,
    pub archive_cards: Vec<Card>,
    pub archive_sel: usize,
    /// 归档视图的滚动起点（渲染时按选中项算出来写回）。
    pub archive_offset: usize,
    /// 当前的筛选词。为 None 时显示全部卡片。
    pub filter: Option<String>,
    /// 打开搜索框之前的筛选词，Esc 时用来还原。
    filter_backup: Option<String>,
    /// 每个分组自己的纵向滚动偏移（保证选中的卡在视口里）。
    pub scroll: Vec<u16>,
    pub hits: Vec<Hit>,
    /// 卡片正面要显示动作图标，所以要一张 card_id → 动作 的缓存。
    pub card_actions: HashMap<i64, Vec<Action>>,
    /// 卡片正面要外显子任务清单，所以缓存整份列表。
    pub card_subtasks: HashMap<i64, Vec<Subtask>>,
    /// 每个分组的分组按钮与钩子（按分组下标对齐）。
    pub group_buttons: Vec<Vec<GroupButton>>,
    pub group_hooks: Vec<Vec<GroupHook>>,
    /// 卡片 → 工作区（分支名、脏状态）。
    pub card_workspaces: HashMap<i64, (String, String, bool)>,
    /// 卡片 → 关联数量，用于正面 `⤷N`。
    pub relation_counts: HashMap<i64, usize>,
    pub settings: Option<SettingsState>,
    pub workspaces: Option<WorkspacesState>,
    pub confirm: Option<Confirm>,
    pub entry: Option<EntryDraft>,
    /// 待执行的钩子（无需确认时由主循环取走执行）
    pub pending_hooks: Option<(i64, Vec<i64>)>,
    pub drag: Option<Drag>,
    /// 键盘拖拽：按 `m` 拿起的卡片，之后每次 hjkl 都是实移动。
    pub picked: Option<i64>,
    pub toast: Option<(String, Instant)>,
    pub should_quit: bool,
    last_click: Option<(Instant, i64)>,
}

impl App {
    pub fn new(store: Store) -> Self {
        let mut app = App {
            store,
            groups: Vec::new(),
            cards: Vec::new(),
            sel_g: 0,
            sel_c: 0,
            col_offset: 0,
            fold_done: false,
            mode: Mode::Board,
            prompt: None,
            detail: None,
            archive_groups: Vec::new(),
            archive_cards: Vec::new(),
            archive_sel: 0,
            archive_offset: 0,
            filter: None,
            filter_backup: None,
            scroll: Vec::new(),
            hits: Vec::new(),
            card_actions: HashMap::new(),
            card_subtasks: HashMap::new(),
            group_buttons: Vec::new(),
            group_hooks: Vec::new(),
            card_workspaces: HashMap::new(),
            relation_counts: HashMap::new(),
            settings: None,
            workspaces: None,
            confirm: None,
            entry: None,
            pending_hooks: None,
            drag: None,
            picked: None,
            toast: None,
            should_quit: false,
            last_click: None,
        };
        app.refresh();
        app
    }

    pub fn refresh(&mut self) {
        if let Ok(groups) = self.store.groups(false, false) {
            self.groups = groups;
        }
        let mut all: Vec<Vec<Card>> = self
            .groups
            .iter()
            .map(|g| self.store.cards_in_group(g.id, false).unwrap_or_default())
            .collect();

        self.group_buttons = self
            .groups
            .iter()
            .map(|g| self.store.group_buttons(g.id).unwrap_or_default())
            .collect();
        self.group_hooks = self
            .groups
            .iter()
            .map(|g| self.store.group_hooks(g.id).unwrap_or_default())
            .collect();

        self.card_actions.clear();
        self.card_subtasks.clear();
        self.card_workspaces.clear();
        self.relation_counts.clear();
        for card in all.iter().flatten() {
            if let Ok(Some(ws)) = self.store.workspace_for_card(card.id) {
                let dirty = kanban_core::git::is_dirty(&ws.path);
                self.card_workspaces
                    .insert(card.id, (ws.path.clone(), ws.branch.clone(), dirty));
            }
            if let Ok(n) = self.store.relation_count(card.id) {
                if n > 0 {
                    self.relation_counts.insert(card.id, n);
                }
            }
        }
        for card in all.iter().flatten() {
            let acts = self.store.actions(card.id).unwrap_or_default();
            if !acts.is_empty() {
                self.card_actions.insert(card.id, acts);
            }
            let subs = self.store.subtasks(card.id).unwrap_or_default();
            if !subs.is_empty() {
                self.card_subtasks.insert(card.id, subs);
            }
        }

        // 搜索：跨分组过滤，命中的分组才显示，避免留下一排空栏。
        if let Some(needle) = &self.filter {
            let needle = needle.to_lowercase();
            for cards in all.iter_mut() {
                cards.retain(|c| self.card_matches(c, &needle));
            }
            let mut groups = Vec::new();
            let mut kept = Vec::new();
            for (i, g) in self.groups.iter().enumerate() {
                if !all[i].is_empty() {
                    groups.push(g.clone());
                    kept.push(all[i].clone());
                }
            }
            self.groups = groups;
            self.cards = kept;
        } else {
            self.cards = all;
        }
        if self.scroll.len() < self.groups.len() {
            self.scroll.resize(self.groups.len(), 0);
        }
        if self.sel_g >= self.groups.len() {
            self.sel_g = self.groups.len().saturating_sub(1);
        }
        let len = self.cards.get(self.sel_g).map(|v| v.len()).unwrap_or(0);
        if self.sel_c >= len {
            self.sel_c = len.saturating_sub(1);
        }
        self.archive_groups = self.store.groups(false, true).unwrap_or_default();
        self.archive_cards = self.store.archived_cards().unwrap_or_default();
        if let Some(d) = &self.detail {
            self.reload_detail(d.card.id);
        }
    }

    fn card_matches(&self, card: &Card, needle: &str) -> bool {
        if card.title.to_lowercase().contains(needle) || card.body.to_lowercase().contains(needle) {
            return true;
        }
        self.card_subtasks
            .get(&card.id)
            .map(|subs| subs.iter().any(|s| s.text.to_lowercase().contains(needle)))
            .unwrap_or(false)
    }

    fn reload_detail(&mut self, card_id: i64) {
        let Ok(card) = self.store.card(card_id) else {
            self.detail = None;
            self.mode = Mode::Board;
            return;
        };
        let subtasks = self.store.subtasks(card_id).unwrap_or_default();
        let actions = self.store.actions(card_id).unwrap_or_default();
        let last_run = self
            .store
            .last_run_for_card(card_id)
            .ok()
            .flatten()
            .map(|r| {
                let code = r
                    .exit_code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "-".into());
                format!(
                    "上次执行 {} · 退出码 {code}",
                    r.started_at.format("%m-%d %H:%M")
                )
            });
        let relations = self
            .store
            .relations_from(card_id)
            .unwrap_or_default()
            .into_iter()
            .map(|r| {
                let title = self
                    .store
                    .card(r.to_card)
                    .map(|c| c.title)
                    .unwrap_or_else(|_| "（已删除）".into());
                (r, title, true)
            })
            .chain(
                self.store
                    .relations_to(card_id)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|r| {
                        let title = self
                            .store
                            .card(r.from_card)
                            .map(|c| c.title)
                            .unwrap_or_else(|_| "（已删除）".into());
                        (r, title, false)
                    }),
            )
            .collect::<Vec<_>>();
        let buttons = self
            .groups
            .iter()
            .find(|g| g.id == card.group_id)
            .map(|g| self.store.group_buttons(g.id).unwrap_or_default())
            .unwrap_or_default();
        let workspace = self.store.workspace_for_card(card_id).ok().flatten();
        let keep_focus = self
            .detail
            .as_ref()
            .map(|d| d.focus)
            .unwrap_or(Focus::Subtask);
        let sel_sub = self
            .detail
            .as_ref()
            .map(|d| d.sel_sub)
            .unwrap_or(0)
            .min(subtasks.len().saturating_sub(1));
        let sel_act = self
            .detail
            .as_ref()
            .map(|d| d.sel_act)
            .unwrap_or(0)
            .min(actions.len().saturating_sub(1));
        let body_scroll = self.detail.as_ref().map(|d| d.body_scroll).unwrap_or(0);
        let sel_rel = self
            .detail
            .as_ref()
            .map(|d| d.sel_rel)
            .unwrap_or(0)
            .min(relations.len().saturating_sub(1));
        let sel_btn = self
            .detail
            .as_ref()
            .map(|d| d.sel_btn)
            .unwrap_or(0)
            .min(buttons.len().saturating_sub(1));
        self.detail = Some(DetailState {
            card,
            subtasks,
            actions,
            relations,
            buttons,
            focus: keep_focus,
            sel_sub,
            sel_act,
            sel_rel,
            sel_btn,
            body_scroll,
            last_run,
            workspace,
        });
    }

    pub fn toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now()));
    }

    pub fn tick(&mut self) {
        if let Some((_, at)) = &self.toast {
            if at.elapsed() > TOAST_TTL {
                self.toast = None;
            }
        }
    }

    pub fn current_card(&self) -> Option<&Card> {
        self.cards.get(self.sel_g)?.get(self.sel_c)
    }

    fn current_card_id(&self) -> Option<i64> {
        self.current_card().map(|c| c.id)
    }

    fn selected_action_id(&self) -> Option<i64> {
        self.detail
            .as_ref()
            .and_then(|d| d.actions.get(d.sel_act))
            .map(|a| a.id)
    }

    // ---------------- 键盘 ----------------

    pub fn on_key(&mut self, key: KeyEvent) -> Option<Effect> {
        if self.confirm.is_some() {
            return self.on_confirm_key(key);
        }
        if self.prompt.is_some() {
            return self.on_prompt_key(key);
        }
        match self.mode {
            Mode::Board => self.on_board_key(key),
            Mode::Detail => self.on_detail_key(key),
            Mode::Archive => self.on_archive_key(key),
            Mode::GroupSettings => self.on_settings_key(key),
            Mode::Workspaces => self.on_workspaces_key(key),
            Mode::EntryEditor => self.on_entry_key(key),
        }
    }

    fn on_prompt_key(&mut self, key: KeyEvent) -> Option<Effect> {
        let Some(prompt) = &mut self.prompt else {
            return None;
        };
        match key.code {
            KeyCode::Esc => {
                let was_search = prompt.kind == PromptKind::Search;
                self.prompt = None;
                if was_search {
                    self.filter = self.filter_backup.take();
                    self.refresh();
                }
            }
            KeyCode::Enter => {
                let kind = prompt.kind;
                let buf = prompt.buf.clone();
                self.prompt = None;
                if kind == PromptKind::Search {
                    self.filter_backup = None;
                }
                self.commit_prompt(kind, buf);
            }
            KeyCode::Backspace => {
                prompt.buf.pop();
                let kind = prompt.kind;
                let buf = prompt.buf.clone();
                if kind == PromptKind::Search {
                    self.apply_search(buf);
                }
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                prompt.buf.push(c);
                let kind = prompt.kind;
                let buf = prompt.buf.clone();
                if kind == PromptKind::Search {
                    self.apply_search(buf);
                }
            }
            _ => {}
        }
        None
    }

    fn apply_search(&mut self, buf: String) {
        self.filter = if buf.trim().is_empty() {
            None
        } else {
            Some(buf.trim().to_string())
        };
        self.refresh();
    }

    fn commit_prompt(&mut self, kind: PromptKind, buf: String) {
        let text = buf.trim().to_string();
        match kind {
            PromptKind::Search => {}
            PromptKind::EntryField => {
                let value = buf.clone();
                self.apply_entry_field(value);
            }
            PromptKind::GroupRepo | PromptKind::GroupWorktreeRoot | PromptKind::GroupBranch => {
                let Some(st) = self.settings.as_ref() else {
                    return;
                };
                let (repo, root, branch) = match kind {
                    PromptKind::GroupRepo => {
                        (text.clone(), st.worktree_root.clone(), st.branch.clone())
                    }
                    PromptKind::GroupWorktreeRoot => {
                        (st.repo.clone(), text.clone(), st.branch.clone())
                    }
                    _ => (st.repo.clone(), st.worktree_root.clone(), text.clone()),
                };
                self.apply_settings_config(repo, root, branch);
            }
            PromptKind::AddRelation => {
                let Some(d) = &self.detail else { return };
                let from = d.card.id;
                let Some(to) = self.resolve_card(&text) else {
                    self.toast("没找到匹配的卡片");
                    return;
                };
                match self
                    .store
                    .add_relation(from, to, kanban_core::RelationKind::ParentChild)
                {
                    Ok(_) => {
                        self.toast("已添加父子关联");
                        self.refresh_detail_after();
                    }
                    Err(e) => self.toast(e.to_string()),
                }
            }
            PromptKind::NewCard => {
                if text.is_empty() {
                    return;
                }
                if let Some(g) = self.groups.get(self.sel_g) {
                    let group_id = g.id;
                    match self.store.create_card(group_id, &text) {
                        Ok(id) => {
                            self.refresh();
                            if self.cards.iter().flatten().any(|c| c.id == id) {
                                let len = self.cards.get(self.sel_g).map(|v| v.len()).unwrap_or(1);
                                self.sel_c = len.saturating_sub(1);
                            } else {
                                self.toast("已创建，但被当前筛选挡住了（Esc 清筛选）");
                            }
                        }
                        Err(e) => self.toast(e.to_string()),
                    }
                }
            }
            PromptKind::EditCardTitle => {
                if text.is_empty() {
                    return;
                }
                if let Some(id) = self.current_card_id() {
                    if let Err(e) = self.store.set_title(id, &text) {
                        self.toast(e.to_string());
                    }
                }
                self.refresh();
            }
            PromptKind::NewGroup => {
                if text.is_empty() {
                    return;
                }
                match self.store.create_group(&text) {
                    Ok(id) => {
                        self.refresh();
                        if let Some(i) = self.groups.iter().position(|g| g.id == id) {
                            self.sel_g = i;
                            self.sel_c = 0;
                        }
                    }
                    Err(e) => self.toast(e.to_string()),
                }
            }
            PromptKind::RenameGroup => {
                if text.is_empty() {
                    return;
                }
                if let Some(g) = self.groups.get(self.sel_g) {
                    let id = g.id;
                    if let Err(e) = self.store.rename_group(id, &text) {
                        self.toast(e.to_string());
                    }
                }
                self.refresh();
            }
            PromptKind::NewSubtask => {
                if text.is_empty() {
                    return;
                }
                if let Some(d) = &self.detail {
                    let card_id = d.card.id;
                    if let Err(e) = self.store.add_subtask(card_id, &text) {
                        self.toast(e.to_string());
                    }
                }
                self.refresh_detail_after();
            }
            PromptKind::NewCommand | PromptKind::NewLink => {
                if text.is_empty() {
                    return;
                }
                let (label, target) = split_action(&text);
                let kind = if matches!(kind, PromptKind::NewCommand) {
                    ActionKind::Command
                } else {
                    ActionKind::Link
                };
                // 交互式命令（ssh / vim / htop…）自动标成需要终端，其余靠 `T` 手动调。
                let needs_terminal =
                    kind == ActionKind::Command && kanban_core::exec::infer_needs_terminal(&target);
                if let Some(d) = &self.detail {
                    let card_id = d.card.id;
                    if let Err(e) =
                        self.store
                            .add_action(card_id, kind, &label, &target, needs_terminal)
                    {
                        self.toast(e.to_string());
                    }
                }
                self.refresh_detail_after();
            }
        }
    }

    fn refresh_detail_after(&mut self) {
        if let Some(id) = self.detail.as_ref().map(|d| d.card.id) {
            self.reload_detail(id);
        }
        self.refresh();
    }

    fn on_board_key(&mut self, key: KeyEvent) -> Option<Effect> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('q') if !ctrl => self.should_quit = true,
            KeyCode::Char('c') if ctrl => self.should_quit = true,
            KeyCode::Esc => {
                if self.filter.is_some() {
                    self.filter = None;
                    self.refresh();
                } else {
                    self.picked = None;
                }
            }

            KeyCode::Char('/') => {
                self.filter_backup = self.filter.clone();
                self.prompt = Some(Prompt {
                    kind: PromptKind::Search,
                    buf: self.filter.clone().unwrap_or_default(),
                });
            }

            KeyCode::Char('h') | KeyCode::Left => self.move_h(-1),
            KeyCode::Char('l') | KeyCode::Right => self.move_h(1),
            KeyCode::Char('j') | KeyCode::Down => self.move_v(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_v(-1),
            KeyCode::Char('H') => self.col_offset = self.col_offset.saturating_sub(1),
            KeyCode::Char('L') => {
                if self.col_offset + 1 < self.groups.len() {
                    self.col_offset += 1;
                }
            }
            KeyCode::Tab => self.fold_done = !self.fold_done,
            KeyCode::Enter => {
                if self.picked.is_some() {
                    // Enter 是「放下」，不是「开详情」。[`m` 拿起后 hjkl 已经在实时移动了]
                    self.picked = None;
                } else {
                    self.open_detail();
                }
            }

            KeyCode::Char('n') => {
                self.prompt = Some(Prompt {
                    kind: PromptKind::NewCard,
                    buf: String::new(),
                })
            }
            KeyCode::Char('i') => {
                if let Some(c) = self.current_card() {
                    self.prompt = Some(Prompt {
                        kind: PromptKind::EditCardTitle,
                        buf: c.title.clone(),
                    })
                }
            }
            KeyCode::Char(' ') => self.toggle_done(),
            KeyCode::Char('e') => {
                if let Some(id) = self.current_card_id() {
                    return Some(Effect::EditBody(id));
                }
            }
            KeyCode::Char('m') => {
                if let Some(id) = self.current_card_id() {
                    self.picked = if self.picked == Some(id) {
                        None
                    } else {
                        Some(id)
                    };
                }
            }
            KeyCode::Char('g') => self.open_settings(),
            KeyCode::Char('w') => return self.run_first_workspace_button(),
            KeyCode::Char('W') => self.open_workspaces(),
            KeyCode::Char('a') => self.archive_current_card(),
            KeyCode::Char('A') => self.archive_current_group(),
            KeyCode::Char('v') => {
                self.mode = Mode::Archive;
                self.archive_sel = 0;
            }
            KeyCode::Char('N') => {
                self.prompt = Some(Prompt {
                    kind: PromptKind::NewGroup,
                    buf: String::new(),
                })
            }
            KeyCode::Char('R') => {
                if let Some(g) = self.groups.get(self.sel_g) {
                    self.prompt = Some(Prompt {
                        kind: PromptKind::RenameGroup,
                        buf: g.name.clone(),
                    })
                }
            }
            KeyCode::Char('<') | KeyCode::Char(',') => self.shift_group(-1),
            KeyCode::Char('>') | KeyCode::Char('.') => self.shift_group(1),
            KeyCode::Backspace => self.delete_current_group(),
            KeyCode::Char(c @ '1'..='9') => {
                let n = c as usize - '1' as usize;
                if let Some(card) = self.current_card() {
                    let actions = self.store.actions(card.id).unwrap_or_default();
                    if let Some(a) = actions.get(n) {
                        return Some(Effect::RunAction(a.id));
                    }
                }
            }
            _ => {}
        }
        None
    }

    fn on_detail_key(&mut self, key: KeyEvent) -> Option<Effect> {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.mode = Mode::Board;
                self.detail = None;
            }
            KeyCode::Tab => {
                if let Some(d) = &mut self.detail {
                    d.focus = match d.focus {
                        Focus::Subtask => Focus::Action,
                        Focus::Action => Focus::Relation,
                        Focus::Relation => Focus::Button,
                        Focus::Button => Focus::Subtask,
                    };
                }
            }
            KeyCode::PageDown => {
                if let Some(d) = &mut self.detail {
                    d.body_scroll = d.body_scroll.saturating_add(5);
                }
            }
            KeyCode::PageUp => {
                if let Some(d) = &mut self.detail {
                    d.body_scroll = d.body_scroll.saturating_sub(5);
                }
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if let Some(d) = &mut self.detail {
                    match d.focus {
                        Focus::Subtask => {
                            if d.sel_sub + 1 < d.subtasks.len() {
                                d.sel_sub += 1;
                            }
                        }
                        Focus::Action => {
                            if d.sel_act + 1 < d.actions.len() {
                                d.sel_act += 1;
                            }
                        }
                        Focus::Relation => {
                            if d.sel_rel + 1 < d.relations.len() {
                                d.sel_rel += 1;
                            }
                        }
                        Focus::Button => {
                            if d.sel_btn + 1 < d.buttons.len() {
                                d.sel_btn += 1;
                            }
                        }
                    }
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if let Some(d) = &mut self.detail {
                    match d.focus {
                        Focus::Subtask => d.sel_sub = d.sel_sub.saturating_sub(1),
                        Focus::Action => d.sel_act = d.sel_act.saturating_sub(1),
                        Focus::Relation => d.sel_rel = d.sel_rel.saturating_sub(1),
                        Focus::Button => d.sel_btn = d.sel_btn.saturating_sub(1),
                    }
                }
            }
            KeyCode::Char('t') => {
                self.prompt = Some(Prompt {
                    kind: PromptKind::NewSubtask,
                    buf: String::new(),
                })
            }
            KeyCode::Char('c') => {
                self.prompt = Some(Prompt {
                    kind: PromptKind::NewCommand,
                    buf: String::new(),
                })
            }
            KeyCode::Char('y') => {
                self.prompt = Some(Prompt {
                    kind: PromptKind::NewLink,
                    buf: String::new(),
                })
            }
            KeyCode::Char(' ') => {
                let focus = self.detail.as_ref().map(|d| d.focus);
                match focus {
                    Some(Focus::Subtask) => self.toggle_subtask(),
                    Some(Focus::Action) => {
                        if let Some(id) = self.selected_action_id() {
                            return Some(Effect::RunAction(id));
                        }
                    }
                    Some(Focus::Relation) => return self.jump_to_selected_relation(),
                    Some(Focus::Button) => return self.run_selected_group_button(),
                    None => {}
                }
            }
            KeyCode::Char('r') => {
                self.prompt = Some(Prompt {
                    kind: PromptKind::AddRelation,
                    buf: String::new(),
                })
            }
            KeyCode::Char('J') => self.shift_in_focus(1),
            KeyCode::Char('K') => self.shift_in_focus(-1),
            KeyCode::Char('d') => {
                let focus = self.detail.as_ref().map(|d| d.focus);
                match focus {
                    Some(Focus::Subtask) => {
                        if let Some(s) =
                            self.detail.as_ref().and_then(|d| d.subtasks.get(d.sel_sub))
                        {
                            let id = s.id;
                            let _ = self.store.delete_subtask(id);
                            self.refresh_detail_after();
                        }
                    }
                    Some(Focus::Action) => {
                        if let Some(id) = self.selected_action_id() {
                            let _ = self.store.delete_action(id);
                            self.refresh_detail_after();
                        }
                    }
                    Some(Focus::Relation) => {
                        if let Some((rel, _, _)) = self
                            .detail
                            .as_ref()
                            .and_then(|d| d.relations.get(d.sel_rel))
                        {
                            let id = rel.id;
                            let _ = self.store.delete_relation(id);
                            self.refresh_detail_after();
                        }
                    }
                    Some(Focus::Button) => {}
                    None => {}
                }
            }
            KeyCode::Char('i') => {
                if let Some(c) = self.detail.as_ref().map(|d| d.card.clone()) {
                    self.prompt = Some(Prompt {
                        kind: PromptKind::EditCardTitle,
                        buf: c.title,
                    });
                }
            }
            KeyCode::Char('e') => {
                if let Some(id) = self.detail.as_ref().map(|d| d.card.id) {
                    return Some(Effect::EditBody(id));
                }
            }
            KeyCode::Char('T') => {
                if let Some(a) = self.detail.as_ref().and_then(|d| d.actions.get(d.sel_act)) {
                    if a.kind == ActionKind::Link {
                        self.toast("链接动作不需要终端");
                    } else {
                        let (id, needs) = (a.id, a.needs_terminal);
                        let _ = self.store.set_action_needs_terminal(id, !needs);
                        let msg = if needs {
                            "已改为不需要终端"
                        } else {
                            "已改为需要终端（ssh/vim 这类）"
                        };
                        self.toast(msg);
                        self.refresh_detail_after();
                    }
                }
            }
            KeyCode::Char(c @ '1'..='9') => {
                let n = c as usize - '1' as usize;
                if let Some(a) = self.detail.as_ref().and_then(|d| d.actions.get(n)) {
                    return Some(Effect::RunAction(a.id));
                }
            }
            _ => {}
        }
        None
    }

    fn on_archive_key(&mut self, key: KeyEvent) -> Option<Effect> {
        match key.code {
            KeyCode::Esc | KeyCode::Char('v') | KeyCode::Char('q') => {
                self.mode = Mode::Board;
            }
            KeyCode::Char('j') | KeyCode::Down => self.archive_move(1),
            KeyCode::Char('k') | KeyCode::Up => self.archive_move(-1),
            KeyCode::Char('u') => self.restore_archived(),
            KeyCode::Char('x') => self.hard_delete_archived(),
            _ => {}
        }
        None
    }

    // ---------------- 移动 ----------------

    fn move_h(&mut self, delta: i32) {
        if let Some(id) = self.picked {
            let target = self.sel_g as i32 + delta;
            if target >= 0 && (target as usize) < self.groups.len() {
                let g = self.groups[target as usize].id;
                let len = self
                    .cards
                    .get(target as usize)
                    .map(|c| c.iter().filter(|x| !x.done).count())
                    .unwrap_or(0);
                let _ = self.store.move_card(id, g, len);
                self.sel_g = target as usize;
                self.refresh();
                self.sel_c = self
                    .cards
                    .get(self.sel_g)
                    .map(|v| v.iter().position(|c| c.id == id).unwrap_or(0))
                    .unwrap_or(0);
            }
            return;
        }
        let target = self.sel_g as i32 + delta;
        if target >= 0 && (target as usize) < self.groups.len() {
            self.sel_g = target as usize;
            self.sel_c = 0;
        }
    }

    fn move_v(&mut self, delta: i32) {
        let len = self.cards.get(self.sel_g).map(|v| v.len()).unwrap_or(0);
        if len == 0 {
            return;
        }
        if let Some(id) = self.picked {
            let target = (self.sel_c as i32 + delta).clamp(0, len as i32 - 1) as usize;
            let target_card = self.cards[self.sel_g].get(target).cloned();
            if let Some(t) = target_card {
                if t.done {
                    return;
                }
                let g = self.groups[self.sel_g].id;
                let undone: Vec<i64> = self.cards[self.sel_g]
                    .iter()
                    .filter(|c| !c.done)
                    .map(|c| c.id)
                    .collect();
                let idx = undone
                    .iter()
                    .position(|&x| x == t.id)
                    .unwrap_or(undone.len());
                let idx = if delta > 0 {
                    (idx + 1).min(undone.len())
                } else {
                    idx
                };
                let _ = self.store.move_card(id, g, idx);
                self.refresh();
                self.sel_c = self
                    .cards
                    .get(self.sel_g)
                    .map(|v| v.iter().position(|c| c.id == id).unwrap_or(0))
                    .unwrap_or(0);
            }
            return;
        }
        let next = (self.sel_c as i32 + delta).clamp(0, len as i32 - 1);
        self.sel_c = next as usize;
    }

    // ---------------- 动作 ----------------

    fn toggle_done(&mut self) {
        let Some(c) = self.current_card() else { return };
        let (id, done) = (c.id, c.done);
        if let Err(e) = self.store.set_done(id, !done) {
            self.toast(e.to_string());
            return;
        }
        self.refresh();

        // 只有「未完成 → 完成」这一次触发钩子；取消勾选不跑反向操作
        if !done {
            self.trigger_completion_hooks(id);
        }
    }

    /// 勾上后看这个分组有没有 on_complete 钩子：要确认的先弹框，否则直接跑。
    fn trigger_completion_hooks(&mut self, card_id: i64) {
        let Ok(card) = self.store.card(card_id) else {
            return;
        };
        let Some(group) = self.store.group(card.group_id).ok() else {
            return;
        };
        let hooks: Vec<GroupHook> = self
            .store
            .group_hooks(group.id)
            .unwrap_or_default()
            .into_iter()
            .filter(|h| h.event == kanban_core::model::HOOK_ON_COMPLETE)
            .collect();
        if hooks.is_empty() {
            return;
        }

        let mut commands = Vec::new();
        for h in &hooks {
            match kanban_core::workspace::hook_command(h, &group, &card) {
                Ok(c) => commands.push(c),
                Err(e) => {
                    self.toast(e.to_string());
                    return;
                }
            }
        }

        let needs_confirm = hooks.iter().any(|h| h.confirm);
        let hook_ids: Vec<i64> = hooks.iter().map(|h| h.id).collect();

        if needs_confirm {
            self.confirm = Some(Confirm {
                title: format!("完成「{}」要执行清理命令", card.title),
                card_id,
                commands,
                hook_ids,
            });
        } else {
            self.pending_hooks = Some((card_id, hook_ids));
        }
    }

    fn archive_current_card(&mut self) {
        if let Some(id) = self.current_card_id() {
            if let Err(e) = self.store.archive_card(id) {
                self.toast(e.to_string());
            } else {
                self.toast("已归档，按 v 翻看");
            }
            self.refresh();
        }
    }

    fn archive_current_group(&mut self) {
        if let Some(g) = self.groups.get(self.sel_g) {
            let id = g.id;
            let name = g.name.clone();
            if let Err(e) = self.store.archive_group(id) {
                self.toast(e.to_string());
            } else {
                self.toast(format!("分组「{name}」已归档，按 v 翻看"));
            }
            self.refresh();
        }
    }

    fn delete_current_group(&mut self) {
        if let Some(g) = self.groups.get(self.sel_g) {
            let id = g.id;
            match self.store.delete_group(id) {
                Ok(()) => {
                    self.toast("空分组已删除");
                    self.refresh();
                }
                Err(e) => self.toast(e.to_string()),
            }
        }
    }

    fn shift_group(&mut self, delta: i64) {
        if let Some(g) = self.groups.get(self.sel_g) {
            let id = g.id;
            let _ = self.store.shift_group(id, delta);
            self.refresh();
            if let Some(i) = self.groups.iter().position(|x| x.id == id) {
                self.sel_g = i;
            }
        }
    }

    fn open_detail(&mut self) {
        if let Some(id) = self.current_card_id() {
            self.reload_detail(id);
            if self.detail.is_some() {
                self.mode = Mode::Detail;
            }
        }
    }

    /// 焦点在子任务上就排子任务，在动作上就排动作。
    fn shift_in_focus(&mut self, delta: i64) {
        let Some(d) = &self.detail else { return };
        match d.focus {
            Focus::Subtask => {
                if let Some(s) = d.subtasks.get(d.sel_sub) {
                    let _ = self.store.shift_subtask(s.id, delta);
                }
            }
            Focus::Action => {
                if let Some(a) = d.actions.get(d.sel_act) {
                    let _ = self.store.shift_action(a.id, delta);
                }
            }
            Focus::Relation | Focus::Button => return,
        }
        self.refresh_detail_after();
    }

    fn toggle_subtask(&mut self) {
        if let Some(s) = self.detail.as_ref().and_then(|d| d.subtasks.get(d.sel_sub)) {
            let (id, done) = (s.id, s.done);
            let _ = self.store.set_subtask_done(id, !done);
            self.refresh_detail_after();
        }
    }

    // ---------------- 归档视图 ----------------

    pub fn archive_total(&self) -> usize {
        self.archive_groups.len() + self.archive_cards.len()
    }

    /// 归档分组里还有多少张卡（不算它们被归档，只是给归档视图显示个数）。
    pub fn cards_for_archived_group(&self, group_id: i64) -> usize {
        self.store
            .cards_in_group(group_id, false)
            .map(|v| v.len())
            .unwrap_or(0)
    }

    fn archived_group_at(&self, idx: usize) -> Option<Group> {
        self.archive_groups.get(idx).cloned()
    }

    fn archived_card_at(&self, idx: usize) -> Option<Card> {
        self.archive_cards
            .get(idx - self.archive_groups.len())
            .cloned()
    }

    fn archive_move(&mut self, delta: i32) {
        let total = self.archive_total() as i32;
        let next = (self.archive_sel as i32 + delta).clamp(0, (total - 1).max(0));
        self.archive_sel = next as usize;
    }

    fn restore_archived(&mut self) {
        if let Some(g) = self.archived_group_at(self.archive_sel) {
            let name = g.name.clone();
            let _ = self.store.unarchive_group(g.id);
            self.toast(format!("分组「{name}」已恢复"));
        } else if let Some(c) = self.archived_card_at(self.archive_sel) {
            let title = c.title.clone();
            let _ = self.store.unarchive_card(c.id);
            self.toast(format!("「{title}」已恢复"));
        }
        self.refresh();
        let total = self.archive_total();
        if self.archive_sel >= total {
            self.archive_sel = total.saturating_sub(1);
        }
    }

    fn hard_delete_archived(&mut self) {
        if let Some(g) = self.archived_group_at(self.archive_sel) {
            match self.store.delete_group(g.id) {
                Ok(()) => self.toast("分组已彻底删除"),
                Err(e) => self.toast(e.to_string()),
            }
        } else if let Some(c) = self.archived_card_at(self.archive_sel) {
            let _ = self.store.hard_delete_card(c.id);
            self.toast("卡片已彻底删除");
        }
        self.refresh();
        let total = self.archive_total();
        if self.archive_sel >= total {
            self.archive_sel = total.saturating_sub(1);
        }
    }

    // ---------------- 鼠标 ----------------

    pub fn on_mouse(&mut self, ev: MouseEvent) -> Option<Effect> {
        match ev.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let hit = *self.hit_at(ev.column, ev.row)?;
                match hit {
                    Hit::Check { group, card, .. } => {
                        self.sel_g = group;
                        self.sel_c = card;
                        self.toggle_done();
                    }
                    Hit::Act { action_id, .. } => return Some(Effect::RunAction(action_id)),
                    Hit::GroupBtn {
                        card_id, button_id, ..
                    } => {
                        return Some(Effect::RunGroupButton { card_id, button_id });
                    }
                    Hit::SubCheck { subtask_id, .. } => {
                        if let Err(e) = self.store.toggle_subtask(subtask_id) {
                            self.toast(e.to_string());
                        }
                        self.refresh();
                    }
                    Hit::Card { group, card, .. } => {
                        let id = self.cards[group][card].id;
                        let now = Instant::now();
                        let double = self
                            .last_click
                            .map(|(t, cid)| cid == id && now.duration_since(t) < DOUBLE_CLICK)
                            .unwrap_or(false);
                        self.last_click = Some((now, id));
                        self.sel_g = group;
                        self.sel_c = card;
                        if double {
                            self.open_detail();
                            self.last_click = None;
                        } else {
                            self.drag = Some(Drag {
                                card_id: id,
                                moved: false,
                            });
                        }
                    }
                    Hit::Column { group, .. } => {
                        self.sel_g = group;
                    }
                }
                None
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(d) = &mut self.drag {
                    d.moved = true;
                }
                None
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let drag = self.drag.take();
                if let Some(d) = drag {
                    if d.moved {
                        self.commit_drag(d.card_id, ev.column, ev.row);
                    }
                }
                None
            }
            MouseEventKind::ScrollDown => {
                if self.mode == Mode::Archive {
                    self.archive_move(1);
                } else {
                    self.move_v(1);
                }
                None
            }
            MouseEventKind::ScrollUp => {
                if self.mode == Mode::Archive {
                    self.archive_move(-1);
                } else {
                    self.move_v(-1);
                }
                None
            }
            _ => None,
        }
    }

    fn hit_at(&self, x: u16, y: u16) -> Option<&Hit> {
        self.hits.iter().rev().find(|h| {
            let r = match h {
                Hit::Card { rect, .. }
                | Hit::Check { rect, .. }
                | Hit::Act { rect, .. }
                | Hit::SubCheck { rect, .. }
                | Hit::GroupBtn { rect, .. }
                | Hit::Column { rect, .. } => *rect,
            };
            x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
        })
    }

    fn commit_drag(&mut self, card_id: i64, x: u16, y: u16) {
        let Some(hit) = self.hit_at(x, y).copied() else {
            return;
        };
        // 落在动作图标上等于取消拖拽。
        let (group, target_idx) = match hit {
            Hit::Card { group, card, .. } => (group, Some(card)),
            Hit::Check { group, card, .. } => (group, Some(card)),
            Hit::Column { group, .. } => (group, None),
            // 落在图标或子任务复选框上等于取消拖拽。
            Hit::Act { .. } | Hit::SubCheck { .. } | Hit::GroupBtn { .. } => return,
        };
        // 已完成段不接受拖拽——改完成状态只能用勾选。
        if let Some(i) = target_idx {
            if self.cards[group].get(i).map(|c| c.done).unwrap_or(false) {
                return;
            }
        }
        let target_group_id = self.groups[group].id;
        let undone: Vec<i64> = self.cards[group]
            .iter()
            .filter(|c| !c.done && c.id != card_id)
            .map(|c| c.id)
            .collect();
        let idx = match target_idx {
            Some(i) => {
                let tid = self.cards[group][i].id;
                undone
                    .iter()
                    .position(|&c| c == tid)
                    .unwrap_or(undone.len())
            }
            None => undone.len(),
        };
        let _ = self.store.move_card(card_id, target_group_id, idx);
        self.refresh();
        self.sel_g = group;
        self.sel_c = self
            .cards
            .get(group)
            .map(|v| v.iter().position(|c| c.id == card_id).unwrap_or(0))
            .unwrap_or(0);
    }
}

fn split_action(text: &str) -> (String, String) {
    match text.split_once('|') {
        Some((label, target)) => {
            let label = label.trim().to_string();
            let target = target.trim().to_string();
            if label.is_empty() {
                (short(&target), target)
            } else {
                (label, target)
            }
        }
        None => (short(text.trim()), text.trim().to_string()),
    }
}

fn short(target: &str) -> String {
    let s: String = target.chars().take(24).collect();
    if target.chars().count() > 24 {
        format!("{s}…")
    } else {
        s
    }
}

impl App {
    // ---------------- 完成钩子确认框 ----------------

    fn on_confirm_key(&mut self, key: KeyEvent) -> Option<Effect> {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                if let Some(c) = self.confirm.take() {
                    self.pending_hooks = Some((c.card_id, c.hook_ids));
                }
            }
            _ => {
                self.confirm = None;
                self.toast("已取消清理命令");
            }
        }
        None
    }

    // ---------------- 分组设置 ----------------

    fn open_settings(&mut self) {
        let Some(g) = self.groups.get(self.sel_g) else {
            return;
        };
        let buttons = self.store.group_buttons(g.id).unwrap_or_default();
        let hooks = self.store.group_hooks(g.id).unwrap_or_default();
        self.settings = Some(SettingsState {
            group_id: g.id,
            group_name: g.name.clone(),
            repo: g.repo_path.clone().unwrap_or_default(),
            worktree_root: g.worktree_root.clone().unwrap_or_default(),
            branch: g.branch_template.clone().unwrap_or_default(),
            buttons,
            hooks,
            focus: SettingsFocus::Fields,
            field: 0,
            sel: 0,
        });
        self.mode = Mode::GroupSettings;
    }

    fn reload_settings(&mut self) {
        let Some(st) = &self.settings else { return };
        let (gid, focus, field, sel) = (st.group_id, st.focus, st.field, st.sel);
        let Ok(g) = self.store.group(gid) else { return };
        let buttons = self.store.group_buttons(gid).unwrap_or_default();
        let hooks = self.store.group_hooks(gid).unwrap_or_default();
        self.settings = Some(SettingsState {
            group_id: gid,
            group_name: g.name.clone(),
            repo: g.repo_path.clone().unwrap_or_default(),
            worktree_root: g.worktree_root.clone().unwrap_or_default(),
            branch: g.branch_template.clone().unwrap_or_default(),
            buttons,
            hooks,
            focus,
            field,
            sel,
        });
        self.refresh();
    }

    fn on_settings_key(&mut self, key: KeyEvent) -> Option<Effect> {
        let Some(st) = &self.settings else {
            self.mode = Mode::Board;
            return None;
        };
        let focus = st.focus;
        let len = match focus {
            SettingsFocus::Fields => 3,
            SettingsFocus::Buttons => st.buttons.len(),
            SettingsFocus::Hooks => st.hooks.len(),
        };

        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.mode = Mode::Board;
                self.settings = None;
                self.refresh();
            }
            KeyCode::Tab => {
                let next = match focus {
                    SettingsFocus::Fields => SettingsFocus::Buttons,
                    SettingsFocus::Buttons => SettingsFocus::Hooks,
                    SettingsFocus::Hooks => SettingsFocus::Fields,
                };
                if let Some(st) = &mut self.settings {
                    st.focus = next;
                    st.sel = 0;
                }
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if let Some(st) = &mut self.settings {
                    match st.focus {
                        SettingsFocus::Fields => st.field = (st.field + 1) % 3,
                        _ => {
                            if st.sel + 1 < len {
                                st.sel += 1
                            }
                        }
                    }
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if let Some(st) = &mut self.settings {
                    match st.focus {
                        SettingsFocus::Fields => st.field = (st.field + 2) % 3,
                        _ => st.sel = st.sel.saturating_sub(1),
                    }
                }
            }
            KeyCode::Enter => {
                let st = self.settings.as_ref()?;
                match focus {
                    SettingsFocus::Fields => {
                        let (kind, buf) = match st.field {
                            0 => (PromptKind::GroupRepo, st.repo.clone()),
                            1 => (PromptKind::GroupWorktreeRoot, st.worktree_root.clone()),
                            _ => (PromptKind::GroupBranch, st.branch.clone()),
                        };
                        self.prompt = Some(Prompt { kind, buf });
                    }
                    SettingsFocus::Hooks => self.open_entry_editor(None),
                    SettingsFocus::Buttons => {
                        let id = self
                            .settings
                            .as_ref()
                            .and_then(|s| s.buttons.get(s.sel))
                            .map(|b| b.id);
                        self.open_entry_editor(id);
                    }
                }
            }
            KeyCode::Char('a') => match focus {
                SettingsFocus::Buttons | SettingsFocus::Hooks => self.open_entry_editor(None),
                SettingsFocus::Fields => {}
            },
            KeyCode::Char('d') => match focus {
                SettingsFocus::Buttons => {
                    if let Some(b) = self.settings.as_ref().and_then(|s| s.buttons.get(s.sel)) {
                        let id = b.id;
                        let _ = self.store.delete_group_button(id);
                        self.reload_settings();
                    }
                }
                SettingsFocus::Hooks => {
                    if let Some(h) = self.settings.as_ref().and_then(|s| s.hooks.get(s.sel)) {
                        let id = h.id;
                        let _ = self.store.delete_group_hook(id);
                        self.reload_settings();
                    }
                }
                SettingsFocus::Fields => {}
            },
            KeyCode::Char('J') | KeyCode::Char('K') => {
                if focus == SettingsFocus::Buttons {
                    let delta = if matches!(key.code, KeyCode::Char('J')) {
                        1
                    } else {
                        -1
                    };
                    if let Some(b) = self.settings.as_ref().and_then(|s| s.buttons.get(s.sel)) {
                        let id = b.id;
                        let _ = self.store.shift_group_button(id, delta);
                        self.reload_settings();
                    }
                }
            }
            KeyCode::Char('p') => {
                if focus == SettingsFocus::Buttons {
                    return self.run_button_from_settings();
                }
            }
            KeyCode::Char('T') => {
                if focus == SettingsFocus::Buttons {
                    self.toggle_button_terminal();
                } else if focus == SettingsFocus::Hooks {
                    self.toggle_hook_confirm();
                }
            }
            _ => {}
        }
        None
    }

    fn toggle_hook_confirm(&mut self) {
        let Some(h) = self.settings.as_ref().and_then(|s| s.hooks.get(s.sel)) else {
            return;
        };
        let (id, confirm) = (h.id, h.confirm);
        let _ = self.store.delete_group_hook(id).and_then(|_| {
            self.store.add_group_hook(
                self.settings.as_ref().unwrap().group_id,
                kanban_core::model::HOOK_ON_COMPLETE,
                &h.run,
                !confirm,
            )
        });
        let msg = if confirm {
            "已改为直接执行（不提示）"
        } else {
            "已改为执行前提示"
        };
        self.toast(msg);
        self.reload_settings();
    }

    fn toggle_button_terminal(&mut self) {
        let Some(b) = self.settings.as_ref().and_then(|s| s.buttons.get(s.sel)) else {
            return;
        };
        if b.kind == ButtonKind::Link {
            self.toast("链接按钮不需要终端");
            return;
        }
        let (id, needs) = (b.id, b.needs_terminal);
        let _ = self.store.set_group_button_terminal(id, !needs);
        self.reload_settings();
    }

    fn run_button_from_settings(&mut self) -> Option<Effect> {
        let st = self.settings.as_ref()?;
        let button = st.buttons.get(st.sel)?;
        let button_id = button.id;
        // 用这个分组里第一张卡试跑
        let group_id = st.group_id;
        let card_id = self
            .cards
            .iter()
            .flatten()
            .find(|c| c.group_id == group_id)
            .map(|c| c.id)?;
        Some(Effect::RunGroupButton { card_id, button_id })
    }

    // ---------------- 工作区总览 ----------------

    fn open_workspaces(&mut self) {
        let items = self
            .store
            .all_workspaces()
            .unwrap_or_default()
            .into_iter()
            .map(|(ws, title, group)| {
                let dirty = kanban_core::git::is_dirty(&ws.path);
                (ws, title, group, dirty)
            })
            .collect();
        self.workspaces = Some(WorkspacesState {
            items,
            sel: 0,
            offset: 0,
        });
        self.mode = Mode::Workspaces;
    }

    fn on_workspaces_key(&mut self, key: KeyEvent) -> Option<Effect> {
        let Some(ws) = &self.workspaces else {
            self.mode = Mode::Board;
            return None;
        };
        let len = ws.items.len();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('W') => {
                self.mode = Mode::Board;
                self.workspaces = None;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if let Some(ws) = &mut self.workspaces {
                    if ws.sel + 1 < len {
                        ws.sel += 1;
                    }
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if let Some(ws) = &mut self.workspaces {
                    ws.sel = ws.sel.saturating_sub(1);
                }
            }
            KeyCode::Enter | KeyCode::Char('o') => {
                let card_id = self
                    .workspaces
                    .as_ref()?
                    .items
                    .get(self.workspaces.as_ref()?.sel)?
                    .0
                    .card_id;
                self.mode = Mode::Board;
                self.workspaces = None;
                return self.run_first_workspace_button_for(card_id);
            }
            KeyCode::Char('d') => {
                let card_id = self
                    .workspaces
                    .as_ref()?
                    .items
                    .get(self.workspaces.as_ref()?.sel)?
                    .0
                    .card_id;
                return Some(Effect::RemoveWorkspace(card_id));
            }
            _ => {}
        }
        None
    }

    // ---------------- 跳转 ----------------

    fn run_first_workspace_button(&mut self) -> Option<Effect> {
        let card_id = self.current_card_id()?;
        self.run_first_workspace_button_for(card_id)
    }

    fn run_first_workspace_button_for(&mut self, card_id: i64) -> Option<Effect> {
        let card = self.store.card(card_id).ok()?;
        let buttons = self.store.group_buttons(card.group_id).unwrap_or_default();
        match buttons.iter().find(|b| b.kind == ButtonKind::Workspace) {
            Some(b) => Some(Effect::RunGroupButton {
                card_id,
                button_id: b.id,
            }),
            None => {
                // 静默什么都不做最难受：明确告诉用户去哪儿配
                self.toast("这个分组还没有工作区按钮，按 g 加一个（类型选「工作区」）");
                None
            }
        }
    }

    fn run_selected_group_button(&mut self) -> Option<Effect> {
        let d = self.detail.as_ref()?;
        let button = d.buttons.get(d.sel_btn)?;
        Some(Effect::RunGroupButton {
            card_id: d.card.id,
            button_id: button.id,
        })
    }

    fn jump_to_selected_relation(&mut self) -> Option<Effect> {
        let d = self.detail.as_ref()?;
        let (rel, _, outgoing) = d.relations.get(d.sel_rel)?;
        let target = if *outgoing {
            rel.to_card
        } else {
            rel.from_card
        };
        self.jump_to_card(target);
        None
    }

    /// 跳到某张卡：先清筛选（否则目标可能被筛掉），再切分组、选中、开详情。
    pub fn jump_to_card(&mut self, card_id: i64) {
        if self.filter.is_some() {
            self.filter = None;
            self.refresh();
        }
        for (gi, cards) in self.cards.iter().enumerate() {
            if let Some(ci) = cards.iter().position(|c| c.id == card_id) {
                self.sel_g = gi;
                self.sel_c = ci;
                self.mode = Mode::Board;
                self.open_detail();
                return;
            }
        }
        // 不在当前看板（可能已归档）
        if let Ok(card) = self.store.card(card_id) {
            if card.archived_at.is_some() {
                self.toast("这张卡已归档，去归档视图看");
            } else if let Some(gid) = self.store.group(card.group_id).ok().map(|g| g.id) {
                let name = self.store.group(gid).map(|g| g.name).unwrap_or_default();
                self.toast(format!("「{}」在分组「{name}」里，先恢复分组", card.title));
            }
        }
    }

    /// 按编号或标题片段找一张卡（关联输入框用）。
    pub fn resolve_card(&self, query: &str) -> Option<i64> {
        let q = query.trim();
        if let Ok(id) = q.parse::<i64>() {
            if self.store.card(id).is_ok() {
                return Some(id);
            }
        }
        if q.is_empty() {
            return None;
        }
        let needle = q.to_lowercase();
        self.cards
            .iter()
            .flatten()
            .find(|c| c.title.to_lowercase().contains(&needle))
            .map(|c| c.id)
    }

    /// 设置面板里改完配置后统一落库（先校验）。
    fn apply_settings_config(&mut self, repo: String, root: String, branch: String) -> bool {
        let Some(st) = &self.settings else {
            return false;
        };
        let gid = st.group_id;
        if let Err(e) =
            kanban_core::workspace::validate_group_config(Some(&repo), Some(&root), Some(&branch))
        {
            self.toast(e.to_string());
            return false;
        }
        let empty = |s: &str| s.trim().is_empty();
        if let Err(e) = self.store.set_group_config(
            gid,
            if empty(&repo) { None } else { Some(&repo) },
            if empty(&root) { None } else { Some(&root) },
            if empty(&branch) { None } else { Some(&branch) },
        ) {
            self.toast(e.to_string());
            return false;
        }
        self.reload_settings();
        true
    }
}

impl App {
    // ---------------- 按钮 / 钩子的表单编辑 ----------------

    /// `editing`: Some(id) 改已有条目，None 新建。
    fn open_entry_editor(&mut self, editing: Option<i64>) {
        let Some(st) = &self.settings else { return };
        let group_id = st.group_id;
        let focus = st.focus;
        let sel = st.sel;

        let draft = match editing {
            Some(id) => {
                if let Some(b) = st.buttons.iter().find(|b| b.id == id) {
                    EntryDraft {
                        kind: DraftKind::Button,
                        editing_id: Some(id),
                        group_id,
                        label: b.label.clone(),
                        button_kind: b.kind,
                        template: b.command().to_string(),
                        needs_terminal: b.needs_terminal,
                        confirm: true,
                        field: 0,
                        error: None,
                    }
                } else if let Some(h) = st.hooks.iter().find(|h| h.id == id) {
                    EntryDraft {
                        kind: DraftKind::Hook,
                        editing_id: Some(id),
                        group_id,
                        label: String::new(),
                        button_kind: ButtonKind::Command,
                        template: h.run.clone(),
                        needs_terminal: false,
                        confirm: h.confirm,
                        field: 0,
                        error: None,
                    }
                } else {
                    return;
                }
            }
            None => EntryDraft {
                kind: if focus == SettingsFocus::Hooks {
                    DraftKind::Hook
                } else {
                    DraftKind::Button
                },
                editing_id: None,
                group_id,
                label: String::new(),
                button_kind: ButtonKind::Workspace,
                template: String::new(),
                needs_terminal: false,
                confirm: true,
                field: 0,
                error: None,
            },
        };
        let _ = sel;
        self.entry = Some(draft);
        self.mode = Mode::EntryEditor;
    }

    fn on_entry_key(&mut self, key: KeyEvent) -> Option<Effect> {
        let Some(d) = &self.entry else {
            self.mode = Mode::GroupSettings;
            return None;
        };
        let field_count = d.fields().len();

        match key.code {
            KeyCode::Esc => {
                self.entry = None;
                self.mode = Mode::GroupSettings;
            }
            KeyCode::Char('s') => self.save_entry(),
            KeyCode::Char('j') | KeyCode::Down | KeyCode::Tab => {
                if let Some(d) = &mut self.entry {
                    d.field = (d.field + 1) % field_count;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if let Some(d) = &mut self.entry {
                    d.field = (d.field + field_count - 1) % field_count;
                }
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                let Some(d) = &mut self.entry else {
                    return None;
                };
                let idx = d.field;
                if d.is_text_field(idx) {
                    let current = d.fields()[idx].1.clone();
                    self.prompt = Some(Prompt {
                        kind: PromptKind::EntryField,
                        buf: current,
                    });
                } else if d.kind == DraftKind::Button && idx == 1 {
                    // 类型：循环
                    d.button_kind = next_kind(d.button_kind, 1);
                } else {
                    // 各种布尔开关
                    if d.kind == DraftKind::Button {
                        d.needs_terminal = !d.needs_terminal;
                    } else {
                        d.confirm = !d.confirm;
                    }
                }
            }
            KeyCode::Char('h') | KeyCode::Left => {
                if let Some(d) = &mut self.entry {
                    if d.kind == DraftKind::Button && d.field == 1 {
                        d.button_kind = next_kind(d.button_kind, -1);
                    }
                }
            }
            KeyCode::Char('l') | KeyCode::Right => {
                if let Some(d) = &mut self.entry {
                    if d.kind == DraftKind::Button && d.field == 1 {
                        d.button_kind = next_kind(d.button_kind, 1);
                    }
                }
            }
            _ => {}
        }
        None
    }

    /// 把草稿写进草稿字段（由 Prompt 回来时调用）。
    fn apply_entry_field(&mut self, value: String) {
        let Some(d) = &mut self.entry else { return };
        match (d.kind, d.field) {
            (DraftKind::Button, 0) => d.label = value,
            (DraftKind::Button, 2) | (DraftKind::Hook, 0) => d.template = value,
            _ => {}
        }
        d.error = None;
    }

    fn save_entry(&mut self) {
        let Some(d) = self.entry.take() else {
            return;
        };

        // 校验：先拿假变量试渲染，把拼错的模板挡在落库之前
        let probe = kanban_core::Vars {
            card_id: 1,
            title: "probe".into(),
            slug: "probe".into(),
            group: "g".into(),
            repo: "/tmp/repo".into(),
            repo_name: "repo".into(),
            worktree: "/tmp/repo.worktrees/1-probe".into(),
            branch: "task/1-probe".into(),
        };

        let fail = |this: &mut Self, mut d: EntryDraft, msg: String| {
            d.error = Some(msg.clone());
            this.entry = Some(d);
            this.toast(msg);
        };

        if d.template.trim().is_empty() {
            return fail(self, d, "模板不能为空".into());
        }
        if d.kind == DraftKind::Button && d.label.trim().is_empty() {
            return fail(self, d, "标签不能为空".into());
        }

        let rendered = if d.kind == DraftKind::Button && d.button_kind == ButtonKind::Link {
            kanban_core::template::render_display(&d.template, &probe)
        } else {
            kanban_core::template::render_command(&d.template, &probe)
        };
        if let Err(e) = rendered {
            return fail(self, d, format!("模板有问题：{e}"));
        }

        let result = match d.kind {
            DraftKind::Button => {
                let (open_tpl, target) = if d.button_kind == ButtonKind::Workspace {
                    (d.template.as_str(), "")
                } else {
                    ("", d.template.as_str())
                };
                let needs_terminal = d.button_kind == ButtonKind::Command
                    && (d.needs_terminal || kanban_core::exec::infer_needs_terminal(&d.template));
                match d.editing_id {
                    Some(id) => self.store.update_group_button(
                        id,
                        &d.label,
                        d.button_kind,
                        open_tpl,
                        target,
                        needs_terminal,
                    ),
                    None => self
                        .store
                        .add_group_button(
                            d.group_id,
                            &d.label,
                            d.button_kind,
                            open_tpl,
                            target,
                            needs_terminal,
                        )
                        .map(|_| ()),
                }
            }
            DraftKind::Hook => match d.editing_id {
                Some(id) => self.store.update_group_hook(id, &d.template, d.confirm),
                None => self
                    .store
                    .add_group_hook(
                        d.group_id,
                        kanban_core::model::HOOK_ON_COMPLETE,
                        &d.template,
                        d.confirm,
                    )
                    .map(|_| ()),
            },
        };

        match result {
            Ok(()) => {
                self.mode = Mode::GroupSettings;
                self.reload_settings();
                self.toast(if d.editing_id.is_some() {
                    "已保存"
                } else {
                    "已添加"
                });
            }
            Err(e) => fail(self, d, e.to_string()),
        }
    }
}

fn next_kind(k: ButtonKind, delta: i32) -> ButtonKind {
    let all = [ButtonKind::Workspace, ButtonKind::Command, ButtonKind::Link];
    let idx = all.iter().position(|x| *x == k).unwrap_or(0) as i32;
    let next = (idx + delta).rem_euclid(all.len() as i32) as usize;
    all[next]
}

#[cfg(test)]
mod tests {
    use super::*;
    use kanban_core::Store;
    use ratatui::crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };

    fn app() -> App {
        App::new(Store::open_in_memory().unwrap())
    }

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn code(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    fn type_text(app: &mut App, s: &str) {
        for c in s.chars() {
            app.on_key(key(c));
        }
    }

    #[test]
    fn new_card_from_prompt() {
        let mut a = app();
        a.on_key(key('n'));
        assert!(a.prompt.is_some());
        type_text(&mut a, "买菜");
        a.on_key(code(KeyCode::Enter));
        assert!(a.prompt.is_none());
        assert_eq!(a.cards[0].len(), 1);
        assert_eq!(a.cards[0][0].title, "买菜");
        assert_eq!(a.sel_c, 0);
    }

    #[test]
    fn prompt_cancel_creates_nothing() {
        let mut a = app();
        a.on_key(key('n'));
        type_text(&mut a, "xx");
        a.on_key(code(KeyCode::Esc));
        assert_eq!(a.cards[0].len(), 0);
    }

    #[test]
    fn space_toggles_done_and_uncheck_returns_to_end() {
        let mut a = app();
        for t in ["a", "b"] {
            a.on_key(key('n'));
            type_text(&mut a, t);
            a.on_key(code(KeyCode::Enter));
        }
        a.sel_c = 0;
        a.on_key(key(' '));
        assert_eq!(a.cards[0][0].title, "b");
        assert!(a.cards[0][1].done);
        assert!(a.cards[0][1].completed_at.is_some());

        a.sel_c = 1;
        a.on_key(key(' '));
        assert!(!a.cards[0][1].done);
        assert!(a.cards[0][1].completed_at.is_none());
        assert_eq!(a.cards[0][1].title, "a");
    }

    #[test]
    fn archive_then_restore_from_archive_view() {
        let mut a = app();
        a.on_key(key('n'));
        type_text(&mut a, "归档我");
        a.on_key(code(KeyCode::Enter));
        a.on_key(key('a'));
        assert_eq!(a.cards[0].len(), 0);
        assert_eq!(a.archive_cards.len(), 1);

        a.on_key(key('v'));
        assert!(matches!(a.mode, Mode::Archive));
        a.on_key(key('u'));
        assert_eq!(a.archive_cards.len(), 0);
        a.on_key(key('v'));
        assert_eq!(a.cards[0].len(), 1);
    }

    #[test]
    fn archive_group_keeps_its_cards_untouched() {
        let mut a = app();
        a.on_key(key('n'));
        type_text(&mut a, "卡");
        a.on_key(code(KeyCode::Enter));
        let group_id = a.groups[0].id;
        a.on_key(key('A'));
        assert!(a.groups.iter().all(|g| g.id != group_id));
        assert_eq!(a.archive_groups.len(), 1);
        // 归档不传染：卡片自己没被盖章，仍在原分组里
        let cards = a.store.cards_in_group(group_id, false).unwrap();
        assert_eq!(cards.len(), 1);
        assert!(cards[0].archived_at.is_none());
    }

    #[test]
    fn keyboard_drag_moves_card_across_groups() {
        let mut a = app();
        a.on_key(key('n'));
        type_text(&mut a, "搬我");
        a.on_key(code(KeyCode::Enter));
        a.on_key(key('m'));
        assert!(a.picked.is_some());
        a.on_key(key('l'));
        assert_eq!(a.cards[0].len(), 0);
        assert_eq!(a.cards[1].len(), 1);
        a.on_key(code(KeyCode::Enter));
    }

    #[test]
    fn mouse_drag_commits_to_target_group() {
        let mut a = app();
        a.on_key(key('n'));
        type_text(&mut a, "拖我");
        a.on_key(code(KeyCode::Enter));

        a.hits.push(Hit::Column {
            rect: Rect::new(0, 1, 30, 20),
            group: 0,
        });
        a.hits.push(Hit::Column {
            rect: Rect::new(30, 1, 30, 20),
            group: 1,
        });
        a.hits.push(Hit::Card {
            rect: Rect::new(1, 2, 28, 2),
            group: 0,
            card: 0,
        });
        a.hits.push(Hit::Column {
            rect: Rect::new(30, 1, 30, 20),
            group: 1,
        });

        let m = |kind, column, row| MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };
        a.on_mouse(m(MouseEventKind::Down(MouseButton::Left), 3, 2));
        a.on_mouse(m(MouseEventKind::Drag(MouseButton::Left), 40, 10));
        a.on_mouse(m(MouseEventKind::Up(MouseButton::Left), 40, 10));

        assert_eq!(a.cards[0].len(), 0);
        assert_eq!(a.cards[1].len(), 1);
        assert!(a.picked.is_none());
    }

    #[test]
    fn detail_subtask_flow() {
        let mut a = app();
        a.on_key(key('n'));
        type_text(&mut a, "带子任务");
        a.on_key(code(KeyCode::Enter));
        a.on_key(code(KeyCode::Enter));
        assert!(matches!(a.mode, Mode::Detail));

        a.on_key(key('t'));
        type_text(&mut a, "第一步");
        a.on_key(code(KeyCode::Enter));
        let d = a.detail.as_ref().unwrap();
        assert_eq!(d.subtasks.len(), 1);
        assert!(!d.subtasks[0].done);

        a.on_key(key(' '));
        let d = a.detail.as_ref().unwrap();
        assert!(d.subtasks[0].done);
        assert!(d.subtasks[0].completed_at.is_some());
        // 父卡不被自动勾上
        assert!(!a.detail.as_ref().unwrap().card.done);
    }
}

#[cfg(test)]
mod feature_tests {
    use super::*;
    use kanban_core::Store;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn app() -> App {
        App::new(Store::open_in_memory().unwrap())
    }

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn code(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    fn type_text(app: &mut App, s: &str) {
        for c in s.chars() {
            app.on_key(key(c));
        }
    }

    fn add_card(app: &mut App, title: &str) {
        app.on_key(key('n'));
        type_text(app, title);
        app.on_key(code(KeyCode::Enter));
    }

    fn total_cards(app: &App) -> usize {
        app.cards.iter().map(|v| v.len()).sum()
    }

    #[test]
    fn search_filters_by_title_and_restores_on_esc() {
        let mut a = app();
        add_card(&mut a, "写周报");
        add_card(&mut a, "买菜");
        assert_eq!(total_cards(&a), 2);

        a.on_key(key('/'));
        type_text(&mut a, "周报");
        assert_eq!(total_cards(&a), 1);
        assert_eq!(a.cards[0][0].title, "写周报");

        // Esc 取消搜索，卡片都回来
        a.on_key(code(KeyCode::Esc));
        assert!(a.filter.is_none());
        assert_eq!(total_cards(&a), 2);
    }

    #[test]
    fn search_keeps_filter_on_enter_and_board_esc_clears_it() {
        let mut a = app();
        add_card(&mut a, "写周报");
        add_card(&mut a, "买菜");

        a.on_key(key('/'));
        type_text(&mut a, "菜");
        a.on_key(code(KeyCode::Enter));
        assert!(a.prompt.is_none());
        assert_eq!(total_cards(&a), 1);

        a.on_key(code(KeyCode::Esc));
        assert!(a.filter.is_none());
        assert_eq!(total_cards(&a), 2);
    }

    #[test]
    fn search_matches_body_and_subtasks() {
        let mut a = app();
        add_card(&mut a, "卡片甲");
        let id = a.cards[0][0].id;
        a.store.set_body(id, "里面提到了 nightly 构建").unwrap();
        a.store.add_subtask(id, "补 metrics 面板").unwrap();
        add_card(&mut a, "卡片乙");
        a.refresh();

        a.on_key(key('/'));
        type_text(&mut a, "nightly");
        assert_eq!(total_cards(&a), 1);
        assert_eq!(a.cards[0][0].title, "卡片甲");

        a.on_key(code(KeyCode::Esc));
        a.on_key(key('/'));
        type_text(&mut a, "metrics");
        assert_eq!(total_cards(&a), 1);
        assert_eq!(a.cards[0][0].title, "卡片甲");
    }

    #[test]
    fn search_hides_groups_without_matches() {
        let mut a = app();
        add_card(&mut a, "甲");
        a.on_key(key('l')); // 去第二个分组
        add_card(&mut a, "乙");
        assert_eq!(a.groups.len(), 3);

        a.on_key(key('/'));
        type_text(&mut a, "甲");
        assert_eq!(a.groups.len(), 1);
        assert_eq!(a.groups[0].name, "待办");
    }

    #[test]
    fn new_card_hidden_by_filter_says_so() {
        let mut a = app();
        add_card(&mut a, "写周报");
        a.on_key(key('/'));
        type_text(&mut a, "周报");
        a.on_key(code(KeyCode::Enter));
        assert_eq!(total_cards(&a), 1);

        // 新建一张不匹配筛选的卡：它不会出现，但要有提示
        add_card(&mut a, "买菜");
        assert_eq!(total_cards(&a), 1);
        let msg = a.toast.as_ref().map(|(m, _)| m.clone()).unwrap_or_default();
        assert!(msg.contains("筛选"), "toast = {msg:?}");
    }

    #[test]
    fn archive_selection_cannot_run_off_the_list() {
        let mut a = app();
        add_card(&mut a, "甲");
        let id = a.cards[0][0].id;
        a.store.archive_card(id).unwrap();
        a.refresh();

        a.on_key(key('v'));
        a.on_key(key('j'));
        a.on_key(key('j'));
        assert_eq!(a.archive_sel, 0, "只有一条时不能越界");
        a.archive_move(-5);
        assert_eq!(a.archive_sel, 0);
        a.archive_move(5);
        assert_eq!(a.archive_sel, 0);
    }

    #[test]
    fn interactive_command_is_marked_needs_terminal() {
        let mut a = app();
        add_card(&mut a, "连服务器");
        let id = a.cards[0][0].id;
        a.on_key(code(KeyCode::Enter));

        a.on_key(key('c'));
        type_text(&mut a, "连跳板 | ssh jump-01");
        a.on_key(code(KeyCode::Enter));
        let acts = a.store.actions(id).unwrap();
        assert_eq!(acts.len(), 1);
        assert!(acts[0].needs_terminal, "ssh 应自动标成需要终端");

        // 普通命令不该被标
        a.on_key(key('c'));
        type_text(&mut a, "跑测试 | cargo test");
        a.on_key(code(KeyCode::Enter));
        let acts = a.store.actions(id).unwrap();
        assert!(!acts[1].needs_terminal);
    }

    #[test]
    fn actions_can_be_reordered_in_detail() {
        let mut a = app();
        add_card(&mut a, "带动作");
        let id = a.cards[0][0].id;
        let first = a
            .store
            .add_action(id, ActionKind::Command, "第一条", "echo 1", false)
            .unwrap();
        a.store
            .add_action(id, ActionKind::Command, "第二条", "echo 2", false)
            .unwrap();
        a.refresh();

        // 进详情，切到动作焦点
        a.on_key(code(KeyCode::Enter));
        a.on_key(code(KeyCode::Tab));
        let d = a.detail.as_ref().unwrap();
        assert_eq!(d.focus, Focus::Action);
        assert_eq!(d.actions[0].id, first);

        // 已经把第一条挪到下面，光标还在第 0 行时按 K 是空操作
        a.on_key(key('J'));
        let d = a.detail.as_ref().unwrap();
        assert_eq!(d.actions[1].id, first);

        // 跟到它身上再往回收
        a.on_key(key('j'));
        a.on_key(key('K'));
        let d = a.detail.as_ref().unwrap();
        assert_eq!(d.actions[0].id, first);
        assert_eq!(d.actions[1].id, first + 1);
    }
}
