//! 轻量 Markdown 渲染（见 ADR-0003 的呈现约定）：
//! 标题、粗体、斜体、行内代码、代码块、列表、引用。
//! 不做表格对齐与图片——那两样在终端里投入产出不成比例。

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

fn h1() -> Style {
    Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD)
}

fn h2() -> Style {
    Style::new()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
}

fn code() -> Style {
    Style::new().fg(Color::Green)
}

fn quote() -> Style {
    Style::new().fg(Color::Gray).add_modifier(Modifier::ITALIC)
}

/// 把正文变成可交给 Paragraph 渲染的行。
pub fn render(body: &str) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut in_code = false;

    for raw in body.lines() {
        let line = raw.trim_end();

        if line.trim_start().starts_with("```") {
            in_code = !in_code;
            out.push(Line::from(Span::styled(
                "╌╌╌╌╌╌╌╌",
                Style::new().fg(Color::DarkGray),
            )));
            continue;
        }

        if in_code {
            out.push(Line::from(Span::styled(format!("│ {line}"), code())));
            continue;
        }

        if line.is_empty() {
            out.push(Line::from(""));
            continue;
        }

        let trimmed = line.trim_start();

        if let Some(rest) = trimmed.strip_prefix("### ") {
            out.push(Line::from(Span::styled(rest.to_string(), h2())));
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("## ") {
            out.push(Line::from(Span::styled(rest.to_string(), h2())));
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("# ") {
            out.push(Line::from(Span::styled(rest.to_string(), h1())));
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("> ") {
            out.push(Line::from(Span::styled(format!("│ {rest}"), quote())));
            continue;
        }

        // 列表与清单项：把减号换成真正的项目符号。
        for marker in ["- ", "* ", "+ "] {
            if let Some(rest) = trimmed.strip_prefix(marker) {
                let (bullet, text) = if let Some(x) = rest.strip_prefix("[ ] ") {
                    ("☐ ".to_string(), x.to_string())
                } else if let Some(x) = rest.strip_prefix("[x] ").or(rest.strip_prefix("[X] ")) {
                    ("☑ ".to_string(), x.to_string())
                } else {
                    ("• ".to_string(), rest.to_string())
                };
                let indent = line.len() - trimmed.len();
                let mut spans = vec![Span::raw(" ".repeat(indent))];
                spans.push(Span::styled(bullet, Style::new().fg(Color::Cyan)));
                spans.extend(inline(&text));
                out.push(Line::from(spans));
                break;
            }
        }
        if trimmed.starts_with("- ") || trimmed.starts_with("* ") || trimmed.starts_with("+ ") {
            continue;
        }

        out.push(Line::from(inline(trimmed)));
    }

    out
}

/// 行内的 `代码`、**粗体**、*斜体*。
fn inline(text: &str) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut buf = String::new();
    let mut i = 0;

    let flush = |buf: &mut String, spans: &mut Vec<Span<'static>>| {
        if !buf.is_empty() {
            spans.push(Span::raw(std::mem::take(buf)));
        }
    };

    while i < chars.len() {
        let c = chars[i];

        if c == '`' {
            if let Some(end) = chars[i + 1..].iter().position(|&x| x == '`') {
                flush(&mut buf, &mut spans);
                let inner: String = chars[i + 1..i + 1 + end].iter().collect();
                spans.push(Span::styled(inner, code()));
                i = i + end + 2;
                continue;
            }
        }

        if c == '*' && chars.get(i + 1) == Some(&'*') {
            if let Some(end) = find(&chars, i + 2, &['*', '*']) {
                flush(&mut buf, &mut spans);
                let inner: String = chars[i + 2..end].iter().collect();
                spans.push(Span::styled(
                    inner,
                    Style::new().add_modifier(Modifier::BOLD),
                ));
                i = end + 2;
                continue;
            }
        }

        if c == '*' && chars.get(i + 1) != Some(&'*') {
            if let Some(end) = find(&chars, i + 1, &['*']) {
                flush(&mut buf, &mut spans);
                let inner: String = chars[i + 1..end].iter().collect();
                spans.push(Span::styled(
                    inner,
                    Style::new().add_modifier(Modifier::ITALIC),
                ));
                i = end + 1;
                continue;
            }
        }

        buf.push(c);
        i += 1;
    }

    flush(&mut buf, &mut spans);
    spans
}

fn find(chars: &[char], from: usize, pat: &[char]) -> Option<usize> {
    if from >= chars.len() {
        return None;
    }
    let mut i = from;
    while i + pat.len() <= chars.len() {
        if chars[i..i + pat.len()] == *pat {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// 卡片正面用的纯文本预览：把 Markdown 记号剥掉，只留字。
pub fn plain(text: &str) -> String {
    let mut s = String::new();
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with("```") {
            continue;
        }
        let t = t
            .trim_start_matches('#')
            .trim_start_matches('>')
            .trim_start_matches('-')
            .trim_start_matches('*')
            .trim_start_matches('+')
            .trim();
        let t = t.trim_start_matches("[ ] ").trim_start_matches("[x] ");
        let t = t.replace(['*', '`'], "");
        if !t.is_empty() {
            if !s.is_empty() {
                s.push(' ');
            }
            s.push_str(&t);
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_markers_for_preview() {
        assert_eq!(plain("# 标题\n- [ ] 做事"), "标题 做事");
    }

    #[test]
    fn renders_lines_without_panicking() {
        let lines = render("# h\n\n- [x] done\n> q\n```\ncode\n```\n**b** `c` *i*");
        assert!(lines.len() > 4);
    }
}
