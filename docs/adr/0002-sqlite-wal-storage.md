# 存储用 SQLite（WAL），不用 JSON 文件

数据落在单个 SQLite 文件，`journal_mode=WAL`，由 `rusqlite` 直接访问，不用 ORM。

JSON 全量读改写在多进程下必然丢更新：将来 TUI 与 Web 版可能同时打开同一份数据。WAL 允许并发读写、各自只改自己那几行。代价是要维护 schema 与迁移（约一两百行），这是本项目唯一一条以后改起来很贵的决定。
