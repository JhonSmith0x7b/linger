//! Programmer-facing text logs. Preserve diagnostic errors, redact credentials centrally.
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        LazyLock, Mutex, OnceLock,
    },
    time::Instant,
};
use tauri::Manager;
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_opener::OpenerExt;
const MAX_FILE_BYTES: u64 = 512 * 1024;
const MAX_ROWS: usize = 1000;
static LOG: OnceLock<Mutex<LogStore>> = OnceLock::new();
static SECRETS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static REQUEST_ID: AtomicU64 = AtomicU64::new(1);
static CREDENTIAL: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"(?i)((?:authorization|api[_-]?key|access[_-]?token|password)[\"']?\s*[:=]\s*[\"']?)(?:Bearer\s+)?[^\s\"',;}]+"#).unwrap()
});
static BEARER: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"(?i)Bearer\s+[^\s"<>]+"#).unwrap());
static QUERY: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"(https?://[^\s?\"<>]+)\?[^\s\"<>]+"#).unwrap());
pub fn register_secret(secret: &str) {
    if !secret.is_empty() {
        if let Ok(mut secrets) = SECRETS.lock() {
            if !secrets.iter().any(|s| s == secret) {
                secrets.push(secret.into());
            }
        }
    }
}
pub fn redact(text: &str) -> String {
    let mut text = text.to_owned();
    if let Ok(secrets) = SECRETS.lock() {
        for secret in secrets.iter() {
            text = text.replace(secret, "[REDACTED]");
        }
    }
    let text = CREDENTIAL.replace_all(&text, "${1}[REDACTED]");
    let text = BEARER.replace_all(&text, "Bearer [REDACTED]");
    QUERY
        .replace_all(&text, "${1}?[REDACTED_QUERY]")
        .into_owned()
}
fn format_line(level: &str, category: &str, event: &str, details: &Value) -> String {
    let mut line = format!(
        "{} {:5} [{}] {}",
        chrono::Local::now().to_rfc3339(),
        level.to_uppercase(),
        category,
        event
    );
    if let Some(fields) = details.as_object() {
        for (key, value) in fields {
            if value.is_null() {
                continue;
            }
            line.push_str(&format!(
                " {key}={}",
                value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.to_string())
            ));
        }
    } else {
        line.push_str(&format!(" {details}"));
    }
    let clean = redact(&line);
    if clean.chars().count() > 65536 {
        format!(
            "{}\n[log entry truncated at 65536 characters]",
            clean.chars().take(65536).collect::<String>()
        )
    } else {
        clean
    }
}
struct LogStore {
    dir: PathBuf,
    rows: VecDeque<String>,
    bytes: u64,
    write_failed: bool,
}
impl LogStore {
    fn open(dir: &Path) -> std::io::Result<Self> {
        fs::create_dir_all(dir)?;
        let mut rows = VecDeque::new();
        for name in ["previous.log", "current.log"] {
            if let Ok(file) = fs::File::open(dir.join(name)) {
                for line in BufReader::new(file).lines().map_while(Result::ok) {
                    rows.push_back(line);
                    if rows.len() > MAX_ROWS {
                        rows.pop_front();
                    }
                }
            }
        }
        let bytes = fs::metadata(dir.join("current.log")).map_or(0, |m| m.len());
        Ok(Self {
            dir: dir.into(),
            rows,
            bytes,
            write_failed: false,
        })
    }
    fn append(
        &mut self,
        level: &str,
        category: &str,
        event: &str,
        details: Value,
    ) -> std::io::Result<()> {
        let line = format_line(level, category, event, &details) + "\n";
        if self.bytes + line.len() as u64 > MAX_FILE_BYTES {
            let old = self.dir.join("previous.log");
            if old.exists() {
                fs::remove_file(&old)?;
            }
            if self.dir.join("current.log").exists() {
                fs::rename(self.dir.join("current.log"), old)?;
            }
            self.bytes = 0;
        }
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options
            .open(self.dir.join("current.log"))?
            .write_all(line.as_bytes())?;
        self.bytes += line.len() as u64;
        for part in line.lines() {
            self.rows.push_back(part.into());
            if self.rows.len() > MAX_ROWS {
                self.rows.pop_front();
            }
        }
        Ok(())
    }
    fn clear(&mut self) -> std::io::Result<()> {
        for name in [
            "current.log",
            "previous.log",
            "current.jsonl",
            "previous.jsonl",
        ] {
            match fs::remove_file(self.dir.join(name)) {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(e),
            }
        }
        self.rows.clear();
        self.bytes = 0;
        self.write_failed = false;
        Ok(())
    }
}
pub fn init(dir: &Path) -> std::io::Result<()> {
    let _ = LOG.set(Mutex::new(LogStore::open(&dir.join("logs"))?));
    record(
        "info",
        "app",
        "app_start",
        json!({"version":env!("CARGO_PKG_VERSION"),"platform":std::env::consts::OS}),
    );
    Ok(())
}
pub fn record(level: &str, category: &str, event: &str, details: Value) {
    if let Some(log) = LOG.get() {
        if let Ok(mut log) = log.lock() {
            log.write_failed = log.append(level, category, event, details).is_err();
        }
    }
}
#[tauri::command]
pub fn get_diagnostic_logs() -> Result<String, String> {
    let log = LOG
        .get()
        .ok_or("日志尚未初始化")?
        .lock()
        .map_err(|_| "无法读取日志")?;
    if log.write_failed {
        return Err("日志写入失败，请检查磁盘空间或应用数据目录权限".into());
    }
    Ok(log.rows.iter().cloned().collect::<Vec<_>>().join("\n"))
}
#[tauri::command]
pub fn clear_diagnostic_logs() -> Result<(), String> {
    LOG.get()
        .ok_or("日志尚未初始化")?
        .lock()
        .map_err(|_| "无法清空日志")?
        .clear()
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub fn copy_diagnostic_logs(app: tauri::AppHandle) -> Result<(), String> {
    app.clipboard()
        .write_text(get_diagnostic_logs()?)
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub fn open_diagnostic_log_folder(app: tauri::AppHandle) -> Result<(), String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("logs");
    app.opener()
        .open_path(dir.to_string_lossy(), None::<&str>)
        .map_err(|e| e.to_string())
}
pub struct RequestTrace {
    id: String,
    category: &'static str,
    started: Instant,
    finished: bool,
}
impl RequestTrace {
    pub fn new(category: &'static str) -> Self {
        let trace = Self {
            id: format!(
                "{}-{}",
                std::process::id(),
                REQUEST_ID.fetch_add(1, Ordering::Relaxed)
            ),
            category,
            started: Instant::now(),
            finished: false,
        };
        record(
            "info",
            category,
            "request_start",
            json!({"request_id":trace.id}),
        );
        trace
    }
    pub fn retry(&self, attempt: usize, error: &str) {
        record(
            "warn",
            self.category,
            "request_retry",
            json!({"request_id":self.id,"attempt":attempt,"next_attempt":attempt+1,"error":error}),
        );
    }
    pub fn success(&mut self, attempt: usize, usage: Option<&Value>) {
        self.finished = true;
        record(
            "info",
            self.category,
            "request_success",
            json!({"request_id":self.id,"attempts":attempt,"elapsed_ms":self.started.elapsed().as_millis() as u64,"usage":usage}),
        );
    }
    pub fn failure(&mut self, attempt: usize, error: &str) {
        self.finished = true;
        record(
            "error",
            self.category,
            "request_failed",
            json!({"request_id":self.id,"attempts":attempt,"elapsed_ms":self.started.elapsed().as_millis() as u64,"error":error}),
        );
    }
}
impl Drop for RequestTrace {
    fn drop(&mut self) {
        if !self.finished {
            record(
                "info",
                self.category,
                "request_cancelled",
                json!({"request_id":self.id,"elapsed_ms":self.started.elapsed().as_millis() as u64}),
            );
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_errors_survive_but_credentials_do_not() {
        register_secret("test-secret-credential");
        let error = "HTTP 400: Unknown parameter input[14].content; test-secret-credential";
        let line = format_line(
            "error",
            "responses",
            "request_failed",
            &json!({"error":error}),
        );
        assert!(line.contains("Unknown parameter input[14].content"));
        assert!(!line.contains("test-secret-credential"));
        assert!(
            !redact(r#"{"api_key":"other-secret"} Authorization: Bearer token-value"#)
                .contains("other-secret")
        );
        assert!(!redact("Authorization: Bearer token-value").contains("token-value"));
        assert!(
            !redact("https://example.com/audio?signature=private").contains("signature=private")
        );
    }
    #[test]
    fn rotation_restart_and_clear_are_bounded() {
        let dir = std::env::temp_dir().join(format!(
            "linger-plain-logs-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let mut log = LogStore::open(&dir).unwrap();
        for i in 0..1800 {
            log.append(
                "info",
                "test",
                "line",
                json!({"index":i,"padding":"x".repeat(600)}),
            )
            .unwrap();
        }
        assert_eq!(log.rows.len(), MAX_ROWS);
        for name in ["current.log", "previous.log"] {
            assert!(fs::metadata(dir.join(name)).unwrap().len() <= MAX_FILE_BYTES);
        }
        let mut restored = LogStore::open(&dir).unwrap();
        assert!(restored.rows.back().unwrap().contains("index=1799"));
        restored.clear().unwrap();
        assert!(LogStore::open(&dir).unwrap().rows.is_empty());
        fs::remove_dir_all(dir).unwrap();
    }
}
