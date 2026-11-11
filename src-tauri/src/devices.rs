//! Native device ownership stays outside the WebView. No screen-source fallback.
use base64::{engine::general_purpose::STANDARD, Engine};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use serde::Serialize;
use std::{
    hash::{Hash, Hasher},
    io::Cursor,
    sync::{
        atomic::{AtomicU32, AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Serialize)]
pub struct CaptureSource {
    pub id: String,
    pub name: String,
}
#[derive(Clone, Serialize)]
pub struct AudioDeviceOption {
    pub id: String,
    pub name: String,
}
#[derive(Serialize)]
pub struct AudioDevices {
    pub inputs: Vec<AudioDeviceOption>,
    pub outputs: Vec<AudioDeviceOption>,
}

fn audio_devices(input: bool) -> Result<Vec<(String, cpal::Device)>, String> {
    let host = cpal::default_host();
    let devices = if input {
        host.input_devices()
    } else {
        host.output_devices()
    }
    .map_err(|e| format!("无法列出音频设备：{e}"))?;
    devices
        .map(|device| {
            let name = device
                .name()
                .map_err(|e| format!("无法读取音频设备名称：{e}"))?;
            Ok((name, device))
        })
        .collect()
}
pub fn list_audio_devices() -> Result<AudioDevices, String> {
    fn options(input: bool) -> Result<Vec<AudioDeviceOption>, String> {
        let mut names: Vec<_> = audio_devices(input)?
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        names.sort();
        names.dedup();
        Ok(names
            .into_iter()
            .map(|name| AudioDeviceOption {
                id: name.clone(),
                name,
            })
            .collect())
    }
    Ok(AudioDevices {
        inputs: options(true)?,
        outputs: options(false)?,
    })
}
fn selected_audio_device(selected: &str, input: bool) -> Result<cpal::Device, String> {
    let kind = if input { "麦克风" } else { "播放设备" };
    if selected.is_empty() {
        let host = cpal::default_host();
        return if input {
            host.default_input_device()
        } else {
            host.default_output_device()
        }
        .ok_or_else(|| format!("未找到系统默认{kind}"));
    }
    let devices = audio_devices(input)?;
    let index = selected_audio_index(
        &devices
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        selected,
    )
    .map_err(|reason| format!("{kind}「{selected}」{reason}；请在设置中重新选择音频设备"))?;
    Ok(devices.into_iter().nth(index).unwrap().1)
}
fn selected_audio_index(names: &[&str], selected: &str) -> Result<usize, &'static str> {
    let mut matches = names
        .iter()
        .enumerate()
        .filter(|(_, name)| **name == selected);
    let index = matches.next().ok_or("已断开或不可用")?.0;
    if matches.next().is_some() {
        return Err("有多个同名设备，无法唯一识别，请重命名设备或选择系统默认");
    }
    Ok(index)
}

pub struct Captured {
    pub data_url: String,
    pub fingerprint: u64,
}

pub const PRIMARY_SOURCE: &str = "monitor:primary";

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

pub fn screen_permission() -> bool {
    #[cfg(target_os = "macos")]
    {
        unsafe { CGPreflightScreenCaptureAccess() }
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

/// Only called from an explicit user action; capture and enumeration never request permission.
pub fn request_screen_permission() -> bool {
    if screen_permission() {
        return true;
    }
    #[cfg(target_os = "macos")]
    {
        unsafe { CGRequestScreenCaptureAccess() }
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

pub fn list_sources() -> Result<Vec<CaptureSource>, String> {
    let monitors = xcap::Monitor::all().map_err(|e| e.to_string())?;
    let mut result = vec![CaptureSource {
        id: PRIMARY_SOURCE.into(),
        name: "主屏幕（跟随系统）".into(),
    }];
    for (index, monitor) in monitors.iter().enumerate() {
        let primary = monitor.is_primary().unwrap_or(false);
        let name = monitor
            .friendly_name()
            .or_else(|_| monitor.name())
            .unwrap_or_else(|_| "显示器".into());
        let description = format!(
            "{} · {} · {}×{} · 位置 ({}, {})",
            if primary {
                "当前主屏幕".into()
            } else {
                format!("扩展屏幕 {}", index + 1)
            },
            name,
            monitor.width().unwrap_or(0),
            monitor.height().unwrap_or(0),
            monitor.x().unwrap_or(0),
            monitor.y().unwrap_or(0)
        );
        if primary {
            result[0].name = format!(
                "主屏幕（跟随系统） · {name} · {}×{}",
                monitor.width().unwrap_or(0),
                monitor.height().unwrap_or(0)
            );
        }
        result.push(CaptureSource {
            id: format!("monitor:{}", monitor.id().map_err(|e| e.to_string())?),
            name: description,
        });
    }
    // Window enumeration may require screen access on macOS; never trigger a prompt just to populate a selector.
    if screen_permission() {
        for window in xcap::Window::all().map_err(|e| e.to_string())? {
            if let (Ok(id), Ok(title), Ok(app)) = (window.id(), window.title(), window.app_name()) {
                if !title.is_empty() {
                    result.push(CaptureSource {
                        id: format!("window:{id}"),
                        name: format!("窗口 · {app} · {title}"),
                    });
                }
            }
        }
    }
    Ok(result)
}

pub fn startup_source(source: &str) -> String {
    // Window IDs are recycled across launches. Monitor selection is retained; a missing monitor is reported, never silently replaced.
    if source.is_empty() || source.starts_with("window:") {
        PRIMARY_SOURCE.into()
    } else {
        source.into()
    }
}

pub fn capture(source: &str) -> Result<Captured, String> {
    if !screen_permission() {
        return Err("当前 Linger 进程没有屏幕录制权限。请在系统设置中授权后完全退出并重新打开 Linger；如果已勾选，请确认运行的是同一个应用版本".into());
    }
    let (capture_source, region) = parse_region(source)?;
    let frame = capture_frame(&capture_source)?;
    let frame = match region {
        Some(region) => crop_region(frame, region)?,
        None => frame,
    };
    encode_capture(frame)
}

fn parse_region(source: &str) -> Result<(String, Option<[u32; 4]>), String> {
    if !source.starts_with("region:") {
        return Ok((source.to_string(), None));
    }
    let parts: Vec<&str> = source.split(':').collect();
    if parts.len() != 6 {
        return Err("区域格式应为 region:屏幕ID:x:y:宽:高".into());
    }
    let values: Vec<u32> = parts[1..]
        .iter()
        .map(|s| s.parse::<u32>())
        .collect::<Result<_, _>>()
        .map_err(|_| "区域参数必须是非负整数")?;
    if values[3] == 0 || values[4] == 0 {
        return Err("区域宽高必须大于零".into());
    }
    Ok((
        format!("monitor:{}", values[0]),
        Some([values[1], values[2], values[3], values[4]]),
    ))
}

fn crop_region(frame: image::RgbaImage, region: [u32; 4]) -> Result<image::RgbaImage, String> {
    let [x, y, width, height] = region;
    let valid_x = x
        .checked_add(width)
        .map(|end| end <= frame.width())
        .unwrap_or(false);
    let valid_y = y
        .checked_add(height)
        .map(|end| end <= frame.height())
        .unwrap_or(false);
    if width == 0 || height == 0 || !valid_x || !valid_y {
        return Err("观察区域超出屏幕像素范围，请重新选择".into());
    }
    Ok(image::imageops::crop_imm(&frame, x, y, width, height).to_image())
}

fn capture_frame(source: &str) -> Result<image::RgbaImage, String> {
    if source == PRIMARY_SOURCE {
        return xcap::Monitor::all()
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|m| m.is_primary().unwrap_or(false))
            .ok_or("没有找到系统主屏幕")?
            .capture_image()
            .map_err(|e| format!("主屏幕采集失败：{e}"));
    }
    let (kind, id) = source.split_once(':').ok_or("请先选择观察屏幕或窗口")?;
    let id: u32 = id.parse().map_err(|_| "观察来源无效")?;
    let frame = match kind {
        "monitor" => xcap::Monitor::all()
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|m| m.id().ok() == Some(id))
            .ok_or("所选屏幕已断开，请重新选择")?
            .capture_image()
            .map_err(|e| format!("屏幕采集失败，请检查系统录屏权限：{e}"))?,
        "window" => {
            let window = xcap::Window::all()
                .map_err(|e| e.to_string())?
                .into_iter()
                .find(|w| w.id().ok() == Some(id))
                .ok_or("所选窗口已关闭，请重新选择")?;
            if window.is_minimized().map_err(|e| e.to_string())? {
                return Err("所选窗口已最小化，暂停观察".into());
            }
            window
                .capture_image()
                .map_err(|e| format!("窗口采集失败，请检查系统录屏权限：{e}"))?
        }
        _ => return Err("未知观察来源".into()),
    };
    Ok(frame)
}

fn encode_capture(frame: image::RgbaImage) -> Result<Captured, String> {
    if frame.width() == 0 || frame.height() == 0 {
        return Err("截图为空".into());
    }
    let image = image::DynamicImage::ImageRgba8(frame);
    let preview = image.thumbnail(64, 64).to_rgb8();
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    preview.as_raw().hash(&mut hash);
    let fingerprint = hash.finish();
    let resized = image
        .resize(1600, 1000, image::imageops::FilterType::Triangle)
        .to_rgb8();
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 80)
        .encode_image(&resized)
        .map_err(|e| e.to_string())?;
    Ok(Captured {
        data_url: format!("data:image/jpeg;base64,{}", STANDARD.encode(jpeg)),
        fingerprint,
    })
}

#[derive(Debug, PartialEq)]
pub enum RecordedAudio {
    Accidental,
    Silent,
    Speech(Vec<u8>),
}

// Only digital silence is filtered: soft speech must still reach ASR.
fn finish_recording(
    samples: &[i16],
    sample_rate: u32,
    held: Duration,
) -> Result<RecordedAudio, String> {
    if held < Duration::from_millis(300) {
        return Ok(RecordedAudio::Accidental);
    }
    if samples.is_empty() {
        return Err("麦克风没有返回音频，请检查输入设备和录音权限".into());
    }
    if samples.iter().all(|sample| sample.unsigned_abs() <= 1) {
        return Ok(RecordedAudio::Silent);
    }
    wav(samples, sample_rate).map(RecordedAudio::Speech)
}

#[derive(Clone, Serialize)]
pub struct MicrophoneStatus {
    pub phase: String,
    pub detail: String,
}
fn mic_status(status: &Mutex<MicrophoneStatus>, phase: &str, detail: impl Into<String>) {
    *status.lock().unwrap() = MicrophoneStatus {
        phase: phase.into(),
        detail: detail.into(),
    };
}
enum RecordingCommand {
    Configure(String),
    Start(String, mpsc::Sender<Result<(), String>>),
    Stop(mpsc::Sender<Result<RecordedAudio, String>>, bool),
}

pub struct Devices {
    recording: mpsc::Sender<RecordingCommand>,
    playback_generation: AtomicU64,
    audio_selection: Mutex<(String, String)>,
    input_level: Arc<AtomicU32>,
    microphone_status: Arc<Mutex<MicrophoneStatus>>,
    volume: AtomicU32,
    playback_gate: Mutex<Option<Arc<rodio::Sink>>>,
}
impl Default for Devices {
    fn default() -> Self {
        Self::new()
    }
}
impl Devices {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        let input_level = Arc::new(AtomicU32::new(0));
        let worker_level = input_level.clone();
        let microphone_status = Arc::new(Mutex::new(MicrophoneStatus {
            phase: "preparing".into(),
            detail: "麦克风尚未初始化".into(),
        }));
        let worker_status = microphone_status.clone();
        thread::spawn(move || recording_worker(rx, worker_level, worker_status));
        Self {
            recording: tx,
            input_level,
            microphone_status,
            playback_generation: AtomicU64::new(0),
            audio_selection: Mutex::new((String::new(), String::new())),
            volume: AtomicU32::new(0.8_f32.to_bits()),
            playback_gate: Mutex::new(None),
        }
    }
    pub fn set_audio_devices(&self, input: String, output: String) {
        *self.audio_selection.lock().unwrap() = (input.clone(), output);
        let _ = self.recording.send(RecordingCommand::Configure(input));
    }
    pub fn microphone_status(&self) -> MicrophoneStatus {
        self.microphone_status.lock().unwrap().clone()
    }
    pub fn input_level(&self) -> f32 {
        f32::from_bits(self.input_level.load(Ordering::Relaxed))
    }
    pub fn start_recording(&self) -> Result<(), String> {
        self.stop_playback();
        let (tx, rx) = mpsc::channel();
        self.recording
            .send(RecordingCommand::Start(
                self.audio_selection.lock().unwrap().0.clone(),
                tx,
            ))
            .map_err(|_| "录音线程已退出")?;
        rx.recv().map_err(|_| "录音线程未响应")?
    }
    pub fn stop_recording(&self) -> Result<RecordedAudio, String> {
        self.finish_capture(false)
    }
    pub fn stop_recording_with_tail(&self) -> Result<RecordedAudio, String> {
        self.finish_capture(true)
    }
    fn finish_capture(&self, tail: bool) -> Result<RecordedAudio, String> {
        let (tx, rx) = mpsc::channel();
        self.recording
            .send(RecordingCommand::Stop(tx, tail))
            .map_err(|_| "录音线程已退出")?;
        rx.recv().map_err(|_| "录音线程未响应")?
    }
    pub fn set_volume(&self, volume: f32) {
        let active = self.playback_gate.lock().unwrap();
        self.volume
            .store(volume.clamp(0.0, 1.0).to_bits(), Ordering::SeqCst);
        if let Some(sink) = active.as_ref() {
            sink.set_volume(volume.clamp(0.0, 1.0));
        }
    }
    pub fn playback_token(&self) -> u64 {
        self.playback_generation.load(Ordering::SeqCst)
    }
    pub fn stop_playback(&self) {
        // Serialize cancellation with attaching audio: a cancelled request cannot
        // append to a new sink after stop_playback returns.
        let mut active = self.playback_gate.lock().unwrap();
        self.playback_generation.fetch_add(1, Ordering::SeqCst);
        if let Some(sink) = active.take() {
            sink.stop();
        }
    }
    /// Capture the token BEFORE checking the caller's turn ID. Keep it through
    /// spawn_blocking; sampling a new token inside that worker revives stale audio.
    pub fn play_for(&self, token: u64, bytes: Vec<u8>) -> Result<(), String> {
        if self.playback_token() != token {
            return Ok(());
        }
        let selected = self.audio_selection.lock().unwrap().1.clone();
        let device = selected_audio_device(&selected, false)?;
        let (_stream, handle) = rodio::OutputStream::try_from_device(&device)
            .map_err(|e| format!("无法打开播放设备：{e}"))?;
        let sink = Arc::new(rodio::Sink::try_new(&handle).map_err(|e| e.to_string())?);
        let source =
            rodio::Decoder::new(Cursor::new(bytes)).map_err(|e| format!("音频解码失败：{e}"))?;
        {
            let mut active = self.playback_gate.lock().unwrap();
            if self.playback_token() != token {
                return Ok(());
            }
            if let Some(previous) = active.take() {
                previous.stop();
            }
            sink.set_volume(f32::from_bits(self.volume.load(Ordering::SeqCst)));
            sink.append(source);
            *active = Some(sink.clone());
        }
        while !sink.empty() {
            if self.playback_token() != token {
                sink.stop();
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let mut active = self.playback_gate.lock().unwrap();
        if active
            .as_ref()
            .map(|s| Arc::ptr_eq(s, &sink))
            .unwrap_or(false)
        {
            *active = None;
        }
        Ok(())
    }
    /// OpenAI raw PCM is signed little-endian 16-bit mono, 24 kHz.
    pub fn play_pcm_for(&self, token: u64, bytes: Vec<u8>) -> Result<(), String> {
        if self.playback_token() != token {
            return Ok(());
        }
        if !bytes.len().is_multiple_of(2) {
            return Err("PCM 音频长度无效".into());
        }
        let samples: Vec<i16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();
        self.play_for(token, wav(&samples, 24_000)?)
    }
}

const RECORDING_LIMIT: Duration = Duration::from_secs(120);
const MIC_STALE: Duration = Duration::from_secs(2);

fn needs_capture_tail(tail: bool, held: Option<Duration>) -> bool {
    tail && held.is_some_and(|held| {
        held >= Duration::from_millis(300) && held + Duration::from_millis(100) < RECORDING_LIMIT
    })
}

struct CaptureSession {
    samples: Vec<i16>,
    started: Instant,
    expired: bool,
}
#[derive(Default)]
struct CaptureBuffer {
    session: Option<CaptureSession>,
}
impl CaptureBuffer {
    fn start(&mut self, samples: Vec<i16>, now: Instant) -> Result<(), String> {
        if self.session.is_some() {
            return Err("正在录音".into());
        }
        self.session = Some(CaptureSession {
            samples,
            started: now,
            expired: false,
        });
        Ok(())
    }
    fn append(
        &mut self,
        samples: impl Iterator<Item = i16>,
        max_samples: usize,
        now: Instant,
    ) -> bool {
        let Some(session) = self.session.as_mut() else {
            return false;
        };
        if session.expired || now.duration_since(session.started) >= RECORDING_LIMIT {
            session.expired = true;
            return true;
        }
        for sample in samples {
            if session.samples.len() == max_samples {
                session.expired = true;
                return true;
            }
            session.samples.push(sample);
        }
        false
    }
}
struct Recording {
    _stream: cpal::Stream,
    buffer: Arc<Mutex<CaptureBuffer>>,
    error: Arc<Mutex<Option<String>>>,
    heartbeat: Arc<AtomicU64>,
    clock: Instant,
    sample_rate: u32,
    device_name: String,
}
impl Recording {
    fn ready(&self) -> bool {
        let last = self.heartbeat.load(Ordering::Relaxed);
        last > 0
            && self.clock.elapsed().as_millis() as u64 + 1 - last < MIC_STALE.as_millis() as u64
    }
    fn health_error(&self) -> Option<String> {
        self.error.lock().unwrap().clone().or_else(|| {
            (self.clock.elapsed() > MIC_STALE && !self.ready())
                .then(|| "麦克风没有继续返回音频，正在重新连接".into())
        })
    }
}
fn recording_worker(
    rx: mpsc::Receiver<RecordingCommand>,
    input_level: Arc<AtomicU32>,
    status: Arc<Mutex<MicrophoneStatus>>,
) {
    let mut selected: Option<String> = None;
    let mut current: Option<Recording> = None;
    let mut pending: Option<(Instant, mpsc::Sender<Result<(), String>>)> = None;
    let mut stopped_reason: Option<String> = None;
    let mut retry_at = Instant::now();
    let mut default_check = Instant::now();
    let mut retry_attempts = 0usize;
    let mut device_present = None;
    loop {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(RecordingCommand::Configure(input)) => {
                if selected.as_ref() != Some(&input) {
                    if let Some((_, reply)) = pending.take() {
                        let _ = reply.send(Err("输入设备已切换，请重新录音".into()));
                    }
                    current = None;
                    selected = Some(input);
                    stopped_reason = Some("输入设备已切换，请重新录音".into());
                    retry_at = Instant::now();
                    retry_attempts = 0;
                    input_level.store(0, Ordering::Relaxed);
                    mic_status(&status, "preparing", "正在准备麦克风");
                }
            }
            Ok(RecordingCommand::Start(input, reply)) => {
                if pending.is_some()
                    || current
                        .as_ref()
                        .is_some_and(|r| r.buffer.lock().unwrap().session.is_some())
                {
                    let _ = reply.send(Err("正在录音或准备麦克风".into()));
                    continue;
                }
                if selected.as_ref() != Some(&input) {
                    current = None;
                    selected = Some(input);
                }
                stopped_reason = None;
                retry_at = Instant::now();
                retry_attempts = 0;
                pending = Some((Instant::now(), reply));
                mic_status(&status, "preparing", "正在准备麦克风，请稍候");
            }
            Ok(RecordingCommand::Stop(reply, tail)) => {
                let held = current.as_ref().and_then(|r| {
                    r.buffer
                        .lock()
                        .unwrap()
                        .session
                        .as_ref()
                        .map(|s| s.started.elapsed())
                });
                if needs_capture_tail(tail, held) {
                    thread::sleep(Duration::from_millis(100));
                }
                if let Some((_, start_reply)) = pending.take() {
                    let _ = start_reply.send(Err("录音已取消".into()));
                }
                let result = current
                    .as_ref()
                    .and_then(|r| {
                        r.buffer
                            .lock()
                            .unwrap()
                            .session
                            .take()
                            .map(|session| (r, session))
                    })
                    .map(|(r, session)| {
                        match r.health_error().or_else(|| {
                            session
                                .expired
                                .then(|| "录音达到两分钟，已停止收集，请重新按键说话".into())
                        }) {
                            Some(error) => Err(error),
                            None => finish_recording(
                                &session.samples,
                                r.sample_rate,
                                held.unwrap_or_else(|| session.started.elapsed()),
                            ),
                        }
                    })
                    .unwrap_or_else(|| {
                        Err(stopped_reason
                            .take()
                            .unwrap_or_else(|| "当前没有录音".into()))
                    });
                input_level.store(0, Ordering::Relaxed);
                let _ = reply.send(result);
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if let Some(r) = current.as_ref() {
            let mut failure = r.health_error();
            if selected.as_deref() == Some("") && default_check.elapsed() >= Duration::from_secs(2)
            {
                default_check = Instant::now();
                let name = selected_audio_device("", true)
                    .and_then(|d| d.name().map_err(|e| e.to_string()));
                if name.as_deref() != Ok(r.device_name.as_str()) {
                    failure = Some("系统默认麦克风已变化，正在重新连接".into());
                }
            }
            if let Some(error) = failure {
                stopped_reason = Some(error.clone());
                current = None;
                input_level.store(0, Ordering::Relaxed);
                mic_status(&status, "error", error);
                retry_at = Instant::now() + Duration::from_secs(2);
            }
        }
        // After bounded failures, only device reappearance or an explicit Start
        // retries opening. Merely polling availability never requests permission.
        if current.is_none()
            && retry_attempts >= 3
            && default_check.elapsed() >= Duration::from_secs(2)
        {
            default_check = Instant::now();
            if let Some(input) = selected.as_ref() {
                let present = selected_audio_device(input, true).is_ok();
                if device_present == Some(false) && present {
                    retry_attempts = 0;
                }
                device_present = Some(present);
            }
        }
        if current.is_none() && retry_attempts < 3 && Instant::now() >= retry_at {
            if let Some(input) = selected.as_ref() {
                mic_status(&status, "preparing", "正在准备麦克风");
                retry_attempts += 1;
                match begin_recording(input_level.clone(), input) {
                    Ok(stream) => current = Some(stream),
                    Err(error) => {
                        device_present = Some(selected_audio_device(input, true).is_ok());
                        stopped_reason = Some(error.clone());
                        mic_status(&status, "error", error);
                    }
                }
                retry_at = Instant::now() + Duration::from_secs(2);
            }
        }
        if pending
            .as_ref()
            .is_some_and(|(started, _)| started.elapsed() >= Duration::from_secs(3))
        {
            let (_, reply) = pending.take().unwrap();
            let error = stopped_reason
                .clone()
                .unwrap_or_else(|| "麦克风准备超时，请检查设备和录音权限后重试".into());
            let _ = reply.send(Err(error));
        }
        if let Some(r) = current.as_ref() {
            if r.ready() {
                retry_attempts = 0;
                if let Some((_, reply)) = pending.take() {
                    // Allocate on the worker, never inside the audio callback.
                    let samples = Vec::with_capacity(r.sample_rate as usize * 120);
                    let result = r.buffer.lock().unwrap().start(samples, Instant::now());
                    let _ = reply.send(result);
                }
                let mut buffer = r.buffer.lock().unwrap();
                if buffer
                    .session
                    .as_ref()
                    .is_some_and(|s| s.expired || s.started.elapsed() >= RECORDING_LIMIT)
                {
                    buffer.session = None;
                    stopped_reason = Some("录音达到两分钟，已停止收集，请重新按键说话".into());
                    input_level.store(0, Ordering::Relaxed);
                }
                if buffer.session.is_some() {
                    mic_status(&status, "recording", "正在听你说");
                } else {
                    mic_status(&status, "ready", "麦克风已就绪，待命声音不保存、不上传");
                }
            }
        }
    }
}
fn begin_recording(input_level: Arc<AtomicU32>, selected: &str) -> Result<Recording, String> {
    let device = selected_audio_device(selected, true)?;
    let device_name = device.name().map_err(|e| e.to_string())?;
    let supported = device
        .default_input_config()
        .map_err(|e| format!("无法打开麦克风，请检查权限：{e}"))?;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let buffer = Arc::new(Mutex::new(CaptureBuffer::default()));
    let error = Arc::new(Mutex::new(None));
    let heartbeat = Arc::new(AtomicU64::new(0));
    let clock = Instant::now();
    let channels = config.channels as usize;
    let max_samples = config.sample_rate.0 as usize * 120;
    macro_rules! build {
        ($ty:ty, $convert:expr) => {{
            let out = buffer.clone();
            let errors = error.clone();
            let level = input_level.clone();
            let pulse = heartbeat.clone();
            device.build_input_stream(
                &config,
                move |data: &[$ty], _: &cpal::InputCallbackInfo| {
                    if data.is_empty() {
                        return;
                    }
                    pulse.store(clock.elapsed().as_millis() as u64 + 1, Ordering::Relaxed);
                    let mut out = out.lock().unwrap();
                    if out.session.is_none() {
                        return;
                    }
                    let mut energy = 0.0_f32;
                    let mut count = 0usize;
                    let samples = data.chunks_exact(channels).map(|frame| {
                        let average = frame.iter().map($convert).sum::<f32>() / channels as f32;
                        energy += average * average;
                        count += 1;
                        (average.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
                    });
                    out.append(samples, max_samples, Instant::now());
                    let rms = if count == 0 {
                        0.0
                    } else {
                        (energy / count as f32).sqrt()
                    };
                    level.store((rms * 8.0).clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
                },
                move |err| {
                    *errors.lock().unwrap() = Some(format!("麦克风中断：{err}"));
                },
                None,
            )
        }};
    }
    let stream = match format {
        cpal::SampleFormat::F32 => build!(f32, |x: &f32| *x),
        cpal::SampleFormat::I16 => build!(i16, |x: &i16| *x as f32 / 32768.0),
        cpal::SampleFormat::U16 => build!(u16, |x: &u16| (*x as f32 - 32768.0) / 32768.0),
        other => return Err(format!("暂不支持麦克风采样格式 {other:?}")),
    }
    .map_err(|e| format!("创建录音流失败：{e}"))?;
    stream.play().map_err(|e| format!("启动麦克风失败：{e}"))?;
    Ok(Recording {
        _stream: stream,
        buffer,
        error,
        heartbeat,
        clock,
        sample_rate: config.sample_rate.0,
        device_name,
    })
}
fn wav(samples: &[i16], sample_rate: u32) -> Result<Vec<u8>, String> {
    let mut cursor = Cursor::new(Vec::new());
    {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::new(&mut cursor, spec).map_err(|e| e.to_string())?;
        for sample in samples {
            writer.write_sample(*sample).map_err(|e| e.to_string())?;
        }
        writer.finalize().map_err(|e| e.to_string())?;
    }
    Ok(cursor.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capture_gate_discards_idle_audio_and_never_carries_between_turns() {
        let mut capture = CaptureBuffer::default();
        let now = Instant::now();
        assert!(!capture.append([1, 2, 3].into_iter(), 8, now));
        assert!(capture.session.is_none());
        capture.start(Vec::with_capacity(8), now).unwrap();
        assert!(!capture.append([4, 5].into_iter(), 8, now));
        assert_eq!(capture.session.take().unwrap().samples, vec![4, 5]);
        capture.append([6, 7].into_iter(), 8, now);
        capture.start(Vec::with_capacity(8), now).unwrap();
        capture.append([8].into_iter(), 8, now);
        assert_eq!(capture.session.take().unwrap().samples, vec![8]);
    }
    #[test]
    fn capture_limit_stops_accepting_samples_without_growing_buffer() {
        let mut capture = CaptureBuffer::default();
        let now = Instant::now();
        capture.start(Vec::with_capacity(2), now).unwrap();
        assert!(capture.append([1, 2, 3].into_iter(), 2, now));
        assert_eq!(capture.session.as_ref().unwrap().samples, vec![1, 2]);
        assert!(capture.session.as_ref().unwrap().expired);
        capture.session = None;
        capture.start(Vec::with_capacity(2), now).unwrap();
        assert!(capture.append([1].into_iter(), 2, now + RECORDING_LIMIT));
        assert!(capture.session.as_ref().unwrap().samples.is_empty());
        assert!(capture.session.as_ref().unwrap().expired);
    }
    #[test]
    fn tail_is_only_for_explicit_nonaccidental_send_within_limit() {
        assert!(needs_capture_tail(true, Some(Duration::from_millis(300))));
        assert!(!needs_capture_tail(false, Some(Duration::from_secs(1))));
        assert!(!needs_capture_tail(true, Some(Duration::from_millis(299))));
        assert!(!needs_capture_tail(true, Some(RECORDING_LIMIT)));
        assert!(!needs_capture_tail(true, None));
    }
    #[test]
    fn duplicate_capture_start_does_not_clear_existing_audio() {
        let mut capture = CaptureBuffer::default();
        let now = Instant::now();
        capture.start(Vec::with_capacity(2), now).unwrap();
        capture.append([9].into_iter(), 2, now);
        assert!(capture.start(Vec::with_capacity(2), now).is_err());
        assert_eq!(capture.session.unwrap().samples, vec![9]);
    }
    #[test]
    fn saved_audio_selection_survives_reordering_and_never_falls_back() {
        assert_eq!(
            selected_audio_index(&["USB mic", "Headset"], "Headset"),
            Ok(1)
        );
        assert_eq!(
            selected_audio_index(&["Headset", "USB mic"], "Headset"),
            Ok(0)
        );
        assert!(selected_audio_index(&["USB mic"], "Headset").is_err());
        assert!(selected_audio_index(&["Headset", "Headset"], "Headset").is_err());
    }
    #[test]
    fn short_presses_are_ignored_but_silence_is_a_deliberate_turn() {
        assert_eq!(
            finish_recording(&[100; 100], 1000, Duration::from_millis(299)).unwrap(),
            RecordedAudio::Accidental
        );
        assert_eq!(
            finish_recording(&[0; 300], 1000, Duration::from_millis(300)).unwrap(),
            RecordedAudio::Silent
        );
        assert!(finish_recording(&[], 1000, Duration::from_secs(1)).is_err());
    }
    #[test]
    fn soft_audio_is_not_discarded_as_silence() {
        assert!(matches!(
            finish_recording(&[2; 300], 1000, Duration::from_secs(1)).unwrap(),
            RecordedAudio::Speech(_)
        ));
        assert!(matches!(
            finish_recording(&[i16::MIN; 300], 1000, Duration::from_secs(1)).unwrap(),
            RecordedAudio::Speech(_)
        ));
    }
    #[test]
    fn volume_changes_do_not_cancel_playback_or_open_devices() {
        let devices = Devices::new();
        let generation = devices.playback_token();
        devices.set_volume(0.25);
        assert_eq!(devices.playback_token(), generation);
        assert_eq!(f32::from_bits(devices.volume.load(Ordering::SeqCst)), 0.25);
        devices.set_volume(0.0);
        assert_eq!(devices.playback_token(), generation);
        assert_eq!(f32::from_bits(devices.volume.load(Ordering::SeqCst)), 0.0);
    }
    #[test]
    fn stale_audio_is_rejected_before_decoder_or_device_access() {
        let devices = Devices::new();
        let stale = devices.playback_token();
        devices.stop_playback();
        // Invalid media would fail decoding if cancellation did not win.
        assert!(devices.play_for(stale, vec![1, 2, 3]).is_ok());
        assert!(devices.play_pcm_for(stale, vec![1]).is_ok());
        assert_ne!(devices.playback_token(), stale);
    }
    #[test]
    fn wav_roundtrips_samples_and_rate() {
        let bytes = wav(&[-32768, 0, 32767], 24_000).unwrap();
        let mut reader = hound::WavReader::new(Cursor::new(bytes)).unwrap();
        assert_eq!(reader.spec().sample_rate, 24_000);
        assert_eq!(reader.spec().channels, 1);
        assert_eq!(
            reader
                .samples::<i16>()
                .map(Result::unwrap)
                .collect::<Vec<_>>(),
            vec![-32768, 0, 32767]
        );
    }
    #[test]
    fn region_bounds_never_expand_or_overflow() {
        let image = image::RgbaImage::new(20, 10);
        let cropped = crop_region(image.clone(), [5, 2, 15, 8]).unwrap();
        assert_eq!(cropped.dimensions(), (15, 8));
        assert!(crop_region(image.clone(), [5, 2, 16, 8]).is_err());
        assert!(crop_region(image.clone(), [u32::MAX, 0, 2, 2]).is_err());
        assert!(crop_region(image, [0, 0, 0, 1]).is_err());
        assert_eq!(
            parse_region("region:3:5:2:15:8").unwrap(),
            ("monitor:3".into(), Some([5, 2, 15, 8]))
        );
        assert!(parse_region("region:3:-1:0:2:2").is_err());
        assert!(parse_region("region:3:0:0:0:2").is_err());
    }
    #[test]
    fn startup_uses_primary_but_preserves_explicit_monitor_choice() {
        assert_eq!(startup_source(""), PRIMARY_SOURCE);
        assert_eq!(startup_source("window:42"), PRIMARY_SOURCE);
        assert_eq!(startup_source("monitor:42"), "monitor:42");
        assert_eq!(
            startup_source("region:42:0:0:100:100"),
            "region:42:0:0:100:100"
        );
    }
    #[test]
    fn invalid_capture_source_fails_without_device_access() {
        assert!(capture("").is_err());
        assert!(capture("window:nope").is_err());
        assert!(capture("unknown:1").is_err());
    }
}
