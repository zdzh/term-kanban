//! 工作区：一张卡对应一个 git worktree，以及"打开它"的动作。
//!
//! 这里**不出现任何具体工具名**（cmux / codex / code…）——那些都在分组的
//! `open` 模板里，换工具只改配置（见 docs/requirements/0001 §4）。

use std::path::Path;

use chrono::Local;

use crate::exec::{self, Outcome};
use crate::git;
use crate::model::{ButtonKind, Card, CardWorkspace, Group, GroupButton, GroupHook};
use crate::store::{Result, Store, StoreError};
use crate::template::{self, Vars};

#[derive(Debug)]
pub struct EnsureOutcome {
    pub workspace: CardWorkspace,
    pub created: bool,
}

/// 保存分组配置前的校验：宁可现在报错，也不要等点击时才炸。
pub fn validate_group_config(
    repo: Option<&str>,
    worktree_root: Option<&str>,
    branch_template: Option<&str>,
) -> Result<()> {
    if let Some(raw) = repo.map(str::trim).filter(|s| !s.is_empty()) {
        let path = template::expand_tilde(raw);
        if !Path::new(&path).is_dir() {
            return Err(StoreError::InvalidConfig(format!("仓库路径不存在：{path}")));
        }
        if !git::is_repo(&path) {
            return Err(StoreError::InvalidConfig(format!("{path} 不是 git 仓库")));
        }
    }

    let probe = Vars {
        card_id: 1,
        title: "probe".into(),
        slug: "probe".into(),
        group: "g".into(),
        repo: "/tmp/repo".into(),
        repo_name: "repo".into(),
        worktree: "/tmp/repo.worktrees/1-probe".into(),
        branch: "task/1-probe".into(),
    };

    if let Some(tpl) = worktree_root.map(str::trim).filter(|s| !s.is_empty()) {
        template::render_plain(tpl, &probe)
            .map_err(|e| StoreError::InvalidConfig(format!("worktree 根目录：{e}")))?;
    }
    if let Some(tpl) = branch_template.map(str::trim).filter(|s| !s.is_empty()) {
        let rendered = template::render_plain(tpl, &probe)
            .map_err(|e| StoreError::InvalidConfig(format!("分支模板：{e}")))?;
        if let Some(bad) = rendered
            .chars()
            .find(|c| c.is_whitespace() || "~^:?*[\\".contains(*c))
        {
            return Err(StoreError::InvalidConfig(format!(
                "分支名不能包含 {bad:?}：{rendered}"
            )));
        }
    }
    Ok(())
}

/// 确保这张卡有工作区：没有就建，有就复用。
pub fn ensure_workspace(store: &Store, group: &Group, card: &Card) -> Result<EnsureOutcome> {
    let vars = Vars::build(group, card);

    if vars.repo.is_empty() {
        return Err(StoreError::InvalidConfig(format!(
            "分组「{}」还没关联 git 仓库",
            group.name
        )));
    }
    if !git::is_repo(&vars.repo) {
        return Err(StoreError::InvalidConfig(format!(
            "{} 不是 git 仓库",
            vars.repo
        )));
    }

    // 已经登记过且目录还在 → 复用
    if let Some(ws) = store.workspace_for_card(card.id)? {
        if Path::new(&ws.path).is_dir() {
            store.touch_workspace(card.id)?;
            return Ok(EnsureOutcome {
                workspace: ws,
                created: false,
            });
        }
    }

    git::worktree_add(&vars.repo, &vars.worktree, &vars.branch)?;

    let ws = CardWorkspace {
        card_id: card.id,
        repo_path: vars.repo.clone(),
        path: vars.worktree.clone(),
        branch: vars.branch.clone(),
        created_at: Local::now(),
        last_opened_at: Some(Local::now()),
    };
    store.save_workspace(&ws)?;

    Ok(EnsureOutcome {
        workspace: ws,
        created: true,
    })
}

/// 点一个分组按钮：按类型分发。
pub fn run_group_button(
    store: &Store,
    group: &Group,
    card: &Card,
    button: &GroupButton,
    echo: bool,
) -> Result<Outcome> {
    let vars = Vars::build(group, card);

    match button.kind {
        ButtonKind::Workspace => {
            let ensured = ensure_workspace(store, group, card)?;
            let mut vars = vars;
            vars.worktree = ensured.workspace.path.clone();
            vars.branch = ensured.workspace.branch.clone();
            let cmd = template::render_command(&button.open_template, &vars)
                .map_err(|e| StoreError::InvalidConfig(format!("按钮「{}」：{e}", button.label)))?;
            exec::run_shell(store, "button", None, &cmd, button.needs_terminal, echo)
        }
        ButtonKind::Command => {
            let cmd = template::render_command(&button.target, &vars)
                .map_err(|e| StoreError::InvalidConfig(format!("按钮「{}」：{e}", button.label)))?;
            exec::run_shell(store, "button", None, &cmd, button.needs_terminal, echo)
        }
        ButtonKind::Link => {
            let url = template::render_display(&button.target, &vars)
                .map_err(|e| StoreError::InvalidConfig(format!("按钮「{}」：{e}", button.label)))?;
            exec::open_url(&url)?;
            Ok(Outcome::LinkOpened)
        }
    }
}

/// 钩子渲染后的命令（用于确认框展示）。
pub fn hook_command(hook: &GroupHook, group: &Group, card: &Card) -> Result<String> {
    let vars = Vars::build(group, card);
    template::render_command(&hook.run, &vars)
        .map_err(|e| StoreError::InvalidConfig(format!("钩子：{e}")))
}

pub fn run_hook(
    store: &Store,
    hook: &GroupHook,
    group: &Group,
    card: &Card,
    echo: bool,
) -> Result<Outcome> {
    let cmd = hook_command(hook, group, card)?;
    exec::run_shell(store, "hook", None, &cmd, false, echo)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_missing_repo() {
        let err = validate_group_config(Some("/definitely/not/here"), None, None).unwrap_err();
        assert!(err.to_string().contains("不存在"));
    }

    #[test]
    fn rejects_non_repo_dir() {
        let err = validate_group_config(Some("/tmp"), None, None).unwrap_err();
        assert!(err.to_string().contains("不是 git 仓库"));
    }

    #[test]
    fn rejects_unknown_template_var() {
        let err = validate_group_config(None, Some("{nope}/x"), None).unwrap_err();
        assert!(err.to_string().contains("未知变量"));
    }

    #[test]
    fn rejects_bad_branch_chars() {
        let err = validate_group_config(None, None, Some("task/{card_id} x")).unwrap_err();
        assert!(err.to_string().contains("分支名"));
    }

    #[test]
    fn accepts_defaults() {
        assert!(validate_group_config(None, None, None).is_ok());
        assert!(validate_group_config(
            None,
            Some("{repo}/../{repo_name}.wt"),
            Some("task/{card_id}-{slug}")
        )
        .is_ok());
    }
}
