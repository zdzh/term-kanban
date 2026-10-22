//! 渲染回归测试：用 ratatui 的 TestBackend 直接读渲染出来的缓冲，
//! 断言的是"屏幕上真的有这些字/这些颜色"，而不是逻辑层。

use kanban_core::model::HOOK_ON_COMPLETE;
use kanban_core::{ActionKind, ButtonKind, Store};
use kanban_tui::app::{App, Mode};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::Color;
use ratatui::Terminal;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn press(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
}

const W: u16 = 96;
const H: u16 = 30;
const SEL_BG: Color = Color::Rgb(26, 52, 66);

fn app() -> App {
    App::new(Store::open_in_memory().unwrap())
}

fn draw(app: &mut App) -> Buffer {
    let mut term = Terminal::new(TestBackend::new(W, H)).unwrap();
    term.draw(|f| kanban_tui::ui::render(app, f)).unwrap();
    term.backend().buffer().clone()
}

/// 全角字符在缓冲里占第一个格子，后一格是空占位；取文字时要把占位跳掉，
/// 否则断言里的 "写周报" 会变成 "写 周 报"。
fn wide(c: char) -> bool {
    let cp = c as u32;
    matches!(cp,
        0x1100..=0x115F | 0x2E80..=0xA4CF | 0xAC00..=0xD7A3 |
        0xF900..=0xFAFF | 0xFE30..=0xFE6F | 0xFF00..=0xFF60 |
        0xFFE0..=0xFFE6 | 0x1F300..=0x1FAFF | 0x2600..=0x27BF)
}

fn text(buf: &Buffer) -> Vec<String> {
    (0..H)
        .map(|y| {
            let mut line = String::new();
            let mut x = 0;
            while x < W {
                let sym = buf[(x, y)].symbol();
                let mut chars = sym.chars();
                if let Some(c) = chars.next() {
                    line.push(c);
                    let is_wide = wide(c);
                    for extra in chars {
                        line.push(extra);
                    }
                    x += if is_wide { 2 } else { 1 };
                } else {
                    x += 1;
                }
            }
            line.trim_end().to_string()
        })
        .collect()
}

fn has(lines: &[String], needle: &str) -> bool {
    lines.iter().any(|l| l.contains(needle))
}

fn row_of(lines: &[String], needle: &str) -> Option<usize> {
    lines.iter().position(|l| l.contains(needle))
}

fn add_card(app: &mut App, title: &str) {
    app.store.create_card(app.groups[0].id, title).unwrap();
    app.refresh();
}

#[test]
fn board_shows_groups_and_selection_marker() {
    let mut a = app();
    add_card(&mut a, "写周报");
    let lines = text(&draw(&mut a));

    assert!(has(&lines, "待办"), "{lines:#?}");
    assert!(has(&lines, "写周报"));
    // 选中标记
    assert!(has(&lines, "▎☐"));
}

#[test]
fn selected_row_highlight_covers_the_whole_row() {
    let mut a = app();
    add_card(&mut a, "写周报");
    add_card(&mut a, "买菜");
    let buf = draw(&mut a);
    let lines = text(&buf);
    let row = row_of(&lines, "写周报").expect("找不到卡片") as u16;

    // 第一栏内部从 x=1 到 x=30（左右各有边框），整行底色都必须是选中色，
    // 包括文字之后的那片空白——这正是「高亮只盖住文字」的回归点。
    let mut x = 1u16;
    while x < 31 {
        let sym = buf[(x, row)].symbol();
        assert_eq!(buf[(x, row)].bg, SEL_BG, "第 {x} 列没被高亮覆盖: {sym:?}");
        // 全角字占两格，第二格是占位（不参与绘制），跳过
        let step = match sym.chars().next() {
            Some(c) if wide(c) => 2,
            _ => 1,
        };
        x += step;
    }

    // 相邻那张卡不该被染色
    let row2 = row_of(&lines, "买菜").unwrap() as u16;
    assert_ne!(buf[(1, row2)].bg, SEL_BG);
}

#[test]
fn extra_actions_collapse_into_a_plus_count() {
    let mut a = app();
    add_card(&mut a, "动作很多");
    let id = a.cards[0][0].id;
    for i in 0..8 {
        a.store
            .add_action(id, ActionKind::Command, "x", &format!("echo {i}"), false)
            .unwrap();
    }
    a.refresh();
    let lines = text(&draw(&mut a));

    assert!(
        has(&lines, "+2"),
        "超过 6 个动作时要显示剩余个数: {lines:#?}"
    );
}

#[test]
fn subtask_completion_time_is_visible_in_detail() {
    let mut a = app();
    add_card(&mut a, "带子任务");
    let id = a.cards[0][0].id;
    let sub = a.store.add_subtask(id, "收集数据").unwrap();
    a.store.set_subtask_done(sub, true).unwrap();
    a.refresh();

    a.on_key(ratatui::crossterm::event::KeyEvent::new(
        ratatui::crossterm::event::KeyCode::Enter,
        ratatui::crossterm::event::KeyModifiers::NONE,
    ));
    assert_eq!(a.mode, Mode::Detail);

    let lines = text(&draw(&mut a));
    let y = row_of(&lines, "收集数据").expect("详情里应看到子任务");
    // 同一行右侧应带上完成时间（MM-DD HH:MM）
    let line = &lines[y];
    let tail = &line[line.find("收集数据").unwrap()..];
    let has_date = tail
        .split_whitespace()
        .any(|w| w.len() == 5 && w.as_bytes().get(2) == Some(&b'-'));
    let has_clock = tail.contains(':');
    assert!(has_date && has_clock, "子任务行没显示完成时间: {line:?}");
}

#[test]
fn overflowing_column_reports_how_many_are_hidden() {
    let mut a = app();
    for i in 0..40 {
        add_card(&mut a, &format!("卡{i}"));
    }
    let lines = text(&draw(&mut a));
    let title = lines
        .iter()
        .find(|l| l.contains("待办") && l.contains('┌'))
        .expect("找不到分组标题");
    assert!(title.contains('↓'), "标题里应提示下面还有几张: {title:?}");
}

#[test]
fn archive_view_lists_archived_cards() {
    let mut a = app();
    add_card(&mut a, "归档我");
    let id = a.cards[0][0].id;
    a.store.archive_card(id).unwrap();
    a.refresh();
    a.mode = Mode::Archive;

    let lines = text(&draw(&mut a));
    assert!(has(&lines, "归档视图"));
    assert!(has(&lines, "归档我"));
}

#[test]
fn search_result_screen_matches_what_is_typed() {
    let mut a = app();
    add_card(&mut a, "写周报");
    add_card(&mut a, "买菜");

    a.on_key(ratatui::crossterm::event::KeyEvent::new(
        ratatui::crossterm::event::KeyCode::Char('/'),
        ratatui::crossterm::event::KeyModifiers::NONE,
    ));
    for c in "周报".chars() {
        a.on_key(ratatui::crossterm::event::KeyEvent::new(
            ratatui::crossterm::event::KeyCode::Char(c),
            ratatui::crossterm::event::KeyModifiers::NONE,
        ));
    }

    let lines = text(&draw(&mut a));
    assert!(
        lines[0].contains("筛选"),
        "顶栏应显示筛选状态: {:?}",
        lines[0]
    );
    assert!(has(&lines, "写周报"));
    assert!(!has(&lines, "买菜"));
}

#[test]
fn group_settings_panel_shows_config_fields_and_buttons() {
    let mut a = app();
    let gid = a.groups[0].id;
    a.store
        .set_group_config(gid, Some("/tmp/repo"), None, None)
        .unwrap();
    a.store
        .add_group_button(
            gid,
            "工作区",
            ButtonKind::Workspace,
            "cmux {worktree}",
            "",
            false,
        )
        .unwrap();
    a.refresh();

    press(&mut a, KeyCode::Char('g'));
    let lines = text(&draw(&mut a));

    assert!(has(&lines, "分组设置"), "{lines:#?}");
    assert!(has(&lines, "仓库"));
    assert!(has(&lines, "/tmp/repo"));
    assert!(has(&lines, "工作区"));
    assert!(has(&lines, "cmux {worktree}"));
}

#[test]
fn workspace_panel_lists_created_workspaces() {
    let mut a = app();
    let gid = a.groups[0].id;
    let cid = a.store.create_card(gid, "有工作区的卡").unwrap();
    a.store
        .save_workspace(&kanban_core::CardWorkspace {
            card_id: cid,
            repo_path: "/tmp/repo".into(),
            path: "/tmp/repo.worktrees/1-x".into(),
            branch: "task/1-x".into(),
            created_at: chrono::Local::now(),
            last_opened_at: Some(chrono::Local::now()),
        })
        .unwrap();
    a.refresh();

    press(&mut a, KeyCode::Char('W'));
    let lines = text(&draw(&mut a));

    assert!(has(&lines, "工作区"), "{lines:#?}");
    assert!(has(&lines, "有工作区的卡"));
    assert!(has(&lines, "task/1-x"));
}

#[test]
fn completing_a_card_with_confirm_hook_opens_a_confirmation() {
    let mut a = app();
    let gid = a.groups[0].id;
    a.store
        .set_group_config(gid, Some("/tmp/repo"), None, None)
        .unwrap();
    a.store
        .add_group_hook(
            gid,
            HOOK_ON_COMPLETE,
            "git worktree remove {worktree}",
            true,
        )
        .unwrap();
    a.store.create_card(gid, "要清理的卡").unwrap();
    a.refresh();

    press(&mut a, KeyCode::Char(' '));
    let lines = text(&draw(&mut a));
    assert!(a.confirm.is_some(), "应该弹确认框: {lines:#?}");
    assert!(has(&lines, "要执行"));
    assert!(has(&lines, "worktree remove"));

    // y 确认后交给主循环执行
    press(&mut a, KeyCode::Char('y'));
    assert!(a.confirm.is_none());
    assert!(a.pending_hooks.is_some());
}

#[test]
fn card_front_shows_relation_count_and_workspace_badge() {
    let mut a = app();
    let gid = a.groups[0].id;
    let a1 = a.store.create_card(gid, "父卡").unwrap();
    let a2 = a.store.create_card(gid, "子卡").unwrap();
    a.store
        .add_relation(a1, a2, kanban_core::RelationKind::ParentChild)
        .unwrap();
    a.store
        .save_workspace(&kanban_core::CardWorkspace {
            card_id: a1,
            repo_path: "/tmp/repo".into(),
            path: "/tmp/repo.worktrees/1-x".into(),
            branch: "task/1-x".into(),
            created_at: chrono::Local::now(),
            last_opened_at: None,
        })
        .unwrap();
    a.refresh();

    let lines = text(&draw(&mut a));
    assert!(has(&lines, "⤷1"), "应显示关联数量: {lines:#?}");
    assert!(has(&lines, "⎇"), "应显示工作区分支: {lines:#?}");
}

#[test]
fn detail_shows_relation_section() {
    let mut a = app();
    let gid = a.groups[0].id;
    let a1 = a.store.create_card(gid, "父卡").unwrap();
    let a2 = a.store.create_card(gid, "子卡").unwrap();
    a.store
        .add_relation(a1, a2, kanban_core::RelationKind::ParentChild)
        .unwrap();
    a.refresh();

    press(&mut a, KeyCode::Enter);
    let lines = text(&draw(&mut a));
    assert!(has(&lines, "关联"), "{lines:#?}");
    assert!(has(&lines, "父 → 子卡"));
}

#[test]
fn settings_panel_has_visible_section_separators() {
    let mut a = app();
    let gid = a.groups[0].id;
    a.store
        .set_group_config(gid, Some("/tmp/repo"), None, None)
        .unwrap();
    a.store
        .add_group_button(
            gid,
            "工作区",
            ButtonKind::Workspace,
            "cmux {worktree}",
            "",
            false,
        )
        .unwrap();
    a.store
        .add_group_hook(gid, HOOK_ON_COMPLETE, "echo done", true)
        .unwrap();
    a.refresh();
    press(&mut a, KeyCode::Char('g'));

    let lines = text(&draw(&mut a));

    // 三段标题都在
    for title in ["仓库", "按钮", "完成钩子"] {
        assert!(has(&lines, title), "缺少分区标题 {title}: {lines:#?}");
    }

    // 每个分区标题所在行都带一条分隔线
    for title in ["仓库", "按钮", "完成钩子"] {
        let line = lines
            .iter()
            .find(|l| l.contains(title) && l.contains('─'))
            .unwrap_or_else(|| panic!("分区「{title}」没有分隔线: {lines:#?}"));
        assert!(line.matches('─').count() > 20, "分隔线太短: {line:?}");
    }
}
