//! git 相关的最小封装：判断仓库、建/删 worktree、探测脏状态。
//!
//! 只用 `git` 命令，不引 libgit2——worktree 的行为要和你手敲命令完全一致。

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::store::{Result, StoreError};

fn run(args: &[&str], cwd: Option<&str>) -> Result<(bool, String)> {
    let mut cmd = Command::new("git");
    if let Some(dir) = cwd {
        cmd.arg("-C").arg(dir);
    }
    cmd.args(args);
    let out = cmd
        .output()
        .map_err(|e| StoreError::Io(format!("执行 git 失败: {e}")))?;
    let mut text = String::from_utf8_lossy(&out.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok((out.status.success(), text.trim().to_string()))
}

/// 目录本身是不是一个 git 仓库的工作树根。
pub fn is_repo(path: &str) -> bool {
    if path.trim().is_empty() {
        return false;
    }
    let p = Path::new(path);
    p.is_dir() && p.join(".git").exists()
}

pub fn branch_exists(repo: &str, branch: &str) -> bool {
    matches!(
        run(
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}")
            ],
            Some(repo)
        ),
        Ok((true, _))
    )
}

/// 建工作区：`git worktree add [-b 新分支] <path> [分支]`。
pub fn worktree_add(repo: &str, path: &str, branch: &str) -> Result<()> {
    if let Some(parent) = Path::new(path).parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| StoreError::Io(format!("建目录失败 {}: {e}", parent.display())))?;
    }

    let (ok, text) = if branch_exists(repo, branch) {
        run(&["worktree", "add", path, branch], Some(repo))?
    } else {
        run(&["worktree", "add", "-b", branch, path], Some(repo))?
    };

    if !ok {
        // 已经被别处建好了（并发点击），当成成功
        if text.contains("already registered") || text.contains("already exists") {
            return Ok(());
        }
        return Err(StoreError::Io(format!("创建 worktree 失败: {text}")));
    }
    Ok(())
}

pub fn worktree_remove(repo: &str, path: &str, force: bool) -> Result<()> {
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(path);
    let (ok, text) = run(&args, Some(repo))?;
    if !ok && !text.contains("is not a working tree") {
        return Err(StoreError::Io(format!("删除 worktree 失败: {text}")));
    }
    Ok(())
}

/// 工作区里有没有未提交的改动（含未跟踪文件）。
pub fn is_dirty(worktree: &str) -> bool {
    if !Path::new(worktree).is_dir() {
        return false;
    }
    match run(&["status", "--porcelain"], Some(worktree)) {
        Ok((true, text)) => !text.is_empty(),
        _ => false,
    }
}

pub fn current_branch(worktree: &str) -> Option<String> {
    match run(&["rev-parse", "--abbrev-ref", "HEAD"], Some(worktree)) {
        Ok((true, text)) if !text.is_empty() => Some(text),
        _ => None,
    }
}

/// 仓库下已注册的 worktree 路径。
pub fn worktree_paths(repo: &str) -> Vec<PathBuf> {
    match run(&["worktree", "list", "--porcelain"], Some(repo)) {
        Ok((true, text)) => text
            .lines()
            .filter_map(|l| l.strip_prefix("worktree "))
            .map(PathBuf::from)
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_repo_is_rejected() {
        assert!(!is_repo(""));
        assert!(!is_repo("/definitely/not/here"));
    }
}
