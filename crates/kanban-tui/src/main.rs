use std::io::{self, BufRead, Write};

use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use kanban_core::{exec, ActionKind, Store};
use kanban_tui::{app, ui};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

type Tui = Terminal<CrosstermBackend<io::Stdout>>;

fn main() -> io::Result<()> {
    exec::migrate_legacy_dir();
    let dir = exec::data_dir();
    std::fs::create_dir_all(&dir)?;
    // KANBAN_DB 可以指到别处，方便测试或把数据放进同步目录。
    let db_path = std::env::var_os("KANBAN_DB")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| dir.join("kanban.db"));
    let store = Store::open(db_path).map_err(|e| io::Error::other(e.to_string()))?;
    let mut app = app::App::new(store);

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    let result = run(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    result
}

fn run(terminal: &mut Tui, app: &mut app::App) -> io::Result<()> {
    loop {
        app.tick();
        terminal.draw(|f| ui::render(app, f))?;

        drain_pending_hooks(app);

        if !event::poll(std::time::Duration::from_millis(200))? {
            if app.should_quit {
                return Ok(());
            }
            continue;
        }

        match event::read()? {
            Event::Key(key) => {
                if let Some(effect) = app.on_key(key) {
                    apply(terminal, app, effect)?;
                }
            }
            Event::Mouse(m) => {
                if let Some(effect) = app.on_mouse(m) {
                    apply(terminal, app, effect)?;
                }
            }
            _ => {}
        }

        if app.should_quit {
            return Ok(());
        }
    }
}

fn apply(terminal: &mut Tui, app: &mut app::App, effect: app::Effect) -> io::Result<()> {
    match effect {
        app::Effect::RunAction(id) => run_action(terminal, app, id)?,
        app::Effect::EditBody(id) => edit_body(terminal, app, id)?,
        app::Effect::RunGroupButton { card_id, button_id } => {
            run_group_button(terminal, app, card_id, button_id)?
        }
        app::Effect::RunHooks { card_id, hook_ids } => run_hooks(app, card_id, &hook_ids),
        app::Effect::RemoveWorkspace(card_id) => remove_workspace(app, card_id),
    }
    Ok(())
}

/// 完成钩子：需要确认的走确认框，确认后从 pending_hooks 取出执行。
fn drain_pending_hooks(app: &mut app::App) {
    if let Some((card_id, ids)) = app.pending_hooks.take() {
        run_hooks(app, card_id, &ids);
    }
}

fn run_hooks(app: &mut app::App, card_id: i64, hook_ids: &[i64]) {
    let Ok(card) = app.store.card(card_id) else {
        return;
    };
    let Ok(group) = app.store.group(card.group_id) else {
        return;
    };
    let hooks = app.store.group_hooks(group.id).unwrap_or_default();
    let mut ok = 0;
    let mut failed: Vec<String> = Vec::new();

    for id in hook_ids {
        let Some(hook) = hooks.iter().find(|h| h.id == *id) else {
            continue;
        };
        match kanban_core::workspace::run_hook(&app.store, hook, &group, &card, false) {
            Ok(kanban_core::exec::Outcome::Finished { exit_code, .. }) => match exit_code {
                Some(0) => ok += 1,
                other => failed.push(format!(
                    "退出码 {}",
                    other
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "被信号终止".into())
                )),
            },
            Ok(_) => ok += 1,
            Err(e) => failed.push(e.to_string()),
        }
    }

    if failed.is_empty() {
        app.toast(format!("完成钩子执行成功（{ok} 条）"));
    } else {
        app.toast(format!("钩子失败：{}", failed.join("；")));
    }
    app.refresh();
}

fn run_group_button(
    terminal: &mut Tui,
    app: &mut app::App,
    card_id: i64,
    button_id: i64,
) -> io::Result<()> {
    let card = match app.store.card(card_id) {
        Ok(c) => c,
        Err(e) => {
            app.toast(e.to_string());
            return Ok(());
        }
    };
    let group = match app.store.group(card.group_id) {
        Ok(g) => g,
        Err(e) => {
            app.toast(e.to_string());
            return Ok(());
        }
    };
    let button = app
        .store
        .group_buttons(group.id)
        .unwrap_or_default()
        .into_iter()
        .find(|b| b.id == button_id);
    let Some(button) = button else {
        app.toast("按钮已不存在");
        return Ok(());
    };

    let had_workspace = app
        .store
        .workspace_for_card(card_id)
        .ok()
        .flatten()
        .is_some();
    let needs_terminal = button.needs_terminal;

    if needs_terminal {
        suspend(terminal)?;
        println!();
        println!("[{}] {}", button.label, button.command());
        println!("{}", "─".repeat(60));
        let _ = io::stdout().flush();
    }

    let outcome = kanban_core::workspace::run_group_button(
        &app.store,
        &group,
        &card,
        &button,
        needs_terminal,
    );

    if needs_terminal {
        println!();
        println!("按回车返回看板…");
        let _ = io::stdout().flush();
        wait_enter();
        resume(terminal)?;
    }

    match outcome {
        Ok(kanban_core::exec::Outcome::LinkOpened) => app.toast("已打开链接"),
        Ok(kanban_core::exec::Outcome::Finished { exit_code, .. }) => {
            if button.kind == kanban_core::ButtonKind::Workspace {
                let ws = app.store.workspace_for_card(card_id).ok().flatten();
                let path = ws.map(|w| w.path).unwrap_or_default();
                if had_workspace {
                    app.toast(format!("已打开工作区 {path}"));
                } else {
                    app.toast(format!("已创建工作区 {path}"));
                }
            } else {
                match exit_code {
                    Some(0) => app.toast(format!("「{}」执行成功", button.label)),
                    other => app.toast(format!(
                        "「{}」退出码 {}",
                        button.label,
                        other
                            .map(|c| c.to_string())
                            .unwrap_or_else(|| "被信号终止".into())
                    )),
                }
            }
        }
        Err(e) => app.toast(e.to_string()),
    }

    app.refresh();
    Ok(())
}

fn remove_workspace(app: &mut app::App, card_id: i64) {
    let Ok(Some(ws)) = app.store.workspace_for_card(card_id) else {
        app.toast("这张卡没有工作区");
        return;
    };
    if kanban_core::git::is_dirty(&ws.path) {
        app.toast("工作区里有未提交的改动，先处理再删");
        return;
    }
    match kanban_core::git::worktree_remove(&ws.repo_path, &ws.path, false) {
        Ok(()) => {
            let _ = app.store.delete_workspace(card_id);
            app.toast("工作区已删除");
        }
        Err(e) => app.toast(e.to_string()),
    }
    app.refresh();
}

fn suspend(terminal: &mut Tui) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    Ok(())
}

fn resume(terminal: &mut Tui) -> io::Result<()> {
    enable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        EnterAlternateScreen,
        EnableMouseCapture
    )?;
    terminal.clear()?;
    Ok(())
}

fn wait_enter() {
    let mut line = String::new();
    let _ = io::stdin().lock().read_line(&mut line);
}

/// 让出界面执行动作（见 ADR-0006）。
fn run_action(terminal: &mut Tui, app: &mut app::App, action_id: i64) -> io::Result<()> {
    let action = match app.store.action(action_id) {
        Ok(a) => a,
        Err(e) => {
            app.toast(e.to_string());
            return Ok(());
        }
    };

    match action.kind {
        ActionKind::Link => {
            if let Err(e) = exec::run(&app.store, &action, false) {
                app.toast(e.to_string());
            }
        }
        ActionKind::Command => {
            suspend(terminal)?;
            println!();
            println!("$ {}", action.target);
            println!("{}", "─".repeat(60));
            let _ = io::stdout().flush();

            let outcome = exec::run(&app.store, &action, true);

            match outcome {
                Ok(exec::Outcome::Finished {
                    exit_code,
                    log_path,
                    ..
                }) => {
                    println!();
                    println!(
                        "[退出码 {}]  日志: {}",
                        exit_code
                            .map(|c| c.to_string())
                            .unwrap_or_else(|| "被信号终止".into()),
                        log_path.display()
                    );
                }
                Ok(_) => {}
                Err(e) => {
                    println!();
                    println!("[执行失败] {e}");
                }
            }
            println!("按回车返回看板…");
            let _ = io::stdout().flush();
            wait_enter();
            resume(terminal)?;
        }
    }

    app.refresh();
    Ok(())
}

/// 正文用外部编辑器写（见 ADR-0003 的呈现约定：走 `$EDITOR`，不内建）。
fn edit_body(terminal: &mut Tui, app: &mut app::App, card_id: i64) -> io::Result<()> {
    let card = match app.store.card(card_id) {
        Ok(c) => c,
        Err(e) => {
            app.toast(e.to_string());
            return Ok(());
        }
    };

    let path = std::env::temp_dir().join(format!("kanban-card-{}.md", card.id));
    std::fs::write(&path, &card.body)?;

    suspend(terminal)?;
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "nvim".into());
    let mut parts = editor.split_whitespace();
    let program = parts.next().unwrap_or("nvim").to_string();
    let args: Vec<String> = parts.map(|s| s.to_string()).collect();
    let status = std::process::Command::new(&program)
        .args(&args)
        .arg(&path)
        .status();
    resume(terminal)?;

    match status {
        Ok(s) if s.success() => {
            let body = std::fs::read_to_string(&path).unwrap_or_default();
            if let Err(e) = app.store.set_body(card.id, &body) {
                app.toast(e.to_string());
            }
        }
        Ok(_) => app.toast("编辑器非正常退出，正文未改动"),
        Err(e) => app.toast(format!("启动编辑器 {program} 失败: {e}")),
    }

    app.refresh();
    Ok(())
}
