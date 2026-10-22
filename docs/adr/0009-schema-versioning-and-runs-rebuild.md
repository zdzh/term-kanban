# schema 用 PRAGMA user_version 版本化，runs 表在 v2 重建

原来只有 `CREATE TABLE IF NOT EXISTS`，加列会直接失败。v2 起用 `PRAGMA user_version` 记录版本，按版本顺序迁移；打开比程序更新的库时拒绝启动，避免旧版写坏新数据。

v2 里 `runs.action_id` 从 NOT NULL 改成可空，因为分组按钮与钩子触发的执行没有 action 可挂。SQLite 不能直接去掉 NOT NULL，只能建新表搬数据再改名——这段看起来多余的 DDL 是必要的，不要"简化"掉。新增 `source`（action/button/hook）与 `command` 两列，执行记账仍然只有一条通道（见 ADR-0006）。
