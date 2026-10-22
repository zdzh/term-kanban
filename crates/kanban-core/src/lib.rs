//! 域模型：分组、卡片、子任务、动作。
//!
//! 这里不含任何前端类型（见 docs/adr/0001-core-tui-layering.md）。

pub mod exec;
pub mod git;
pub mod model;
pub mod store;
pub mod template;
pub mod workspace;

pub use model::{
    Action, ActionKind, ButtonKind, Card, CardWorkspace, Group, GroupButton, GroupHook, Relation,
    RelationKind, Subtask,
};
pub use store::{Store, StoreError};
pub use template::Vars;
