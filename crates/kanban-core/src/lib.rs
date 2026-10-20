//! 域模型：分组、卡片、子任务、动作。
//!
//! 这里不含任何前端类型（见 docs/adr/0001-core-tui-layering.md）。

pub mod exec;
pub mod model;
pub mod store;

pub use model::{Action, ActionKind, Card, Group, Subtask};
pub use store::{Store, StoreError};
