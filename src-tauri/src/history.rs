//! Local interaction archive. Images are bounded files, never automatic prompt content.
use base64::{engine::general_purpose::STANDARD, Engine};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    io::{Cursor, Write},
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
    time::Duration,
};

type Result<T> = std::result::Result<T, String>;
const RETENTION_SECONDS: i64 = 24 * 60 * 60;
const MAX_ARCHIVE_BYTES: u64 = 500 * 1024 * 1024;
const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;

pub struct History {
    connection: Mutex<Connection>,
    images: PathBuf,
}

fn error(e: impl std::fmt::Display) -> String {
    format!("历史资料存储失败：{e}")
}
fn short(text: &str, chars: usize) -> String {
    text.chars().take(chars).collect()
}
fn now() -> String {
    Utc::now().to_rfc3339()
}

impl History {
    pub fn open(dir: &Path) -> Result<Self> {
        fs::create_dir_all(dir).map_err(error)?;
        let images = dir.join("screenshots");
        fs::create_dir_all(&images).map_err(error)?;
        let connection = Connection::open(dir.join("history.sqlite3")).map_err(error)?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(error)?;
        connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;
            CREATE TABLE IF NOT EXISTS interactions (
                id INTEGER PRIMARY KEY AUTOINCREMENT, trigger TEXT NOT NULL,
                text TEXT NOT NULL, summary TEXT NOT NULL DEFAULT '', created_at TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS history_messages (
                message_id INTEGER PRIMARY KEY, interaction_id INTEGER NOT NULL REFERENCES interactions(id) ON DELETE CASCADE,
                role TEXT NOT NULL, content TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS image_blobs (
                id INTEGER PRIMARY KEY AUTOINCREMENT, fingerprint TEXT NOT NULL, bytes INTEGER NOT NULL,
                available INTEGER NOT NULL DEFAULT 1, created_at TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS screenshots (
                id INTEGER PRIMARY KEY AUTOINCREMENT, interaction_id INTEGER NOT NULL REFERENCES interactions(id) ON DELETE CASCADE,
                blob_id INTEGER NOT NULL REFERENCES image_blobs(id), source TEXT NOT NULL, captured_at TEXT NOT NULL,
                expired INTEGER NOT NULL DEFAULT 0);
            CREATE TABLE IF NOT EXISTS screenshot_links (
                interaction_id INTEGER NOT NULL REFERENCES interactions(id) ON DELETE CASCADE,
                screenshot_id INTEGER NOT NULL REFERENCES screenshots(id) ON DELETE CASCADE,
                PRIMARY KEY(interaction_id,screenshot_id));
            INSERT OR IGNORE INTO screenshot_links SELECT interaction_id,id FROM screenshots;
            CREATE TABLE IF NOT EXISTS history_tools (
                id INTEGER PRIMARY KEY AUTOINCREMENT, interaction_id INTEGER NOT NULL REFERENCES interactions(id) ON DELETE CASCADE,
                name TEXT NOT NULL, result TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS memory_history_sources (
                memory_id INTEGER PRIMARY KEY, updated_at TEXT NOT NULL,
                interaction_id INTEGER NOT NULL REFERENCES interactions(id) ON DELETE CASCADE);
            UPDATE interactions SET trigger='observation' WHERE trigger='legacy' AND EXISTS(SELECT 1 FROM history_messages m WHERE m.interaction_id=interactions.id AND m.role='proactive');
            UPDATE interactions SET trigger='chat' WHERE trigger='legacy';
            CREATE TABLE IF NOT EXISTS history_pointers (name TEXT PRIMARY KEY, screenshot_id INTEGER NOT NULL);
            CREATE INDEX IF NOT EXISTS history_time ON interactions(created_at);
            CREATE INDEX IF NOT EXISTS history_message_interaction ON history_messages(interaction_id);
            CREATE INDEX IF NOT EXISTS screenshot_interaction ON screenshots(interaction_id);
            CREATE INDEX IF NOT EXISTS screenshot_time ON screenshots(captured_at);
            CREATE INDEX IF NOT EXISTS screenshot_blob ON screenshots(blob_id);
            CREATE INDEX IF NOT EXISTS history_tool_interaction ON history_tools(interaction_id);
            CREATE INDEX IF NOT EXISTS image_fingerprint ON image_blobs(fingerprint,available);") .map_err(error)?;
        let archive = Self {
            connection: Mutex::new(connection),
            images,
        };
        archive.cleanup()?;
        archive.remove_orphan_files()?;
        Ok(archive)
    }
    fn lock(&self) -> Result<MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| "历史资料锁不可用".into())
    }
    fn path(&self, id: i64) -> PathBuf {
        self.images.join(format!("{id}.jpg"))
    }

    pub fn start_interaction(&self, trigger: &str, text: &str) -> Result<i64> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO interactions(trigger,text,created_at) VALUES(?1,?2,?3)",
            params![short(trigger, 40), short(text, 20000), now()],
        )
        .map_err(error)?;
        Ok(conn.last_insert_rowid())
    }
    pub fn link_message(
        &self,
        interaction: i64,
        message_id: i64,
        role: &str,
        text: &str,
    ) -> Result<()> {
        self.lock()?.execute("INSERT INTO history_messages(message_id,interaction_id,role,content) VALUES(?1,?2,?3,?4)",
            params![message_id,interaction,role,short(text,20000)]).map_err(error)?;
        Ok(())
    }
    /// Legacy messages retain their actual timestamps and IDs. Do not invent turns.
    pub fn import_messages(&self, messages: &[crate::store::Message]) -> Result<()> {
        let mut conn = self.lock()?;
        let tx = conn.transaction().map_err(error)?;
        for message in messages {
            let exists: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM history_messages WHERE message_id=?1)",
                    [message.id],
                    |r| r.get(0),
                )
                .map_err(error)?;
            if exists {
                continue;
            }
            tx.execute(
                "INSERT INTO interactions(trigger,text,created_at) VALUES(?1,?2,?3)",
                params![
                    if message.role == "proactive" {
                        "observation"
                    } else {
                        "chat"
                    },
                    short(&message.content, 20000),
                    message.created_at
                ],
            )
            .map_err(error)?;
            let id = tx.last_insert_rowid();
            tx.execute(
                "INSERT INTO history_messages VALUES(?1,?2,?3,?4)",
                params![message.id, id, message.role, short(&message.content, 20000)],
            )
            .map_err(error)?;
        }
        tx.commit().map_err(error)
    }
    pub fn record_tool(&self, interaction: i64, name: &str, result: &Value) -> Result<()> {
        // Store only selected tool results supplied by the caller; never raw API requests.
        let text = short(&result.to_string(), 2000);
        let mut conn = self.lock()?;
        let tx = conn.transaction().map_err(error)?;
        tx.execute(
            "INSERT INTO history_tools(interaction_id,name,result) VALUES(?1,?2,?3)",
            params![interaction, short(name, 80), text],
        )
        .map_err(error)?;
        if matches!(name, "remember" | "update_memory") {
            if let (Some(id), Some(updated)) =
                (result["id"].as_i64(), result["updated_at"].as_str())
            {
                tx.execute("INSERT INTO memory_history_sources(memory_id,updated_at,interaction_id) VALUES(?1,?2,?3) ON CONFLICT(memory_id) DO UPDATE SET updated_at=excluded.updated_at,interaction_id=excluded.interaction_id",params![id,updated,interaction]).map_err(error)?;
            }
        }
        tx.commit().map_err(error)
    }
    pub fn memory_source(&self, memory_id: i64, updated_at: &str) -> Result<Option<i64>> {
        self.lock()?.query_row("SELECT interaction_id FROM memory_history_sources WHERE memory_id=?1 AND updated_at=?2",params![memory_id,updated_at],|r|r.get(0)).optional().map_err(error)
    }
    pub fn update_summary(&self, interaction: i64, summary: &str) -> Result<()> {
        self.lock()?
            .execute(
                "UPDATE interactions SET summary=?1 WHERE id=?2",
                params![short(summary, 400), interaction],
            )
            .map_err(error)?;
        Ok(())
    }
    pub fn save_screenshot(&self, interaction: i64, data_url: &str, source: &str) -> Result<i64> {
        let encoded = data_url
            .strip_prefix("data:image/jpeg;base64,")
            .ok_or("只允许保存 JPEG 截图")?;
        if encoded.len() > MAX_IMAGE_BYTES * 4 / 3 + 4 {
            return Err("截图超过 5 MB 限制".into());
        }
        let bytes = STANDARD.decode(encoded).map_err(|_| "截图编码无效")?;
        if bytes.len() > MAX_IMAGE_BYTES {
            return Err("截图超过 5 MB 限制".into());
        }
        let mut reader =
            image::ImageReader::with_format(Cursor::new(&bytes), image::ImageFormat::Jpeg);
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(4096);
        limits.max_image_height = Some(4096);
        limits.max_alloc = Some(64 * 1024 * 1024);
        reader.limits(limits);
        reader
            .decode()
            .map_err(|_| "截图不是有效的受限 JPEG 图片")?;
        self.cleanup()?;
        let mut hash = DefaultHasher::new();
        bytes.hash(&mut hash);
        let fingerprint = format!("{:016x}", hash.finish());
        let mut conn = self.lock()?;
        let tx = conn.transaction().map_err(error)?;
        let trigger: String = tx
            .query_row(
                "SELECT trigger FROM interactions WHERE id=?1",
                [interaction],
                |r| r.get(0),
            )
            .optional()
            .map_err(error)?
            .ok_or("关联互动不存在")?;
        let candidates = {
            let mut stmt = tx
                .prepare(
                    "SELECT id FROM image_blobs WHERE fingerprint=?1 AND bytes=?2 AND available=1",
                )
                .map_err(error)?;
            let rows = stmt
                .query_map(params![fingerprint, bytes.len() as i64], |r| {
                    r.get::<_, i64>(0)
                })
                .map_err(error)?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(error)?
        };
        // Hash is just a lookup accelerator; exact comparison prevents collisions.
        let existing = candidates
            .into_iter()
            .find(|id| fs::read(self.path(*id)).is_ok_and(|saved| saved == bytes));
        let mut created_file = None;
        let blob = if let Some(id) = existing {
            id
        } else {
            tx.execute(
                "INSERT INTO image_blobs(fingerprint,bytes,created_at) VALUES(?1,?2,?3)",
                params![fingerprint, bytes.len() as i64, now()],
            )
            .map_err(error)?;
            let id = tx.last_insert_rowid();
            let temporary = self.images.join(format!("{id}.tmp"));
            let final_path = self.path(id);
            let write = (|| -> std::io::Result<()> {
                let mut file = fs::File::create(&temporary)?;
                file.write_all(&bytes)?;
                file.sync_all()?;
                fs::rename(&temporary, &final_path)
            })();
            if let Err(e) = write {
                let _ = fs::remove_file(temporary);
                return Err(error(e));
            }
            created_file = Some(final_path);
            id
        };
        let inserted = (|| -> Result<i64> {
            tx.execute("INSERT INTO screenshots(interaction_id,blob_id,source,captured_at) VALUES(?1,?2,?3,?4)",params![interaction,blob,short(source,160),now()]).map_err(error)?;
            let id = tx.last_insert_rowid();
            tx.execute(
                "INSERT INTO screenshot_links VALUES(?1,?2)",
                params![interaction, id],
            )
            .map_err(error)?;
            let pointer = if ["observe", "observation", "proactive"].contains(&trigger.as_str()) {
                "latest_observation"
            } else {
                "discussed_screenshot"
            };
            tx.execute("INSERT INTO history_pointers(name,screenshot_id) VALUES(?1,?2) ON CONFLICT(name) DO UPDATE SET screenshot_id=excluded.screenshot_id",params![pointer,id]).map_err(error)?;
            Ok(id)
        })();
        let id = match inserted {
            Ok(id) => id,
            Err(e) => {
                if let Some(path) = created_file {
                    let _ = fs::remove_file(path);
                }
                return Err(e);
            }
        };
        if let Err(e) = tx.commit() {
            if let Some(path) = created_file {
                let _ = fs::remove_file(path);
            }
            return Err(error(e));
        }
        drop(conn);
        self.cleanup()?;
        Ok(id)
    }
    pub fn attach_screenshot(&self, interaction: i64, id: i64) -> Result<()> {
        self.cleanup()?;
        let conn = self.lock()?;
        let metadata = self.shot_metadata(&conn, id)?;
        if metadata["available"] != true {
            return Err("历史截图已过期或已清空".into());
        }
        conn.execute(
            "INSERT OR IGNORE INTO screenshot_links VALUES(?1,?2)",
            params![interaction, id],
        )
        .map_err(error)?;
        Ok(())
    }
    pub fn set_discussed(&self, id: i64) -> Result<()> {
        let conn = self.lock()?;
        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM screenshots WHERE id=?1)",
                [id],
                |r| r.get(0),
            )
            .map_err(error)?;
        if !exists {
            return Err("截图不存在".into());
        }
        conn.execute("INSERT INTO history_pointers VALUES('discussed_screenshot',?1) ON CONFLICT(name) DO UPDATE SET screenshot_id=excluded.screenshot_id",[id]).map_err(error)?;
        Ok(())
    }
    fn shot_metadata(&self, conn: &Connection, id: i64) -> Result<Value> {
        conn.query_row("SELECT s.id,s.interaction_id,s.source,s.captured_at,s.expired,b.id,b.available FROM screenshots s JOIN image_blobs b ON b.id=s.blob_id WHERE s.id=?1",[id],|r| {
            let blob:i64=r.get(5)?;
            let available = r.get::<_,i64>(4)? == 0 && r.get::<_,i64>(6)? == 1 && self.path(blob).is_file();
            Ok(json!({"screenshot_id":r.get::<_,i64>(0)?,"interaction_id":r.get::<_,i64>(1)?,"source":r.get::<_,String>(2)?,"captured_at":r.get::<_,String>(3)?,"available":available,"historical":true}))
        }).optional().map_err(error)?.ok_or_else(||"截图不存在".into())
    }
    fn shots(&self, conn: &Connection, interaction: i64) -> Result<Vec<Value>> {
        let mut stmt=conn.prepare("SELECT screenshot_id FROM screenshot_links WHERE interaction_id=?1 ORDER BY screenshot_id DESC LIMIT 5").map_err(error)?;
        let ids = stmt
            .query_map([interaction], |r| r.get::<_, i64>(0))
            .map_err(error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(error)?;
        ids.into_iter()
            .map(|id| self.shot_metadata(conn, id))
            .collect()
    }
    pub fn pointers(&self) -> Result<Value> {
        self.cleanup()?;
        let conn = self.lock()?;
        let mut pointers = serde_json::Map::new();
        for name in ["latest_observation", "discussed_screenshot"] {
            let id: Option<i64> = conn
                .query_row(
                    "SELECT screenshot_id FROM history_pointers WHERE name=?1",
                    [name],
                    |r| r.get(0),
                )
                .optional()
                .map_err(error)?;
            pointers.insert(
                name.into(),
                id.and_then(|id| self.shot_metadata(&conn, id).ok())
                    .unwrap_or(Value::Null),
            );
        }
        Ok(Value::Object(pointers))
    }
    pub fn recent_context(&self, message_ids: &[i64]) -> Result<Value> {
        self.cleanup()?;
        let conn = self.lock()?;
        let mut records = Vec::new();
        for message in message_ids.iter().take(12) {
            if records.len() == 4 {
                break;
            }
            let message_row: Option<(i64, String, String)> = conn
                .query_row(
                    "SELECT interaction_id,role,content FROM history_messages WHERE message_id=?1",
                    [message],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()
                .map_err(error)?;
            if let Some((interaction, role, content)) = message_row {
                let shots = self.shots(&conn, interaction)?;
                if let Some(shot) = shots.first() {
                    records.push(
                        json!({"message_id":message,"interaction_id":interaction,"role":role,
                        "snippet":short(&content,40),"screenshots":[{
                            "screenshot_id":shot["screenshot_id"],"captured_at":shot["captured_at"],
                            "available":shot["available"]}]}),
                    );
                }
            }
        }
        Ok(json!(records))
    }
    pub fn search(
        &self,
        query: &str,
        after: Option<&str>,
        before: Option<&str>,
        kind: Option<&str>,
    ) -> Result<Value> {
        for value in [after, before].into_iter().flatten() {
            DateTime::parse_from_rfc3339(value).map_err(|_| "历史时间须为带时区的 RFC3339 时间")?;
        }
        self.cleanup()?;
        let query = short(query.trim(), 200);
        let conn = self.lock()?;
        let mut stmt=conn.prepare("SELECT i.id,i.trigger,i.created_at,i.summary,i.text FROM interactions i WHERE
            (?1='' OR instr(lower(i.text),lower(?1))>0 OR instr(lower(i.summary),lower(?1))>0 OR EXISTS(SELECT 1 FROM history_messages m WHERE m.interaction_id=i.id AND instr(lower(m.content),lower(?1))>0) OR EXISTS(SELECT 1 FROM history_tools t WHERE t.interaction_id=i.id AND instr(lower(t.result),lower(?1))>0))
            AND (?2 IS NULL OR julianday(i.created_at)>=julianday(?2)) AND (?3 IS NULL OR julianday(i.created_at)<=julianday(?3))
            AND (?4 IS NULL OR ?4='' OR i.trigger=?4 OR (?4='screenshot' AND EXISTS(SELECT 1 FROM screenshot_links s WHERE s.interaction_id=i.id)))
            ORDER BY i.id DESC LIMIT 5").map_err(error)?;
        let rows = stmt
            .query_map(params![query, after, before, kind], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                ))
            })
            .map_err(error)?;
        let rows = rows
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(error)?;
        let mut items = Vec::new();
        for (id, trigger, created, summary, text) in rows {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM screenshot_links WHERE interaction_id=?1",
                    [id],
                    |r| r.get(0),
                )
                .map_err(error)?;
            let mut preview = if summary.is_empty() {
                text.clone()
            } else {
                summary.clone()
            };
            if !query.is_empty() && !preview.to_lowercase().contains(&query.to_lowercase()) {
                if text.to_lowercase().contains(&query.to_lowercase()) {
                    preview = text;
                } else {
                    let matched: Option<String> = conn.query_row(
                        "SELECT content FROM history_messages WHERE interaction_id=?1 AND instr(lower(content),lower(?2))>0 ORDER BY message_id DESC LIMIT 1",params![id,query],|r|r.get(0)
                    ).optional().map_err(error)?;
                    preview = if let Some(matched) = matched {
                        matched
                    } else {
                        conn.query_row("SELECT result FROM history_tools WHERE interaction_id=?1 AND instr(lower(result),lower(?2))>0 ORDER BY id DESC LIMIT 1",params![id,query],|r|r.get(0)).optional().map_err(error)?.unwrap_or(preview)
                    };
                }
            }
            let lower = preview.to_lowercase();
            let offset = if query.is_empty() {
                0
            } else {
                lower
                    .find(&query.to_lowercase())
                    .map(|offset| lower[..offset].chars().count().saturating_sub(35))
                    .unwrap_or(0)
            };
            let snippet = preview.chars().skip(offset).take(140).collect::<String>();
            items.push(json!({"interaction_id":id,"trigger":trigger,"created_at":created,"snippet":snippet,"screenshot_count":count}));
        }
        Ok(json!({"items":items,"limit":5,"historical_only":true}))
    }
    pub fn read(&self, id: i64) -> Result<Value> {
        self.cleanup()?;
        let conn = self.lock()?;
        let mut record=conn.query_row("SELECT trigger,text,summary,created_at FROM interactions WHERE id=?1",[id],|r|Ok(json!({"interaction_id":id,"trigger":r.get::<_,String>(0)?,"text":short(&r.get::<_,String>(1)?,200),"summary":short(&r.get::<_,String>(2)?,200),"created_at":r.get::<_,String>(3)?}))).optional().map_err(error)?.ok_or("互动不存在或已清空")?;
        let mut stmt=conn.prepare("SELECT message_id,role,content FROM history_messages WHERE interaction_id=?1 ORDER BY message_id LIMIT 4").map_err(error)?;
        let messages=stmt.query_map([id],|r|Ok(json!({"message_id":r.get::<_,i64>(0)?,"role":r.get::<_,String>(1)?,"content":short(&r.get::<_,String>(2)?,300)}))).map_err(error)?.collect::<std::result::Result<Vec<_>,_>>().map_err(error)?;
        let mut stmt=conn.prepare("SELECT name,result FROM history_tools WHERE interaction_id=?1 ORDER BY id DESC LIMIT 2").map_err(error)?;
        let tools = stmt
            .query_map([id], |r| {
                Ok(json!({"name":r.get::<_,String>(0)?,"result":short(&r.get::<_,String>(1)?,200)}))
            })
            .map_err(error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(error)?;
        record["messages"] = json!(messages);
        record["tools"] = json!(tools);
        record["screenshots"] = json!(self
            .shots(&conn, id)?
            .into_iter()
            .take(2)
            .collect::<Vec<_>>());
        record["historical_only"] = json!(true);
        let neighbors: Vec<i64> = {
            let mut stmt=conn.prepare("SELECT id FROM interactions WHERE id IN ((SELECT MAX(id) FROM interactions WHERE id<?1),(SELECT MIN(id) FROM interactions WHERE id>?1)) ORDER BY id").map_err(error)?;
            let rows = stmt.query_map([id], |r| r.get(0)).map_err(error)?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(error)?
        };
        record["neighbor_ids"] = json!(neighbors);
        let mut previews = Vec::new();
        for neighbor in neighbors {
            previews.push(conn.query_row("SELECT trigger,text,created_at FROM interactions WHERE id=?1",[neighbor],|r|Ok(json!({"interaction_id":neighbor,"trigger":r.get::<_,String>(0)?,"snippet":short(&r.get::<_,String>(1)?,80),"created_at":r.get::<_,String>(2)?}))).map_err(error)?);
        }
        record["neighbors"] = json!(previews);
        record["bounded_preview"] = json!(true);
        Ok(record)
    }
    pub fn view(&self, id: i64) -> Result<(Value, String)> {
        self.cleanup()?;
        let conn = self.lock()?;
        let metadata = self.shot_metadata(&conn, id)?;
        if metadata["available"] != true {
            return Err("历史截图已过期或已清空，不能查看原图".into());
        }
        let blob: i64 = conn
            .query_row("SELECT blob_id FROM screenshots WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .map_err(error)?;
        let bytes = fs::read(self.path(blob)).map_err(|_| "历史截图文件已不可用")?;
        Ok((
            metadata,
            format!("data:image/jpeg;base64,{}", STANDARD.encode(bytes)),
        ))
    }
    pub fn cleanup(&self) -> Result<()> {
        self.cleanup_with(Utc::now().timestamp(), MAX_ARCHIVE_BYTES)
    }
    fn cleanup_with(&self, timestamp: i64, max_bytes: u64) -> Result<()> {
        let conn = self.lock()?;
        conn.execute(
            "UPDATE screenshots SET expired=1 WHERE expired=0 AND unixepoch(captured_at)<?1",
            [timestamp - RETENTION_SECONDS],
        )
        .map_err(error)?;
        let blobs = {
            let mut stmt=conn.prepare("SELECT b.id,b.bytes,MAX(s.captured_at),SUM(CASE WHEN s.expired=0 THEN 1 ELSE 0 END) FROM image_blobs b LEFT JOIN screenshots s ON s.blob_id=b.id WHERE b.available=1 GROUP BY b.id ORDER BY MAX(s.captured_at),b.id").map_err(error)?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)? as u64,
                        r.get::<_, i64>(3)?,
                    ))
                })
                .map_err(error)?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(error)?
        };
        let mut total: u64 = blobs.iter().map(|b| b.1).sum();
        for (id, bytes, active) in blobs {
            if active == 0 || total > max_bytes || !self.path(id).is_file() {
                match fs::remove_file(self.path(id)) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(error(e)),
                }
                conn.execute("UPDATE image_blobs SET available=0 WHERE id=?1", [id])
                    .map_err(error)?;
                conn.execute("UPDATE screenshots SET expired=1 WHERE blob_id=?1", [id])
                    .map_err(error)?;
                total = total.saturating_sub(bytes);
            }
        }
        Ok(())
    }
    pub fn clear_screenshots(&self) -> Result<()> {
        self.lock()?
            .execute("UPDATE screenshots SET expired=1", [])
            .map_err(error)?;
        self.cleanup_with(Utc::now().timestamp(), 0)
    }
    pub fn clear_all(&self) -> Result<()> {
        self.clear_screenshots()?;
        self.lock()?.execute_batch("BEGIN; DELETE FROM history_pointers; DELETE FROM memory_history_sources; DELETE FROM history_tools; DELETE FROM history_messages; DELETE FROM screenshot_links; DELETE FROM screenshots; DELETE FROM image_blobs; DELETE FROM interactions; COMMIT;").map_err(error)?;
        Ok(())
    }
    #[cfg(test)]
    pub fn stats(&self) -> Result<Value> {
        self.cleanup()?;
        let conn = self.lock()?;
        let (count, bytes): (i64, i64) = conn
            .query_row(
                "SELECT COUNT(*),COALESCE(SUM(bytes),0) FROM image_blobs WHERE available=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(error)?;
        Ok(
            json!({"image_count":count,"bytes":bytes,"max_bytes":MAX_ARCHIVE_BYTES,"retention_hours":24}),
        )
    }
    fn remove_orphan_files(&self) -> Result<()> {
        let conn = self.lock()?;
        for entry in fs::read_dir(&self.images).map_err(error)? {
            let entry = entry.map_err(error)?;
            let path = entry.path();
            let Some(id) = path
                .file_stem()
                .and_then(|v| v.to_str())
                .and_then(|v| v.parse::<i64>().ok())
            else {
                continue;
            };
            let extension = path.extension().and_then(|v| v.to_str());
            if extension != Some("jpg") && extension != Some("tmp") {
                continue;
            }
            let referenced: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM image_blobs WHERE id=?1 AND available=1)",
                    [id],
                    |r| r.get(0),
                )
                .map_err(error)?;
            if extension == Some("tmp") || !referenced {
                fs::remove_file(path).map_err(error)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "linger-history-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn jpeg(color: u8) -> String {
        let image = image::RgbImage::from_pixel(2, 2, image::Rgb([color, 10, 200]));
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut bytes)
            .encode_image(&image)
            .unwrap();
        format!("data:image/jpeg;base64,{}", STANDARD.encode(bytes))
    }
    #[test]
    fn deduplicates_pixels_but_keeps_capture_identity_and_discussion_pointer() {
        let temp = Temp::new();
        let h = History::open(&temp.0).unwrap();
        let first = h.start_interaction("observation", "报错窗口").unwrap();
        let a = h
            .save_screenshot(first, &jpeg(1), "monitor:primary")
            .unwrap();
        h.link_message(first, 1, "proactive", "这里报错了").unwrap();
        h.set_discussed(a).unwrap();
        let second = h.start_interaction("observation", "画面相同").unwrap();
        let b = h
            .save_screenshot(second, &jpeg(1), "monitor:primary")
            .unwrap();
        assert_ne!(a, b);
        assert_eq!(h.stats().unwrap()["image_count"], 1);
        assert_eq!(
            h.pointers().unwrap()["latest_observation"]["screenshot_id"],
            b
        );
        assert_eq!(
            h.pointers().unwrap()["discussed_screenshot"]["screenshot_id"],
            a
        );
        let chat = h.start_interaction("chat", "第二行呢").unwrap();
        h.attach_screenshot(chat, a).unwrap();
        assert_eq!(h.read(chat).unwrap()["screenshots"][0]["screenshot_id"], a);
        assert_eq!(
            h.read(chat).unwrap()["screenshots"][0]["interaction_id"],
            first
        );
        assert_eq!(
            h.recent_context(&[1]).unwrap()[0]["screenshots"][0]["screenshot_id"],
            a
        );
        assert_eq!(h.view(a).unwrap().1, jpeg(1));
    }
    #[test]
    fn expired_images_keep_historical_metadata_and_cannot_be_loaded() {
        let temp = Temp::new();
        let h = History::open(&temp.0).unwrap();
        let interaction = h.start_interaction("chat", "old").unwrap();
        let shot = h.save_screenshot(interaction, &jpeg(4), "screen").unwrap();
        h.lock()
            .unwrap()
            .execute(
                "UPDATE screenshots SET captured_at='2000-01-01T00:00:00+00:00'",
                [],
            )
            .unwrap();
        h.cleanup().unwrap();
        assert_eq!(h.stats().unwrap()["bytes"], 0);
        assert!(h.view(shot).unwrap_err().contains("过期"));
        assert_eq!(
            h.read(interaction).unwrap()["screenshots"][0]["available"],
            false
        );
        assert_eq!(
            h.pointers().unwrap()["discussed_screenshot"]["screenshot_id"],
            shot
        );
        assert!(!h.pointers().unwrap()["discussed_screenshot"]["available"]
            .as_bool()
            .unwrap());
        assert_eq!(fs::read_dir(&h.images).unwrap().count(), 0);
    }
    #[test]
    fn byte_cap_evicts_oldest_blobs_and_clear_preserves_text() {
        let temp = Temp::new();
        let h = History::open(&temp.0).unwrap();
        let interaction = h.start_interaction("chat", "keep me").unwrap();
        let a = h.save_screenshot(interaction, &jpeg(1), "a").unwrap();
        let b = h.save_screenshot(interaction, &jpeg(240), "b").unwrap();
        let bytes:u64=h.lock().unwrap().query_row("SELECT bytes FROM image_blobs WHERE id=(SELECT blob_id FROM screenshots WHERE id=?1)",[b],|r|r.get(0)).unwrap();
        h.cleanup_with(Utc::now().timestamp(), bytes).unwrap();
        assert!(h.view(a).is_err());
        assert!(h.view(b).is_ok());
        h.clear_screenshots().unwrap();
        assert!(h.view(b).is_err());
        assert_eq!(h.read(interaction).unwrap()["text"], "keep me");
        h.clear_all().unwrap();
        assert!(h.read(interaction).is_err());
        assert_eq!(h.pointers().unwrap()["discussed_screenshot"], Value::Null);
    }
    #[test]
    fn search_is_literal_bounded_and_import_is_idempotent() {
        let temp = Temp::new();
        let h = History::open(&temp.0).unwrap();
        for id in 1..=9 {
            let message = crate::store::Message {
                id,
                role: "user".into(),
                content: format!("登录失败 {id} %_"),
                created_at: "2026-01-02T03:00:00+00:00".into(),
            };
            h.import_messages(std::slice::from_ref(&message)).unwrap();
            h.import_messages(&[message]).unwrap();
        }
        assert_eq!(
            h.lock()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM interactions", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            9
        );
        let result = h.search("登录", None, None, None).unwrap();
        assert_eq!(result["items"].as_array().unwrap().len(), 5);
        assert_eq!(
            h.search("%_", None, None, None).unwrap()["items"]
                .as_array()
                .unwrap()
                .len(),
            5
        );
        assert!(h
            .search("登录", Some("2027-01-01T00:00:00Z"), None, None)
            .unwrap()["items"]
            .as_array()
            .unwrap()
            .is_empty());
        assert!(h.search("登录", Some("yesterday"), None, None).is_err());
        assert!(
            h.search("登录", None, None, Some("observation")).unwrap()["items"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
    #[test]
    fn imported_roles_matching_snippets_and_versioned_memory_sources() {
        let temp = Temp::new();
        let h = History::open(&temp.0).unwrap();
        h.import_messages(&[
            crate::store::Message {
                id: 1,
                role: "proactive".into(),
                content: "画面中有报错".into(),
                created_at: now(),
            },
            crate::store::Message {
                id: 2,
                role: "user".into(),
                content: "怎么解决".into(),
                created_at: now(),
            },
        ])
        .unwrap();
        assert_eq!(
            h.search("", None, None, Some("observation")).unwrap()["items"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            h.search("", None, None, Some("chat")).unwrap()["items"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let interaction = h.start_interaction("chat", "怎么解决").unwrap();
        h.link_message(
            interaction,
            3,
            "assistant",
            "这是账号登录失败，可以重新登录",
        )
        .unwrap();
        let found = h.search("登录", None, None, Some("chat")).unwrap();
        assert!(found["items"][0]["snippet"]
            .as_str()
            .unwrap()
            .contains("登录"));
        h.record_tool(
            interaction,
            "remember",
            &json!({"id":12,"updated_at":"revision-one","content":"fact"}),
        )
        .unwrap();
        assert_eq!(
            h.memory_source(12, "revision-one").unwrap(),
            Some(interaction)
        );
        assert_eq!(h.memory_source(12, "manual-update").unwrap(), None);
        h.clear_all().unwrap();
        assert_eq!(h.memory_source(12, "revision-one").unwrap(), None);
    }
    #[test]
    fn reopens_and_removes_orphan_files_without_importing_images_into_context() {
        let temp = Temp::new();
        let h = History::open(&temp.0).unwrap();
        let interaction = h.start_interaction("chat", "hello").unwrap();
        let shot = h.save_screenshot(interaction, &jpeg(3), "screen").unwrap();
        h.link_message(interaction, 1, "user", "hello").unwrap();
        h.record_tool(
            interaction,
            "web_search",
            &json!({"url":"https://example.com","title":"source"}),
        )
        .unwrap();
        fs::write(h.images.join("999.tmp"), "partial").unwrap();
        fs::write(h.images.join("998.jpg"), "orphan").unwrap();
        drop(h);
        let h = History::open(&temp.0).unwrap();
        assert!(h.view(shot).is_ok());
        assert!(!h.images.join("999.tmp").exists());
        assert!(!h.images.join("998.jpg").exists());
        assert!(!h.read(interaction).unwrap().to_string().contains("base64"));
        assert!(!h
            .recent_context(&[1])
            .unwrap()
            .to_string()
            .contains("base64"));
    }
    #[test]
    fn refuses_invalid_images_and_missing_interactions_without_orphans() {
        let temp = Temp::new();
        let h = History::open(&temp.0).unwrap();
        let interaction = h.start_interaction("chat", "hello").unwrap();
        assert!(h
            .save_screenshot(interaction, "data:image/png;base64,abc", "screen")
            .is_err());
        assert!(h
            .save_screenshot(interaction, "data:image/jpeg;base64,YWJj", "screen")
            .is_err());
        assert!(h.save_screenshot(999, &jpeg(1), "screen").is_err());
        assert_eq!(h.stats().unwrap()["image_count"], 0);
        assert_eq!(fs::read_dir(&h.images).unwrap().count(), 0);
    }
}
