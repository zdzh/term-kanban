use kanban_core::ActionKind;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::app::{App, Focus, Hit, Mode, MIN_COL_W};
use crate::markdown;

const SEL_BG: Color = Color::Rgb(26, 52, 66);
const DIM: Color = Color::Rgb(110, 110, 120);
const PICK_BG: Color = Color::Rgb(58, 50, 18);

pub fn render(app: &mut App, f: &mut Frame) {
    app.hits.clear();
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .split(f.area());

    render_top(app, f, chunks[0]);

    match app.mode {
        Mode::Board => render_board(app, f, chunks[1]),
        Mode::Detail => {
            render_board(app, f, chunks[1]);
            render_detail(app, f, f.area());
        }
        Mode::Archive => render_archive(app, f, chunks[1]),
    }

    render_status(app, f, chunks[2]);

    if app.prompt.is_some() {
        render_prompt(app, f, f.area());
    }
    render_toast(app, f);
}

fn render_top(app: &App, f: &mut Frame, area: Rect) {
    let total_open = app.cards.iter().flatten().filter(|c| !c.done).count();
    let total_done = app.cards.iter().flatten().filter(|c| c.done).count();
    let title = match app.mode {
        Mode::Archive => " 归档视图 ".to_string(),
        _ => format!(" 看板 · 未完成 {total_open} · 已完成 {total_done} "),
    };
    let line = Line::from(vec![
        Span::styled(
            title,
            Style::new()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            app.groups
                .get(app.sel_g)
                .map(|g| g.name.clone())
                .unwrap_or_default(),
            Style::new().fg(Color::Cyan),
        ),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn render_board(app: &mut App, f: &mut Frame, area: Rect) {
    // 平铺：能放几栏就放几栏，然后把整个宽度平分给它们（余数补给前几栏）。
    let fit = ((area.width / MIN_COL_W) as usize).max(1);
    let visible = fit.min(app.groups.len().max(1));

    if app.sel_g < app.col_offset {
        app.col_offset = app.sel_g;
    }
    if app.sel_g >= app.col_offset + visible {
        app.col_offset = app.sel_g + 1 - visible;
    }
    let end = (app.col_offset + visible).min(app.groups.len());

    let per = area.width / visible as u16;
    let spare = area.width % visible as u16;

    let mut x = area.x;
    for (i, gi) in (app.col_offset..end).enumerate() {
        let width = per + if (i as u16) < spare { 1 } else { 0 };
        let rect = Rect::new(x, area.y, width, area.height);
        render_group(app, f, rect, gi, i == 0);
        x += width;
    }

    if app.groups.is_empty() {
        let hint = Paragraph::new("还没有分组，按 N 建一个").style(Style::new().fg(DIM));
        f.render_widget(hint, area);
    }
}

fn render_group(app: &mut App, f: &mut Frame, rect: Rect, gi: usize, _first: bool) {
    let group = &app.groups[gi];
    let cards = &app.cards[gi];
    let open = cards.iter().filter(|c| !c.done).count();
    let done = cards.len() - open;

    let selected = gi == app.sel_g;
    let border_style = if selected {
        Style::new().fg(Color::Cyan)
    } else {
        Style::new().fg(DIM)
    };

    let title = Line::from(vec![
        Span::styled(
            format!(" {} ", group.name),
            Style::new()
                .fg(if selected { Color::Cyan } else { Color::White })
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("{open}"), Style::new().fg(Color::White)),
        Span::styled("/", Style::new().fg(DIM)),
        Span::styled(format!("{done}"), Style::new().fg(Color::Green)),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    app.hits.push(Hit::Column { rect, group: gi });

    if inner.height == 0 || inner.width < 6 {
        return;
    }

    let card_actions: Vec<Vec<kanban_core::Action>> = cards
        .iter()
        .map(|c| app.card_actions.get(&c.id).cloned().unwrap_or_default())
        .collect();

    let mut y = inner.y;
    let bottom = inner.y + inner.height;
    let mut divider_drawn = false;

    for (ci, card) in cards.iter().enumerate() {
        if card.done && app.fold_done {
            continue;
        }
        if card.done && !divider_drawn {
            if y >= bottom {
                break;
            }
            let line = "─".repeat(inner.width as usize);
            f.render_widget(
                Paragraph::new(line).style(Style::new().fg(DIM)),
                Rect::new(inner.x, y, inner.width, 1),
            );
            y += 1;
            divider_drawn = true;
            if y >= bottom {
                break;
            }
        }
        if y + 2 > bottom {
            break;
        }

        let is_sel = selected && ci == app.sel_c;
        let is_picked = app.picked == Some(card.id);
        // 选中用「左侧竖条 + 背景 + 青色加粗标题」三重标记，扫一眼就能定位。
        let base_bg = if is_sel {
            Some(SEL_BG)
        } else if is_picked {
            Some(PICK_BG)
        } else {
            None
        };
        let mut style = Style::new();
        if let Some(bg) = base_bg {
            style = style.bg(bg);
        }
        let marker = if is_sel {
            Span::styled("▎", Style::new().fg(Color::Cyan).patch(style))
        } else if is_picked {
            Span::styled("▎", Style::new().fg(Color::Yellow).patch(style))
        } else {
            Span::styled(" ", style)
        };

        // 第一行：竖条 + 勾选框 + 标题
        let check = if card.done { "☑ " } else { "☐ " };
        let check_style = if card.done {
            Style::new().fg(Color::Green)
        } else {
            Style::new().fg(if is_sel { Color::White } else { Color::Gray })
        };
        let title_style = if card.done {
            Style::new().fg(DIM).add_modifier(Modifier::CROSSED_OUT)
        } else if is_sel {
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(Color::White)
        };
        let avail = inner.width.saturating_sub(3) as usize;
        let title_text = truncate(&card.title, avail);
        let line1 = Line::from(vec![
            marker,
            Span::styled(check, check_style.patch(style)),
            Span::styled(title_text, title_style.patch(style)),
        ]);

        // 第二行：进度 / 动作图标 / 完成时间
        let mut meta: Vec<Span> = Vec::new();
        let mut act_hits: Vec<(u16, i64)> = Vec::new();
        let mut x = inner.x;
        let meta_style = Style::new().fg(DIM).patch(style);

        if card.done {
            let when = card
                .completed_at
                .map(|t| t.format("%m-%d %H:%M").to_string())
                .unwrap_or_else(|| "-".into());
            meta.push(Span::styled(
                format!("  ✓ {when}"),
                Style::new().fg(Color::Green).patch(style),
            ));
        } else {
            let acts = &card_actions[ci];
            let subs = app.card_subtasks.get(&card.id).cloned().unwrap_or_default();
            let mut head = String::from("  ");
            if !subs.is_empty() {
                let done = subs.iter().filter(|s| s.done).count();
                head.push_str(&format!("{}/{}", done, subs.len()));
                if done == subs.len() {
                    head.push_str(" ✓");
                }
                head.push(' ');
            }
            meta.push(Span::styled(head.clone(), meta_style));
            x += display_width(&head);
            for a in acts.iter().take(6) {
                let color = match a.kind {
                    ActionKind::Command => Color::Magenta,
                    ActionKind::Link => Color::Blue,
                };
                meta.push(Span::styled(
                    a.glyph(),
                    Style::new()
                        .fg(color)
                        .add_modifier(Modifier::BOLD)
                        .patch(style),
                ));
                meta.push(Span::styled(" ", meta_style));
                act_hits.push((x, a.id));
                x += 2;
            }
        }

        // 第三行起：有子任务就外显清单（点复选框即可勾）；没有子任务才显示两行正文预览。
        let subs = if card.done {
            Vec::new()
        } else {
            app.card_subtasks.get(&card.id).cloned().unwrap_or_default()
        };
        let mut extra: Vec<Line> = Vec::new();
        let mut sub_hits: Vec<(u16, i64)> = Vec::new();
        let text_w = inner.width.saturating_sub(4) as usize;

        if !subs.is_empty() {
            for s in &subs {
                if (extra.len() as u16) + 2 > bottom.saturating_sub(y) {
                    break;
                }
                let sub_style = if s.done {
                    Style::new().fg(DIM).add_modifier(Modifier::CROSSED_OUT)
                } else {
                    Style::new().fg(Color::Gray)
                };
                extra.push(Line::from(vec![
                    Span::styled("  ", style),
                    Span::styled(
                        if s.done { "☑ " } else { "☐ " },
                        Style::new()
                            .fg(if s.done { Color::Green } else { Color::Gray })
                            .patch(style),
                    ),
                    Span::styled(truncate(&s.text, text_w), sub_style.patch(style)),
                ]));
                sub_hits.push((inner.x + 2, s.id));
            }
        } else if !card.body.trim().is_empty() && !card.done {
            let text = markdown::plain(&card.body);
            for chunk in wrap_plain(&text, text_w).into_iter().take(2) {
                extra.push(Line::from(Span::styled(
                    format!("  {chunk}"),
                    Style::new().fg(DIM).patch(style),
                )));
            }
        }

        // 放不下就不加额外行，避免最后一张卡把列撑破。
        let max_extra = bottom.saturating_sub(y + 2);
        if extra.len() as u16 > max_extra {
            extra.truncate(max_extra as usize);
        }
        let height = 2 + extra.len() as u16;

        let mut all = vec![line1, Line::from(meta)];
        all.extend(extra);
        f.render_widget(
            Paragraph::new(all),
            Rect::new(inner.x, y, inner.width, height),
        );

        app.hits.push(Hit::Card {
            rect: Rect::new(inner.x, y, inner.width, height),
            group: gi,
            card: ci,
        });
        app.hits.push(Hit::Check {
            rect: Rect::new(inner.x + 1, y, 3, 1),
            group: gi,
            card: ci,
        });
        // 动作图标与子任务复选框最后推，这样它们优先吃掉点击。
        for (ax, action_id) in act_hits {
            app.hits.push(Hit::Act {
                rect: Rect::new(ax, y + 1, 2, 1),
                action_id,
            });
        }
        for (i, (sx, subtask_id)) in sub_hits.iter().enumerate() {
            let _ = i;
            app.hits.push(Hit::SubCheck {
                rect: Rect::new(*sx, y + 2 + (i as u16), 2, 1),
                subtask_id: *subtask_id,
            });
        }

        // 任务之间不留空行：间距靠标题加粗和竖条区分。
        y += height;
    }
}

fn render_detail(app: &mut App, f: &mut Frame, area: Rect) {
    let Some(d) = &app.detail else { return };
    let popup = centered(area, 84, 84);
    f.render_widget(Clear, popup);

    let card = d.card.clone();
    let subtasks = d.subtasks.clone();
    let actions = d.actions.clone();
    let focus = d.focus;
    let sel_sub = d.sel_sub;
    let sel_act = d.sel_act;
    let body_scroll = d.body_scroll;
    let last_run = d.last_run.clone();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(Color::Cyan))
        .title(" 卡片详情  Esc 返回 Tab 切换焦点 ");
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let sub_h = (subtasks.len() as u16 + 2).min(inner.height / 3);
    let act_h = (actions.len() as u16 + 2).min(inner.height / 3);
    let rows = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(3),
        Constraint::Length(sub_h),
        Constraint::Length(act_h),
    ])
    .split(inner);

    // 标题 + 状态
    let check = if card.done { "☑" } else { "☐" };
    let status = card
        .completed_at
        .map(|t| format!("完成于 {}", t.format("%Y-%m-%d %H:%M")))
        .unwrap_or_else(|| "未完成".into());
    let head = Line::from(vec![
        Span::styled(format!("{check} "), Style::new().fg(Color::Green)),
        Span::styled(
            card.title.clone(),
            Style::new().add_modifier(Modifier::BOLD),
        ),
    ]);
    let sub = Line::from(vec![
        Span::styled(format!("  {status}"), Style::new().fg(DIM)),
        Span::styled(
            last_run.map(|r| format!("   {r}")).unwrap_or_default(),
            Style::new().fg(DIM),
        ),
    ]);
    f.render_widget(Paragraph::new(vec![head, sub]), rows[0]);

    // 正文
    let body_block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::new().fg(DIM))
        .title(Span::styled(" 正文 · e 编辑 ", Style::new().fg(DIM)));
    let body_inner = body_block.inner(rows[1]);
    f.render_widget(body_block, rows[1]);
    if card.body.trim().is_empty() {
        f.render_widget(
            Paragraph::new("（空）按 e 用 $EDITOR 写 Markdown").style(Style::new().fg(DIM)),
            body_inner,
        );
    } else {
        f.render_widget(
            Paragraph::new(markdown::render(&card.body))
                .wrap(Wrap { trim: false })
                .scroll((body_scroll, 0)),
            body_inner,
        );
    }

    // 子任务
    let sub_focused = focus == Focus::Subtask;
    let sub_block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::new().fg(if sub_focused { Color::Cyan } else { DIM }))
        .title(Span::styled(
            format!(
                " 子任务 {}/{} · t 新增 space 勾选 J/K 排序 d 删除 ",
                subtasks.iter().filter(|s| s.done).count(),
                subtasks.len()
            ),
            Style::new().fg(if sub_focused { Color::Cyan } else { DIM }),
        ));
    let sub_inner = sub_block.inner(rows[2]);
    f.render_widget(sub_block, rows[2]);
    let mut lines = Vec::new();
    for (i, s) in subtasks.iter().enumerate() {
        let sel = sub_focused && i == sel_sub;
        let style = if sel {
            Style::new().bg(SEL_BG)
        } else {
            Style::new()
        };
        lines.push(Line::from(vec![
            Span::styled(
                if s.done { "☑ " } else { "☐ " },
                Style::new()
                    .fg(if s.done { Color::Green } else { Color::Gray })
                    .patch(style),
            ),
            Span::styled(
                s.text.clone(),
                if s.done {
                    Style::new().fg(DIM).patch(style)
                } else {
                    style
                },
            ),
        ]));
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "（没有子任务，按 t 加一条）",
            Style::new().fg(DIM),
        )));
    }
    f.render_widget(Paragraph::new(lines), sub_inner);

    // 动作
    let act_focused = focus == Focus::Action;
    let act_block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::new().fg(if act_focused { Color::Cyan } else { DIM }))
        .title(Span::styled(
            " 动作 · c 加命令 y 加链接 space 执行 d 删除 1-9 直接执行 ",
            Style::new().fg(if act_focused { Color::Cyan } else { DIM }),
        ));
    let act_inner = act_block.inner(rows[3]);
    f.render_widget(act_block, rows[3]);
    let mut lines = Vec::new();
    for (i, a) in actions.iter().enumerate() {
        let sel = act_focused && i == sel_act;
        let style = if sel {
            Style::new().bg(SEL_BG)
        } else {
            Style::new()
        };
        let color = match a.kind {
            ActionKind::Command => Color::Magenta,
            ActionKind::Link => Color::Blue,
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{} ", i + 1), Style::new().fg(DIM).patch(style)),
            Span::styled(
                format!("{} ", a.glyph()),
                Style::new()
                    .fg(color)
                    .add_modifier(Modifier::BOLD)
                    .patch(style),
            ),
            Span::styled(a.label.clone(), style),
            Span::styled(
                format!("  {}", truncate(&a.target, 40)),
                Style::new().fg(DIM).patch(style),
            ),
        ]));
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "（没有动作，按 c 挂一条命令，或按 y 挂一个链接）",
            Style::new().fg(DIM),
        )));
    }
    f.render_widget(Paragraph::new(lines), act_inner);
}

fn render_archive(app: &mut App, f: &mut Frame, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(Color::Yellow))
        .title(" 归档视图 · j/k 选择 u 恢复 x 彻底删除 v/Esc 返回 ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();
    let mut idx = 0usize;
    for g in &app.archive_groups {
        let sel = idx == app.archive_sel;
        let style = if sel {
            Style::new().bg(SEL_BG)
        } else {
            Style::new()
        };
        lines.push(Line::from(Span::styled(
            format!(
                "▸ 分组「{}」（连同 {} 张卡）",
                g.name,
                app.cards_for_archived_group(g.id)
            ),
            Style::new()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
                .patch(style),
        )));
        idx += 1;
    }
    for c in &app.archive_cards {
        let sel = idx == app.archive_sel;
        let style = if sel {
            Style::new().bg(SEL_BG)
        } else {
            Style::new()
        };
        let when = c
            .archived_at
            .map(|t| t.format("%m-%d").to_string())
            .unwrap_or_default();
        lines.push(Line::from(vec![
            Span::styled("  ☐ ", Style::new().fg(DIM).patch(style)),
            Span::styled(c.title.clone(), style),
            Span::styled(format!("   {when} 归档"), Style::new().fg(DIM).patch(style)),
        ]));
        idx += 1;
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "（归档是空的）",
            Style::new().fg(DIM),
        )));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn render_status(app: &App, f: &mut Frame, area: Rect) {
    let hint = match app.mode {
        Mode::Board if app.picked.is_some() => {
            "已拿起卡片 · hjkl 挪动 · Enter/m 放下 · Esc 取消".to_string()
        }
        Mode::Board => "hjkl 移动 n 新建 space 勾选 Enter 详情 e 正文 m 拿起 a 归档 v 归档视图 A 归档分组 N 新分组 q 退出".to_string(),
        Mode::Detail => "Tab 切焦点 · space 勾选/执行 · 1-9 跑动作 · t 子任务 · c/y 加动作 · e 正文 · Esc 返回".to_string(),
        Mode::Archive => "j/k 选择 · u 恢复 · x 彻底删除 · v 返回".to_string(),
    };
    f.render_widget(
        Paragraph::new(hint)
            .style(Style::new().fg(DIM))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn render_prompt(app: &App, f: &mut Frame, area: Rect) {
    let Some(p) = &app.prompt else { return };
    let w = area.width.saturating_sub(8).min(70);
    let rect = Rect::new(
        area.x + (area.width - w) / 2,
        area.y + area.height / 2,
        w,
        3,
    );
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(Color::Cyan))
        .title(Span::styled(
            format!(" {} · Enter 确认 Esc 取消 ", p.hint()),
            Style::new().fg(Color::Cyan),
        ));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let text = Line::from(vec![
        Span::styled(p.buf.clone(), Style::new().fg(Color::White)),
        Span::styled("▌", Style::new().fg(Color::Cyan)),
    ]);
    f.render_widget(Paragraph::new(text), inner);
}

fn render_toast(app: &App, f: &mut Frame) {
    let Some((msg, _)) = &app.toast else { return };
    let area = f.area();
    let w = (display_width(msg) + 4).min(area.width);
    let rect = Rect::new(area.x + area.width.saturating_sub(w + 2), area.y + 1, w, 3);
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(Color::Yellow));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    f.render_widget(
        Paragraph::new(msg.clone()).style(Style::new().fg(Color::Yellow)),
        inner,
    );
}

fn centered(area: Rect, pct_x: u16, pct_y: u16) -> Rect {
    let w = area.width * pct_x / 100;
    let h = area.height * pct_y / 100;
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

/// 终端里的显示宽度：CJK 与全角符号占两格。
fn display_width(s: &str) -> u16 {
    s.chars()
        .map(|c| {
            let cp = c as u32;
            let wide = matches!(cp,
                0x1100..=0x115F | 0x2E80..=0xA4CF | 0xAC00..=0xD7A3 |
                0xF900..=0xFAFF | 0xFE30..=0xFE6F | 0xFF00..=0xFF60 |
                0xFFE0..=0xFFE6 | 0x1F300..=0x1FAFF | 0x2600..=0x27BF);
            if wide {
                2
            } else {
                1
            }
        })
        .sum()
}

fn wrap_plain(text: &str, max: usize) -> Vec<String> {
    if max == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut w = 0usize;
    for c in text.chars() {
        let cw = display_width(&c.to_string()) as usize;
        if w + cw > max {
            out.push(std::mem::take(&mut cur));
            w = 0;
        }
        cur.push(c);
        w += cw;
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn truncate(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut w = 0usize;
    for c in s.chars() {
        let cw = display_width(&c.to_string()) as usize;
        if w + cw > max.saturating_sub(1) {
            out.push('…');
            break;
        }
        w += cw;
        out.push(c);
    }
    out
}
