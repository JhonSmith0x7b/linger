use chrono::{Local, TimeZone, Utc};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    path::Path,
    sync::{Mutex, MutexGuard},
    time::Duration,
};

type Result<T> = std::result::Result<T, String>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: i64,
    pub role: String,
    pub content: String,
    pub created_at: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    pub id: i64,
    pub content: String,
    pub updated_at: String,
    pub scope: String,
    pub expires_at: Option<String>,
}

pub struct Store {
    connection: Mutex<Connection>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let mut connection = Connection::open(path).map_err(db_error)?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(db_error)?;
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;
            CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT, role TEXT NOT NULL,
                content TEXT NOT NULL, created_at TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS memories (
                id INTEGER PRIMARY KEY AUTOINCREMENT, content TEXT NOT NULL,
                updated_at TEXT NOT NULL, source TEXT NOT NULL DEFAULT 'explicit',
                source_through INTEGER, deleted INTEGER NOT NULL DEFAULT 0);
            CREATE TABLE IF NOT EXISTS metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            INSERT OR IGNORE INTO metadata (key,value) VALUES ('memory_revision','0');
        ",
            )
            .map_err(db_error)?;
        // Additive migration for databases created by the initial development build.
        let columns = {
            let mut stmt = connection
                .prepare("PRAGMA table_info(memories)")
                .map_err(db_error)?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(1))
                .map_err(db_error)?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(db_error)?
        };
        if !columns.iter().any(|name| name == "scope") {
            connection
                .execute(
                    "ALTER TABLE memories ADD COLUMN scope TEXT NOT NULL DEFAULT 'long_term'",
                    [],
                )
                .map_err(db_error)?;
        }
        if !columns.iter().any(|name| name == "expires_at") {
            connection
                .execute("ALTER TABLE memories ADD COLUMN expires_at TEXT", [])
                .map_err(db_error)?;
        }
        expire_in_connection(&mut connection, true)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }
    fn lock(&self) -> Result<MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| "数据库锁失效，请重启应用".into())
    }
    pub fn messages(&self) -> Result<Vec<Message>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare("SELECT id,role,content,created_at FROM (SELECT * FROM messages ORDER BY id DESC LIMIT 200) ORDER BY id").map_err(db_error)?;
        let rows = stmt
            .query_map([], message_row)
            .map_err(db_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(db_error)?;
        Ok(rows)
    }
    pub fn messages_after(&self, id: i64, limit: usize) -> Result<Vec<Message>> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare(
                "SELECT id,role,content,created_at FROM messages WHERE id>?1 ORDER BY id LIMIT ?2",
            )
            .map_err(db_error)?;
        let rows = stmt
            .query_map(params![id, limit.min(1000) as i64], message_row)
            .map_err(db_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(db_error)?;
        Ok(rows)
    }
    /// Literal substring lookup works for short Chinese queries without FTS tokenization assumptions.
    pub fn search_messages(&self, query: &str, limit: usize) -> Result<Vec<Message>> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        let conn = self.lock()?;
        let mut stmt = conn.prepare("SELECT id,role,content,created_at FROM messages WHERE instr(lower(content),lower(?1))>0 ORDER BY id DESC LIMIT ?2").map_err(db_error)?;
        let rows = stmt
            .query_map(params![query, limit.min(50) as i64], message_row)
            .map_err(db_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(db_error)?;
        Ok(rows)
    }
    pub fn add_message(&self, role: &str, content: &str) -> Result<Message> {
        if !["user", "assistant", "system", "tool", "proactive"].contains(&role) {
            return Err("未知消息角色".into());
        }
        let conn = self.lock()?;
        let timestamp = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO messages(role,content,created_at) VALUES (?1,?2,?3)",
            params![role, content, timestamp],
        )
        .map_err(db_error)?;
        Ok(Message {
            id: conn.last_insert_rowid(),
            role: role.into(),
            content: content.into(),
            created_at: timestamp,
        })
    }
    pub fn memories(&self) -> Result<Vec<Memory>> {
        self.expire_memories()?;
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare("SELECT id,content,updated_at,scope,expires_at FROM memories WHERE deleted=0 ORDER BY id DESC")
            .map_err(db_error)?;
        let rows = stmt
            .query_map([], |row| {
                Ok(Memory {
                    id: row.get(0)?,
                    content: row.get(1)?,
                    updated_at: row.get(2)?,
                    scope: row.get(3)?,
                    expires_at: row.get(4)?,
                })
            })
            .map_err(db_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(db_error)?;
        Ok(rows)
    }
    pub fn search_memories(&self, query: &str, limit: usize) -> Result<Vec<Memory>> {
        self.expire_memories()?;
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        let conn = self.lock()?;
        let mut stmt = conn.prepare("SELECT id,content,updated_at,scope,expires_at FROM memories WHERE deleted=0 AND instr(lower(content),lower(?1))>0 ORDER BY id DESC LIMIT ?2").map_err(db_error)?;
        let rows = stmt
            .query_map(params![query.trim(), limit.min(50) as i64], |row| {
                Ok(Memory {
                    id: row.get(0)?,
                    content: row.get(1)?,
                    updated_at: row.get(2)?,
                    scope: row.get(3)?,
                    expires_at: row.get(4)?,
                })
            })
            .map_err(db_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(db_error)?;
        Ok(rows)
    }
    pub fn expire_memories(&self) -> Result<usize> {
        let mut conn = self.lock()?;
        expire_in_connection(&mut conn, false)
    }
    #[cfg(test)]
    pub fn add_memory(&self, content: &str) -> Result<Memory> {
        self.add_memory_scoped(content, "long_term")
    }
    pub fn add_memory_scoped(&self, content: &str, scope: &str) -> Result<Memory> {
        let expires_at = scope_expiration(scope)?;
        let content = valid_memory(content)?;
        let mut conn = self.lock()?;
        let tx = conn.transaction().map_err(db_error)?;
        let timestamp = Utc::now().to_rfc3339();
        tx.execute(
            "INSERT INTO memories(content,updated_at,scope,expires_at) VALUES (?1,?2,?3,?4)",
            params![content, timestamp, scope, expires_at],
        )
        .map_err(db_error)?;
        let id = tx.last_insert_rowid();
        invalidate_memory_context(&tx)?;
        tx.commit().map_err(db_error)?;
        Ok(Memory {
            id,
            content: content.into(),
            updated_at: timestamp,
            scope: scope.into(),
            expires_at,
        })
    }
    pub fn update_memory(&self, id: i64, content: &str) -> Result<Memory> {
        self.expire_memories()?;
        let content = valid_memory(content)?;
        let mut conn = self.lock()?;
        let tx = conn.transaction().map_err(db_error)?;
        let timestamp = Utc::now().to_rfc3339();
        let changed = tx.execute("UPDATE memories SET content=?1,updated_at=?2,source='explicit' WHERE id=?3 AND deleted=0",params![content,timestamp,id]).map_err(db_error)?;
        if changed == 0 {
            return Err("该记忆不存在或已删除".into());
        }
        let (scope, expires_at): (String, Option<String>) = tx
            .query_row(
                "SELECT scope,expires_at FROM memories WHERE id=?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(db_error)?;
        invalidate_memory_context(&tx)?;
        tx.commit().map_err(db_error)?;
        Ok(Memory {
            id,
            content: content.into(),
            updated_at: timestamp,
            scope,
            expires_at,
        })
    }
    pub fn delete_memory(&self, id: i64) -> Result<()> {
        let mut conn = self.lock()?;
        let tx = conn.transaction().map_err(db_error)?;
        let changed = tx
            .execute(
                "UPDATE memories SET deleted=1,updated_at=?1 WHERE id=?2 AND deleted=0",
                params![Utc::now().to_rfc3339(), id],
            )
            .map_err(db_error)?;
        if changed == 0 {
            return Err("该记忆不存在或已删除".into());
        }
        invalidate_memory_context(&tx)?;
        tx.commit().map_err(db_error)
    }
    pub fn load_settings(&self) -> Result<Option<Value>> {
        self.get_meta("settings")?
            .map(|s| serde_json::from_str(&s).map_err(|_| "设置数据损坏".into()))
            .transpose()
    }
    pub fn save_settings(&self, settings: &Value) -> Result<()> {
        if settings
            .as_object()
            .is_some_and(|s| s.keys().any(|k| k.to_lowercase().contains("key")))
        {
            return Err("API Key 不得写入普通设置".into());
        }
        self.set_meta("settings", &settings.to_string())
    }
    pub fn get_meta(&self, key: &str) -> Result<Option<String>> {
        self.lock()?
            .query_row("SELECT value FROM metadata WHERE key=?1", [key], |r| {
                r.get(0)
            })
            .optional()
            .map_err(db_error)
    }
    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.lock()?.execute("INSERT INTO metadata(key,value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![key,value]).map_err(db_error)?;
        Ok(())
    }
    pub fn memory_revision(&self) -> Result<i64> {
        Ok(self
            .get_meta("memory_revision")?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0))
    }
    /// Optimistic transaction: stale extraction never overwrites an explicit edit or deletion.
    /// Only the processed range advances; newer messages remain pending.
    pub fn apply_memory_batch(
        &self,
        expected_revision: i64,
        contents: &[String],
        summary: &str,
        processed_through: i64,
    ) -> Result<bool> {
        self.expire_memories()?;
        for content in contents {
            valid_memory(content)?;
        }
        let mut conn = self.lock()?;
        let tx = conn.transaction().map_err(db_error)?;
        let revision: i64 = tx
            .query_row(
                "SELECT CAST(value AS INTEGER) FROM metadata WHERE key='memory_revision'",
                [],
                |r| r.get(0),
            )
            .map_err(db_error)?;
        if revision != expected_revision {
            return Ok(false);
        }
        let timestamp = Utc::now().to_rfc3339();
        for content in contents {
            tx.execute("INSERT INTO memories(content,updated_at,source,source_through) SELECT ?1,?2,'batch',?3 WHERE NOT EXISTS(SELECT 1 FROM memories WHERE content=?1)",params![content.trim(),timestamp,processed_through]).map_err(db_error)?;
        }
        for (key, value) in [
            ("summary", summary.to_owned()),
            ("processed_through", processed_through.to_string()),
            ("last_organized_at", timestamp),
            ("organized_at", Utc::now().timestamp().to_string()),
            ("memory_revision", (revision + 1).to_string()),
        ] {
            tx.execute("INSERT INTO metadata(key,value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![key,value]).map_err(db_error)?;
        }
        tx.commit().map_err(db_error)?;
        Ok(true)
    }
}
fn scope_expiration(scope: &str) -> Result<Option<String>> {
    match scope {
        "long_term" | "session" => Ok(None),
        "today" => {
            let tomorrow = Local::now()
                .date_naive()
                .succ_opt()
                .ok_or("无法确定次日日期")?;
            let midnight = tomorrow.and_hms_opt(0, 0, 0).ok_or("无法确定到期时间")?;
            // Some timezones transition at midnight. Use the first valid local instant.
            for minutes in 0..=180 {
                if let Some(end) = Local
                    .from_local_datetime(&(midnight + chrono::Duration::minutes(minutes)))
                    .earliest()
                {
                    return Ok(Some(end.with_timezone(&Utc).to_rfc3339()));
                }
            }
            Err("当前时区无法确定今日记忆的到期时间".into())
        }
        _ => Err("记忆范围须为 session、today 或 long_term".into()),
    }
}
fn expire_in_connection(conn: &mut Connection, startup: bool) -> Result<usize> {
    let tx = conn.transaction().map_err(db_error)?;
    let changed = tx.execute(
        "UPDATE memories SET deleted=1 WHERE deleted=0 AND ((?1 AND scope='session') OR (expires_at IS NOT NULL AND julianday(expires_at)<=julianday(?2)))",
        params![startup, Utc::now().to_rfc3339()],
    ).map_err(db_error)?;
    if changed > 0 {
        invalidate_memory_context(&tx)?;
    }
    tx.commit().map_err(db_error)?;
    Ok(changed)
}
fn message_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Message> {
    Ok(Message {
        id: row.get(0)?,
        role: row.get(1)?,
        content: row.get(2)?,
        created_at: row.get(3)?,
    })
}
fn valid_memory(content: &str) -> Result<&str> {
    let content = content.trim();
    if content.is_empty() || content.chars().count() > 2000 {
        return Err("记忆内容须为 1–2000 个字符".into());
    }
    Ok(content)
}
fn invalidate_memory_context(tx: &Transaction<'_>) -> Result<()> {
    tx.execute(
        "UPDATE metadata SET value=CAST(value AS INTEGER)+1 WHERE key='memory_revision'",
        [],
    )
    .map_err(db_error)?;
    tx.execute("DELETE FROM metadata WHERE key='summary'", [])
        .map_err(db_error)?;
    // Raw history remains visible, but historical preferences before this boundary are not authoritative.
    tx.execute("INSERT INTO metadata(key,value) VALUES ('memory_history_floor',(SELECT CAST(COALESCE(MAX(id),0) AS TEXT) FROM messages)) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[]).map_err(db_error)?;
    Ok(())
}
fn db_error(error: rusqlite::Error) -> String {
    format!("本地数据库操作失败：{error}")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn store() -> Store {
        Store::open(Path::new(":memory:")).unwrap()
    }
    #[test]
    fn history_search_supports_short_chinese_and_literal_wildcards() {
        let db = store();
        db.add_message("user", "请重点看左边 100% 的进度").unwrap();
        db.add_message("assistant", "好的").unwrap();
        assert_eq!(db.search_messages("左边", 10).unwrap().len(), 1);
        assert_eq!(db.search_messages("%", 10).unwrap().len(), 1);
        assert_eq!(db.messages_after(1, 10).unwrap().len(), 1);
    }
    #[test]
    fn explicit_changes_reject_stale_batches_and_invalidate_summary() {
        let db = store();
        let memory = db.add_memory("看左边").unwrap();
        let revision = db.memory_revision().unwrap();
        db.set_meta("summary", "看左边").unwrap();
        db.update_memory(memory.id, "看右边").unwrap();
        assert!(!db
            .apply_memory_batch(revision, &["看左边".into()], "旧摘要", 10)
            .unwrap());
        assert_eq!(db.memories().unwrap()[0].content, "看右边");
        assert!(db.get_meta("summary").unwrap().is_none());
        db.delete_memory(memory.id).unwrap();
        assert!(db.memories().unwrap().is_empty());
    }
    #[test]
    fn batch_does_not_resurrect_exact_deleted_memory() {
        let db = store();
        let memory = db.add_memory("看左边").unwrap();
        db.delete_memory(memory.id).unwrap();
        let revision = db.memory_revision().unwrap();
        assert!(db
            .apply_memory_batch(revision, &["看左边".into()], "", 4)
            .unwrap());
        assert!(db.memories().unwrap().is_empty());
        assert_eq!(
            db.get_meta("processed_through").unwrap().as_deref(),
            Some("4")
        );
    }
    #[test]
    fn data_survives_reopening_database() {
        let path = std::env::temp_dir().join(format!(
            "linger-store-test-{}-{}.sqlite",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        {
            let db = Store::open(&path).unwrap();
            db.add_message("user", "记住我喜欢安静").unwrap();
            db.add_memory("喜欢安静").unwrap();
            db.save_settings(&serde_json::json!({"voice":"coral"}))
                .unwrap();
        }
        {
            let db = Store::open(&path).unwrap();
            assert_eq!(db.messages().unwrap()[0].content, "记住我喜欢安静");
            assert_eq!(db.memories().unwrap()[0].content, "喜欢安静");
            assert_eq!(db.load_settings().unwrap().unwrap()["voice"], "coral");
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn today_expiry_invalidates_summary_history_and_stale_batch() {
        let db = store();
        db.add_message("user", "今天看左边").unwrap();
        let memory = db.add_memory_scoped("今天看左边", "today").unwrap();
        assert_eq!(memory.scope, "today");
        assert!(memory.expires_at.is_some());
        db.set_meta("summary", "用户今天关注左边").unwrap();
        let revision = db.memory_revision().unwrap();
        db.lock()
            .unwrap()
            .execute(
                "UPDATE memories SET expires_at='2000-01-01T00:00:00Z' WHERE id=?1",
                [memory.id],
            )
            .unwrap();
        assert_eq!(db.expire_memories().unwrap(), 1);
        assert_eq!(db.expire_memories().unwrap(), 0);
        assert!(db.memories().unwrap().is_empty());
        assert!(db.get_meta("summary").unwrap().is_none());
        assert_eq!(
            db.get_meta("memory_history_floor").unwrap().as_deref(),
            Some("1")
        );
        assert!(!db
            .apply_memory_batch(revision, &["看左边".into()], "旧摘要", 1)
            .unwrap());
    }
    #[test]
    fn session_memory_expires_on_restart_but_long_term_survives() {
        let path = std::env::temp_dir().join(format!(
            "linger-session-test-{}-{}.sqlite",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        {
            let db = Store::open(&path).unwrap();
            db.add_memory_scoped("这次关注右上角", "session").unwrap();
            db.add_memory("喜欢安静").unwrap();
            assert_eq!(db.memories().unwrap().len(), 2);
            db.set_meta("summary", "这次关注右上角").unwrap();
        }
        {
            let db = Store::open(&path).unwrap();
            assert_eq!(db.memories().unwrap().len(), 1);
            assert_eq!(db.memories().unwrap()[0].scope, "long_term");
            assert!(db.get_meta("summary").unwrap().is_none());
        }
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn update_preserves_scope_and_batch_sets_timestamp_atomically() {
        let db = store();
        let memory = db.add_memory_scoped("看左", "today").unwrap();
        let updated = db.update_memory(memory.id, "看右").unwrap();
        assert_eq!(updated.scope, "today");
        assert_eq!(updated.expires_at, memory.expires_at);
        let revision = db.memory_revision().unwrap();
        db.apply_memory_batch(revision, &[], "摘要", 0).unwrap();
        assert!(db
            .get_meta("organized_at")
            .unwrap()
            .unwrap()
            .parse::<i64>()
            .is_ok());
        assert!(db.add_memory_scoped("测试", "forever-ish").is_err());
    }

    #[test]
    fn memory_search_includes_old_active_entries_but_excludes_expired_and_deleted() {
        let db = store();
        let old = db.add_memory("喜欢安静").unwrap();
        let deleted = db.add_memory("喜欢安静的雨天").unwrap();
        db.delete_memory(deleted.id).unwrap();
        let expired = db.add_memory_scoped("今天喜欢安静", "today").unwrap();
        db.lock()
            .unwrap()
            .execute(
                "UPDATE memories SET expires_at='2000-01-01T00:00:00Z' WHERE id=?1",
                [expired.id],
            )
            .unwrap();
        for i in 0..10 {
            db.add_memory(&format!("其他记录 {i}")).unwrap();
        }
        let results = db.search_memories("安静", 5).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, old.id);
        assert!(db.search_memories("", 5).unwrap().is_empty());
    }

    #[test]
    fn refuses_secrets_in_settings() {
        let db = store();
        assert!(db
            .save_settings(&serde_json::json!({"api_key":"secret"}))
            .is_err());
        db.save_settings(&serde_json::json!({"base_url":"http://localhost/v1"}))
            .unwrap();
        assert!(db.load_settings().unwrap().is_some());
    }
}
