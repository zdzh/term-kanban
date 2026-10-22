//! 模板命令：把 `{card_id}` `{worktree}` 这类变量替换成实际值。
//!
//! 这是整套「分组按钮 / 钩子」的唯一机制——加一个按钮或换一个 agent，
//! 都只是写一行模板，不需要改代码。
//!
//! 安全约定（见 docs/requirements/0001 §7.3）：
//! - 拼进 shell 的值一律做单引号转义；
//! - `{title}` 是自由文本，**不允许**出现在命令里，只能用于显示。

use std::fmt;

use crate::model::{Card, Group};

#[derive(Debug)]
pub enum TemplateError {
    Unknown(String),
    TitleInCommand,
    Unbalanced,
}

impl fmt::Display for TemplateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TemplateError::Unknown(v) => {
                write!(f, "未知变量 {{{v}}}；可用变量：{}", VAR_NAMES.join(" "))
            }
            TemplateError::TitleInCommand => write!(
                f,
                "命令里不能用 {{title}}（自由文本会变成注入面）：要引用标题请用 {{slug}}，\
                 需要把标题当参数传给脚本请用 {{title_arg}}"
            ),
            TemplateError::Unbalanced => write!(f, "模板里的花括号没有配对"),
        }
    }
}

impl std::error::Error for TemplateError {}

pub const VAR_NAMES: &[&str] = &[
    "card_id",
    "slug",
    "title",
    "title_arg",
    "group",
    "repo",
    "repo_name",
    "worktree",
    "branch",
];

/// 一次模板替换用到的全部值。
#[derive(Debug, Clone)]
pub struct Vars {
    pub card_id: i64,
    pub title: String,
    pub slug: String,
    pub group: String,
    pub repo: String,
    pub repo_name: String,
    pub worktree: String,
    pub branch: String,
}

impl Vars {
    /// 由分组配置 + 卡片算出所有变量（worktree 路径与分支名走默认规则）。
    pub fn build(group: &Group, card: &Card) -> Self {
        let repo = group
            .repo_path
            .as_deref()
            .map(expand_tilde)
            .unwrap_or_default();
        let repo_name = if repo.is_empty() {
            String::new()
        } else {
            std::path::Path::new(&repo)
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default()
        };
        let slug = slugify(&card.title);

        let root_tpl = group
            .worktree_root
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("{repo}/../{repo_name}.worktrees");
        let branch_tpl = group
            .branch_template
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("task/{card_id}-{slug}");

        // 先算出 worktree / branch 本身要用的最小变量集
        let mut base = Vars {
            card_id: card.id,
            title: card.title.clone(),
            slug: slug.clone(),
            group: group.name.clone(),
            repo: repo.clone(),
            repo_name: repo_name.clone(),
            worktree: String::new(),
            branch: String::new(),
        };
        base.branch = render_plain(branch_tpl, &base).unwrap_or_default();
        let root = render_plain(root_tpl, &base).unwrap_or_default();
        base.worktree = if root.is_empty() {
            String::new()
        } else {
            format!("{root}/{}-{}", card.id, slug)
        };
        base
    }

    fn get(&self, name: &str) -> Option<&str> {
        Some(match name {
            "card_id" => return None, // 数字单独处理
            "slug" => &self.slug,
            "title" => &self.title,
            "group" => &self.group,
            "repo" => &self.repo,
            "repo_name" => &self.repo_name,
            "worktree" => &self.worktree,
            "branch" => &self.branch,
            _ => return None,
        })
    }
}

/// 只替换、不转义（用于计算路径与分支名本身）。
pub fn render_plain(tpl: &str, vars: &Vars) -> Result<String, TemplateError> {
    substitute(tpl, vars, false, true)
}

/// 渲染一条**要交给 shell 的**命令：值做单引号转义，且拒绝 `{title}`。
pub fn render_command(tpl: &str, vars: &Vars) -> Result<String, TemplateError> {
    substitute(tpl, vars, true, false)
}

/// 渲染给人看的字符串（确认框里显示命令、卡片上的标签等）。
pub fn render_display(tpl: &str, vars: &Vars) -> Result<String, TemplateError> {
    substitute(tpl, vars, false, false)
}

fn substitute(
    tpl: &str,
    vars: &Vars,
    escape: bool,
    allow_title: bool,
) -> Result<String, TemplateError> {
    let mut out = String::with_capacity(tpl.len() + 32);
    let mut rest = tpl;

    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('}') else {
            return Err(TemplateError::Unbalanced);
        };
        let name = &after[..end];

        let value = if name == "card_id" {
            vars.card_id.to_string()
        } else if name == "title" {
            if !allow_title && escape {
                return Err(TemplateError::TitleInCommand);
            }
            vars.title.clone()
        } else if name == "title_arg" {
            // 唯一允许把标题放进命令的方式：整体当成**一个参数**再转义，
            // 空格、引号、分号都只会成为参数内容，不会被 shell 解释。
            vars.title.clone()
        } else {
            match vars.get(name) {
                Some(v) => v.to_string(),
                None => return Err(TemplateError::Unknown(name.to_string())),
            }
        };

        if escape {
            out.push_str(&shell_escape(&value));
        } else {
            out.push_str(&value);
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);

    if out.contains('}') && !tpl.contains('}') {
        return Err(TemplateError::Unbalanced);
    }
    Ok(out)
}

/// 单引号包裹；内部的单引号用 `'\''` 断开。
pub fn shell_escape(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_./:@%+".contains(c))
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// 标题 → 适合做分支名/目录名的短横线串。
pub fn slugify(title: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
        if out.len() >= 40 {
            break;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "task".to_string()
    } else {
        trimmed
    }
}

/// `~/x` → `/Users/you/x`
pub fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        let home = std::env::var("HOME").unwrap_or_default();
        format!("{home}/{rest}")
    } else {
        path.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Local;

    fn vars() -> Vars {
        Vars {
            card_id: 12,
            title: "修 nightly CI".into(),
            slug: "ci".into(),
            group: "本周".into(),
            repo: "/w/app".into(),
            repo_name: "app".into(),
            worktree: "/w/app.worktrees/12-ci".into(),
            branch: "task/12-ci".into(),
        }
    }

    #[test]
    fn plain_substitution() {
        assert_eq!(
            render_plain("{repo}/../{repo_name}.worktrees", &vars()).unwrap(),
            "/w/app/../app.worktrees"
        );
        assert_eq!(
            render_plain("task/{card_id}-{slug}", &vars()).unwrap(),
            "task/12-ci"
        );
    }

    #[test]
    fn command_values_are_escaped() {
        let mut v = vars();
        v.worktree = "/w/app.worktrees/12-ci x".into();
        assert_eq!(
            render_command("cmux {worktree}", &v).unwrap(),
            "cmux '/w/app.worktrees/12-ci x'"
        );
    }

    #[test]
    fn title_is_rejected_in_commands() {
        assert!(matches!(
            render_command("echo {title}", &vars()),
            Err(TemplateError::TitleInCommand)
        ));
        // 但显示场景允许
        assert_eq!(render_display("{title}", &vars()).unwrap(), "修 nightly CI");
    }

    #[test]
    fn unknown_variable_is_an_error() {
        assert!(matches!(
            render_plain("{nope}", &vars()),
            Err(TemplateError::Unknown(_))
        ));
    }

    #[test]
    fn slugify_handles_cjk_and_symbols() {
        assert_eq!(slugify("修 nightly CI"), "nightly-ci");
        assert_eq!(slugify("重构 store 层"), "store");
        assert_eq!(slugify("全部中文"), "task");
        assert_eq!(slugify("---"), "task");
    }

    #[test]
    fn defaults_come_from_group_and_card() {
        let group = Group {
            id: 1,
            name: "本周".into(),
            position: 0,
            archived_at: None,
            repo_path: Some("/w/app".into()),
            worktree_root: None,
            branch_template: None,
        };
        let card = Card {
            id: 12,
            group_id: 1,
            title: "修 nightly CI".into(),
            body: String::new(),
            done: false,
            completed_at: None::<chrono::DateTime<Local>>,
            position: 0,
            archived_at: None,
        };
        let v = Vars::build(&group, &card);
        assert_eq!(v.repo, "/w/app");
        assert_eq!(v.repo_name, "app");
        assert_eq!(v.branch, "task/12-nightly-ci");
        assert_eq!(v.worktree, "/w/app/../app.worktrees/12-nightly-ci");
    }
}

#[cfg(test)]
mod title_arg_tests {
    use super::*;

    fn vars() -> Vars {
        Vars {
            card_id: 7,
            title: "修 bug; rm -rf /".into(),
            slug: "bug-rm-rf".into(),
            group: "本周".into(),
            repo: "/w/app".into(),
            repo_name: "app".into(),
            worktree: "/w/app.worktrees/7-bug".into(),
            branch: "task/7-bug".into(),
        }
    }

    #[test]
    fn title_arg_is_escaped_as_one_argument() {
        let out = render_command("script --title {title_arg} --repo {repo}", &vars()).unwrap();
        assert_eq!(out, "script --title '修 bug; rm -rf /' --repo /w/app");
        // 分号被包在引号里，不会变成第二条命令
        assert!(!out.contains("; rm -rf / "));
    }

    #[test]
    fn bare_title_still_rejected_in_commands() {
        assert!(matches!(
            render_command("script --title {title}", &vars()),
            Err(TemplateError::TitleInCommand)
        ));
        // 显示场景仍然可以
        assert_eq!(
            render_display("{title}", &vars()).unwrap(),
            "修 bug; rm -rf /"
        );
    }
}
