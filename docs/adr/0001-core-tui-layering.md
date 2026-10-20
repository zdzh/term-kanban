# core 与前端分层，Web 作为第三个适配器

`kanban-core` 拥有域模型、存储与动作执行，不依赖任何 `ratatui` 类型；`kanban-tui` 只负责渲染与输入，拖动等手势状态机全在前端。现在不启动 HTTP 服务——将来出 Web 版时新增 `kanban-server`（axum）把 core 包成 API，而不是现在就付进程、端口与鉴权的成本。

**考虑过的方案**：TUI 直接连本地 HTTP，保证 API 与未来 Web 同构。否决原因：每个拖拽手势多几毫秒延迟，且第一周就要写鉴权。**约束**：core 的变更接口必须是 `MoveCard { card_id, to_group, position }` 这类意图，而不是行号/像素。
