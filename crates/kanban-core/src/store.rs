use std::fmt;
use std::path::Path;

use chrono::{DateTime, Local};
use rusqlite::{params, Connection, OptionalExtension};

use crate::model::{
    Action, ActionKind, ButtonKind, Card, CardWorkspace, Group, GroupButton, GroupHook, Relation,
    RelationKind, Run, Subtask,
};

#[derive(Debug)]
pub enum StoreError {
    Sql(rusqlite::Error),
    NotFound(&'static str),
    /// 非空分组不许删：想删就先把里面的卡移走或归档。
    GroupNotEmpty(i64),
    /// 文件、进程、日志这类外部世界的问题。
    Io(String),
    /// 数据库是更新版本写的，旧程序不敢动它。
    SchemaTooNew(i64),
    /// 会成环的关联。
    RelationCycle,
    /// 配置本身不合法（路径不存在、模板变量拼错等）。
    InvalidConfig(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Sql(e) => write!(f, "数据库错误: {e}"),
            StoreError::NotFound(what) => write!(f, "找不到{what}"),
            StoreError::GroupNotEmpty(id) => {
                write!(f, "分组 {id} 里还有卡片，不能删除；请先归档或移走")
            }
            StoreError::Io(msg) => write!(f, "{msg}"),
            StoreError::SchemaTooNew(v) => write!(
                f,
                "数据库 schema 版本 {v} 比本程序支持的 {SCHEMA_VERSION} 新，请升级程序"
            ),
            StoreError::RelationCycle => write!(f, "这条关联会形成环，已拒绝"),
            StoreError::InvalidConfig(msg) => write!(f, "配置不合法：{msg}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        StoreError::Sql(e)
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// 当前 schema 版本，写在 `PRAGMA user_version` 里。
///
/// v1：最初的四张表（groups / cards / subtasks / actions）+ runs
/// v2：分组配置字段、分组按钮、分组钩子、卡片工作区、卡片关联；
///     runs 重建以支持"按钮/钩子触发的执行"（action_id 可为空）
pub const SCHEMA_VERSION: i64 = 2;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS groups (
    id          INTEGER PRIMARY KEY,
    name        TEXT    NOT NULL,
    position    INTEGER NOT NULL,
    archived_at TEXT
);

CREATE TABLE IF NOT EXISTS cards (
    id           INTEGER PRIMARY KEY,
    group_id     INTEGER NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    title        TEXT    NOT NULL,
    body         TEXT    NOT NULL DEFAULT '',
    done         INTEGER NOT NULL DEFAULT 0,
    completed_at TEXT,
    position     INTEGER NOT NULL,
    archived_at  TEXT
);

CREATE TABLE IF NOT EXISTS subtasks (
    id           INTEGER PRIMARY KEY,
    card_id      INTEGER NOT NULL REFERENCES cards(id) ON DELETE CASCADE,
    text         TEXT    NOT NULL,
    done         INTEGER NOT NULL DEFAULT 0,
    completed_at TEXT,
    position     INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS actions (
    id             INTEGER PRIMARY KEY,
    card_id        INTEGER NOT NULL REFERENCES cards(id) ON DELETE CASCADE,
    kind           TEXT    NOT NULL,
    label          TEXT    NOT NULL,
    target         TEXT    NOT NULL,
    needs_terminal INTEGER NOT NULL DEFAULT 0,
    position       INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS runs (
    id         INTEGER PRIMARY KEY,
    action_id  INTEGER NOT NULL REFERENCES actions(id) ON DELETE CASCADE,
    started_at TEXT    NOT NULL,
    ended_at   TEXT,
    exit_code  INTEGER,
    log_path   TEXT    NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_cards_group ON cards(group_id);
CREATE INDEX IF NOT EXISTS idx_subtasks_card ON subtasks(card_id);
CREATE INDEX IF NOT EXISTS idx_actions_card ON actions(card_id);
"#;

pub struct Store {
    conn: Connection,
}

fn now() -> DateTime<Local> {
    Local::now()
}

fn ts(t: DateTime<Local>) -> String {
    t.to_rfc3339()
}

fn parse_ts(s: Option<String>) -> Option<DateTime<Local>> {
    s.and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
        .map(|t| t.with_timezone(&Local))
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        // WAL：让未来的 Web 版与 TUI 能同时读写同一份数据。
        let _: String = conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;

        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(StoreError::SchemaTooNew(version));
        }

        conn.execute_batch(SCHEMA)?;
        if version < 2 {
            // 迁移要么全成要么全不成：中途被打断会留下半套 schema
            let tx = conn.transaction()?;
            Self::migrate_to_v2(&tx)?;
            tx.execute_batch(&format!("PRAGMA user_version={SCHEMA_VERSION}"))?;
            tx.commit()?;
        }

        let store = Store { conn };
        if store.groups(false, false)?.is_empty() {
            for name in ["待办", "本周", "以后"] {
                store.create_group(name)?;
            }
        }
        Ok(store)
    }

    /// v1 → v2。可重复执行：加列遇到"已存在"就跳过。
    fn migrate_to_v2(conn: &Connection) -> Result<()> {
        Self::add_column(conn, "groups", "repo_path", "TEXT")?;
        Self::add_column(conn, "groups", "worktree_root", "TEXT")?;
        Self::add_column(conn, "groups", "branch_template", "TEXT")?;

        // runs 原来把 action_id 设成 NOT NULL，按钮与钩子没有 action 可挂，
        // 只能重建这张表（SQLite 不能直接去掉 NOT NULL）。
        let has_old_shape: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('runs') WHERE name='action_id' AND \"notnull\"=1",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n > 0)
            .unwrap_or(false);
        if has_old_shape {
            conn.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS runs_v2 (
                    id         INTEGER PRIMARY KEY,
                    action_id  INTEGER REFERENCES actions(id) ON DELETE CASCADE,
                    source     TEXT    NOT NULL DEFAULT 'action',
                    command    TEXT    NOT NULL DEFAULT '',
                    started_at TEXT    NOT NULL,
                    ended_at   TEXT,
                    exit_code  INTEGER,
                    log_path   TEXT    NOT NULL
                );
                DELETE FROM runs_v2;
                INSERT INTO runs_v2 (id, action_id, source, started_at, ended_at, exit_code, log_path)
                    SELECT id, action_id, 'action', started_at, ended_at, exit_code, log_path FROM runs;
                DROP TABLE runs;
                ALTER TABLE runs_v2 RENAME TO runs;
                "#,
            )?;
        }

        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS group_buttons (
                id             INTEGER PRIMARY KEY,
                group_id       INTEGER NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
                label          TEXT    NOT NULL,
                kind           TEXT    NOT NULL,
                open_template  TEXT    NOT NULL DEFAULT '',
                target         TEXT    NOT NULL DEFAULT '',
                needs_terminal INTEGER NOT NULL DEFAULT 0,
                position       INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS group_hooks (
                id       INTEGER PRIMARY KEY,
                group_id INTEGER NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
                event    TEXT    NOT NULL,
                run      TEXT    NOT NULL,
                confirm  INTEGER NOT NULL DEFAULT 1,
                position INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS card_workspaces (
                card_id        INTEGER PRIMARY KEY REFERENCES cards(id) ON DELETE CASCADE,
                repo_path      TEXT NOT NULL,
                path           TEXT NOT NULL,
                branch         TEXT NOT NULL,
                created_at     TEXT NOT NULL,
                last_opened_at TEXT,
                UNIQUE(path)
            );

            CREATE TABLE IF NOT EXISTS card_relations (
                id         INTEGER PRIMARY KEY,
                from_card  INTEGER NOT NULL REFERENCES cards(id) ON DELETE CASCADE,
                to_card    INTEGER NOT NULL REFERENCES cards(id) ON DELETE CASCADE,
                kind       TEXT    NOT NULL,
                created_at TEXT    NOT NULL,
                UNIQUE(from_card, to_card, kind),
                CHECK (from_card <> to_card)
            );

            CREATE INDEX IF NOT EXISTS idx_group_buttons_group ON group_buttons(group_id);
            CREATE INDEX IF NOT EXISTS idx_group_hooks_group ON group_hooks(group_id);
            CREATE INDEX IF NOT EXISTS idx_relations_from ON card_relations(from_card);
            CREATE INDEX IF NOT EXISTS idx_relations_to ON card_relations(to_card);
            "#,
        )?;
        Ok(())
    }

    fn add_column(conn: &Connection, table: &str, column: &str, decl: &str) -> Result<()> {
        let sql = format!("ALTER TABLE {table} ADD COLUMN {column} {decl}");
        match conn.execute_batch(&sql) {
            Ok(()) => Ok(()),
            Err(e) if e.to_string().contains("duplicate column") => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    // ---------- 分组 ----------

    /// `include_archived`: 归档视图要能看见被归档的分组。
    pub fn groups(&self, include_archived: bool, only_archived: bool) -> Result<Vec<Group>> {
        let mut sql = String::from(
            "SELECT id, name, position, archived_at, repo_path, worktree_root, branch_template
             FROM groups WHERE 1=1",
        );
        if only_archived {
            sql.push_str(" AND archived_at IS NOT NULL");
        } else if !include_archived {
            sql.push_str(" AND archived_at IS NULL");
        }
        sql.push_str(" ORDER BY position, id");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([], Self::map_group)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn map_group(r: &rusqlite::Row<'_>) -> rusqlite::Result<Group> {
        Ok(Group {
            id: r.get(0)?,
            name: r.get(1)?,
            position: r.get(2)?,
            archived_at: parse_ts(r.get(3)?),
            repo_path: r.get(4)?,
            worktree_root: r.get(5)?,
            branch_template: r.get(6)?,
        })
    }

    pub fn group(&self, id: i64) -> Result<Group> {
        self.conn
            .query_row(
                "SELECT id, name, position, archived_at, repo_path, worktree_root, branch_template
                 FROM groups WHERE id=?1",
                params![id],
                Self::map_group,
            )
            .optional()?
            .ok_or(StoreError::NotFound("分组"))
    }

    pub fn create_group(&self, name: &str) -> Result<i64> {
        let pos: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(position), -1) + 1 FROM groups",
            [],
            |r| r.get(0),
        )?;
        self.conn.execute(
            "INSERT INTO groups (name, position) VALUES (?1, ?2)",
            params![name, pos],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn rename_group(&self, id: i64, name: &str) -> Result<()> {
        self.conn
            .execute("UPDATE groups SET name=?2 WHERE id=?1", params![id, name])?;
        Ok(())
    }

    /// 分组左移（-1）或右移（+1）。
    pub fn shift_group(&self, id: i64, delta: i64) -> Result<()> {
        let all = self.groups(false, false)?;
        let Some(idx) = all.iter().position(|g| g.id == id) else {
            return Err(StoreError::NotFound("分组"));
        };
        let target = idx as i64 + delta;
        if target < 0 || target >= all.len() as i64 {
            return Ok(());
        }
        let mut order: Vec<i64> = all.iter().map(|g| g.id).collect();
        order.swap(idx, target as usize);
        for (i, gid) in order.iter().enumerate() {
            self.conn.execute(
                "UPDATE groups SET position=?2 WHERE id=?1",
                params![gid, i as i64],
            )?;
        }
        Ok(())
    }

    /// 归档分组：只在分组自己身上盖章，组内卡片一个字节都不动。
    pub fn archive_group(&self, id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE groups SET archived_at=?2 WHERE id=?1",
            params![id, ts(now())],
        )?;
        Ok(())
    }

    pub fn unarchive_group(&self, id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE groups SET archived_at=NULL WHERE id=?1",
            params![id],
        )?;
        Ok(())
    }

    /// 只允许删空分组。
    pub fn delete_group(&self, id: i64) -> Result<()> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM cards WHERE group_id=?1",
            params![id],
            |r| r.get(0),
        )?;
        if count > 0 {
            return Err(StoreError::GroupNotEmpty(id));
        }
        self.conn
            .execute("DELETE FROM groups WHERE id=?1", params![id])?;
        Ok(())
    }

    // ---------- 卡片 ----------

    /// 一个分组里的卡片，按看板的显示顺序：
    /// 未完成段（position 升序）在前，已完成段（completed_at 倒序）在后。
    pub fn cards_in_group(&self, group_id: i64, include_archived: bool) -> Result<Vec<Card>> {
        let mut sql = String::from(
            "SELECT id, group_id, title, body, done, completed_at, position, archived_at
             FROM cards WHERE group_id=?1",
        );
        if !include_archived {
            sql.push_str(" AND archived_at IS NULL");
        }
        sql.push_str(" ORDER BY done ASC, CASE WHEN done=0 THEN position ELSE 0 END ASC, completed_at DESC, id");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![group_id], Self::map_card)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// 归档视图里的散卡：自己被打上归档标记的卡片。
    pub fn archived_cards(&self) -> Result<Vec<Card>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, group_id, title, body, done, completed_at, position, archived_at
             FROM cards WHERE archived_at IS NOT NULL ORDER BY archived_at DESC",
        )?;
        let rows = stmt.query_map([], Self::map_card)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn map_card(r: &rusqlite::Row<'_>) -> rusqlite::Result<Card> {
        Ok(Card {
            id: r.get(0)?,
            group_id: r.get(1)?,
            title: r.get(2)?,
            body: r.get(3)?,
            done: r.get::<_, i64>(4)? != 0,
            completed_at: parse_ts(r.get(5)?),
            position: r.get(6)?,
            archived_at: parse_ts(r.get(7)?),
        })
    }

    pub fn card(&self, id: i64) -> Result<Card> {
        self.conn
            .query_row(
                "SELECT id, group_id, title, body, done, completed_at, position, archived_at
                 FROM cards WHERE id=?1",
                params![id],
                Self::map_card,
            )
            .optional()?
            .ok_or(StoreError::NotFound("卡片"))
    }

    /// 新卡追加到未完成段末尾。
    pub fn create_card(&self, group_id: i64, title: &str) -> Result<i64> {
        let pos: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(position), -1) + 1 FROM cards WHERE group_id=?1 AND done=0",
            params![group_id],
            |r| r.get(0),
        )?;
        self.conn.execute(
            "INSERT INTO cards (group_id, title, body, position) VALUES (?1, ?2, '', ?3)",
            params![group_id, title, pos],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn set_title(&self, id: i64, title: &str) -> Result<()> {
        self.conn
            .execute("UPDATE cards SET title=?2 WHERE id=?1", params![id, title])?;
        Ok(())
    }

    pub fn set_body(&self, id: i64, body: &str) -> Result<()> {
        self.conn
            .execute("UPDATE cards SET body=?2 WHERE id=?1", params![id, body])?;
        Ok(())
    }

    /// 勾选/取消完成。勾上写完成时间；取消即清空完成时间，并追加到未完成段末尾。
    pub fn set_done(&self, id: i64, done: bool) -> Result<()> {
        if done {
            self.conn.execute(
                "UPDATE cards SET done=1, completed_at=?2 WHERE id=?1",
                params![id, ts(now())],
            )?;
        } else {
            let card = self.card(id)?;
            let pos: i64 = self.conn.query_row(
                "SELECT COALESCE(MAX(position), -1) + 1 FROM cards
                 WHERE group_id=?1 AND done=0 AND id<>?2",
                params![card.group_id, id],
                |r| r.get(0),
            )?;
            self.conn.execute(
                "UPDATE cards SET done=0, completed_at=NULL, position=?2 WHERE id=?1",
                params![id, pos],
            )?;
        }
        Ok(())
    }

    /// 换分组或在未完成段内重新排序。`index` 是落点在未完成段里的下标。
    pub fn move_card(&self, card_id: i64, to_group: i64, index: usize) -> Result<()> {
        let card = self.card(card_id)?;
        if card.done {
            // 拖拽不能改变完成状态；已完成的卡片不接受拖拽。
            return Ok(());
        }
        self.conn.execute(
            "UPDATE cards SET group_id=?2 WHERE id=?1",
            params![card_id, to_group],
        )?;

        let mut ids: Vec<i64> = {
            let mut stmt = self.conn.prepare(
                "SELECT id FROM cards WHERE group_id=?1 AND done=0 AND id<>?2 ORDER BY position, id",
            )?;
            let rows = stmt.query_map(params![to_group, card_id], |r| r.get::<_, i64>(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let idx = index.min(ids.len());
        ids.insert(idx, card_id);
        for (i, cid) in ids.iter().enumerate() {
            self.conn.execute(
                "UPDATE cards SET position=?2 WHERE id=?1",
                params![cid, i as i64],
            )?;
        }
        Ok(())
    }

    pub fn archive_card(&self, id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE cards SET archived_at=?2 WHERE id=?1",
            params![id, ts(now())],
        )?;
        Ok(())
    }

    /// 恢复到原分组、原位置——归档从来没动过 group_id 与 position。
    pub fn unarchive_card(&self, id: i64) -> Result<()> {
        self.conn
            .execute("UPDATE cards SET archived_at=NULL WHERE id=?1", params![id])?;
        Ok(())
    }

    /// 硬删除只存在于归档视图（见 docs/adr/0007-no-hard-delete-on-board.md）。
    pub fn hard_delete_card(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM cards WHERE id=?1", params![id])?;
        Ok(())
    }

    // ---------- 子任务 ----------

    pub fn subtasks(&self, card_id: i64) -> Result<Vec<Subtask>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, card_id, text, done, completed_at, position
             FROM subtasks WHERE card_id=?1 ORDER BY position, id",
        )?;
        let rows = stmt.query_map(params![card_id], |r| {
            Ok(Subtask {
                id: r.get(0)?,
                card_id: r.get(1)?,
                text: r.get(2)?,
                done: r.get::<_, i64>(3)? != 0,
                completed_at: parse_ts(r.get(4)?),
                position: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn add_subtask(&self, card_id: i64, text: &str) -> Result<i64> {
        let pos: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(position), -1) + 1 FROM subtasks WHERE card_id=?1",
            params![card_id],
            |r| r.get(0),
        )?;
        self.conn.execute(
            "INSERT INTO subtasks (card_id, text, position) VALUES (?1, ?2, ?3)",
            params![card_id, text, pos],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn set_subtask_done(&self, id: i64, done: bool) -> Result<()> {
        if done {
            self.conn.execute(
                "UPDATE subtasks SET done=1, completed_at=?2 WHERE id=?1",
                params![id, ts(now())],
            )?;
        } else {
            self.conn.execute(
                "UPDATE subtasks SET done=0, completed_at=NULL WHERE id=?1",
                params![id],
            )?;
        }
        Ok(())
    }

    /// 按当前状态取反：看板上直接点子任务的复选框时用。
    pub fn toggle_subtask(&self, id: i64) -> Result<()> {
        let done: i64 = self
            .conn
            .query_row("SELECT done FROM subtasks WHERE id=?1", params![id], |r| {
                r.get(0)
            })
            .optional()?
            .ok_or(StoreError::NotFound("子任务"))?;
        self.set_subtask_done(id, done == 0)
    }

    pub fn delete_subtask(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM subtasks WHERE id=?1", params![id])?;
        Ok(())
    }

    /// 子任务上移（-1）或下移（+1）。
    pub fn shift_subtask(&self, id: i64, delta: i64) -> Result<()> {
        let card_id: i64 = self
            .conn
            .query_row(
                "SELECT card_id FROM subtasks WHERE id=?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?
            .ok_or(StoreError::NotFound("子任务"))?;
        let all = self.subtasks(card_id)?;
        let Some(idx) = all.iter().position(|s| s.id == id) else {
            return Ok(());
        };
        let target = idx as i64 + delta;
        if target < 0 || target >= all.len() as i64 {
            return Ok(());
        }
        let mut order: Vec<i64> = all.iter().map(|s| s.id).collect();
        order.swap(idx, target as usize);
        for (i, sid) in order.iter().enumerate() {
            self.conn.execute(
                "UPDATE subtasks SET position=?2 WHERE id=?1",
                params![sid, i as i64],
            )?;
        }
        Ok(())
    }

    // ---------- 动作 ----------

    pub fn actions(&self, card_id: i64) -> Result<Vec<Action>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, card_id, kind, label, target, needs_terminal, position
             FROM actions WHERE card_id=?1 ORDER BY position, id",
        )?;
        let rows = stmt.query_map(params![card_id], |r| {
            let kind: String = r.get(2)?;
            Ok(Action {
                id: r.get(0)?,
                card_id: r.get(1)?,
                kind: ActionKind::parse(&kind).unwrap_or(ActionKind::Command),
                label: r.get(3)?,
                target: r.get(4)?,
                needs_terminal: r.get::<_, i64>(5)? != 0,
                position: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn action(&self, id: i64) -> Result<Action> {
        let mut stmt = self.conn.prepare(
            "SELECT id, card_id, kind, label, target, needs_terminal, position
             FROM actions WHERE id=?1",
        )?;
        let a = stmt
            .query_map(params![id], |r| {
                let kind: String = r.get(2)?;
                Ok(Action {
                    id: r.get(0)?,
                    card_id: r.get(1)?,
                    kind: ActionKind::parse(&kind).unwrap_or(ActionKind::Command),
                    label: r.get(3)?,
                    target: r.get(4)?,
                    needs_terminal: r.get::<_, i64>(5)? != 0,
                    position: r.get(6)?,
                })
            })?
            .next()
            .transpose()?;
        a.ok_or(StoreError::NotFound("动作"))
    }

    pub fn add_action(
        &self,
        card_id: i64,
        kind: ActionKind,
        label: &str,
        target: &str,
        needs_terminal: bool,
    ) -> Result<i64> {
        let pos: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(position), -1) + 1 FROM actions WHERE card_id=?1",
            params![card_id],
            |r| r.get(0),
        )?;
        self.conn.execute(
            "INSERT INTO actions (card_id, kind, label, target, needs_terminal, position)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                card_id,
                kind.as_str(),
                label,
                target,
                needs_terminal as i64,
                pos
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn delete_action(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM actions WHERE id=?1", params![id])?;
        Ok(())
    }

    /// 动作上移（-1）或下移（+1）。
    pub fn shift_action(&self, id: i64, delta: i64) -> Result<()> {
        let card_id: i64 = self
            .conn
            .query_row(
                "SELECT card_id FROM actions WHERE id=?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?
            .ok_or(StoreError::NotFound("动作"))?;
        let all = self.actions(card_id)?;
        let Some(idx) = all.iter().position(|a| a.id == id) else {
            return Ok(());
        };
        let target = idx as i64 + delta;
        if target < 0 || target >= all.len() as i64 {
            return Ok(());
        }
        let mut order: Vec<i64> = all.iter().map(|a| a.id).collect();
        order.swap(idx, target as usize);
        for (i, aid) in order.iter().enumerate() {
            self.conn.execute(
                "UPDATE actions SET position=?2 WHERE id=?1",
                params![aid, i as i64],
            )?;
        }
        Ok(())
    }

    /// 标记一个命令是否需要终端（ssh/vim 这类交互程序要，构建类不要）。
    pub fn set_action_needs_terminal(&self, id: i64, needs: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE actions SET needs_terminal=?2 WHERE id=?1",
            params![id, needs as i64],
        )?;
        Ok(())
    }

    // ---------- 分组配置 ----------

    /// 三个配置字段一起写；传 None 表示清空。
    pub fn set_group_config(
        &self,
        id: i64,
        repo_path: Option<&str>,
        worktree_root: Option<&str>,
        branch_template: Option<&str>,
    ) -> Result<()> {
        let clean = |v: Option<&str>| v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        self.conn.execute(
            "UPDATE groups SET repo_path=?2, worktree_root=?3, branch_template=?4 WHERE id=?1",
            params![
                id,
                clean(repo_path),
                clean(worktree_root),
                clean(branch_template)
            ],
        )?;
        Ok(())
    }

    // ---------- 分组按钮 ----------

    pub fn group_buttons(&self, group_id: i64) -> Result<Vec<GroupButton>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, group_id, label, kind, open_template, target, needs_terminal, position
             FROM group_buttons WHERE group_id=?1 ORDER BY position, id",
        )?;
        let rows = stmt.query_map(params![group_id], |r| {
            let kind: String = r.get(3)?;
            Ok(GroupButton {
                id: r.get(0)?,
                group_id: r.get(1)?,
                label: r.get(2)?,
                kind: ButtonKind::parse(&kind).unwrap_or(ButtonKind::Command),
                open_template: r.get(4)?,
                target: r.get(5)?,
                needs_terminal: r.get::<_, i64>(6)? != 0,
                position: r.get(7)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn add_group_button(
        &self,
        group_id: i64,
        label: &str,
        kind: ButtonKind,
        open_template: &str,
        target: &str,
        needs_terminal: bool,
    ) -> Result<i64> {
        let pos: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(position), -1) + 1 FROM group_buttons WHERE group_id=?1",
            params![group_id],
            |r| r.get(0),
        )?;
        self.conn.execute(
            "INSERT INTO group_buttons
                (group_id, label, kind, open_template, target, needs_terminal, position)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                group_id,
                label,
                kind.as_str(),
                open_template,
                target,
                needs_terminal as i64,
                pos
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn set_group_button_terminal(&self, id: i64, needs: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE group_buttons SET needs_terminal=?2 WHERE id=?1",
            params![id, needs as i64],
        )?;
        Ok(())
    }

    pub fn update_group_button(
        &self,
        id: i64,
        label: &str,
        kind: ButtonKind,
        open_template: &str,
        target: &str,
        needs_terminal: bool,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE group_buttons
             SET label=?2, kind=?3, open_template=?4, target=?5, needs_terminal=?6
             WHERE id=?1",
            params![
                id,
                label,
                kind.as_str(),
                open_template,
                target,
                needs_terminal as i64
            ],
        )?;
        Ok(())
    }

    pub fn delete_group_button(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM group_buttons WHERE id=?1", params![id])?;
        Ok(())
    }

    pub fn shift_group_button(&self, id: i64, delta: i64) -> Result<()> {
        let group_id: i64 = self
            .conn
            .query_row(
                "SELECT group_id FROM group_buttons WHERE id=?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?
            .ok_or(StoreError::NotFound("按钮"))?;
        let all = self.group_buttons(group_id)?;
        let Some(idx) = all.iter().position(|b| b.id == id) else {
            return Ok(());
        };
        let target = idx as i64 + delta;
        if target < 0 || target >= all.len() as i64 {
            return Ok(());
        }
        let mut order: Vec<i64> = all.iter().map(|b| b.id).collect();
        order.swap(idx, target as usize);
        for (i, bid) in order.iter().enumerate() {
            self.conn.execute(
                "UPDATE group_buttons SET position=?2 WHERE id=?1",
                params![bid, i as i64],
            )?;
        }
        Ok(())
    }

    // ---------- 分组钩子 ----------

    pub fn group_hooks(&self, group_id: i64) -> Result<Vec<GroupHook>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, group_id, event, run, confirm, position
             FROM group_hooks WHERE group_id=?1 ORDER BY position, id",
        )?;
        let rows = stmt.query_map(params![group_id], |r| {
            Ok(GroupHook {
                id: r.get(0)?,
                group_id: r.get(1)?,
                event: r.get(2)?,
                run: r.get(3)?,
                confirm: r.get::<_, i64>(4)? != 0,
                position: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn add_group_hook(
        &self,
        group_id: i64,
        event: &str,
        run: &str,
        confirm: bool,
    ) -> Result<i64> {
        let pos: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(position), -1) + 1 FROM group_hooks WHERE group_id=?1",
            params![group_id],
            |r| r.get(0),
        )?;
        self.conn.execute(
            "INSERT INTO group_hooks (group_id, event, run, confirm, position)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![group_id, event, run, confirm as i64, pos],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn update_group_hook(&self, id: i64, run: &str, confirm: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE group_hooks SET run=?2, confirm=?3 WHERE id=?1",
            params![id, run, confirm as i64],
        )?;
        Ok(())
    }

    pub fn delete_group_hook(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM group_hooks WHERE id=?1", params![id])?;
        Ok(())
    }

    // ---------- 卡片工作区 ----------

    pub fn workspace_for_card(&self, card_id: i64) -> Result<Option<CardWorkspace>> {
        let mut stmt = self.conn.prepare(
            "SELECT card_id, repo_path, path, branch, created_at, last_opened_at
             FROM card_workspaces WHERE card_id=?1",
        )?;
        let row = stmt
            .query_map(params![card_id], |r| {
                Ok(CardWorkspace {
                    card_id: r.get(0)?,
                    repo_path: r.get(1)?,
                    path: r.get(2)?,
                    branch: r.get(3)?,
                    created_at: parse_ts(Some(r.get::<_, String>(4)?)).unwrap_or_else(now),
                    last_opened_at: parse_ts(r.get(5)?),
                })
            })?
            .next()
            .transpose()?;
        Ok(row)
    }

    pub fn save_workspace(&self, ws: &CardWorkspace) -> Result<()> {
        self.conn.execute(
            "INSERT INTO card_workspaces (card_id, repo_path, path, branch, created_at, last_opened_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(card_id) DO UPDATE SET
                repo_path=excluded.repo_path,
                path=excluded.path,
                branch=excluded.branch,
                last_opened_at=excluded.last_opened_at",
            params![
                ws.card_id,
                ws.repo_path,
                ws.path,
                ws.branch,
                ts(ws.created_at),
                ws.last_opened_at.map(ts)
            ],
        )?;
        Ok(())
    }

    pub fn touch_workspace(&self, card_id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE card_workspaces SET last_opened_at=?2 WHERE card_id=?1",
            params![card_id, ts(now())],
        )?;
        Ok(())
    }

    pub fn delete_workspace(&self, card_id: i64) -> Result<()> {
        self.conn.execute(
            "DELETE FROM card_workspaces WHERE card_id=?1",
            params![card_id],
        )?;
        Ok(())
    }

    /// 所有活工作区，带卡片标题与分组名（给总览面板用）。
    pub fn all_workspaces(&self) -> Result<Vec<(CardWorkspace, String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT w.card_id, w.repo_path, w.path, w.branch, w.created_at, w.last_opened_at,
                    c.title, g.name
             FROM card_workspaces w
             JOIN cards c ON c.id = w.card_id
             JOIN groups g ON g.id = c.group_id
             ORDER BY w.last_opened_at DESC, w.created_at DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                CardWorkspace {
                    card_id: r.get(0)?,
                    repo_path: r.get(1)?,
                    path: r.get(2)?,
                    branch: r.get(3)?,
                    created_at: parse_ts(Some(r.get::<_, String>(4)?)).unwrap_or_else(now),
                    last_opened_at: parse_ts(r.get(5)?),
                },
                r.get::<_, String>(6)?,
                r.get::<_, String>(7)?,
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    // ---------- 卡片关联 ----------

    pub fn relations_from(&self, card_id: i64) -> Result<Vec<Relation>> {
        self.query_relations("from_card", card_id)
    }

    pub fn relations_to(&self, card_id: i64) -> Result<Vec<Relation>> {
        self.query_relations("to_card", card_id)
    }

    fn query_relations(&self, column: &str, card_id: i64) -> Result<Vec<Relation>> {
        let sql = format!(
            "SELECT id, from_card, to_card, kind, created_at
             FROM card_relations WHERE {column}=?1 ORDER BY id"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![card_id], |r| {
            let kind: String = r.get(3)?;
            Ok(Relation {
                id: r.get(0)?,
                from_card: r.get(1)?,
                to_card: r.get(2)?,
                kind: RelationKind::parse(&kind).unwrap_or(RelationKind::ParentChild),
                created_at: parse_ts(Some(r.get::<_, String>(4)?)).unwrap_or_else(now),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// 卡片挂了多少关联（正面显示 `⤷N` 用）。
    pub fn relation_count(&self, card_id: i64) -> Result<usize> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM card_relations WHERE from_card=?1 OR to_card=?1",
            params![card_id],
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }

    /// 加一条关联；会成环的拒绝。
    pub fn add_relation(&self, from: i64, to: i64, kind: RelationKind) -> Result<i64> {
        if from == to {
            return Err(StoreError::InvalidConfig("不能关联到自己".into()));
        }
        // 从 to 出发能走到 from，说明加这条边会成环
        let mut stack = vec![to];
        let mut seen = std::collections::HashSet::new();
        while let Some(cur) = stack.pop() {
            if cur == from {
                return Err(StoreError::RelationCycle);
            }
            if !seen.insert(cur) {
                continue;
            }
            for r in self.relations_from(cur)? {
                stack.push(r.to_card);
            }
        }
        match self.conn.execute(
            "INSERT INTO card_relations (from_card, to_card, kind, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![from, to, kind.as_str(), ts(now())],
        ) {
            Ok(_) => Ok(self.conn.last_insert_rowid()),
            Err(e) if e.to_string().contains("UNIQUE") => {
                Err(StoreError::InvalidConfig("这条关联已经存在".into()))
            }
            Err(e) => Err(e.into()),
        }
    }

    pub fn delete_relation(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM card_relations WHERE id=?1", params![id])?;
        Ok(())
    }

    // ---------- 运行记录 ----------

    pub fn start_run(
        &self,
        action_id: Option<i64>,
        source: &str,
        command: &str,
        log_path: &str,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO runs (action_id, source, command, started_at, log_path)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![action_id, source, command, ts(now()), log_path],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn finish_run(&self, run_id: i64, exit_code: Option<i32>) -> Result<()> {
        self.conn.execute(
            "UPDATE runs SET ended_at=?2, exit_code=?3 WHERE id=?1",
            params![run_id, ts(now()), exit_code],
        )?;
        Ok(())
    }

    pub fn last_run_for_card(&self, card_id: i64) -> Result<Option<Run>> {
        let mut stmt = self.conn.prepare(
            "SELECT r.id, r.action_id, r.source, r.command, r.started_at, r.ended_at,
                    r.exit_code, r.log_path
             FROM runs r JOIN actions a ON a.id = r.action_id
             WHERE a.card_id=?1 ORDER BY r.id DESC LIMIT 1",
        )?;
        let run = stmt
            .query_map(params![card_id], Self::map_run)?
            .next()
            .transpose()?;
        Ok(run)
    }

    fn map_run(r: &rusqlite::Row<'_>) -> rusqlite::Result<Run> {
        Ok(Run {
            id: r.get(0)?,
            action_id: r.get(1)?,
            source: r.get(2)?,
            command: r.get(3)?,
            started_at: parse_ts(Some(r.get::<_, String>(4)?)).unwrap_or_else(now),
            ended_at: parse_ts(r.get(5)?),
            exit_code: r.get(6)?,
            log_path: r.get(7)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        Store::open_in_memory().unwrap()
    }

    #[test]
    fn seeds_three_groups() {
        let s = store();
        let gs = s.groups(false, false).unwrap();
        assert_eq!(gs.len(), 3);
    }

    #[test]
    fn new_card_goes_to_end_of_open_segment() {
        let s = store();
        let g = s.groups(false, false).unwrap()[0].id;
        let a = s.create_card(g, "a").unwrap();
        let b = s.create_card(g, "b").unwrap();
        let cards = s.cards_in_group(g, false).unwrap();
        assert_eq!(cards.iter().map(|c| c.id).collect::<Vec<_>>(), vec![a, b]);
    }

    #[test]
    fn done_card_sinks_and_sorts_by_completed_at_desc() {
        let s = store();
        let g = s.groups(false, false).unwrap()[0].id;
        let a = s.create_card(g, "a").unwrap();
        let b = s.create_card(g, "b").unwrap();
        s.set_done(a, true).unwrap();
        let cards = s.cards_in_group(g, false).unwrap();
        assert_eq!(cards[0].id, b);
        assert_eq!(cards[1].id, a);
        assert!(cards[1].completed_at.is_some());
    }

    #[test]
    fn uncheck_clears_completed_at_and_appends_to_end() {
        let s = store();
        let g = s.groups(false, false).unwrap()[0].id;
        let a = s.create_card(g, "a").unwrap();
        let b = s.create_card(g, "b").unwrap();
        s.set_done(a, true).unwrap();
        s.set_done(a, false).unwrap();
        let cards = s.cards_in_group(g, false).unwrap();
        assert_eq!(cards.last().unwrap().id, a);
        assert_eq!(cards.last().unwrap().completed_at, None);
        assert_eq!(cards[0].id, b);
    }

    #[test]
    fn move_card_reorders_within_group() {
        let s = store();
        let g = s.groups(false, false).unwrap()[0].id;
        let a = s.create_card(g, "a").unwrap();
        let b = s.create_card(g, "b").unwrap();
        s.move_card(b, g, 0).unwrap();
        let cards = s.cards_in_group(g, false).unwrap();
        assert_eq!(cards[0].id, b);
        assert_eq!(cards[1].id, a);
    }

    #[test]
    fn group_archive_does_not_touch_cards() {
        let s = store();
        let g = s.groups(false, false).unwrap()[0].id;
        let a = s.create_card(g, "a").unwrap();
        s.archive_group(g).unwrap();
        assert!(s.groups(false, false).unwrap().iter().all(|x| x.id != g));
        // 卡片自己没被盖章
        assert_eq!(s.card(a).unwrap().archived_at, None);
        s.unarchive_group(g).unwrap();
        assert_eq!(s.cards_in_group(g, false).unwrap().len(), 1);
    }

    #[test]
    fn non_empty_group_cannot_be_deleted() {
        let s = store();
        let g = s.groups(false, false).unwrap()[0].id;
        s.create_card(g, "a").unwrap();
        assert!(matches!(
            s.delete_group(g),
            Err(StoreError::GroupNotEmpty(_))
        ));
    }

    #[test]
    fn subtask_does_not_auto_complete_parent() {
        let s = store();
        let g = s.groups(false, false).unwrap()[0].id;
        let c = s.create_card(g, "c").unwrap();
        let st = s.add_subtask(c, "x").unwrap();
        s.set_subtask_done(st, true).unwrap();
        assert!(!s.card(c).unwrap().done);
    }
}
