#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod api;
mod credentials;
mod devices;
mod overlay;
mod session;
mod settings;
mod shortcut_capture;
mod speech_text;
mod store;

use devices::{CaptureSource, Devices};
use serde_json::json;
use settings::Settings;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};
use store::{Memory, Message, Store};
use tauri::{Emitter, Manager, State};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use tauri_plugin_opener::OpenerExt;

pub struct Core {
    settings: Mutex<Settings>,
    credentials: credentials::Credentials,
    last_status: Mutex<serde_json::Value>,
    observation_status: Mutex<serde_json::Value>,
    store: Store,
    devices: Devices,
    generation: AtomicU64,
    turn_gate: Mutex<()>,
    recording_epoch: AtomicU64,
    observation_request: AtomicU64,
    memory_retry_after: AtomicU64,
    busy: AtomicBool,
    recording: AtomicBool,
    cancelled: tokio::sync::watch::Sender<u64>,
    last_frame: Mutex<Option<u64>>,
    activity: Mutex<String>,
    memory_busy: AtomicBool,
    shortcut_capture: Mutex<shortcut_capture::ShortcutCapture>,
}
impl Core {
    fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }
    fn key_for(&self, base: &str, retry: bool) -> Result<String, String> {
        self.credentials.get(base, retry, || {
            key_entry(base)?.get_password().map_err(|e| match e {
                keyring::Error::NoEntry => "请在设置中保存当前地址的 API Key".into(),
                _ => "系统凭据访问未获允许或不可用，请允许 Linger 访问钥匙串后点击读取模型重试"
                    .into(),
            })
        })
    }
    fn key(&self) -> Result<String, String> {
        self.key_for(&self.settings().base_url, false)
    }
    fn api(&self) -> Result<api::Api, String> {
        let base = self.settings().base_url;
        api::Api::new(&base, &self.key_for(&base, false)?)
    }
    fn interrupt(&self) {
        let _gate = self.turn_gate.lock().unwrap();
        self.interrupt_locked();
    }
    fn interrupt_locked(&self) {
        let id = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.cancelled.send_replace(id);
        self.devices.stop_playback();
        self.busy.store(false, Ordering::SeqCst);
    }
}
fn key_entry(base: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new("com.linger.desktop", base).map_err(|e| format!("系统凭据存储不可用：{e}"))
}
fn status(app: &tauri::AppHandle, phase: &str, detail: &str) {
    let value = json!({"phase":phase,"detail":detail});
    *app.state::<Arc<Core>>().last_status.lock().unwrap() = value.clone();
    let _ = app.emit("linger:status", value);
    if matches!(phase, "recording" | "error")
        && !app
            .get_webview_window("main")
            .is_some_and(|w| w.is_focused().unwrap_or(false))
    {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(error) = show_compact(&app) {
                eprintln!("无法显示录音状态：{error}");
            }
        });
    }
}
fn publish(app: &tauri::AppHandle, msg: &Message) {
    let _ = app.emit("linger:message", msg);
}

#[tauri::command]
fn open_external_url(app: tauri::AppHandle, url: String) -> Result<(), String> {
    let parsed = reqwest::Url::parse(&url).map_err(|_| "链接无效")?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err("仅支持打开 HTTP(S) 网页链接".into());
    }
    app.opener()
        .open_url(parsed.as_str(), None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn copy_text(app: tauri::AppHandle, text: String) -> Result<(), String> {
    if text.chars().count() > 60000 {
        return Err("复制文本过长".into());
    }
    app.clipboard()
        .write_text(text)
        .map_err(|_| "无法写入系统剪贴板，请手动选中译文复制".into())
}

#[tauri::command]
async fn translate_text(core: State<'_, Arc<Core>>, text: String) -> Result<String, String> {
    let settings = core.settings();
    // Snapshot credentials and configuration together; translation is independent of companion turns.
    let key = core.key_for(&settings.base_url, false)?;
    let api = api::Api::new(&settings.base_url, &key)?;
    api.translate(
        &settings.translation_model,
        &settings.translation_language,
        &text,
    )
    .await
}

#[tauri::command]
fn get_observation_status(core: State<Arc<Core>>) -> serde_json::Value {
    core.observation_status.lock().unwrap().clone()
}

#[tauri::command]
fn get_status(core: State<Arc<Core>>) -> serde_json::Value {
    core.last_status.lock().unwrap().clone()
}

#[tauri::command]
fn get_settings(core: State<Arc<Core>>) -> Settings {
    core.settings()
}
#[tauri::command]
fn key_status(core: State<Arc<Core>>) -> bool {
    core.key().is_ok()
}

#[tauri::command]
async fn list_models(
    core: State<'_, Arc<Core>>,
    base_url: String,
    key: Option<String>,
) -> Result<Vec<String>, String> {
    // Bind saved credentials to the requested destination, never to a separate mutable setting.
    let base = base_url.trim().trim_end_matches('/').to_owned();
    // Validate before consulting the credential store, even when no Key has been entered.
    api::Api::new(&base, "validation-only")?;
    let key = match key.filter(|value| !value.trim().is_empty()) {
        Some(key) => key,
        None => core.key_for(&base, true)?,
    };
    api::Api::new(&base, &key)?.list_models().await
}
#[tauri::command]
fn set_api_key(core: State<Arc<Core>>, key: String) -> Result<(), String> {
    let base = core.settings().base_url;
    let entry = key_entry(&base)?;
    let key = key.trim();
    core.credentials
        .write(&base, if key.is_empty() { None } else { Some(key) }, || {
            if key.is_empty() {
                match entry.delete_credential() {
                    Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                    Err(e) => Err(e.to_string()),
                }
            } else {
                entry.set_password(key).map_err(|e| e.to_string())
            }
        })
}
#[tauri::command]
fn save_settings(
    app: tauri::AppHandle,
    core: State<Arc<Core>>,
    mut settings: Settings,
) -> Result<(), String> {
    if core.shortcut_capture.lock().unwrap().active().is_some() {
        return Err("请先完成或取消快捷键录入，再保存设置".into());
    }
    settings.validate()?;
    let old = core.settings();
    let old_registered = app.global_shortcut().is_registered(old.shortcut.as_str());
    let new_shortcut = settings
        .shortcut
        .parse::<tauri_plugin_global_shortcut::Shortcut>()
        .map_err(|_| "快捷键格式无效，请重新按键录入".to_string())?;
    let old_id = old
        .shortcut
        .parse::<tauri_plugin_global_shortcut::Shortcut>()
        .ok()
        .map(|shortcut| shortcut.id());
    let register_new = Some(new_shortcut.id()) != old_id || !old_registered;
    if register_new {
        register_shortcut(&app, &settings.shortcut)?;
        if old_registered {
            if let Err(e) = app.global_shortcut().unregister(old.shortcut.as_str()) {
                let _ = app.global_shortcut().unregister(settings.shortcut.as_str());
                return Err(e.to_string());
            }
        }
    }
    if let Err(e) = core
        .store
        .save_settings(&serde_json::to_value(&settings).map_err(|e| e.to_string())?)
    {
        if register_new {
            let _ = app.global_shortcut().unregister(settings.shortcut.as_str());
            if old_registered {
                let _ = register_shortcut(&app, &old.shortcut);
            }
        }
        return Err(e);
    }
    core.interrupt();
    core.devices.set_volume(settings.volume);
    core.devices.set_audio_devices(
        settings.audio_input_device.clone(),
        settings.audio_output_device.clone(),
    );
    *core.settings.lock().unwrap() = settings.clone();
    *core.last_frame.lock().unwrap() = None;
    core.activity.lock().unwrap().clear();
    let _ = app.emit("linger:settings", settings);
    Ok(())
}
#[tauri::command]
fn list_messages(core: State<Arc<Core>>) -> Result<Vec<Message>, String> {
    core.store.messages()
}
#[tauri::command]
fn list_memories(core: State<Arc<Core>>) -> Result<Vec<Memory>, String> {
    core.store.memories()
}
#[tauri::command]
fn save_memory(
    core: State<Arc<Core>>,
    content: String,
    scope: Option<String>,
) -> Result<Memory, String> {
    core.store
        .add_memory_scoped(&content, scope.as_deref().unwrap_or("long_term"))
}
#[tauri::command]
fn update_memory(core: State<Arc<Core>>, id: i64, content: String) -> Result<Memory, String> {
    core.store.update_memory(id, &content)
}
#[tauri::command]
fn delete_memory(core: State<Arc<Core>>, id: i64) -> Result<(), String> {
    core.store.delete_memory(id)
}
#[tauri::command]
async fn list_audio_devices() -> Result<devices::AudioDevices, String> {
    tokio::task::spawn_blocking(devices::list_audio_devices)
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn list_sources() -> Result<Vec<CaptureSource>, String> {
    tokio::task::spawn_blocking(devices::list_sources)
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
fn interrupt(app: tauri::AppHandle, core: State<Arc<Core>>) {
    core.interrupt();
    status(&app, "idle", "已停止播报");
}
#[tauri::command]
fn send_text(app: tauri::AppHandle, core: State<Arc<Core>>, text: String) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err("请输入内容".into());
    }
    if text.len() > 16000 {
        return Err("单次输入过长，请分段发送".into());
    }
    if core.recording.load(Ordering::SeqCst) {
        return Err("请先松开录音按钮".into());
    }
    core.key()?;
    session::launch(app, core.inner().clone(), session::Trigger::Text(text));
    Ok(())
}
#[tauri::command]
fn start_recording(app: tauri::AppHandle, core: State<Arc<Core>>) -> Result<(), String> {
    let capture = core.shortcut_capture.lock().unwrap();
    if capture.active().is_some() {
        status(&app, "error", "正在录入快捷键，暂不开始语音录音");
        return Err("正在录入快捷键，暂不开始语音录音".into());
    }
    if core.recording.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    drop(capture);
    core.interrupt();
    if let Err(e) = core.devices.start_recording() {
        core.recording.store(false, Ordering::SeqCst);
        status(&app, "error", &e);
        return Err(e);
    }
    status(&app, "recording", "正在录音，松开提交");
    // A lost key-up must never leave the microphone recording indefinitely.
    let c = core.inner().clone();
    let a = app.clone();
    let generation = c.recording_epoch.fetch_add(1, Ordering::SeqCst) + 1;
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(120)).await;
        if c.recording_epoch.load(Ordering::SeqCst) == generation
            && c.recording.swap(false, Ordering::SeqCst)
        {
            let _ = c.devices.stop_recording();
            status(&a, "idle", "录音达到两分钟，已停止；请重新按键说话");
        }
    });
    Ok(())
}
#[tauri::command]
fn stop_recording(app: tauri::AppHandle, core: State<Arc<Core>>) -> Result<(), String> {
    if !core.recording.swap(false, Ordering::SeqCst) {
        return Ok(());
    }
    core.recording_epoch.fetch_add(1, Ordering::SeqCst);
    let recording = core.devices.stop_recording().inspect_err(|e| {
        status(&app, "error", e);
    })?;
    let trigger = match recording {
        devices::RecordedAudio::Accidental => {
            status(&app, "idle", "按键时间太短，已忽略误触");
            return Ok(());
        }
        devices::RecordedAudio::Silent => session::Trigger::Unheard,
        devices::RecordedAudio::Speech(wav) => session::Trigger::Audio(wav),
    };
    core.key().inspect_err(|e| status(&app, "error", e))?;
    session::launch(app, core.inner().clone(), trigger);
    Ok(())
}
#[tauri::command]
async fn set_observing(
    app: tauri::AppHandle,
    core: State<'_, Arc<Core>>,
    enabled: bool,
) -> Result<(), String> {
    let request = core.observation_request.fetch_add(1, Ordering::SeqCst) + 1;
    if !enabled {
        session::pause_observation(&app, &core)?;
        core.interrupt();
        status(&app, "idle", "主动陪伴已暂停");
        return Ok(());
    }
    core.key().inspect_err(|e| status(&app, "error", e))?;
    let selected = core.settings().capture_source;
    let source = if selected.is_empty() {
        devices::PRIMARY_SOURCE.to_owned()
    } else {
        selected
    };
    status(&app, "observing", "正在检查录屏权限并准备观察");
    let capture_source = source.clone();
    let result = tokio::task::spawn_blocking(move || {
        if !devices::request_screen_permission() {
            return Err(
                "当前进程没有录屏权限。请在系统设置中允许 Linger，完全退出并重新打开后再开启陪伴"
                    .into(),
            );
        }
        devices::capture(&capture_source)
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(|result| result);
    if core.observation_request.load(Ordering::SeqCst) != request {
        return Ok(());
    }
    let frame = match result {
        Ok(frame) => frame,
        Err(error) => {
            session::pause_observation(&app, &core)?;
            let detail = format!("无法开启主动陪伴，已暂停：{error}");
            status(&app, "error", &detail);
            return Err(detail);
        }
    };
    let updated = {
        let mut settings = core.settings.lock().unwrap();
        if core.observation_request.load(Ordering::SeqCst) != request {
            return Ok(());
        }
        settings.capture_source = source;
        settings.observing = true;
        core.store
            .save_settings(&serde_json::to_value(&*settings).map_err(|e| e.to_string())?)?;
        settings.clone()
    };
    *core.last_frame.lock().unwrap() = None;
    let _ = app.emit("linger:settings", updated);
    status(&app, "idle", "主动陪伴已开启，正在进行首次观察");
    session::launch(
        app,
        core.inner().clone(),
        session::Trigger::ObserveFrame(frame),
    );
    Ok(())
}

#[tauri::command]
fn get_screen_permission() -> bool {
    devices::screen_permission()
}

#[tauri::command]
async fn request_screen_permission() -> Result<bool, String> {
    tokio::task::spawn_blocking(devices::request_screen_permission)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn set_capture_source(
    app: tauri::AppHandle,
    core: State<'_, Arc<Core>>,
    source: String,
) -> Result<(), String> {
    let request = core.observation_request.fetch_add(1, Ordering::SeqCst) + 1;
    let requested = source.clone();
    let available = tokio::task::spawn_blocking(devices::list_sources)
        .await
        .map_err(|e| e.to_string())??;
    if core.observation_request.load(Ordering::SeqCst) != request {
        return Ok(());
    }
    if !available.iter().any(|item| item.id == requested) {
        return Err("所选屏幕或窗口已不可用，请刷新来源列表".into());
    }
    let updated = {
        let mut settings = core.settings.lock().unwrap();
        if core.observation_request.load(Ordering::SeqCst) != request {
            return Ok(());
        }
        settings.capture_source = source;
        core.store
            .save_settings(&serde_json::to_value(&*settings).map_err(|e| e.to_string())?)?;
        settings.clone()
    };
    *core.last_frame.lock().unwrap() = None;
    let _ = app.emit("linger:settings", updated.clone());
    if updated.observing {
        core.interrupt();
        session::launch(app, core.inner().clone(), session::Trigger::Observe);
    }
    Ok(())
}

#[tauri::command]
fn test_voice(
    app: tauri::AppHandle,
    core: State<Arc<Core>>,
    profile: settings::VoiceProfile,
) -> Result<(), String> {
    if core.recording.load(Ordering::SeqCst) {
        return Err("请先结束录音".into());
    }
    core.key()?;
    profile.validate()?;
    session::launch(
        app,
        core.inner().clone(),
        session::Trigger::Preview(profile),
    );
    Ok(())
}

#[tauri::command]
fn set_volume(app: tauri::AppHandle, core: State<Arc<Core>>, volume: f32) -> Result<(), String> {
    if !volume.is_finite() || !(0.0..=1.0).contains(&volume) {
        return Err("音量范围为 0–1".into());
    }
    let updated = {
        let mut settings = core.settings.lock().unwrap();
        let mut updated = settings.clone();
        updated.volume = volume;
        core.store
            .save_settings(&serde_json::to_value(&updated).map_err(|e| e.to_string())?)?;
        core.devices.set_volume(volume);
        *settings = updated.clone();
        updated
    };
    let _ = app.emit("linger:settings", updated);
    Ok(())
}

fn show_compact(app: &tauri::AppHandle) -> Result<(), String> {
    overlay::show(app)
}

#[tauri::command]
async fn toggle_compact(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("compact") {
        if window.is_visible().map_err(|e| e.to_string())? {
            return window.hide().map_err(|e| e.to_string());
        }
    }
    show_compact(&app)
}

#[tauri::command]
fn begin_shortcut_capture(app: tauri::AppHandle, core: State<Arc<Core>>) -> Result<u64, String> {
    let mut capture = core.shortcut_capture.lock().unwrap();
    if core.recording.load(Ordering::SeqCst) {
        return Err("请先结束语音录音".into());
    }
    let token = capture.begin()?;
    drop(capture);
    core.interrupt();
    status(&app, "idle", "正在设置快捷键，语音录音暂时停用");
    let shortcut = core.settings().shortcut;
    if app.global_shortcut().is_registered(shortcut.as_str()) {
        if let Err(error) = app.global_shortcut().unregister(shortcut.as_str()) {
            core.shortcut_capture.lock().unwrap().finish(token);
            return Err(format!("无法暂时释放原快捷键：{error}"));
        }
    }
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || {
            let core = handle.state::<Arc<Core>>();
            if let Err(error) = restore_shortcut_capture(&handle, &core, token) {
                status(&handle, "error", &error);
            }
        });
    });
    Ok(token)
}

fn restore_shortcut_capture(app: &tauri::AppHandle, core: &Core, token: u64) -> Result<(), String> {
    let mut capture = core.shortcut_capture.lock().unwrap();
    if capture.active() != Some(token) {
        return Ok(());
    }
    let shortcut = core.settings().shortcut;
    let result = if app.global_shortcut().is_registered(shortcut.as_str()) {
        Ok(())
    } else {
        register_shortcut(app, &shortcut)
    };
    capture.finish(token);
    result.map_err(|error| format!("原快捷键恢复失败，请重新保存设置或更换组合键：{error}"))
}

#[tauri::command]
fn end_shortcut_capture(
    app: tauri::AppHandle,
    core: State<Arc<Core>>,
    token: u64,
) -> Result<(), String> {
    restore_shortcut_capture(&app, &core, token)
}

fn register_shortcut(app: &tauri::AppHandle, shortcut: &str) -> Result<(), String> {
    app.global_shortcut()
        .on_shortcut(shortcut, |app, _, event| {
            let state = app.state::<Arc<Core>>();
            if state.shortcut_capture.lock().unwrap().active().is_some() {
                return;
            }
            let result = match event.state {
                ShortcutState::Pressed => start_recording(app.clone(), state),
                ShortcutState::Released => stop_recording(app.clone(), state),
            };
            if let Err(e) = result {
                status(app, "error", &e);
            }
        })
        .map_err(|e| format!("快捷键注册失败：{e}"))
}
fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let store = Store::open(&dir.join("linger.sqlite3")).map_err(std::io::Error::other)?;
            let raw = store.load_settings().map_err(std::io::Error::other)?;
            let mut settings = match &raw {
                Some(value) => {
                    Settings::from_stored(value.clone()).map_err(std::io::Error::other)?
                }
                None => Settings::default(),
            };
            if let Some(raw) = raw {
                if raw
                    .get("settings_version")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
                    < 2
                {
                    if store
                        .get_meta("settings_before_voice_profiles")
                        .map_err(std::io::Error::other)?
                        .is_none()
                    {
                        store
                            .set_meta("settings_before_voice_profiles", &raw.to_string())
                            .map_err(std::io::Error::other)?;
                    }
                    store
                        .save_settings(&serde_json::to_value(&settings)?)
                        .map_err(std::io::Error::other)?;
                }
            }
            // Window IDs can be recycled; retain monitor choices and otherwise follow the system primary display.
            settings.observing = false;
            settings.capture_source = devices::startup_source(&settings.capture_source);
            let shortcut = settings.shortcut.clone();
            let (cancelled, _) = tokio::sync::watch::channel(0);
            let core = Arc::new(Core {
                settings: Mutex::new(settings),
                credentials: credentials::Credentials::default(),
                observation_status: Mutex::new(json!({"phase":"idle","detail":"本次启动尚未进行自动观察","at":null,"captured_at":null})),
                last_status: Mutex::new(json!({"phase":"idle","detail":"在这里陪你"})),
                store,
                devices: Devices::new(),
                generation: AtomicU64::new(0),
                turn_gate: Mutex::new(()),
                recording_epoch: AtomicU64::new(0),
                observation_request: AtomicU64::new(0),
                memory_retry_after: AtomicU64::new(0),
                busy: AtomicBool::new(false),
                recording: AtomicBool::new(false),
                cancelled,
                last_frame: Mutex::new(None),
                activity: Mutex::new(String::new()),
                memory_busy: AtomicBool::new(false),
                shortcut_capture: Mutex::new(shortcut_capture::ShortcutCapture::default()),
            });
            core.devices.set_volume(core.settings().volume);
            core.devices.set_audio_devices(core.settings().audio_input_device, core.settings().audio_output_device);
            app.manage(core.clone());
            app.manage(overlay::Overlay::default());
            overlay::start_hit_testing(app.handle().clone());
            if let Err(e) = register_shortcut(app.handle(), &shortcut) {
                eprintln!("{e}");
            }
            let show =
                tauri::menu::MenuItem::with_id(app, "show", "打开 Linger", true, None::<&str>)?;
            let panel =
                tauri::menu::MenuItem::with_id(app, "panel", "打开悬浮面板", true, None::<&str>)?;
            let pause =
                tauri::menu::MenuItem::with_id(app, "pause", "暂停主动陪伴", true, None::<&str>)?;
            let quit = tauri::menu::MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = tauri::menu::Menu::with_items(app, &[&show, &panel, &pause, &quit])?;
            let icon = tauri::image::Image::new_owned([111, 163, 138, 255].repeat(32 * 32), 32, 32);
            tauri::tray::TrayIconBuilder::new()
                .icon(icon)
                .tooltip("Linger")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "panel" => {
                        let app = app.clone();
                        tauri::async_runtime::spawn(async move {
                            if let Err(error) = show_compact(&app) {
                                status(&app, "error", &error);
                            }
                        });
                    }
                    "show" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    "pause" => {
                        let app = app.clone();
                        tauri::async_runtime::spawn(async move {
                            let _ = set_observing(app.clone(), app.state(), false).await;
                        });
                    }
                    "quit" => {
                        app.state::<Arc<Core>>().interrupt();
                        app.exit(0);
                    }
                    _ => {}
                })
                .build(app)?;
            session::start_scheduler(app.handle().clone(), core);
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "main"
                && matches!(
                    event,
                    tauri::WindowEvent::CloseRequested { .. } | tauri::WindowEvent::Focused(false)
                )
            {
                let core = window.state::<Arc<Core>>();
                let token = core.shortcut_capture.lock().unwrap().active();
                if let Some(token) = token {
                    if let Err(error) = restore_shortcut_capture(window.app_handle(), &core, token)
                    {
                        status(window.app_handle(), "error", &error);
                    }
                }
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            get_status,
            get_observation_status,
            translate_text,
            copy_text,
            open_external_url,
            save_settings,
            begin_shortcut_capture,
            end_shortcut_capture,
            key_status,
            list_models,
            set_api_key,
            list_messages,
            list_memories,
            save_memory,
            update_memory,
            delete_memory,
            list_sources,
            list_audio_devices,
            get_screen_permission,
            request_screen_permission,
            set_capture_source,
            interrupt,
            send_text,
            start_recording,
            stop_recording,
            set_observing,
            test_voice,
            set_volume,
            toggle_compact,
            overlay::compact_set_layout,
            overlay::compact_set_hit_regions,
            overlay::compact_set_interacting,
            overlay::compact_drag,
            overlay::compact_focus,
            overlay::compact_open_main
        ])
        .run(tauri::generate_context!())
        .expect("Linger 启动失败");
}
