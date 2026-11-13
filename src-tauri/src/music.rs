//! Small authenticated loopback transport for the opt-in YouTube Music extension.
//! Commands are delivered once. An ambiguous timeout never retries a player mutation.
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::oneshot;

const PORT: u16 = 47831;
const MAX_BODY: usize = 32 * 1024;
const MAX_HEADERS: usize = 8 * 1024;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECTED_WINDOW: Duration = Duration::from_secs(15);

struct Pending {
    id: u64,
    action: String,
    args: Value,
    deadline: Instant,
    delivered: bool,
    reply: oneshot::Sender<Result<Value, String>>,
}
#[derive(Default)]
struct BridgeState {
    client: Option<String>,
    last_poll: Option<Instant>,
    pending: Option<Pending>,
}

pub struct MusicBridge {
    token: String,
    state: Mutex<BridgeState>,
    next_id: AtomicU64,
    stopped: AtomicBool,
    listener: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl MusicBridge {
    pub fn start(dir: &Path) -> Result<Arc<Self>, String> {
        let token = load_token(dir)?;
        let listener = TcpListener::bind(("127.0.0.1", PORT))
            .map_err(|e| format!("音乐连接端口 {PORT} 无法监听：{e}"))?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let mut id_seed = [0u8; 8];
        getrandom::getrandom(&mut id_seed).map_err(|e| format!("生成音乐连接标识失败：{e}"))?;
        let bridge = Arc::new(Self {
            token,
            state: Mutex::new(BridgeState::default()),
            next_id: AtomicU64::new(u64::from_le_bytes(id_seed) & ((1u64 << 52) - 1)),
            stopped: AtomicBool::new(false),
            listener: Mutex::new(None),
        });
        let service = Arc::clone(&bridge);
        let listener_thread = std::thread::Builder::new()
            .name("music-bridge".into())
            .spawn(move || {
                let connections = Arc::new(AtomicUsize::new(0));
                while !service.stopped.load(Ordering::SeqCst) {
                    let stream = match listener.accept() {
                        Ok((stream, _)) => stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(50));
                            continue;
                        }
                        Err(_) => break,
                    };
                    if connections.fetch_add(1, Ordering::SeqCst) >= 8 {
                        connections.fetch_sub(1, Ordering::SeqCst);
                        continue;
                    }
                    let service = Arc::clone(&service);
                    let connections = Arc::clone(&connections);
                    std::thread::spawn(move || {
                        service.serve(stream);
                        connections.fetch_sub(1, Ordering::SeqCst);
                    });
                }
            })
            .map_err(|e| format!("无法启动音乐连接：{e}"))?;
        *bridge.listener.lock().unwrap() = Some(listener_thread);
        Ok(bridge)
    }

    /// Only return this from the local settings UI; never include it in model context or logs.
    pub fn info(&self) -> Value {
        let state = self.state.lock().unwrap();
        json!({"enabled":!self.stopped.load(Ordering::SeqCst),"port":PORT,"token":self.token,"connected":!self.stopped.load(Ordering::SeqCst) && connected(&state),
            "busy":state.pending.is_some(),"client":state.client})
    }

    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        let mut state = self.state.lock().unwrap();
        state.last_poll = None;
        if let Some(pending) = state.pending.take() {
            let _ = pending.reply.send(Err(
                "音乐连接已关闭；已发出的操作可能仍会完成，请在浏览器确认。".into(),
            ));
        }
        drop(state);
        // Wake the listener so the port is released promptly on the next loop.
        let _ = TcpStream::connect(("127.0.0.1", PORT));
        if let Some(thread) = self.listener.lock().unwrap().take() {
            let _ = thread.join();
        }
    }

    pub async fn execute(&self, action: &str, args: Value) -> Result<Value, String> {
        validate_action(action, &args)?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        {
            let mut state = self.state.lock().unwrap();
            if self.stopped.load(Ordering::SeqCst) || !connected(&state) {
                return Err(
                    "音乐浏览器未连接。请在扩展中连接 Linger 并绑定已登录的 YouTube Music 标签页。"
                        .into(),
                );
            }
            if state.pending.is_some() {
                return Err("已有音乐操作正在进行，请等待完成。".into());
            }
            state.pending = Some(Pending {
                id,
                action: action.into(),
                args,
                deadline: Instant::now() + COMMAND_TIMEOUT,
                delivered: false,
                reply: tx,
            });
        }
        let _cleanup = PendingGuard { bridge: self, id };
        match tokio::time::timeout(COMMAND_TIMEOUT, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err("音乐连接已中断，操作结果未知；不会自动重复操作。".into()),
            Err(_) => Err(
                "音乐操作在 30 秒内未得到确认，结果未知；请查询状态，不要直接重复播放或下一首。"
                    .into(),
            ),
        }
    }

    fn serve(&self, mut stream: TcpStream) {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
        let request = match read_request(&mut stream) {
            Ok(request) => request,
            Err(_) => {
                respond(&mut stream, 400, None, json!({"error":"invalid request"}));
                return;
            }
        };
        let origin = request.headers.get("origin").map(String::as_str);
        if request.method == "OPTIONS" {
            if origin
                .and_then(|o| o.strip_prefix("chrome-extension://"))
                .is_some_and(valid_client)
            {
                respond(&mut stream, 204, origin, Value::Null);
            } else {
                respond(&mut stream, 403, None, json!({"error":"invalid origin"}));
            }
            return;
        }
        let client = match authenticate(&request.headers, &self.token) {
            Some(client) => client,
            None => {
                respond(&mut stream, 403, None, json!({"error":"unauthorized"}));
                return;
            }
        };
        let (code, result) = self.handle(&request, client);
        respond(&mut stream, code, origin, result);
    }

    fn handle(&self, request: &Request, client: &str) -> (u16, Value) {
        let mut state = self.state.lock().unwrap();
        if self.stopped.load(Ordering::SeqCst) {
            return (409, json!({"error":"bridge disabled"}));
        }
        if state.client.as_deref().is_some_and(|bound| bound != client) {
            return (
                403,
                json!({"error":"another extension is already bound; disable and re-enable music connection before rebinding"}),
            );
        }
        match (request.method.as_str(), request.path.as_str()) {
            ("GET", "/poll") => {
                state.client = Some(client.into());
                state.last_poll = Some(Instant::now());
                if state
                    .pending
                    .as_ref()
                    .is_some_and(|p| p.deadline <= Instant::now())
                {
                    if let Some(pending) = state.pending.take() {
                        let _ = pending.reply.send(Err("音乐指令已过期，未重试。".into()));
                    }
                }
                let command = state.pending.as_mut().and_then(|pending| {
                    if pending.delivered {
                        return None;
                    }
                    pending.delivered = true;
                    Some(json!({"id":pending.id,"action":pending.action,"args":pending.args,
                        "expires_at_ms":(SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default()
                            + pending.deadline.saturating_duration_since(Instant::now())).as_millis() as u64}))
                });
                (200, json!({"command":command}))
            }
            ("POST", "/result") => {
                if state.client.as_deref() != Some(client) {
                    return (403, json!({"error":"poll before returning results"}));
                }
                let body: Value = match serde_json::from_slice(&request.body) {
                    Ok(value) => value,
                    Err(_) => return (400, json!({"error":"invalid JSON"})),
                };
                let valid = state.pending.as_ref().is_some_and(|p| {
                    body["id"].as_u64() == Some(p.id) && p.delivered && p.deadline > Instant::now()
                });
                if !valid {
                    return (409, json!({"error":"stale or unknown command"}));
                }
                if !body.get("result").is_some_and(Value::is_object)
                    && !body.get("error").is_some_and(Value::is_string)
                {
                    return (
                        400,
                        json!({"error":"result object or error string required"}),
                    );
                }
                state.last_poll = Some(Instant::now());
                let pending = state.pending.take().unwrap();
                let result = if let Some(error) = body["error"].as_str() {
                    Err(error.chars().take(1000).collect())
                } else {
                    Ok(body["result"].clone())
                };
                let _ = pending.reply.send(result);
                (200, json!({"ok":true}))
            }
            ("GET", "/status") => (
                200,
                json!({"connected":connected(&state),"busy":state.pending.is_some()}),
            ),
            _ => (404, json!({"error":"not found"})),
        }
    }
}

struct PendingGuard<'a> {
    bridge: &'a MusicBridge,
    id: u64,
}
impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        let mut state = self.bridge.state.lock().unwrap();
        if state.pending.as_ref().is_some_and(|p| p.id == self.id) {
            state.pending.take();
        }
    }
}
fn connected(state: &BridgeState) -> bool {
    state
        .last_poll
        .is_some_and(|last| last.elapsed() < CONNECTED_WINDOW)
        || state
            .pending
            .as_ref()
            .is_some_and(|pending| pending.delivered && pending.deadline > Instant::now())
}
fn validate_action(action: &str, args: &Value) -> Result<(), String> {
    if !args.is_object() || args.to_string().len() > 2048 {
        return Err("音乐参数无效或过长。".into());
    }
    match action {
        "search"
            if args["query"]
                .as_str()
                .is_some_and(|s| !s.trim().is_empty() && s.chars().count() <= 200) =>
        {
            Ok(())
        }
        "play" | "queue"
            if args["video_id"].as_str().is_some_and(|s| {
                s.len() == 11
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            }) =>
        {
            Ok(())
        }
        "pause" | "resume" | "next" | "status" => Ok(()),
        _ => Err("不支持的音乐操作或无效的歌曲 ID。".into()),
    }
}

fn load_token(dir: &Path) -> Result<String, String> {
    let path = dir.join("music-bridge-token");
    match fs::read_to_string(&path) {
        Ok(token) => {
            if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(
                    "音乐连接凭据文件格式无效，请删除 music-bridge-token 后重新连接。".into(),
                );
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
                    .map_err(|e| e.to_string())?;
            }
            Ok(token)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let mut bytes = [0u8; 32];
            getrandom::getrandom(&mut bytes).map_err(|e| format!("生成音乐连接凭据失败：{e}"))?;
            let token: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options
                .open(path)
                .map_err(|e| format!("保存音乐连接凭据失败：{e}"))?;
            file.write_all(token.as_bytes())
                .map_err(|e| e.to_string())?;
            Ok(token)
        }
        Err(e) => Err(format!("读取音乐连接凭据失败：{e}")),
    }
}

struct Request {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}
fn read_request(stream: &mut impl Read) -> Result<Request, String> {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut bytes = Vec::new();
    let header_end = loop {
        if Instant::now() > deadline {
            return Err("request timed out".into());
        }
        let mut byte = [0];
        if stream.read(&mut byte).map_err(|e| e.to_string())? == 0 {
            return Err("unexpected EOF".into());
        }
        bytes.push(byte[0]);
        if bytes.ends_with(b"\r\n\r\n") {
            break bytes.len();
        }
        if bytes.len() >= MAX_HEADERS {
            return Err("headers too large".into());
        }
    };
    let text = std::str::from_utf8(&bytes[..header_end]).map_err(|e| e.to_string())?;
    let mut lines = text.split("\r\n");
    let first: Vec<_> = lines.next().unwrap_or("").split(' ').collect();
    if first.len() != 3 || first[2] != "HTTP/1.1" {
        return Err("invalid request line".into());
    }
    let method = first[0].to_owned();
    let path = first[1].to_owned();
    let mut headers = HashMap::new();
    for line in lines.take_while(|line| !line.is_empty()) {
        let (key, value) = line.split_once(':').ok_or("invalid header")?;
        if headers
            .insert(key.to_ascii_lowercase(), value.trim().to_owned())
            .is_some()
        {
            return Err("duplicate header".into());
        }
    }
    if headers.get("host").map(String::as_str) != Some("127.0.0.1:47831")
        || headers.contains_key("transfer-encoding")
    {
        return Err("invalid host or encoding".into());
    }
    let length = headers
        .get("content-length")
        .map(|s| s.parse::<usize>())
        .transpose()
        .map_err(|e| e.to_string())?
        .unwrap_or(0);
    if length > MAX_BODY || (method != "POST" && length > 0) {
        return Err("invalid body size".into());
    }
    let mut body = vec![0; length];
    let mut read = 0;
    while read < length {
        if Instant::now() > deadline {
            return Err("request timed out".into());
        }
        let count = stream.read(&mut body[read..]).map_err(|e| e.to_string())?;
        if count == 0 {
            return Err("unexpected EOF".into());
        }
        read += count;
    }
    Ok(Request {
        method,
        path,
        headers,
        body,
    })
}
fn valid_client(client: &str) -> bool {
    client.len() == 32 && client.bytes().all(|b| (b'a'..=b'p').contains(&b))
}
fn authenticate<'a>(headers: &'a HashMap<String, String>, token: &str) -> Option<&'a str> {
    let client = headers.get("x-linger-client")?;
    if !valid_client(client) {
        return None;
    }
    let expected = format!("Bearer {token}");
    let actual = headers.get("authorization")?;
    // Fixed-sized token comparison, not a prefix match.
    if expected.len() != actual.len()
        || expected
            .bytes()
            .zip(actual.bytes())
            .fold(0u8, |diff, (a, b)| diff | (a ^ b))
            != 0
    {
        return None;
    }
    if headers
        .get("origin")
        .is_some_and(|origin| origin != &format!("chrome-extension://{client}"))
    {
        return None;
    }
    Some(client)
}
fn respond(stream: &mut TcpStream, code: u16, origin: Option<&str>, value: Value) {
    let body = if code == 204 {
        String::new()
    } else {
        value.to_string()
    };
    let reason = match code {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        _ => "Error",
    };
    let cors = origin.map(|origin| format!("Access-Control-Allow-Origin: {origin}\r\nVary: Origin\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Authorization, Content-Type, X-Linger-Client\r\nAccess-Control-Allow-Private-Network: true\r\n")).unwrap_or_default();
    let response = format!("HTTP/1.1 {code} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n{cors}\r\n{body}", body.len());
    let _ = stream.write_all(response.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    fn bridge() -> MusicBridge {
        MusicBridge {
            token: "a".repeat(64),
            state: Mutex::new(BridgeState::default()),
            next_id: AtomicU64::new(1),
            stopped: AtomicBool::new(false),
            listener: Mutex::new(None),
        }
    }
    fn request(method: &str, path: &str, body: Value) -> Request {
        Request {
            method: method.into(),
            path: path.into(),
            headers: HashMap::new(),
            body: body.to_string().into_bytes(),
        }
    }
    #[test]
    fn authentication_requires_token_and_extension_identity() {
        let mut headers = HashMap::from([
            ("authorization".into(), "Bearer secret".into()),
            ("x-linger-client".into(), "a".repeat(32)),
        ]);
        assert!(authenticate(&headers, "secret").is_some());
        headers.insert("origin".into(), "https://evil.example".into());
        assert!(authenticate(&headers, "secret").is_none());
        headers.insert(
            "origin".into(),
            format!("chrome-extension://{}", "a".repeat(32)),
        );
        assert!(authenticate(&headers, "secret").is_some());
        assert!(authenticate(&headers, "wrong").is_none());
        headers.remove("x-linger-client");
        assert!(authenticate(&headers, "secret").is_none());
    }
    #[test]
    fn parser_rejects_oversized_duplicate_or_rebound_requests() {
        for raw in [
            "POST /result HTTP/1.1\r\nHost: 127.0.0.1:47831\r\nContent-Length: 999999\r\n\r\n",
            "GET /poll HTTP/1.1\r\nHost: 127.0.0.1:47831\r\nHost: example.com\r\n\r\n",
            "GET /poll HTTP/1.1\r\nHost: example.com:47831\r\n\r\n",
        ] {
            assert!(read_request(&mut raw.as_bytes()).is_err());
        }
        assert!(read_request(
            &mut "GET /poll HTTP/1.1\r\nHost: 127.0.0.1:47831\r\n\r\n".as_bytes()
        )
        .is_ok());
    }
    #[test]
    fn command_delivered_once_and_stale_results_cannot_complete_it() {
        let bridge = bridge();
        let client = "a".repeat(32);
        let (tx, mut rx) = oneshot::channel();
        bridge.state.lock().unwrap().pending = Some(Pending {
            id: 7,
            action: "next".into(),
            args: json!({}),
            deadline: Instant::now() + COMMAND_TIMEOUT,
            delivered: false,
            reply: tx,
        });
        assert_eq!(
            bridge
                .handle(
                    &request("POST", "/result", json!({"id":7,"result":{}})),
                    &client
                )
                .0,
            403
        );
        assert_eq!(
            bridge
                .handle(&request("GET", "/poll", Value::Null), &client)
                .1["command"]["id"],
            7
        );
        assert!(bridge
            .handle(&request("GET", "/poll", Value::Null), &client)
            .1["command"]
            .is_null());
        assert_eq!(
            bridge
                .handle(
                    &request("POST", "/result", json!({"id":8,"result":{}})),
                    &client
                )
                .0,
            409
        );
        assert!(rx.try_recv().is_err());
        assert_eq!(
            bridge
                .handle(
                    &request("POST", "/result", json!({"id":7,"result":{"playing":true}})),
                    &client
                )
                .0,
            200
        );
        assert_eq!(rx.try_recv().unwrap().unwrap()["playing"], true);
        assert_eq!(
            bridge
                .handle(
                    &request("POST", "/result", json!({"id":7,"result":{}})),
                    &client
                )
                .0,
            409
        );
    }
    #[test]
    fn expired_and_cancelled_commands_are_never_delivered() {
        let bridge = bridge();
        let (tx, mut rx) = oneshot::channel();
        bridge.state.lock().unwrap().pending = Some(Pending {
            id: 1,
            action: "next".into(),
            args: json!({}),
            deadline: Instant::now() - Duration::from_secs(1),
            delivered: false,
            reply: tx,
        });
        assert!(bridge
            .handle(&request("GET", "/poll", Value::Null), &"a".repeat(32))
            .1["command"]
            .is_null());
        assert!(rx.try_recv().unwrap().is_err());
        let (tx, _) = oneshot::channel();
        bridge.state.lock().unwrap().pending = Some(Pending {
            id: 2,
            action: "next".into(),
            args: json!({}),
            deadline: Instant::now() + COMMAND_TIMEOUT,
            delivered: false,
            reply: tx,
        });
        drop(PendingGuard {
            bridge: &bridge,
            id: 2,
        });
        assert!(bridge.state.lock().unwrap().pending.is_none());
    }

    #[tokio::test]
    async fn dropping_execute_cancels_undelivered_command() {
        let bridge = Arc::new(bridge());
        bridge.handle(&request("GET", "/poll", Value::Null), &"a".repeat(32));
        let worker = Arc::clone(&bridge);
        let task = tokio::spawn(async move { worker.execute("next", json!({})).await });
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(bridge.state.lock().unwrap().pending.is_some());
        task.abort();
        let _ = task.await;
        assert!(bridge.state.lock().unwrap().pending.is_none());
        assert!(bridge
            .handle(&request("GET", "/poll", Value::Null), &"a".repeat(32))
            .1["command"]
            .is_null());
    }

    #[test]
    fn pairing_token_is_private_and_persistent() {
        let mut suffix = [0u8; 8];
        getrandom::getrandom(&mut suffix).unwrap();
        let dir =
            std::env::temp_dir().join(format!("linger-music-test-{}", u64::from_le_bytes(suffix)));
        fs::create_dir(&dir).unwrap();
        let token = load_token(&dir).unwrap();
        assert_eq!(token.len(), 64);
        assert_eq!(load_token(&dir).unwrap(), token);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(dir.join("music-bridge-token"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn socket_transport_auth_poll_execute_and_result_roundtrip() {
        // Ephemeral TCP port avoids touching a running user's bridge. The protocol
        // Host remains its production value, independently of the TCP test port.
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let bridge = Arc::new(bridge());
        let server = Arc::clone(&bridge);
        let server_thread = std::thread::spawn(move || {
            for stream in listener.incoming().take(4) {
                server.serve(stream.unwrap());
            }
        });
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap();
        let url = format!("http://{address}");
        let unauthenticated = client
            .get(format!("{url}/poll"))
            .header("Host", "127.0.0.1:47831")
            .send()
            .await
            .unwrap();
        assert_eq!(unauthenticated.status(), reqwest::StatusCode::FORBIDDEN);
        let headers = || {
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert("Host", "127.0.0.1:47831".parse().unwrap());
            headers.insert(
                "Authorization",
                format!("Bearer {}", "a".repeat(64)).parse().unwrap(),
            );
            headers.insert("X-Linger-Client", "a".repeat(32).parse().unwrap());
            headers.insert(
                "Origin",
                format!("chrome-extension://{}", "a".repeat(32))
                    .parse()
                    .unwrap(),
            );
            headers
        };
        let first: Value = client
            .get(format!("{url}/poll"))
            .headers(headers())
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(first["command"].is_null());
        let worker = Arc::clone(&bridge);
        let command = tokio::spawn(async move { worker.execute("status", json!({})).await });
        tokio::task::yield_now().await;
        assert!(bridge.state.lock().unwrap().pending.is_some());
        let poll: Value = client
            .get(format!("{url}/poll"))
            .headers(headers())
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(poll["command"]["action"], "status");
        assert!(
            poll["command"]["expires_at_ms"].as_u64().unwrap()
                > SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64
        );
        let response = client
            .post(format!("{url}/result"))
            .headers(headers())
            .json(&json!({
                "id":poll["command"]["id"], "result":{"video_id":"2FUiZYrPZWk","paused":false}
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let actual = command.await.unwrap().unwrap();
        assert_eq!(actual["video_id"], "2FUiZYrPZWk");
        assert_eq!(actual["paused"], false);
        assert!(bridge.state.lock().unwrap().pending.is_none());
        server_thread.join().unwrap();
    }
    #[test]
    fn validate_only_supported_actions_and_bounded_ids() {
        assert!(validate_action("search", &json!({"query":"王菲"})).is_ok());
        assert!(validate_action("play", &json!({"video_id":"2FUiZYrPZWk"})).is_ok());
        assert!(validate_action("play", &json!({"video_id":"https://example.com"})).is_err());
        assert!(validate_action("search", &json!({"query":" "})).is_err());
        assert!(validate_action("eval", &json!({})).is_err());
    }
}
