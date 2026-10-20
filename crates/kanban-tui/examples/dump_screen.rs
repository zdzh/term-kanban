//! 把界面渲染到 ratatui 的 TestBackend，然后把每个格子 dump 成 JSON。
//!
//! 用 `docs/make-screenshot.py` 吃这份 JSON 生成 PNG——比解析 ANSI 转义序列可靠得多。
//!
//! ```shell
//! cargo run --example dump_screen -- /tmp/demo.db 96 28 > /tmp/screen.json
//! ```

use std::io::Write;

use kanban_core::Store;
use kanban_tui::app::App;
use ratatui::backend::TestBackend;
use ratatui::style::{Color, Modifier};
use ratatui::Terminal;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let db = args.next().unwrap_or_else(|| ":memory:".into());
    let width: u16 = args.next().and_then(|s| s.parse().ok()).unwrap_or(96);
    let height: u16 = args.next().and_then(|s| s.parse().ok()).unwrap_or(28);
    // 第 4 个参数是"先按哪些键"，用 \r \t \e 表示 Enter / Tab / Esc
    let keys = args.next().unwrap_or_default();

    let store = if db == ":memory:" {
        Store::open_in_memory()?
    } else {
        Store::open(&db)?
    };
    let mut app = App::new(store);

    for k in parse_keys(&keys) {
        app.on_key(k);
    }

    let mut term = Terminal::new(TestBackend::new(width, height))?;
    term.draw(|f| kanban_tui::ui::render(&mut app, f))?;
    let buffer = term.backend().buffer();

    let mut out = String::from("{\"cells\":[");
    for y in 0..height {
        for x in 0..width {
            let cell = &buffer[(x, y)];
            let (fg_r, fg_g, fg_b) = rgb(cell.fg);
            let bg_json = match cell.bg {
                Color::Reset => "null".to_string(),
                other => {
                    let (r, g, b) = rgb(other);
                    format!("[{r},{g},{b}]")
                }
            };
            if x > 0 || y > 0 {
                out.push(',');
            }
            out.push_str(&format!(
                "[{x},{y},{},{fg_r},{fg_g},{fg_b},{bg_json},{},{}]",
                json_str(cell.symbol()),
                cell.modifier.contains(Modifier::BOLD) as u8,
                cell.modifier.contains(Modifier::CROSSED_OUT) as u8,
            ));
        }
    }
    out.push_str("]}");

    let mut stdout = std::io::stdout();
    stdout.write_all(out.as_bytes())?;
    Ok(())
}

fn parse_keys(spec: &str) -> Vec<ratatui::crossterm::event::KeyEvent> {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut out = Vec::new();
    let mut chars = spec.chars();
    while let Some(c) = chars.next() {
        let code = match c {
            '\\' => match chars.next() {
                Some('r') | Some('n') => KeyCode::Enter,
                Some('t') => KeyCode::Tab,
                Some('e') => KeyCode::Esc,
                Some(' ') => KeyCode::Char(' '),
                Some(other) => KeyCode::Char(other),
                None => break,
            },
            ' ' => KeyCode::Char(' '),
            other => KeyCode::Char(other),
        };
        out.push(KeyEvent::new(code, KeyModifiers::NONE));
    }
    out
}

fn json_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push(' '),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn rgb(c: Color) -> (u8, u8, u8) {
    match c {
        Color::Reset => (229, 229, 229),
        Color::Black => (0, 0, 0),
        Color::Red => (205, 49, 49),
        Color::Green => (13, 188, 121),
        Color::Yellow => (229, 229, 16),
        Color::Blue => (36, 114, 200),
        Color::Magenta => (188, 63, 188),
        Color::Cyan => (17, 168, 205),
        Color::Gray => (229, 229, 229),
        Color::DarkGray => (102, 102, 102),
        Color::LightRed => (241, 76, 76),
        Color::LightGreen => (35, 209, 139),
        Color::LightYellow => (245, 245, 67),
        Color::LightBlue => (59, 142, 234),
        Color::LightMagenta => (214, 112, 214),
        Color::LightCyan => (41, 184, 219),
        Color::White => (255, 255, 255),
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Indexed(n) => (110, 110, 120).max(if n == 0 { (0, 0, 0) } else { (200, 200, 200) }),
    }
}
