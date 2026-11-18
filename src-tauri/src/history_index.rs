//! Derived index in history.sqlite3. Raw messages, interaction summaries and tool
//! results stay authoritative. Search ranks documents before callers group by turn.
use rusqlite::{functions::FunctionFlags, Connection, Result, TransactionBehavior};

fn is_han(c: char) -> bool {
    matches!(c as u32, 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xf900..=0xfaff | 0x20000..=0x323af)
}

/// Keep both individual Han characters and adjacent pairs. For languages with
/// spaces, preserve whole words so "paint" does not match "painting".
pub(super) fn terms(text: &str) -> Vec<String> {
    let mut output = Vec::new();
    let mut word = String::new();
    let mut previous_han = None;
    for ch in text.chars() {
        if ch.is_alphanumeric() && !is_han(ch) {
            word.extend(ch.to_lowercase());
            previous_han = None;
            continue;
        }
        if !word.is_empty() {
            output.push(std::mem::take(&mut word));
        }
        if is_han(ch) {
            output.push(ch.to_string());
            if let Some(previous) = previous_han {
                output.push(format!("{previous}{ch}"));
            }
            previous_han = Some(ch);
        } else {
            previous_han = None;
        }
    }
    if !word.is_empty() {
        output.push(word);
    }
    output
}

pub(super) fn match_any(words: &[String]) -> String {
    words
        .iter()
        .map(|s| format!("\"{}\"", s.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ")
}

pub(super) fn initialize(conn: &mut Connection) -> Result<()> {
    conn.create_scalar_function(
        "linger_search_tokens",
        1,
        FunctionFlags::SQLITE_UTF8
            | FunctionFlags::SQLITE_DETERMINISTIC
            | FunctionFlags::SQLITE_INNOCUOUS,
        |ctx| Ok(terms(&ctx.get::<String>(0)?).join(" ")),
    )?;

    // Serialize the one-time migration with all archive writers. A crash rolls
    // back both the schema marker and the partially built inverted index.
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let version: i64 = tx.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version == 0 {
        tx.execute_batch(
            "CREATE TABLE search_documents(
                 id INTEGER PRIMARY KEY, interaction_id INTEGER NOT NULL REFERENCES interactions(id) ON DELETE CASCADE,
                 source TEXT NOT NULL, source_id INTEGER NOT NULL, tokens TEXT NOT NULL,
                 UNIQUE(source, source_id));
             CREATE INDEX search_document_interaction ON search_documents(interaction_id);
             CREATE VIRTUAL TABLE search_fts USING fts5(
                 tokens, content='search_documents', content_rowid='id', tokenize='unicode61');
             CREATE TABLE search_pending(
                 source TEXT NOT NULL, source_id INTEGER NOT NULL,
                 PRIMARY KEY(source,source_id)) WITHOUT ROWID;"
        )?;
    }

    // Persist only the changed source IDs. Older versions and SQLite clients can
    // write the archive without registering our tokenizer. A current connection
    // consumes its own change markers inside the original statement/transaction.
    for (source, table, id, interaction, content) in [
        ("interaction", "interactions", "id", "id", "text"),
        ("summary", "interactions", "id", "id", "summary"),
        (
            "message",
            "history_messages",
            "message_id",
            "interaction_id",
            "content",
        ),
        ("tool", "history_tools", "id", "interaction_id", "result"),
    ] {
        if version == 0 {
            tx.execute_batch(&format!(
                "INSERT INTO search_documents(interaction_id,source,source_id,tokens)
                     SELECT {interaction},'{source}',{id},linger_search_tokens({content})
                     FROM {table} WHERE {content}<>'';
                 CREATE TRIGGER search_{source}_insert AFTER INSERT ON {table} WHEN new.{content}<>'' BEGIN
                     INSERT OR REPLACE INTO search_pending VALUES('{source}',new.{id});
                 END;
                 CREATE TRIGGER search_{source}_update AFTER UPDATE ON {table}
                 WHEN old.{content}<>new.{content} OR old.{id}<>new.{id} OR old.{interaction}<>new.{interaction} BEGIN
                     INSERT OR REPLACE INTO search_pending SELECT '{source}',old.{id} WHERE old.{id}<>new.{id};
                     INSERT OR REPLACE INTO search_pending VALUES('{source}',new.{id});
                 END;
                 CREATE TRIGGER search_{source}_delete AFTER DELETE ON {table} WHEN old.{content}<>'' BEGIN
                     INSERT OR REPLACE INTO search_pending VALUES('{source}',old.{id});
                 END;"
            ))?;
        }
        tx.execute_batch(&format!(
            "CREATE TEMP TRIGGER IF NOT EXISTS search_refresh_{source}
             AFTER DELETE ON main.search_pending WHEN old.source='{source}' BEGIN
                 DELETE FROM search_documents WHERE source='{source}' AND source_id=old.source_id;
                 INSERT INTO search_documents(interaction_id,source,source_id,tokens)
                 SELECT {interaction},'{source}',{id},linger_search_tokens({content})
                 FROM {table} WHERE {id}=old.source_id AND {content}<>'';
             END;"
        ))?;
    }
    if version == 0 {
        tx.execute_batch(
            "INSERT INTO search_fts(search_fts) VALUES('rebuild');
             CREATE TRIGGER search_documents_insert AFTER INSERT ON search_documents BEGIN
                 INSERT INTO search_fts(rowid,tokens) VALUES(new.id,new.tokens);
             END;
             CREATE TRIGGER search_documents_delete AFTER DELETE ON search_documents BEGIN
                 INSERT INTO search_fts(search_fts,rowid,tokens) VALUES('delete',old.id,old.tokens);
             END;
             CREATE TRIGGER search_documents_update AFTER UPDATE ON search_documents BEGIN
                 INSERT INTO search_fts(search_fts,rowid,tokens) VALUES('delete',old.id,old.tokens);
                 INSERT INTO search_fts(rowid,tokens) VALUES(new.id,new.tokens);
             END;
             PRAGMA user_version=1;",
        )?;
    }
    tx.execute_batch(
        "CREATE TEMP TRIGGER IF NOT EXISTS search_flush_insert AFTER INSERT ON main.search_pending BEGIN
             DELETE FROM search_pending WHERE source=new.source AND source_id=new.source_id;
         END;"
    )?;
    sync_pending(&tx)?;
    tx.commit()
}

pub(super) fn sync_pending(conn: &Connection) -> Result<()> {
    // Normal reads do not acquire a write lock. Deleting a marker invokes the
    // TEMP refresh trigger, indexing the current, committed source row or its removal.
    if conn.query_row("SELECT EXISTS(SELECT 1 FROM search_pending)", [], |r| {
        r.get::<_, bool>(0)
    })? {
        conn.execute("DELETE FROM search_pending", [])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count(conn: &Connection, q: &str) -> i64 {
        conn.query_row(
            "SELECT count(*) FROM search_fts WHERE search_fts MATCH ?1",
            [q],
            |row| row.get(0),
        )
        .unwrap()
    }

    #[test]
    fn legacy_writers_can_insert_update_and_delete_without_the_tokenizer() {
        let uri = "file:linger-index-legacy-writer?mode=memory&cache=shared";
        let mut current = Connection::open(uri).unwrap();
        current.execute_batch(
            "CREATE TABLE interactions(id INTEGER PRIMARY KEY,text TEXT,summary TEXT);
             CREATE TABLE history_messages(message_id INTEGER PRIMARY KEY,interaction_id INTEGER,content TEXT);
             CREATE TABLE history_tools(id INTEGER PRIMARY KEY,interaction_id INTEGER,result TEXT);"
        ).unwrap();
        initialize(&mut current).unwrap();
        current
            .execute_batch("INSERT INTO interactions VALUES(1,'原始地点','');")
            .unwrap();
        let old = Connection::open(uri).unwrap();
        old.execute_batch(
            "UPDATE interactions SET text='东侧书亭' WHERE id=1;
             INSERT INTO history_messages VALUES(2,1,'已经出发');
             INSERT INTO history_tools VALUES(3,1,'unplayed');
             DELETE FROM history_tools WHERE id=3;",
        )
        .unwrap();
        sync_pending(&current).unwrap();
        assert_eq!(count(&current, "原始"), 0);
        assert_eq!(count(&current, "书亭"), 1);
        assert_eq!(count(&current, "出发"), 1);
        assert_eq!(count(&current, "unplayed"), 0);
        drop(current);
        old.execute_batch("UPDATE history_messages SET content='到达' WHERE message_id=2;")
            .unwrap();
        let mut reopened = Connection::open(uri).unwrap();
        initialize(&mut reopened).unwrap();
        assert_eq!(count(&reopened, "出发"), 0);
        assert_eq!(count(&reopened, "到达"), 1);
        assert_eq!(
            reopened
                .query_row("SELECT count(*) FROM search_pending", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn backfill_and_triggers_index_each_source_atomically() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "PRAGMA foreign_keys=ON;
             CREATE TABLE interactions(id INTEGER PRIMARY KEY,text TEXT,summary TEXT);
             CREATE TABLE history_messages(message_id INTEGER PRIMARY KEY,interaction_id INTEGER REFERENCES interactions(id) ON DELETE CASCADE,content TEXT);
             CREATE TABLE history_tools(id INTEGER PRIMARY KEY,interaction_id INTEGER REFERENCES interactions(id) ON DELETE CASCADE,result TEXT);
             INSERT INTO interactions VALUES(1,'相约周末','');
             INSERT INTO history_messages VALUES(12,1,'东侧书亭');
             INSERT INTO history_tools VALUES(1,1,'Wildfire');"
        ).unwrap();
        initialize(&mut conn).unwrap();
        initialize(&mut conn).unwrap();
        for q in ["周末", "书亭", "wildfire"] {
            assert_eq!(count(&conn, q), 1);
        }
        conn.execute_batch(
            "UPDATE interactions SET summary='今日散步' WHERE id=1;
                            UPDATE history_messages SET content='改到图书馆' WHERE message_id=12;",
        )
        .unwrap();
        assert_eq!(count(&conn, "散步"), 1);
        assert_eq!(count(&conn, "书亭"), 0);
        assert_eq!(count(&conn, "书馆"), 1);
        conn.execute_batch("BEGIN; DELETE FROM interactions WHERE id=1; ROLLBACK;")
            .unwrap();
        assert_eq!(count(&conn, "wildfire"), 1);
        conn.execute_batch(
            "DELETE FROM interactions WHERE id=1;
           INSERT INTO search_fts(search_fts,rank) VALUES('integrity-check',1);",
        )
        .unwrap();
        for q in ["周末", "散步", "书馆", "wildfire"] {
            assert_eq!(count(&conn, q), 0);
        }
        conn.execute_batch("INSERT INTO interactions VALUES(2,'周末','');")
            .unwrap();
        assert_eq!(count(&conn, "周末"), 1);
    }
}
