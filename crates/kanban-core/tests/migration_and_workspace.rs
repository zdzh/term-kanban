//! 迁移与工作区的集成测试：都走公开 API，不碰内部实现。

use std::path::{Path, PathBuf};
use std::process::Command;

use kanban_core::model::HOOK_ON_COMPLETE;
use kanban_core::{git, workspace, ButtonKind, RelationKind, Store};

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kanban-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// 造一个带一次提交的临时 git 仓库。
fn temp_repo(name: &str) -> PathBuf {
    let dir = tmp(name);
    std::fs::create_dir_all(&dir).unwrap();
    let d = dir.to_string_lossy().to_string();
    Command::new("git")
        .args(["-C", &d, "init", "-q"])
        .status()
        .expect("git init");
    std::fs::write(dir.join("README.md"), "hi\n").unwrap();
    Command::new("git")
        .args(["-C", &d, "add", "."])
        .status()
        .unwrap();
    Command::new("git")
        .args([
            "-C",
            &d,
            "-c",
            "user.email=t@example.com",
            "-c",
            "user.name=t",
            "commit",
            "-qm",
            "init",
        ])
        .status()
        .unwrap();
    dir
}

#[test]
fn migrates_a_v1_database_without_losing_data() {
    let path = tmp("migrate").join("kanban.db");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();

    // 手工造一个 v1 结构的库（没有分组配置列、没有新表、runs 的 action_id 还是 NOT NULL）
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE groups (
                id INTEGER PRIMARY KEY, name TEXT NOT NULL,
                position INTEGER NOT NULL, archived_at TEXT);
            CREATE TABLE cards (
                id INTEGER PRIMARY KEY,
                group_id INTEGER NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
                title TEXT NOT NULL, body TEXT NOT NULL DEFAULT '',
                done INTEGER NOT NULL DEFAULT 0, completed_at TEXT,
                position INTEGER NOT NULL, archived_at TEXT);
            CREATE TABLE subtasks (
                id INTEGER PRIMARY KEY,
                card_id INTEGER NOT NULL REFERENCES cards(id) ON DELETE CASCADE,
                text TEXT NOT NULL, done INTEGER NOT NULL DEFAULT 0,
                completed_at TEXT, position INTEGER NOT NULL);
            CREATE TABLE actions (
                id INTEGER PRIMARY KEY,
                card_id INTEGER NOT NULL REFERENCES cards(id) ON DELETE CASCADE,
                kind TEXT NOT NULL, label TEXT NOT NULL, target TEXT NOT NULL,
                needs_terminal INTEGER NOT NULL DEFAULT 0, position INTEGER NOT NULL);
            CREATE TABLE runs (
                id INTEGER PRIMARY KEY,
                action_id INTEGER NOT NULL REFERENCES actions(id) ON DELETE CASCADE,
                started_at TEXT NOT NULL, ended_at TEXT, exit_code INTEGER,
                log_path TEXT NOT NULL);

            INSERT INTO groups (id, name, position) VALUES (1, '旧分组', 0);
            INSERT INTO cards (id, group_id, title, position) VALUES (1, 1, '旧卡片', 0);
            INSERT INTO subtasks (id, card_id, text, position) VALUES (1, 1, '旧子任务', 0);
            INSERT INTO actions (id, card_id, kind, label, target, position)
                VALUES (1, 1, 'command', '旧动作', 'echo hi', 0);
            INSERT INTO runs (id, action_id, started_at, log_path)
                VALUES (1, 1, '2026-01-01T00:00:00+08:00', '/tmp/x.log');
            PRAGMA user_version=0;
            "#,
        )
        .unwrap();
    }

    let store = Store::open(&path).expect("旧库应该能打开");

    // 数据一条不少
    assert_eq!(store.groups(false, false).unwrap().len(), 1);
    let cards = store.cards_in_group(1, false).unwrap();
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].title, "旧卡片");
    assert_eq!(store.subtasks(1).unwrap().len(), 1);
    assert_eq!(store.actions(1).unwrap().len(), 1);
    assert!(store.last_run_for_card(1).unwrap().is_some());

    // 新能力可用
    store
        .set_group_config(1, Some("/tmp"), None, None)
        .expect("分组配置列应该已加上");
    store
        .add_group_button(
            1,
            "工作区",
            ButtonKind::Workspace,
            "cmux {worktree}",
            "",
            false,
        )
        .unwrap();
    store
        .add_group_hook(1, HOOK_ON_COMPLETE, "echo done", true)
        .unwrap();
    assert_eq!(store.group_buttons(1).unwrap().len(), 1);
    assert_eq!(store.group_hooks(1).unwrap().len(), 1);

    // 版本号已升级
    let conn = rusqlite::Connection::open(&path).unwrap();
    let v: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, kanban_core::store::SCHEMA_VERSION);
}

#[test]
fn refuses_a_database_from_a_newer_version() {
    let path = tmp("newer").join("kanban.db");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("PRAGMA user_version=99;").unwrap();
    }
    let err = match Store::open(&path) {
        Ok(_) => panic!("不该打开成功"),
        Err(e) => e,
    };
    assert!(err.to_string().contains("比本程序支持的"));
}

#[test]
fn workspace_is_created_once_and_then_reused() {
    let repo = temp_repo("ws");
    let store = Store::open_in_memory().unwrap();
    let gid = store.groups(false, false).unwrap()[0].id;
    store
        .set_group_config(gid, Some(&repo.to_string_lossy()), None, None)
        .unwrap();
    let group = store.group(gid).unwrap();
    let cid = store.create_card(gid, "修 nightly CI").unwrap();
    let card = store.card(cid).unwrap();

    let first = workspace::ensure_workspace(&store, &group, &card).expect("第一次应该建出来");
    assert!(first.created);
    assert!(Path::new(&first.workspace.path).is_dir());
    assert!(first.workspace.branch.starts_with("task/"));

    let second = workspace::ensure_workspace(&store, &group, &card).expect("第二次应该复用");
    assert!(!second.created);
    assert_eq!(second.workspace.path, first.workspace.path);

    // 清理
    let _ = git::worktree_remove(&first.workspace.repo_path, &first.workspace.path, true);
    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(Path::new(&first.workspace.path).parent().unwrap());
}

#[test]
fn workspace_without_repo_is_a_clear_error() {
    let store = Store::open_in_memory().unwrap();
    let gid = store.groups(false, false).unwrap()[0].id;
    let group = store.group(gid).unwrap();
    let cid = store.create_card(gid, "x").unwrap();
    let card = store.card(cid).unwrap();
    let err = match workspace::ensure_workspace(&store, &group, &card) {
        Ok(_) => panic!("不该成功"),
        Err(e) => e,
    };
    assert!(err.to_string().contains("还没关联 git 仓库"), "{err}");
}

#[test]
fn hook_command_renders_worktree_and_rejects_title() {
    let store = Store::open_in_memory().unwrap();
    let gid = store.groups(false, false).unwrap()[0].id;
    store
        .set_group_config(gid, Some("/tmp/repo"), None, None)
        .unwrap();
    let group = store.group(gid).unwrap();
    let cid = store.create_card(gid, "清理我").unwrap();
    let card = store.card(cid).unwrap();

    let hook = kanban_core::GroupHook {
        id: 1,
        group_id: gid,
        event: HOOK_ON_COMPLETE.into(),
        run: "git -C {repo} worktree remove {worktree}".into(),
        confirm: true,
        position: 0,
    };
    let cmd = workspace::hook_command(&hook, &group, &card).unwrap();
    assert!(cmd.contains("worktree remove"), "{cmd}");
    assert!(cmd.contains(&cid.to_string()));

    let bad = kanban_core::GroupHook {
        run: "echo {title}".into(),
        ..hook
    };
    assert!(workspace::hook_command(&bad, &group, &card).is_err());
}

#[test]
fn relations_reject_cycles_and_duplicates() {
    let store = Store::open_in_memory().unwrap();
    let gid = store.groups(false, false).unwrap()[0].id;
    let a = store.create_card(gid, "A").unwrap();
    let b = store.create_card(gid, "B").unwrap();

    store.add_relation(a, b, RelationKind::ParentChild).unwrap();
    assert_eq!(store.relations_from(a).unwrap().len(), 1);
    assert_eq!(store.relations_to(b).unwrap().len(), 1);
    assert_eq!(store.relation_count(a).unwrap(), 1);

    // 反向成环
    assert!(store.add_relation(b, a, RelationKind::ParentChild).is_err());
    // 重复
    assert!(store.add_relation(a, b, RelationKind::ParentChild).is_err());
    // 自环
    assert!(store.add_relation(a, a, RelationKind::ParentChild).is_err());
}

#[test]
fn workspace_button_injects_cmux_group_when_open_cmd_is_cmux() {
    // 只验证"命令改写"这一步；本机可能装了 cmux，测试不得改用户状态。
    std::env::set_var("KANBAN_DISABLE_CMUX", "1");
    let repo = temp_repo("grp");
    let store = Store::open_in_memory().unwrap();
    let gid = store.groups(false, false).unwrap()[0].id;
    store
        .set_group_config(gid, Some(&repo.to_string_lossy()), None, None)
        .unwrap();

    // 没有 cmux 分组时不该乱加参数（cmux 不可用 → ensure 返回 None）
    let (cmd, group) = workspace::group_aware_open("cmux {worktree}", &repo.to_string_lossy());
    // 本机有 cmux 时会补 --group；没有时原样返回。两种都算对。
    if let Some(g) = group {
        assert!(
            cmd.contains("new-workspace"),
            "必须使用 cmux 正式 API: {cmd}"
        );
        assert!(cmd.contains("--cwd"), "{cmd}");
        assert!(cmd.contains("--group"), "{cmd}");
        assert!(cmd.contains(&g), "{cmd}");
    } else {
        assert_eq!(cmd, "cmux {worktree}");
    }

    // 非 cmux 命令一律不动
    let (cmd, group) = workspace::group_aware_open("code {worktree}", &repo.to_string_lossy());
    assert_eq!(cmd, "code {worktree}");
    assert!(group.is_none());

    // 已经带 --group 的不重复加
    let (cmd, _) = workspace::group_aware_open(
        "cmux {worktree} --group workspace_group:1",
        &repo.to_string_lossy(),
    );
    assert_eq!(cmd.matches("--group").count(), 1);

    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn worktree_path_has_no_dotdot_segments() {
    let store = Store::open_in_memory().unwrap();
    let gid = store.groups(false, false).unwrap()[0].id;
    store
        .set_group_config(gid, Some("/Users/me/app"), None, None)
        .unwrap();
    let group = store.group(gid).unwrap();
    let cid = store.create_card(gid, "修 bug").unwrap();
    let card = store.card(cid).unwrap();

    let v = kanban_core::Vars::build(&group, &card);
    assert!(
        !v.worktree.contains(".."),
        "worktree 路径不该含 ..: {}",
        v.worktree
    );
    assert_eq!(v.worktree, "/Users/me/app.worktrees/1-bug");
}

#[test]
fn stale_workspace_path_is_cleaned_on_reuse() {
    let repo = temp_repo("stale");
    let store = Store::open_in_memory().unwrap();
    let gid = store.groups(false, false).unwrap()[0].id;
    store
        .set_group_config(gid, Some(&repo.to_string_lossy()), None, None)
        .unwrap();
    let group = store.group(gid).unwrap();
    let cid = store.create_card(gid, "x").unwrap();
    let card = store.card(cid).unwrap();

    // 先正常建一个工作区
    let first = workspace::ensure_workspace(&store, &group, &card).unwrap();

    // 把数据库里的路径改成旧版本那种带 `..` 的形式
    let dirty = format!(
        "{}/../{}.worktrees/{}",
        repo.to_string_lossy(),
        repo.file_name().unwrap().to_string_lossy(),
        first.workspace.path.rsplit('/').next().unwrap()
    );
    assert!(dirty.contains(".."));
    let mut ws = first.workspace.clone();
    ws.path = dirty.clone();
    store.save_workspace(&ws).unwrap();

    // 复用时应自动修正成干净路径
    let again = workspace::ensure_workspace(&store, &group, &card).unwrap();
    assert!(!again.created);
    assert!(
        !again.workspace.path.contains(".."),
        "{}",
        again.workspace.path
    );
    assert_eq!(again.workspace.path, first.workspace.path);
    // 数据库里也修好了
    assert_eq!(
        store.workspace_for_card(cid).unwrap().unwrap().path,
        first.workspace.path
    );

    let _ = git::worktree_remove(&first.workspace.repo_path, &first.workspace.path, true);
    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(Path::new(&first.workspace.path).parent().unwrap());
}
