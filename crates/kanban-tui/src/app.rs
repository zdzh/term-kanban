use std::collections::HashMap;
use std::time::{Duration, Instant};

use kanban_core::{Action, ActionKind, Card, Group, Store, Subtask};
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;

/// 一栏的最小宽度；放得下几栏就按栏数平分屏幕，不留空白。
pub const MIN_COL_W: u16 = 24;
const TOAST_TTL: Duration = Duration::from_secs(4);
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Board,
    Detail,
    Archive,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Subtask,
    Action,
}

pub struct DetailState {
    pub card: Card,
    pub subtasks: Vec<Subtask>,
    pub actions: Vec<Action>,
    pub focus: Focus,
    pub sel_sub: usize,
    pub sel_act: usize,
    pub body_scroll: u16,
    pub last_run: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    NewCard,
    EditCardTitle,
    NewGroup,
    RenameGroup,
    NewSubtask,
    NewCommand,
    NewLink,
}

pub struct Prompt {
    pub kind: PromptKind,
    pub buf: String,
}

impl Prompt {
    pub fn hint(&self) -> &'static str {
        match self.kind {
            PromptKind::NewCard => "新卡片标题",
            PromptKind::EditCardTitle => "改标题",
            PromptKind::NewGroup => "新分组名",
            PromptKind::RenameGroup => "改分组名",
            PromptKind::NewSubtask => "新子任务",
            PromptKind::NewCommand => "命令动作  格式：标签 | shell 命令",
            PromptKind::NewLink => "链接动作  格式：标签 | URL",
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
    pub hits: Vec<Hit>,
    /// 卡片正面要显示动作图标，所以要一张 card_id → 动作 的缓存。
    pub card_actions: HashMap<i64, Vec<Action>>,
    /// 卡片正面要外显子任务清单，所以缓存整份列表。
    pub card_subtasks: HashMap<i64, Vec<Subtask>>,
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
            hits: Vec::new(),
            card_actions: HashMap::new(),
            card_subtasks: HashMap::new(),
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
        self.cards = self
            .groups
            .iter()
            .map(|g| self.store.cards_in_group(g.id, false).unwrap_or_default())
            .collect();

        self.card_actions.clear();
        self.card_subtasks.clear();
        for card in self.cards.iter().flatten() {
            let acts = self.store.actions(card.id).unwrap_or_default();
            if !acts.is_empty() {
                self.card_actions.insert(card.id, acts);
            }
            let subs = self.store.subtasks(card.id).unwrap_or_default();
            if !subs.is_empty() {
                self.card_subtasks.insert(card.id, subs);
            }
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
        self.detail = Some(DetailState {
            card,
            subtasks,
            actions,
            focus: keep_focus,
            sel_sub,
            sel_act,
            body_scroll,
            last_run,
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
        if self.prompt.is_some() {
            return self.on_prompt_key(key);
        }
        match self.mode {
            Mode::Board => self.on_board_key(key),
            Mode::Detail => self.on_detail_key(key),
            Mode::Archive => self.on_archive_key(key),
        }
    }

    fn on_prompt_key(&mut self, key: KeyEvent) -> Option<Effect> {
        let Some(prompt) = &mut self.prompt else {
            return None;
        };
        match key.code {
            KeyCode::Esc => {
                self.prompt = None;
            }
            KeyCode::Enter => {
                let kind = prompt.kind;
                let buf = prompt.buf.clone();
                self.prompt = None;
                self.commit_prompt(kind, buf);
            }
            KeyCode::Backspace => {
                prompt.buf.pop();
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                prompt.buf.push(c);
            }
            _ => {}
        }
        None
    }

    fn commit_prompt(&mut self, kind: PromptKind, buf: String) {
        let text = buf.trim().to_string();
        match kind {
            PromptKind::NewCard => {
                if text.is_empty() {
                    return;
                }
                if let Some(g) = self.groups.get(self.sel_g) {
                    match self.store.create_card(g.id, &text) {
                        Ok(_) => {
                            self.refresh();
                            let len = self.cards.get(self.sel_g).map(|v| v.len()).unwrap_or(1);
                            self.sel_c = len.saturating_sub(1);
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
                // 命令默认不在前端让出终端也能跑；ssh/vim 这类改 needs_terminal 走配置。
                let needs_terminal = kind == ActionKind::Command
                    && matches!(
                        target.split_whitespace().next(),
                        Some("ssh") | Some("vim") | Some("nvim") | Some("less") | Some("top")
                    );
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
            KeyCode::Esc => self.picked = None,

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
                        Focus::Action => Focus::Subtask,
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
                    }
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if let Some(d) = &mut self.detail {
                    match d.focus {
                        Focus::Subtask => d.sel_sub = d.sel_sub.saturating_sub(1),
                        Focus::Action => d.sel_act = d.sel_act.saturating_sub(1),
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
                    None => {}
                }
            }
            KeyCode::Char('J') => {
                if let Some(d) = &self.detail {
                    if d.focus == Focus::Subtask {
                        if let Some(s) = d.subtasks.get(d.sel_sub) {
                            let id = s.id;
                            let _ = self.store.shift_subtask(id, 1);
                            self.refresh_detail_after();
                        }
                    }
                }
            }
            KeyCode::Char('K') => {
                if let Some(d) = &self.detail {
                    if d.focus == Focus::Subtask {
                        if let Some(s) = d.subtasks.get(d.sel_sub) {
                            let id = s.id;
                            let _ = self.store.shift_subtask(id, -1);
                            self.refresh_detail_after();
                        }
                    }
                }
            }
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
        let total = self.archive_total();
        match key.code {
            KeyCode::Esc | KeyCode::Char('v') | KeyCode::Char('q') => {
                self.mode = Mode::Board;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if self.archive_sel + 1 < total {
                    self.archive_sel += 1;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.archive_sel = self.archive_sel.saturating_sub(1)
            }
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
        if let Some(c) = self.current_card() {
            let (id, done) = (c.id, c.done);
            if let Err(e) = self.store.set_done(id, !done) {
                self.toast(e.to_string());
            }
            self.refresh();
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
                self.move_v(1);
                None
            }
            MouseEventKind::ScrollUp => {
                self.move_v(-1);
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
            Hit::Act { .. } | Hit::SubCheck { .. } => return,
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
