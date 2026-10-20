//! 前端逻辑与渲染。`main.rs` 只是把它接到真实终端上。
//!
//! 单独拆成 lib 是为了让测试和 `examples/dump_screen.rs` 能在
//! ratatui 的 TestBackend 里渲染同一套代码。

pub mod app;
pub mod markdown;
pub mod ui;
