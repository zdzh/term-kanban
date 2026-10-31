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

/// 确保这个仓库在 cmux 里有一个分组，返回它的 ref。
///
/// 匹配顺序：`idempotency_key`（`repo:<真实路径>`，最可靠）→ 分组名 == 仓库目录名。
/// 都没有就建一个。建组时带 `--idempotency-key`，并发/重试不会建出两个。
pub fn ensure_cmux_group(repo_path: &str) -> Option<String> {
    let repo = std::path::Path::new(repo_path);
    let name = repo.file_name()?.to_string_lossy().to_string();
    let real = std::fs::canonicalize(repo)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| repo_path.to_string());
    let key = format!("repo:{real}");

    if let Some(groups) = cmux_groups() {
        for g in &groups {
            if g.idempotency_key.as_deref() == Some(key.as_str()) {
                return Some(g.reference.clone());
            }
        }
        for g in &groups {
            if g.name == name {
                return Some(g.reference.clone());
            }
        }
    }

    let out = std::process::Command::new("cmux")
        .args([
            "workspace",
            "group",
            "create",
            "--name",
            &name,
            "--idempotency-key",
            &key,
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    text.split_whitespace()
        .find(|t| t.starts_with("workspace_group:"))
        .map(|t| t.to_string())
}

struct CmuxGroup {
    reference: String,
    name: String,
    idempotency_key: Option<String>,
}

/// 极简 JSON 提取：只取 `groups[]` 里每项的 ref / name / idempotency_key。
///
/// 不引 serde——这里只要三个字段，cmux 的输出格式也稳定。
fn cmux_groups() -> Option<Vec<CmuxGroup>> {
    let out = std::process::Command::new("cmux")
        .args(["workspace", "group", "list", "--json"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let mut groups = Vec::new();
    for chunk in text.split('{').skip(1) {
        let Some(reference) = json_field(chunk, "ref") else {
            continue;
        };
        groups.push(CmuxGroup {
            reference,
            name: json_field(chunk, "name").unwrap_or_default(),
            idempotency_key: json_field(chunk, "idempotency_key"),
        });
    }
    Some(groups)
}

fn json_field(chunk: &str, field: &str) -> Option<String> {
    let needle = format!("\"{field}\"");
    let pos = chunk.find(&needle)?;
    let rest = &chunk[pos + needle.len()..];
    let rest = rest.trim_start_matches([':', ' ', '\n', '\t']);
    let stripped = rest.strip_prefix('"')?;
    let end = stripped.find('"')?;
    let value = &stripped[..end];
    if value == "null" {
        None
    } else {
        Some(value.to_string())
    }
}

/// 若打开命令是 cmux 的「开目录」形式，补上 `--group`，让同一仓库的工作区落进同一个分组。
///
/// 只在命令里出现 `cmux` 且没有 `--group` 时动手；`code` / `codex` / 自定义脚本一律不动，
/// 保持「换工具只改配置」这条约定。
pub fn group_aware_open(open_cmd: &str, repo_path: &str) -> (String, Option<String>) {
    let trimmed = open_cmd.trim();
    let mut parts = trimmed.split_whitespace();
    let bin = parts.next().unwrap_or("");
    if bin.rsplit('/').next() != Some("cmux") {
        return (open_cmd.to_string(), None);
    }
    if trimmed.contains("--group") {
        return (open_cmd.to_string(), None);
    }
    let rest: Vec<&str> = parts.collect();
    let plain_path = match rest.as_slice() {
        [] => true,
        [only] => !only.starts_with('-'),
        ["open", only] => !only.starts_with('-'),
        _ => false,
    };
    if !plain_path {
        return (open_cmd.to_string(), None);
    }

    match ensure_cmux_group(repo_path) {
        Some(group) => (format!("{trimmed} --group {group}"), Some(group)),
        None => (open_cmd.to_string(), None),
    }
}

/// cmux 建组会生成一个空锚点 workspace（落在调用者当前目录、名字就是组名）。
/// 第一个真实成员进来后把锚点换掉，并关掉那个空条目。
pub fn adopt_cmux_anchor(group: &str, worktree: &str) {
    let real = std::fs::canonicalize(worktree)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| worktree.to_string());

    let Ok(out) = std::process::Command::new("cmux")
        .args(["workspace", "list", "--json"])
        .output()
    else {
        return;
    };
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let mut ws_ref = None;
    for chunk in text.split('{').skip(1) {
        let cwd = json_field(chunk, "current_directory").unwrap_or_default();
        if cwd == real || cwd == worktree {
            ws_ref = json_field(chunk, "ref");
            break;
        }
    }
    let Some(ws) = ws_ref else { return };

    let Ok(out) = std::process::Command::new("cmux")
        .args(["workspace", "group", "list", "--json"])
        .output()
    else {
        return;
    };
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    for chunk in text.split('{').skip(1) {
        if json_field(chunk, "ref").as_deref() != Some(group) {
            continue;
        }
        if !chunk.contains("\"anchor_workspace_is_generated\" : true") {
            return;
        }
        let old = json_field(chunk, "anchor_workspace_ref");
        let _ = std::process::Command::new("cmux")
            .args([
                "workspace-group",
                "set-anchor",
                "--group",
                group,
                "--workspace",
                &ws,
            ])
            .output();
        if let Some(old) = old {
            if old != ws {
                let _ = std::process::Command::new("cmux")
                    .args(["workspace", "close", &old])
                    .output();
            }
        }
        return;
    }
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
            let (cmd, group) = group_aware_open(&cmd, &vars.repo);
            let outcome = exec::run_shell(store, "button", None, &cmd, button.needs_terminal, echo)?;
            // cmux 建组时会留一个空锚点，把它换成刚开的真实 workspace
            if let (Some(group), true) = (group, matches!(outcome, Outcome::Finished { exit_code: Some(0), .. })) {
                adopt_cmux_anchor(&group, &ensured.workspace.path);
            }
            Ok(outcome)
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
