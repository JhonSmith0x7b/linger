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
/// A summary and its exact coverage boundary are read with the remaining raw messages.
#[derive(Debug, Clone)]
pub struct ContextSnapshot {
    pub revision: i64,
    pub summary: String,
    pub through: i64,
    pub floor: i64,
    pub messages: Vec<Message>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    pub id: i64,
    pub content: String,
    pub updated_at: String,
    pub scope: String,
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserProfile {
    pub content: String,
    pub revision: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct IntroductionState {
    pub introduced: bool,
    pub asked_name: bool,
    pub legacy_history: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReferenceVoice {
    pub voice_id: String,
    pub name: String,
    pub target_model: String,
    pub status: String,
    pub created_at: String,
    #[serde(default)]
    pub in_use_by: Vec<String>,
}

pub fn reference_voice_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 60 || name.chars().any(char::is_control) {
        return Err("音色名称请填写 1–60 个字符，不含换行或控制字符".into());
    }
    Ok(name.to_owned())
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
            CREATE TABLE IF NOT EXISTS reference_voices (
                endpoint TEXT NOT NULL, voice_id TEXT NOT NULL, name TEXT NOT NULL,
                target_model TEXT NOT NULL, status TEXT NOT NULL, created_at TEXT NOT NULL,
                deleted INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(endpoint,voice_id));
            CREATE TABLE IF NOT EXISTS metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS summary_versions (
                id INTEGER PRIMARY KEY AUTOINCREMENT, created_at TEXT NOT NULL,
                previous_through INTEGER NOT NULL, through INTEGER NOT NULL,
                previous_summary TEXT NOT NULL, summary TEXT NOT NULL,
                before_tokens INTEGER NOT NULL, after_tokens INTEGER NOT NULL);
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
    pub fn last_user_message_time(&self) -> Result<Option<String>> {
        self.lock()?
            .query_row(
                "SELECT created_at FROM messages WHERE role='user' ORDER BY id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_error)
    }
    #[cfg(test)]
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
    /// Read one chronological UI page; the model context keeps its independent history budget.
    pub fn message_page(&self, before_id: Option<i64>, limit: usize) -> Result<Vec<Message>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT id,role,content,created_at FROM (SELECT id,role,content,created_at FROM messages WHERE (?1 IS NULL OR id<?1) ORDER BY id DESC LIMIT ?2) ORDER BY id"
        ).map_err(db_error)?;
        let rows = stmt
            .query_map(params![before_id, limit.clamp(1, 100) as i64], message_row)
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
    #[cfg(test)]
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
    pub fn introduction_state(&self) -> Result<IntroductionState> {
        let conn = self.lock()?;
        let state = read_introduction(&conn)?;
        // Initialize once, before any new turn. Existing chats are not assumed
        // to be strangers merely because this feature was just installed.
        conn.execute(
            "INSERT OR IGNORE INTO metadata(key,value) VALUES ('introduction_state',?1)",
            [serde_json::to_string(&state).map_err(|e| e.to_string())?],
        )
        .map_err(db_error)?;
        Ok(state)
    }

    pub fn add_companion_message(
        &self,
        role: &str,
        content: &str,
        introduced: bool,
        asked_name: bool,
    ) -> Result<Message> {
        if !["assistant", "proactive"].contains(&role) || content.trim().is_empty() {
            return Err("角色回复不能为空，且必须为助手或主动陪伴消息".into());
        }
        let mut conn = self.lock()?;
        let tx = conn.transaction().map_err(db_error)?;
        let mut state = read_introduction(&tx)?;
        state.introduced |= introduced;
        state.asked_name |= asked_name;
        let timestamp = Utc::now().to_rfc3339();
        tx.execute(
            "INSERT INTO messages(role,content,created_at) VALUES (?1,?2,?3)",
            params![role, content, timestamp],
        )
        .map_err(db_error)?;
        let message = Message {
            id: tx.last_insert_rowid(),
            role: role.into(),
            content: content.into(),
            created_at: timestamp,
        };
        tx.execute("INSERT INTO metadata(key,value) VALUES ('introduction_state',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [serde_json::to_string(&state).map_err(|e| e.to_string())?]).map_err(db_error)?;
        tx.commit().map_err(db_error)?;
        Ok(message)
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
        // Adding a fact must not erase the recent dialogue or its already committed summary.
        // Still invalidate any in-flight extraction that did not see the new memory.
        tx.execute(
            "UPDATE metadata SET value=CAST(value AS INTEGER)+1 WHERE key='memory_revision'",
            [],
        )
        .map_err(db_error)?;
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
        let previous: Option<String> = tx
            .query_row(
                "SELECT content FROM memories WHERE id=?1 AND deleted=0",
                [id],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_error)?;
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
        invalidate_memory_context(
            &tx,
            serde_json::json!({"kind":"updated","id":id,"previous":previous,"current":content}),
        )?;
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
        let previous: Option<String> = tx
            .query_row(
                "SELECT content FROM memories WHERE id=?1 AND deleted=0",
                [id],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_error)?;
        let changed = tx
            .execute(
                "UPDATE memories SET deleted=1,updated_at=?1 WHERE id=?2 AND deleted=0",
                params![Utc::now().to_rfc3339(), id],
            )
            .map_err(db_error)?;
        if changed == 0 {
            return Err("该记忆不存在或已删除".into());
        }
        invalidate_memory_context(
            &tx,
            serde_json::json!({"kind":"removed","id":id,"previous":previous}),
        )?;
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
    pub fn list_reference_voices(&self, endpoint: &str) -> Result<Vec<ReferenceVoice>> {
        let mut conn = self.lock()?;
        let tx = conn.transaction().map_err(db_error)?;
        // Migrate the old per-model latest-only records without overwriting names or reviving deleted voices.
        let prefix = format!("reference_voice:{endpoint}:");
        let legacy = {
            let mut stmt = tx
                .prepare("SELECT value FROM metadata WHERE substr(key,1,?1)=?2")
                .map_err(db_error)?;
            let rows = stmt
                .query_map(params![prefix.chars().count(), prefix], |r| {
                    r.get::<_, String>(0)
                })
                .map_err(db_error)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(db_error)?;
            rows
        };
        for encoded in legacy {
            let Ok(value) = serde_json::from_str::<Value>(&encoded) else {
                continue;
            };
            let (Some(id), Some(model)) =
                (value["voice_id"].as_str(), value["target_model"].as_str())
            else {
                continue;
            };
            if crate::voice_clone::model_for_voice(&format!("{model}-migration")).as_deref()
                != Some(model)
                || !id.starts_with("cosyvoice-")
                || id.len() > 256
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
            {
                continue;
            }
            let name = value["name"]
                .as_str()
                .and_then(|n| reference_voice_name(n).ok())
                .unwrap_or_else(|| {
                    format!(
                        "参考音色 · {}",
                        id.chars()
                            .rev()
                            .take(6)
                            .collect::<String>()
                            .chars()
                            .rev()
                            .collect::<String>()
                    )
                });
            tx.execute("INSERT OR IGNORE INTO reference_voices(endpoint,voice_id,name,target_model,status,created_at) VALUES (?1,?2,?3,?4,?5,?6)",
                params![endpoint,id,name,model,"UNKNOWN",Utc::now().to_rfc3339()]).map_err(db_error)?;
        }
        tx.commit().map_err(db_error)?;
        let mut stmt = conn.prepare("SELECT voice_id,name,target_model,status,created_at FROM reference_voices WHERE endpoint=?1 AND deleted=0 ORDER BY created_at DESC,voice_id").map_err(db_error)?;
        let rows = stmt
            .query_map([endpoint], |r| {
                Ok(ReferenceVoice {
                    voice_id: r.get(0)?,
                    name: r.get(1)?,
                    target_model: r.get(2)?,
                    status: r.get(3)?,
                    created_at: r.get(4)?,
                    in_use_by: Vec::new(),
                })
            })
            .map_err(db_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(db_error)?;
        Ok(rows)
    }
    pub fn upsert_reference_voice(&self, endpoint: &str, voice: &ReferenceVoice) -> Result<()> {
        let name = reference_voice_name(&voice.name)?;
        self.lock()?.execute("INSERT INTO reference_voices(endpoint,voice_id,name,target_model,status,created_at) VALUES (?1,?2,?3,?4,?5,?6)
            ON CONFLICT(endpoint,voice_id) DO UPDATE SET target_model=excluded.target_model,status=excluded.status WHERE reference_voices.deleted=0",
            params![endpoint,voice.voice_id,name,voice.target_model,voice.status,voice.created_at]).map_err(db_error)?;
        Ok(())
    }
    pub fn rename_reference_voice(&self, endpoint: &str, voice_id: &str, name: &str) -> Result<()> {
        let name = reference_voice_name(name)?;
        let changed = self.lock()?.execute("UPDATE reference_voices SET name=?3 WHERE endpoint=?1 AND voice_id=?2 AND deleted=0",params![endpoint,voice_id,name]).map_err(db_error)?;
        if changed == 0 {
            return Err("此音色不在当前连接的音色库中，请刷新列表".into());
        }
        Ok(())
    }
    pub fn delete_reference_voice(&self, endpoint: &str, voice_id: &str) -> Result<()> {
        let mut conn = self.lock()?;
        let tx = conn.transaction().map_err(db_error)?;
        // Retain a tombstone so stale cloud pages or drafts cannot resurrect a deleted voice.
        tx.execute(
            "UPDATE reference_voices SET deleted=1 WHERE endpoint=?1 AND voice_id=?2",
            params![endpoint, voice_id],
        )
        .map_err(db_error)?;
        let prefix = format!("reference_voice:{endpoint}:");
        let legacy = {
            let mut stmt = tx
                .prepare("SELECT key,value FROM metadata WHERE substr(key,1,?1)=?2")
                .map_err(db_error)?;
            let rows = stmt
                .query_map(params![prefix.chars().count(), prefix], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })
                .map_err(db_error)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(db_error)?;
            rows
        };
        for (key, encoded) in legacy {
            if serde_json::from_str::<Value>(&encoded)
                .is_ok_and(|v| v["voice_id"].as_str() == Some(voice_id))
            {
                tx.execute("DELETE FROM metadata WHERE key=?1", [key])
                    .map_err(db_error)?;
            }
        }
        tx.commit().map_err(db_error)
    }
    pub fn reference_voice_deleted(&self, _endpoint: &str, voice_id: &str) -> Result<bool> {
        self.lock()?
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM reference_voices WHERE voice_id=?1 AND deleted=1)",
                [voice_id],
                |r| r.get::<_, bool>(0),
            )
            .map_err(db_error)
    }
    /// Corrections are authoritative over older summary wording until folded into
    /// the next successfully committed state. Never infer a removed fact anew.
    pub fn memory_context_changes(&self) -> Result<String> {
        let raw = self
            .get_meta("memory_context_changes")?
            .unwrap_or_else(|| "[]".into());
        let changes: Vec<Value> = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
        bounded_context_changes(&changes)
    }
    pub fn user_profile(&self) -> Result<UserProfile> {
        let conn = self.lock()?;
        read_user_profile(&conn)
    }
    pub fn save_user_profile(&self, content: &str, expected_revision: i64) -> Result<UserProfile> {
        let content = content.trim();
        if content.chars().count() > 2000
            || tiktoken_rs::o200k_base_singleton()
                .encode_ordinary(content)
                .len()
                > 1000
        {
            return Err("用户档案最多 2000 字符、1000 token，请精简后保存".into());
        }
        let mut conn = self.lock()?;
        let tx = conn.transaction().map_err(db_error)?;
        let current = read_user_profile(&tx)?;
        if current.revision != expected_revision {
            return Err("用户档案已被其他窗口或 AI 更新，请重新载入后再修改".into());
        }
        let profile = UserProfile {
            content: content.to_owned(),
            revision: current.revision + 1,
        };
        tx.execute("INSERT INTO metadata(key,value) VALUES ('user_profile',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [&profile.content]).map_err(db_error)?;
        tx.execute("INSERT INTO metadata(key,value) VALUES ('user_profile_revision',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [profile.revision.to_string()]).map_err(db_error)?;
        append_context_change(&tx, serde_json::json!({"kind":"profile_updated"}))?;
        // The current profile supersedes past preferences. Reject in-flight work,
        // but retain the committed summary and coverage: resetting them replays
        // the entire archive into the foreground context.
        tx.execute(
            "UPDATE metadata SET value=CAST(value AS INTEGER)+1 WHERE key='memory_revision'",
            [],
        )
        .map_err(db_error)?;
        tx.commit().map_err(db_error)?;
        Ok(profile)
    }
    pub fn clear_conversation(&self) -> Result<()> {
        let mut conn = self.lock()?;
        let tx = conn.transaction().map_err(db_error)?;
        tx.execute("DELETE FROM messages", []).map_err(db_error)?;
        tx.execute("DELETE FROM summary_versions", [])
            .map_err(db_error)?;
        tx.execute("DELETE FROM metadata WHERE key IN ('summary','summary_through','processed_through','organized_at','last_organized_at','memory_history_floor','introduction_state','memory_context_changes')", []).map_err(db_error)?;
        tx.execute(
            "UPDATE metadata SET value=CAST(value AS INTEGER)+1 WHERE key='memory_revision'",
            [],
        )
        .map_err(db_error)?;
        // Preserve sqlite_sequence: a later message must not reuse an old ID.
        tx.commit().map_err(db_error)
    }
    /// One lock prevents pairing a new summary with an old raw-message boundary.
    pub fn context_snapshot(&self) -> Result<ContextSnapshot> {
        self.expire_memories()?;
        let conn = self.lock()?;
        let revision = read_meta_i64(&conn, "memory_revision")?;
        let floor = read_meta_i64(&conn, "memory_history_floor")?;
        let (summary, through) = read_context_summary(&conn)?;
        let mut stmt = conn
            .prepare("SELECT id,role,content,created_at FROM messages WHERE id>?1 ORDER BY id")
            .map_err(db_error)?;
        let messages = stmt
            .query_map([floor.max(through)], message_row)
            .map_err(db_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(db_error)?;
        Ok(ContextSnapshot {
            revision,
            summary,
            through,
            floor,
            messages,
        })
    }

    /// Commit only the captured prefix. Messages appended during generation remain raw.
    pub fn apply_context_compaction(
        &self,
        expected_revision: i64,
        expected_through: i64,
        through: i64,
        summary: &str,
        contents: &[String],
    ) -> Result<bool> {
        if summary.trim().is_empty() {
            return Err("压缩摘要为空，保留原始对话".into());
        }
        for content in contents {
            valid_memory(content)?;
        }
        self.expire_memories()?;
        let mut conn = self.lock()?;
        let tx = conn.transaction().map_err(db_error)?;
        let revision = read_meta_i64(&tx, "memory_revision")?;
        let (previous_summary, current_through) = read_context_summary(&tx)?;
        if revision != expected_revision || current_through != expected_through {
            return Ok(false);
        }
        let floor = read_meta_i64(&tx, "memory_history_floor")?;
        let boundary_exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM messages WHERE id=?1)",
                [through],
                |r| r.get(0),
            )
            .map_err(db_error)?;
        if through <= current_through.max(floor) || !boundary_exists {
            return Err("压缩消息边界无效，保留原始对话".into());
        }
        let timestamp = Utc::now().to_rfc3339();
        // Audit the exact accepted text and its replaced source, atomically with
        // the coverage boundary. These counts describe summary+source text,
        // not the complete API request (role/profile/tools are excluded).
        let source_messages = {
            let mut statement = tx.prepare("SELECT id,role,content,created_at FROM messages WHERE id>?1 AND id<=?2 ORDER BY id").map_err(db_error)?;
            let rows = statement
                .query_map(params![current_through.max(floor), through], message_row)
                .map_err(db_error)?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(db_error)?
        };
        let tokenizer = tiktoken_rs::o200k_base_singleton();
        let before_tokens = tokenizer.encode_ordinary(&previous_summary).len()
            + tokenizer
                .encode_ordinary(
                    &serde_json::to_string(&source_messages).map_err(|e| e.to_string())?,
                )
                .len();
        let after_tokens = tokenizer.encode_ordinary(summary).len();
        tx.execute("INSERT INTO summary_versions(created_at,previous_through,through,previous_summary,summary,before_tokens,after_tokens) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![timestamp, current_through, through, previous_summary, summary, before_tokens, after_tokens]).map_err(db_error)?;
        tx.execute("DELETE FROM summary_versions WHERE id NOT IN (SELECT id FROM summary_versions ORDER BY id DESC LIMIT 30)", []).map_err(db_error)?;
        let pending_changes: Option<String> = tx
            .query_row(
                "SELECT value FROM metadata WHERE key='memory_context_changes'",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_error)?;
        if let Some(pending_changes) = pending_changes {
            let mut changes: Vec<Value> =
                serde_json::from_str(&pending_changes).map_err(|e| e.to_string())?;
            // Partial prefix compaction must not retire corrections while older
            // contradictory source messages still remain in the raw tail.
            changes.retain(|change| {
                change["through"]
                    .as_i64()
                    .is_none_or(|boundary| boundary > through)
            });
            tx.execute(
                "UPDATE metadata SET value=?1 WHERE key='memory_context_changes'",
                [serde_json::to_string(&changes).map_err(|e| e.to_string())?],
            )
            .map_err(db_error)?;
        }
        for content in contents {
            tx.execute("INSERT INTO memories(content,updated_at,source,source_through) SELECT ?1,?2,'batch',?3 WHERE NOT EXISTS(SELECT 1 FROM memories WHERE content=?1)",params![content.trim(),timestamp,through]).map_err(db_error)?;
        }
        for (key, value) in [
            ("summary", summary.to_owned()),
            ("summary_through", through.to_string()),
            ("processed_through", through.to_string()),
            ("last_organized_at", timestamp),
            ("organized_at", Utc::now().timestamp().to_string()),
            ("memory_revision", (revision + 1).to_string()),
        ] {
            tx.execute("INSERT INTO metadata(key,value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![key,value]).map_err(db_error)?;
        }
        tx.commit().map_err(db_error)?;
        Ok(true)
    }

    #[cfg(test)]
    pub fn memory_revision(&self) -> Result<i64> {
        Ok(self
            .get_meta("memory_revision")?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0))
    }
    /// Optimistic transaction: stale extraction never overwrites an explicit edit or deletion.
    /// Only the processed range advances; newer messages remain pending.
    #[cfg(test)]
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
fn read_meta_i64(conn: &Connection, key: &str) -> Result<i64> {
    Ok(conn
        .query_row("SELECT value FROM metadata WHERE key=?1", [key], |r| {
            r.get::<_, String>(0)
        })
        .optional()
        .map_err(db_error)?
        .and_then(|v| v.parse().ok())
        .unwrap_or(0))
}
fn read_context_summary(conn: &Connection) -> Result<(String, i64)> {
    let through = read_meta_i64(conn, "summary_through")?;
    let summary: Option<String> = conn
        .query_row("SELECT value FROM metadata WHERE key='summary'", [], |r| {
            r.get(0)
        })
        .optional()
        .map_err(db_error)?;
    // Old processed_through was extraction progress, not proof of context coverage.
    match summary {
        Some(summary) if !summary.trim().is_empty() => Ok((summary, through.max(0))),
        _ => Ok((String::new(), 0)),
    }
}

fn read_introduction(conn: &Connection) -> Result<IntroductionState> {
    let stored: Option<String> = conn
        .query_row(
            "SELECT value FROM metadata WHERE key='introduction_state'",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_error)?;
    if let Some(stored) = stored {
        return serde_json::from_str(&stored).map_err(|_| "相识状态格式无效".into());
    }
    let legacy_history = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM messages WHERE role IN ('assistant','proactive'))",
            [],
            |r| r.get(0),
        )
        .map_err(db_error)?;
    Ok(IntroductionState {
        legacy_history,
        ..Default::default()
    })
}

fn read_user_profile(conn: &Connection) -> Result<UserProfile> {
    let content = conn
        .query_row(
            "SELECT value FROM metadata WHERE key='user_profile'",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_error)?
        .unwrap_or_default();
    let revision = conn
        .query_row(
            "SELECT CAST(value AS INTEGER) FROM metadata WHERE key='user_profile_revision'",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_error)?
        .unwrap_or(0);
    Ok(UserProfile { content, revision })
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
    let expiring = {
        let mut stmt = tx.prepare("SELECT id,content FROM memories WHERE deleted=0 AND ((?1 AND scope='session') OR (expires_at IS NOT NULL AND julianday(expires_at)<=julianday(?2)))").map_err(db_error)?;
        let rows = stmt
            .query_map(params![startup, Utc::now().to_rfc3339()], |r| {
                Ok(serde_json::json!({"id":r.get::<_,i64>(0)?,"previous":r.get::<_,String>(1)?}))
            })
            .map_err(db_error)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(db_error)?
    };
    let changed = tx.execute(
        "UPDATE memories SET deleted=1 WHERE deleted=0 AND ((?1 AND scope='session') OR (expires_at IS NOT NULL AND julianday(expires_at)<=julianday(?2)))",
        params![startup, Utc::now().to_rfc3339()],
    ).map_err(db_error)?;
    if changed > 0 {
        invalidate_memory_context(
            &tx,
            serde_json::json!({"kind":"expired","memories":expiring}),
        )?;
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
// This is prompt-facing metadata, not an archive dump. Full persisted corrections
// are used for coverage tracking; previews are explicitly labelled when partial.
fn bounded_context_changes(changes: &[Value]) -> Result<String> {
    if changes.is_empty() {
        return Ok("[]".into());
    }
    fn preview(value: &Value) -> Value {
        let text = value.as_str().unwrap_or_default();
        let excerpt: String = text.chars().take(80).collect();
        serde_json::json!({"text":excerpt,"truncated":text.chars().count()>80})
    }
    fn compact(change: &Value) -> Value {
        let mut value = serde_json::json!({"kind":change["kind"],"at":change["at"],"through":change["through"]});
        match change["kind"].as_str() {
            Some("profile_updated") => {
                value["rule"] = serde_json::json!("当前完整用户档案替代旧档案偏好，包括已移除的内容；不得从旧摘要恢复。正文见当前档案。");
            }
            Some("expired") => {
                let memories = change["memories"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                value["memories"] = serde_json::json!(memories.iter().rev().take(4).map(|m| serde_json::json!({"id":m["id"],"previous_preview":preview(&m["previous"])})).collect::<Vec<_>>());
                value["omitted_memories"] = serde_json::json!(memories.len().saturating_sub(4));
            }
            _ => {
                value["id"] = change["id"].clone();
                value["previous_preview"] = preview(&change["previous"]);
                if change.get("current").is_some() {
                    value["current_preview"] = preview(&change["current"]);
                }
            }
        }
        value
    }
    let mut payload = serde_json::json!({
        "rule":"近期权威修正优先于旧摘要。预览不是完整事实；truncated 或 omitted 表示省略。已删除/过期内容不再有效；有效记忆与当前档案优先，不能从旧摘要恢复被撤销的规则。必要时读取当前档案或搜索有效记忆。",
        "changes":[], "omitted_changes":changes.len()
    });
    let tokenizer = tiktoken_rs::o200k_base_singleton();
    let mut selected = Vec::new();
    for change in changes.iter().rev() {
        selected.push(compact(change));
        payload["changes"] = serde_json::json!(selected);
        payload["omitted_changes"] = serde_json::json!(changes.len() - selected.len());
        let candidate = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
        if tokenizer.encode_ordinary(&candidate).len() > 800 {
            selected.pop();
            break;
        }
    }
    payload["changes"] = serde_json::json!(selected);
    payload["omitted_changes"] = serde_json::json!(changes.len() - selected.len());
    serde_json::to_string(&payload).map_err(|e| e.to_string())
}

fn append_context_change(tx: &Transaction<'_>, mut change: Value) -> Result<()> {
    let stored: Option<String> = tx
        .query_row(
            "SELECT value FROM metadata WHERE key='memory_context_changes'",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_error)?;
    let mut changes: Vec<Value> = stored
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    change["at"] = serde_json::json!(Utc::now().to_rfc3339());
    change["through"] = serde_json::json!(tx
        .query_row("SELECT COALESCE(MAX(id),0) FROM messages", [], |r| r
            .get::<_, i64>(0))
        .map_err(db_error)?);
    changes.push(change);
    if changes.len() > 20 {
        changes.drain(..changes.len() - 20);
    }
    tx.execute("INSERT INTO metadata(key,value) VALUES ('memory_context_changes',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [serde_json::to_string(&changes).map_err(|e| e.to_string())?]).map_err(db_error)?;
    Ok(())
}
fn invalidate_memory_context(tx: &Transaction<'_>, change: Value) -> Result<()> {
    tx.execute(
        "UPDATE metadata SET value=CAST(value AS INTEGER)+1 WHERE key='memory_revision'",
        [],
    )
    .map_err(db_error)?;
    // Keep both committed coverage and recent original messages. Corrections are
    // supplied separately so old summary statements cannot override current data.
    append_context_change(tx, change)
}
fn db_error(error: rusqlite::Error) -> String {
    format!("本地数据库操作失败：{error}")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn context_snapshot_never_silently_caps_raw_history() {
        let db = store();
        for i in 0..250 {
            db.add_message("user", &format!("message {i}")).unwrap();
        }
        db.set_meta("summary", "legacy summary").unwrap();
        db.set_meta("processed_through", "200").unwrap();
        let snapshot = db.context_snapshot().unwrap();
        assert_eq!(snapshot.messages.len(), 250);
        assert_eq!(snapshot.through, 0);
        assert_eq!(snapshot.summary, "legacy summary");
    }

    #[test]
    fn compaction_commits_prefix_and_preserves_concurrent_append() {
        let db = store();
        let first = db.add_message("user", "今晚运动完成").unwrap();
        let second = db.add_message("assistant", "好，继续聊游戏").unwrap();
        let snapshot = db.context_snapshot().unwrap();
        let newer = db.add_message("user", "新话题").unwrap();
        assert!(db
            .apply_context_compaction(
                snapshot.revision,
                snapshot.through,
                second.id,
                "今晚已运动，随后聊游戏",
                &[]
            )
            .unwrap());
        let after = db.context_snapshot().unwrap();
        assert_eq!(after.summary, "今晚已运动，随后聊游戏");
        assert_eq!(after.through, second.id);
        assert_eq!(
            after.messages.iter().map(|m| m.id).collect::<Vec<_>>(),
            vec![newer.id]
        );
        assert!(db.messages().unwrap().iter().any(|m| m.id == first.id));
        assert!(!db
            .apply_context_compaction(
                snapshot.revision,
                snapshot.through,
                newer.id,
                "过时结果",
                &[]
            )
            .unwrap());
    }

    #[test]
    fn compaction_rejects_invalid_empty_and_cleared_snapshots() {
        let db = store();
        let first = db.add_message("user", "原话").unwrap();
        let snapshot = db.context_snapshot().unwrap();
        for (through, summary) in [(first.id, ""), (0, "摘要"), (first.id + 100, "摘要")] {
            assert!(db
                .apply_context_compaction(
                    snapshot.revision,
                    snapshot.through,
                    through,
                    summary,
                    &[]
                )
                .is_err());
        }
        assert_eq!(db.context_snapshot().unwrap().messages.len(), 1);
        db.clear_conversation().unwrap();
        assert!(!db
            .apply_context_compaction(snapshot.revision, snapshot.through, first.id, "旧对话", &[])
            .unwrap());
        let current = db.context_snapshot().unwrap();
        assert!(current.messages.is_empty());
        assert!(current.summary.is_empty());
        assert_eq!(current.through, 0);
    }

    #[test]
    fn remembering_new_fact_preserves_context_and_rejects_stale_compaction() {
        let db = store();
        let old = db.add_message("user", "今晚运动完成").unwrap();
        let initial = db.context_snapshot().unwrap();
        assert!(db
            .apply_context_compaction(initial.revision, 0, old.id, "今晚已经运动", &[])
            .unwrap());
        let recent = db.add_message("user", "现在聊游戏").unwrap();
        let in_flight = db.context_snapshot().unwrap();
        db.add_memory_scoped("喜欢玩游戏", "long_term").unwrap();
        let after = db.context_snapshot().unwrap();
        assert_eq!(after.summary, in_flight.summary);
        assert_eq!(after.through, old.id);
        assert_eq!(after.floor, in_flight.floor);
        assert_eq!(
            after.messages.iter().map(|m| m.id).collect::<Vec<_>>(),
            vec![recent.id]
        );
        assert!(after.revision > in_flight.revision);
        assert!(!db
            .apply_context_compaction(
                in_flight.revision,
                in_flight.through,
                recent.id,
                "过时摘要",
                &[]
            )
            .unwrap());
    }

    #[test]
    fn profile_edit_preserves_summary_boundary_and_rejects_stale_work() {
        let db = store();
        let first = db.add_message("user", "今晚运动完成").unwrap();
        let snapshot = db.context_snapshot().unwrap();
        assert!(db
            .apply_context_compaction(snapshot.revision, 0, first.id, "今晚已运动", &[])
            .unwrap());
        let recent = db.add_message("user", "现在聊游戏").unwrap();
        let before = db.context_snapshot().unwrap();
        db.save_user_profile("每天提醒运动", 0).unwrap();
        let after = db.context_snapshot().unwrap();
        assert_eq!(after.summary, before.summary);
        assert_eq!(after.through, first.id);
        assert_eq!(after.floor, before.floor);
        assert_eq!(
            after.messages.iter().map(|m| m.id).collect::<Vec<_>>(),
            vec![recent.id]
        );
        assert!(!db
            .apply_context_compaction(before.revision, before.through, recent.id, "过时摘要", &[])
            .unwrap());
    }

    #[test]
    fn summary_versions_are_atomic_bounded_and_clear_with_conversation() {
        let db = store();
        for i in 0..35 {
            let message = db.add_message("user", &format!("第{i}次对话")).unwrap();
            let snapshot = db.context_snapshot().unwrap();
            assert!(db
                .apply_context_compaction(
                    snapshot.revision,
                    snapshot.through,
                    message.id,
                    &format!("近期状态{i}"),
                    &[]
                )
                .unwrap());
            // Stale commits cannot add audit entries.
            assert!(!db
                .apply_context_compaction(
                    snapshot.revision,
                    snapshot.through,
                    message.id,
                    "过期",
                    &[]
                )
                .unwrap());
        }
        {
            let conn = db.lock().unwrap();
            assert_eq!(
                conn.query_row("SELECT COUNT(*) FROM summary_versions", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                30
            );
            let entry: (String,String,i64,i64,i64,i64) = conn.query_row("SELECT previous_summary,summary,previous_through,through,before_tokens,after_tokens FROM summary_versions ORDER BY id DESC LIMIT 1", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).unwrap();
            assert_eq!(entry.0, "近期状态33");
            assert_eq!(entry.1, "近期状态34");
            assert_eq!(entry.3, entry.2 + 1);
            assert!(entry.4 > entry.5 && entry.5 > 0);
        }
        db.clear_conversation().unwrap();
        assert_eq!(
            db.lock()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM summary_versions", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn corrections_survive_stale_work_and_fold_only_on_valid_commit() {
        let db = store();
        let message = db.add_message("user", "今天已完成").unwrap();
        let before = db.context_snapshot().unwrap();
        db.save_user_profile("每日提醒", 0).unwrap();
        assert!(!db
            .apply_context_compaction(before.revision, 0, message.id, "过时", &[])
            .unwrap());
        assert!(db
            .memory_context_changes()
            .unwrap()
            .contains("profile_updated"));
        let current = db.context_snapshot().unwrap();
        assert!(db
            .apply_context_compaction(current.revision, 0, message.id, "今天已完成，不再提醒", &[])
            .unwrap());
        assert_eq!(db.memory_context_changes().unwrap(), "[]");
    }

    #[test]
    fn corrections_survive_partial_prefix_until_old_raw_tail_is_covered() {
        let db = store();
        let first = db.add_message("user", "旧偏好在前缀").unwrap();
        let last = db.add_message("user", "旧偏好也在尾部").unwrap();
        db.save_user_profile("新偏好", 0).unwrap();
        let snapshot = db.context_snapshot().unwrap();
        assert!(db
            .apply_context_compaction(snapshot.revision, 0, first.id, "按新偏好整理", &[])
            .unwrap());
        assert!(db
            .memory_context_changes()
            .unwrap()
            .contains("profile_updated"));
        let snapshot = db.context_snapshot().unwrap();
        assert!(db
            .apply_context_compaction(snapshot.revision, first.id, last.id, "按新偏好整理", &[])
            .unwrap());
        assert_eq!(db.memory_context_changes().unwrap(), "[]");
    }

    #[test]
    fn correction_injection_is_bounded_for_profiles_and_bulk_expiry() {
        let db = store();
        let mut revision = 0;
        for i in 0..25 {
            let profile = format!("{}{}", "🙂".repeat(990), i);
            revision = db.save_user_profile(&profile, revision).unwrap().revision;
        }
        let profile_hints = db.memory_context_changes().unwrap();
        assert!(!profile_hints.contains("🙂"));
        assert!(
            tiktoken_rs::o200k_base_singleton()
                .encode_ordinary(&profile_hints)
                .len()
                <= 800
        );
        for i in 0..30 {
            db.add_memory_scoped(&format!("{}{}", "过期关注".repeat(400), i), "today")
                .unwrap();
        }
        db.lock()
            .unwrap()
            .execute("UPDATE memories SET expires_at='2000-01-01T00:00:00Z'", [])
            .unwrap();
        assert_eq!(db.expire_memories().unwrap(), 30);
        let hints = db.memory_context_changes().unwrap();
        assert!(
            tiktoken_rs::o200k_base_singleton()
                .encode_ordinary(&hints)
                .len()
                <= 800
        );
        assert!(hints.contains("omitted"));
        assert!(hints.contains("expired"));
        assert!(hints.contains("truncated"));
        let value: Value = serde_json::from_str(&hints).unwrap();
        assert_eq!(value["changes"][0]["omitted_memories"], 26);
    }

    #[test]
    fn last_user_time_ignores_observations_beyond_context_window() {
        let db = store();
        assert_eq!(db.last_user_message_time().unwrap(), None);
        let user = db.add_message("user", "你好").unwrap();
        for _ in 0..201 {
            db.add_message("proactive", "自动观察").unwrap();
        }
        assert!(db.messages().unwrap().iter().all(|m| m.role != "user"));
        assert_eq!(
            db.last_user_message_time().unwrap().as_deref(),
            Some(user.created_at.as_str())
        );
    }
    fn store() -> Store {
        Store::open(Path::new(":memory:")).unwrap()
    }
    #[test]
    fn introductions_commit_with_message_and_reset_only_with_conversation() {
        let db = store();
        assert_eq!(
            db.introduction_state().unwrap(),
            IntroductionState::default()
        );
        assert!(db
            .add_companion_message("assistant", " ", true, true)
            .is_err());
        assert_eq!(
            db.introduction_state().unwrap(),
            IntroductionState::default()
        );
        // A concrete first request can complete without introducing the character.
        db.add_companion_message("assistant", "这是问题的答案。", false, false)
            .unwrap();
        assert_eq!(
            db.introduction_state().unwrap(),
            IntroductionState::default()
        );
        db.add_companion_message("proactive", "我是凛，怎么称呼你？", true, true)
            .unwrap();
        let state = db.introduction_state().unwrap();
        assert!(state.introduced && state.asked_name && !state.legacy_history);
        db.add_companion_message("assistant", "继续聊", false, false)
            .unwrap();
        db.save_user_profile("", 0).unwrap();
        assert_eq!(db.introduction_state().unwrap(), state);
        db.clear_conversation().unwrap();
        assert_eq!(
            db.introduction_state().unwrap(),
            IntroductionState::default()
        );
    }

    #[test]
    fn failed_introduction_transaction_cannot_save_message_or_flags() {
        let db = store();
        db.introduction_state().unwrap();
        db.lock().unwrap().execute_batch("CREATE TRIGGER reject_intro BEFORE UPDATE ON metadata WHEN NEW.key='introduction_state' BEGIN SELECT RAISE(ABORT, 'test rejection'); END;").unwrap();
        assert!(db
            .add_companion_message("assistant", "我是凛", true, false)
            .is_err());
        assert!(db.messages().unwrap().is_empty());
        assert_eq!(
            db.introduction_state().unwrap(),
            IntroductionState::default()
        );
    }

    #[test]
    fn upgrading_old_chat_does_not_force_a_new_introduction() {
        let db = store();
        db.add_message("assistant", "以前的聊天").unwrap();
        let state = db.introduction_state().unwrap();
        assert!(state.legacy_history);
        assert!(!state.introduced && !state.asked_name);
        assert_eq!(db.introduction_state().unwrap(), state);
    }

    #[test]
    fn profile_edits_are_bounded_and_reject_stale_replacements() {
        let db = store();
        assert_eq!(
            db.user_profile().unwrap(),
            UserProfile {
                content: String::new(),
                revision: 0
            }
        );
        let saved = db
            .save_user_profile("# 交流偏好\n喜欢自然的角色陪伴", 0)
            .unwrap();
        assert_eq!(saved.revision, 1);
        assert!(db.save_user_profile("过期窗口覆盖", 0).is_err());
        assert_eq!(db.user_profile().unwrap(), saved);
        assert!(db.save_user_profile(&"字".repeat(2001), 1).is_err());
        assert!(db.save_user_profile(&"🙂".repeat(1001), 1).is_err());
        assert_eq!(db.save_user_profile("", 1).unwrap().content, "");
    }

    #[test]
    fn clearing_conversation_preserves_identity_and_blocks_stale_extraction() {
        let db = store();
        db.save_settings(&serde_json::json!({"personality":"凛"}))
            .unwrap();
        db.add_memory("喜欢主动聊天").unwrap();
        let profile = db.save_user_profile("# 偏好\n保持角色身份", 0).unwrap();
        let old = db.add_message("user", "旧话题").unwrap();
        let revision = db.memory_revision().unwrap();
        db.set_meta("summary", "旧摘要").unwrap();
        db.set_meta("processed_through", &old.id.to_string())
            .unwrap();
        db.clear_conversation().unwrap();
        assert!(db.messages().unwrap().is_empty());
        assert!(db.search_messages("旧", 5).unwrap().is_empty());
        assert!(db.get_meta("summary").unwrap().is_none());
        assert!(db.get_meta("processed_through").unwrap().is_none());
        assert_eq!(db.user_profile().unwrap(), profile);
        assert_eq!(db.memories().unwrap().len(), 1);
        assert_eq!(db.load_settings().unwrap().unwrap()["personality"], "凛");
        assert!(!db
            .apply_memory_batch(revision, &["旧事实".into()], "旧摘要复活", old.id)
            .unwrap());
        assert!(db.add_message("user", "新话题").unwrap().id > old.id);
    }

    #[test]
    fn voice_library_keeps_multiple_named_voices_and_cloud_refresh_preserves_names() {
        let db = store();
        let endpoint = "https://example.com/api/v1/services/audio/tts/SpeechSynthesizer";
        let mut voice = ReferenceVoice {
            voice_id: "cosyvoice-v3-flash-linger-one".into(),
            name: "温柔的声音".into(),
            target_model: "cosyvoice-v3-flash".into(),
            status: "DEPLOYING".into(),
            created_at: "2026-11-11T00:00:00Z".into(),
            in_use_by: Vec::new(),
        };
        db.upsert_reference_voice(endpoint, &voice).unwrap();
        voice.voice_id = "cosyvoice-v3-flash-linger-two".into();
        voice.name = "第二个声音".into();
        db.upsert_reference_voice(endpoint, &voice).unwrap();
        db.rename_reference_voice(endpoint, &voice.voice_id, "中文自定义名称")
            .unwrap();
        voice.status = "OK".into();
        voice.name = "云端默认名称".into();
        db.upsert_reference_voice(endpoint, &voice).unwrap();
        let rows = db.list_reference_voices(endpoint).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows.iter()
                .find(|v| v.voice_id == voice.voice_id)
                .unwrap()
                .name,
            "中文自定义名称"
        );
        assert_eq!(
            rows.iter()
                .find(|v| v.voice_id == voice.voice_id)
                .unwrap()
                .status,
            "OK"
        );
        assert!(db
            .list_reference_voices("another endpoint")
            .unwrap()
            .is_empty());
        assert!(db
            .rename_reference_voice(endpoint, &voice.voice_id, "")
            .is_err());
        assert!(db
            .rename_reference_voice(endpoint, &voice.voice_id, &"字".repeat(61))
            .is_err());
    }
    #[test]
    fn legacy_voice_migration_and_delete_tombstone_prevent_resurrection() {
        let db = store();
        let endpoint = "https://example.com/tts";
        let id = "cosyvoice-v3-flash-linger-old";
        let meta_key = format!("reference_voice:{endpoint}:cosyvoice-v3-flash");
        db.set_meta(
            &meta_key,
            &serde_json::json!({"voice_id":id,"target_model":"cosyvoice-v3-flash"}).to_string(),
        )
        .unwrap();
        let rows = db.list_reference_voices(endpoint).unwrap();
        assert_eq!(rows.len(), 1);
        db.rename_reference_voice(endpoint, id, "迁移后的名字")
            .unwrap();
        assert_eq!(
            db.list_reference_voices(endpoint).unwrap()[0].name,
            "迁移后的名字"
        );
        db.delete_reference_voice(endpoint, id).unwrap();
        assert!(db.get_meta(&meta_key).unwrap().is_none());
        assert!(db.list_reference_voices(endpoint).unwrap().is_empty());
        assert!(db.reference_voice_deleted(endpoint, id).unwrap());
        assert!(db
            .reference_voice_deleted("https://another-account-alias/tts", id)
            .unwrap());
        db.upsert_reference_voice(endpoint, &rows[0]).unwrap();
        assert!(db.list_reference_voices(endpoint).unwrap().is_empty());
    }

    #[test]
    fn history_pages_are_bounded_ordered_and_do_not_overlap() {
        let db = store();
        for index in 0..125 {
            db.add_message("user", &format!("message {index}")).unwrap();
        }
        let latest = db.message_page(None, 50).unwrap();
        assert_eq!(latest.len(), 50);
        assert_eq!((latest[0].id, latest[49].id), (76, 125));
        db.add_message("assistant", "new live reply").unwrap();
        let older = db.message_page(Some(latest[0].id), 50).unwrap();
        assert_eq!((older[0].id, older[49].id), (26, 75));
        let first = db.message_page(Some(older[0].id), 50).unwrap();
        assert_eq!(first.len(), 25);
        assert_eq!((first[0].id, first[24].id), (1, 25));
        assert!(db.message_page(Some(1), 50).unwrap().is_empty());
        assert_eq!(db.message_page(None, usize::MAX).unwrap().len(), 100);
        assert_eq!(db.message_page(None, 0).unwrap().len(), 1);
        assert_eq!(db.messages().unwrap().len(), 126);
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
    fn explicit_changes_preserve_context_and_reject_stale_batches() {
        let db = store();
        let memory = db.add_memory("看左边").unwrap();
        let revision = db.memory_revision().unwrap();
        db.set_meta("summary", "看左边").unwrap();
        db.update_memory(memory.id, "看右边").unwrap();
        assert!(!db
            .apply_memory_batch(revision, &["看左边".into()], "旧摘要", 10)
            .unwrap());
        assert_eq!(db.memories().unwrap()[0].content, "看右边");
        assert_eq!(db.get_meta("summary").unwrap().as_deref(), Some("看左边"));
        assert!(db.memory_context_changes().unwrap().contains("看右边"));
        db.delete_memory(memory.id).unwrap();
        assert!(db.memory_context_changes().unwrap().contains("removed"));
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
            db.save_user_profile("喜欢自然陪伴", 0).unwrap();
            db.add_message("user", "记住我喜欢安静").unwrap();
            db.add_memory("喜欢安静").unwrap();
            db.save_settings(&serde_json::json!({"voice":"coral"}))
                .unwrap();
        }
        {
            let db = Store::open(&path).unwrap();
            assert_eq!(
                db.user_profile().unwrap(),
                UserProfile {
                    content: "喜欢自然陪伴".into(),
                    revision: 1
                }
            );
            assert_eq!(db.messages().unwrap()[0].content, "记住我喜欢安静");
            assert_eq!(db.memories().unwrap()[0].content, "喜欢安静");
            assert_eq!(db.load_settings().unwrap().unwrap()["voice"], "coral");
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn today_expiry_preserves_continuity_and_rejects_stale_batch() {
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
        assert_eq!(
            db.get_meta("summary").unwrap().as_deref(),
            Some("用户今天关注左边")
        );
        assert!(db.get_meta("memory_history_floor").unwrap().is_none());
        assert!(db.memory_context_changes().unwrap().contains("expired"));
        assert_eq!(db.context_snapshot().unwrap().messages.len(), 1);
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
            assert_eq!(
                db.get_meta("summary").unwrap().as_deref(),
                Some("这次关注右上角")
            );
            assert!(db.memory_context_changes().unwrap().contains("expired"));
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
