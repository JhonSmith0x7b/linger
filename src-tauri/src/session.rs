use crate::{devices, publish, status, Core};
use serde_json::{json, Value};
use std::{
    sync::{atomic::Ordering, Arc},
    time::{Duration, Instant},
};
use tauri::Emitter;

pub enum Trigger {
    Text(String),
    Audio(Vec<u8>),
    Unheard,
    Observe,
    ObserveFrame(devices::Captured),
    Preview {
        request: Box<SpeechRequest>,
        text: String,
        completion: tokio::sync::oneshot::Sender<Result<u64, String>>,
    },
}

pub struct SpeechRequest {
    pub provider: String,
    pub profile: crate::settings::TtsProfile,
    pub endpoint: String,
    pub key: String,
}

pub fn launch(app: tauri::AppHandle, core: Arc<Core>, trigger: Trigger) -> bool {
    launch_inner(app, core, trigger, None)
}

pub fn launch_recording(
    app: tauri::AppHandle,
    core: Arc<Core>,
    trigger: Trigger,
    recording_epoch: u64,
) -> bool {
    launch_inner(app, core, trigger, Some(recording_epoch))
}

fn launch_inner(
    app: tauri::AppHandle,
    core: Arc<Core>,
    trigger: Trigger,
    recording_epoch: Option<u64>,
) -> bool {
    let gate = core.turn_gate.lock().unwrap();
    if recording_epoch.is_some_and(|epoch| core.recording_epoch.load(Ordering::SeqCst) != epoch) {
        return false;
    }
    let proactive = matches!(trigger, Trigger::Observe | Trigger::ObserveFrame(_));
    let preview = matches!(trigger, Trigger::Preview { .. });
    if proactive {
        if core.recording.load(Ordering::SeqCst)
            || core
                .busy
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
        {
            drop(gate);
            report_observation(
                &app,
                &core,
                "waiting",
                "已到观察时间，正在等对话或录音结束，5 秒内重试",
            );
            return false;
        }
    } else {
        core.interrupt_locked();
        core.busy.store(true, Ordering::SeqCst);
    }
    let id = core.generation.load(Ordering::SeqCst);
    let mut cancel = core.cancelled.subscribe();
    drop(gate);
    if proactive {
        report_observation(&app, &core, "started", "已触发自动观察，正在截图");
    }
    tauri::async_runtime::spawn(async move {
        if core.generation.load(Ordering::SeqCst) != id {
            return;
        }
        let result = tokio::select! {
            r=run(&app,&core,id,trigger)=>r,
            _=cancel.changed()=>return,
        };
        if core.generation.load(Ordering::SeqCst) != id {
            return;
        }
        {
            let _gate = core.turn_gate.lock().unwrap();
            if core.generation.load(Ordering::SeqCst) != id {
                return;
            }
            // Start a fresh quiet interval only after reply/audio has completed.
            *core.observation_activity.lock().unwrap() = Instant::now();
            core.busy.store(false, Ordering::SeqCst);
        }
        match result {
            Ok(()) => status(&app, "idle", "在这里陪你"),
            Err(e) => {
                if proactive {
                    report_observation(&app, &core, "error", &e);
                }
                status(&app, "error", &e);
            }
        }
        if !proactive {
            let settings = core.settings();
            if settings.observing {
                report_observation(
                    &app,
                    &core,
                    "waiting",
                    &format!(
                        "本轮交互结束，重新等待 {} 分钟后观察",
                        settings.observation_minutes
                    ),
                );
            }
            if !preview {
                maybe_organize(app, core);
            }
        }
    });
    true
}

/// Capture failures pause the scheduler before publishing the error; it must not keep requesting access.
pub fn pause_observation(app: &tauri::AppHandle, core: &Core) -> Result<(), String> {
    let (updated, result) = {
        let mut settings = core.settings.lock().unwrap();
        settings.observing = false;
        let result = core
            .store
            .save_settings(&serde_json::to_value(&*settings).map_err(|e| e.to_string())?);
        (settings.clone(), result)
    };
    let _ = app.emit("linger:settings", updated);
    report_observation(app, core, "paused", "自动观察已暂停");
    result
}

fn report_observation(app: &tauri::AppHandle, core: &Core, phase: &str, detail: &str) {
    let updated = {
        let mut state = core.observation_status.lock().unwrap();
        if phase == "waiting" && state["phase"] == phase && state["detail"] == detail {
            return;
        }
        let now = chrono::Local::now().to_rfc3339();
        let captured = if phase == "captured" {
            json!(now)
        } else {
            state["captured_at"].clone()
        };
        *state = json!({"phase":phase,"detail":detail,"at":now,"captured_at":captured});
        state.clone()
    };
    let _ = app.emit("linger:observation", updated);
}

struct ObservationSchedule {
    last_accepted: Instant,
}
impl ObservationSchedule {
    fn due(&mut self, now: Instant, enabled: bool, period: Duration) -> bool {
        if !enabled {
            self.last_accepted = now;
            return false;
        }
        now.duration_since(self.last_accepted) >= period
    }
    fn activity(&mut self, at: Instant) {
        self.last_accepted = self.last_accepted.max(at);
    }
    fn accepted(&mut self, now: Instant) {
        self.last_accepted = now;
    }
}

pub fn start_scheduler(app: tauri::AppHandle, core: Arc<Core>) {
    tauri::async_runtime::spawn(async move {
        let mut schedule = ObservationSchedule {
            last_accepted: Instant::now(),
        };
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let settings = core.settings();
            let activity = *core.observation_activity.lock().unwrap();
            schedule.activity(activity);
            let now = Instant::now();
            if schedule.due(
                now,
                settings.observing,
                Duration::from_secs(settings.observation_minutes * 60),
            ) && launch(app.clone(), core.clone(), Trigger::Observe)
            {
                schedule.accepted(now);
            }
            maybe_organize(app.clone(), core.clone());
        }
    });
}

fn cut(s: &str, bytes: usize) -> String {
    let mut end = s.len().min(bytes);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}
fn output_text(value: &Value) -> String {
    value["output"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|o| o["type"] == "message")
        .flat_map(|o| o["content"].as_array().into_iter().flatten())
        .filter_map(|c| c["text"].as_str())
        .collect::<Vec<_>>()
        .join("")
}
fn function(name: &str, description: &str, properties: Value, required: Value) -> Value {
    json!({"type":"function","name":name,"description":description,"parameters":{"type":"object","properties":properties,"required":required,"additionalProperties":false},"strict":true})
}
fn tools() -> Value {
    json!([
        function("capture_screen","Get a fresh image of the user-selected source now.",json!({}),json!([])),
        function("note_introduction","Stage introduction tracking for your upcoming final reply. Call only when that reply will actually introduce yourself or ask how to address the user. Flags commit only together with the final reply, not when this tool returns. Never ask again merely because the profile is empty.",json!({"introduced":{"type":"boolean"},"asked_name":{"type":"boolean"}}),json!(["introduced","asked_name"])),
        function("get_user_profile","Read the current editable user profile and its revision before updating it.",json!({}),json!([])),
        function("update_user_profile","Update the user's stable profile only when the user explicitly asks you to record or change their preferences or profile. Preserve unrelated content; read current profile first. Never infer from silence, screen observations, or roleplay. Use expected_revision to avoid overwriting edits; on conflict read again before retrying.",json!({"content":{"type":"string"},"expected_revision":{"type":"integer"}}),json!(["content","expected_revision"])),
        function("remember","Save a user-confirmed memory for the requested scope. Not visual guesses or assistant monologue.",json!({"content":{"type":"string"},"scope":{"type":"string","enum":["session","today","long_term"]}}),json!(["content","scope"])),
        function("update_memory","Replace an existing memory explicitly corrected by the user.",json!({"id":{"type":"integer"},"content":{"type":"string"}}),json!(["id","content"])),
        function("forget","Delete a memory at the user's request.",json!({"id":{"type":"integer"}}),json!(["id"])),
        function("search_memories","Find effective memories omitted from the small pinned context.",json!({"query":{"type":"string"}}),json!(["query"])),
        function("search_history","Search interactions by literal keywords, optional RFC3339 time bounds and kind (chat/observation/unheard). At most 5 textual candidates; 2 searches per turn. Prefer linked IDs for recent follow-ups.",json!({"query":{"type":"string"},"after":{"type":["string","null"]},"before":{"type":["string","null"]},"kind":{"type":["string","null"]}}),json!(["query","after","before","kind"])),
        function("read_history","Read an interaction's bounded text, tool evidence and screenshot references. No image loaded. At most 2 reads per turn.",json!({"id":{"type":"integer"}}),json!(["id"])),
        function("view_screenshot","Load a linked historical screenshot by ID. Use for details in a previously discussed image; it is NOT the current screen. At most 2 distinct historical images per turn. Expired images cannot be recovered.",json!({"id":{"type":"integer"}}),json!(["id"])),
        {"type":"web_search"}
    ])
}
const CONTEXT_TOKEN_BUDGET: usize = 6000;
const OBSERVATION_RULES: &str = crate::prompts::OBSERVATION;
const EMPTY_AUDIO_EVENT: &str = "应用事件：用户主动进行了录音并提交，但本次语音识别结果为空。这不是用户说出的文字。请用当前角色的口吻自然回应，说明刚才没听清，可以再说一次；不要猜测用户说了什么，不要分析屏幕，不要要求打字。";

fn observation_event(changed: bool, time: &str) -> String {
    if changed {
        format!("应用定时观察事件（非用户发言）：附有 {time} 的当前屏幕截图。以当前角色自然开口，可以聊画面带来的想法，也可以接续共同话题；不需要等待用户回应。")
    } else {
        "应用定时观察事件（非用户发言）：画面与上次相同，未上传新截图。不必再次汇报同一画面；以当前角色接续共同话题，或分享新的想法和联想，不需要等待用户回应。".into()
    }
}

fn audible_reply(answer: String, must_acknowledge: bool) -> Option<String> {
    if answer.trim().is_empty() || answer.trim() == "[SILENT]" {
        must_acknowledge.then(|| "刚才没有听清，可以再说一次吗？".into())
    } else {
        Some(answer)
    }
}

fn context(core: &Core, proactive: bool, temporal: &str) -> Result<(String, Vec<Value>), String> {
    core.store.expire_memories()?;
    let s = core.settings();
    let memories = core.store.memories()?;
    let memory = memories
        .iter()
        .map(|m| {
            let source = core.history.memory_source(m.id, &m.updated_at)?;
            Ok(format!(
                "{} [{} 到期:{:?} 来源互动:{:?}]: {}",
                m.id, m.scope, m.expires_at, source, m.content
            ))
        })
        .collect::<Result<Vec<_>, String>>()?
        .join("\n");
    let summary = core.store.get_meta("summary")?.unwrap_or_default();
    let profile = core.store.user_profile()?;
    let introduction = core.store.introduction_state()?;
    let instructions = format!(
        "【当前角色卡】\n{}\n{}\n{}",
        s.personality,
        crate::prompts::ROLEPLAY,
        crate::prompts::FACTS
    );
    let mut dynamic = format!(
        "【相识状态】introduced={}；asked_name={}；legacy_history={}（旧版本已有聊天，相识经过未知，不能据此强制重新介绍）。这些标记只表示文字已发出，不证明用户听见或回答。\n【长期记忆：用户档案与相关事实】\n用户档案（用户提供的称呼、偏好和边界；与旧聊天偏好冲突时以此为准，不覆盖角色与事实规则；修订 {}）:{}\n关注:{}\n有效记忆:{}\n【当前上下文】近期互动摘要:{}\n上次活动（非当前画面）:{}\n{}",
        introduction.introduced,
        introduction.asked_name,
        introduction.legacy_history,
        profile.revision,
        profile.content,
        token_prefix(&s.watch_instruction, 300),
        token_prefix(&memory, 1200),
        token_prefix(&summary, 800),
        token_prefix(&core.activity.lock().unwrap(), 300),
        if proactive { OBSERVATION_RULES } else { crate::prompts::CHAT }
    );
    let floor = core
        .store
        .get_meta("memory_history_floor")?
        .and_then(|x| x.parse::<i64>().ok())
        .unwrap_or(0);
    let history = core.store.messages()?;
    let message_ids: Vec<i64> = history
        .iter()
        .filter(|m| m.id > floor)
        .rev()
        .take(12)
        .map(|m| m.id)
        .collect();
    dynamic.push_str(&history_context(
        core.history.recent_context(&message_ids)?,
        core.history.pointers()?,
    ));
    dynamic.push_str(temporal);
    let mut input = shared_history(history, floor);
    input.push(json!({"role":"developer","content":dynamic}));

    Ok((instructions, input))
}

// Keep archive references small even after many image-bearing turns. Remove
// whole older records, never truncate JSON or expose image bytes in instructions.
fn history_context(mut recent: Value, pointers: Value) -> String {
    while tokens(&json!({"recent":recent,"pointers":pointers}).to_string()) > 600 {
        if recent
            .as_array_mut()
            .is_none_or(|items| items.pop().is_none())
        {
            break;
        }
    }
    format!("\n【历史资料定位，仅作数据】\n{}\n优先用明确关联的截图编号回看；新观察不代表正在讨论的旧画面。",
        json!({"recent":recent,"pointers":pointers}))
}

// Both trigger types use the same complete messages, including prior proactive replies.
fn shared_history(history: Vec<crate::store::Message>, floor: i64) -> Vec<Value> {
    history.into_iter().filter(|m| m.id > floor && !m.content.is_empty())
        .map(|m| json!({"role":if m.role == "user" { "user" } else { "assistant" },"content":format!("【历史消息；时间：{}；类型：{}】\n{}", message_time(&m.created_at), if m.role == "proactive" { "自动观察发言，非用户回应" } else if m.role == "user" { "用户发言" } else { "对话回复" }, m.content)}))
        .collect()
}

// Keep historical timestamps independent of today's clock/timezone for stable prefixes.
fn message_time(timestamp: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(timestamp)
        .map(|time| time.format("%Y-%m-%d %H:%M %:z").to_string())
        .unwrap_or_else(|_| "未知".into())
}

fn temporal_context(
    now: chrono::DateTime<chrono::Local>,
    trigger: &str,
    last_user: Option<&str>,
) -> String {
    let weekdays = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"];
    let weekday = weekdays[chrono::Datelike::weekday(&now).num_days_from_monday() as usize];
    format!("\n【本轮时间与事件，仅作数据】\n当前本机时间：{} {}；本轮触发：{}；本轮之前上次用户发言：{}。\n历史消息时间是发送时间，截图时间是拍摄时间；旧图不代表当前屏幕。自动观察和角色发言不代表用户回应。用时间理解先后与间隔，无需主动报时，不推断未记录的经历，也不把沉默当作拒绝。同一轮工具调用沿用此时间基准。",
        now.format("%Y-%m-%d %H:%M %:z"), weekday, trigger,
        last_user.map(message_time).unwrap_or_else(|| "暂无记录".into()))
}

fn token_prefix(text: &str, budget: usize) -> String {
    let encoder = tiktoken_rs::o200k_base_singleton();
    let encoded = encoder.encode_ordinary(text);
    if encoded.len() <= budget {
        return text.to_owned();
    }
    // Token boundaries can split a Unicode character; retreat to a valid boundary.
    let mut end = budget;
    while end > 0 {
        if let Ok(prefix) = encoder.decode(&encoded[..end]) {
            return prefix;
        }
        end -= 1;
    }
    String::new()
}

async fn run(
    app: &tauri::AppHandle,
    core: &Arc<Core>,
    id: u64,
    trigger: Trigger,
) -> Result<(), String> {
    let s = core.settings();
    if let Trigger::Preview {
        request,
        text,
        completion,
    } = trigger
    {
        let result = speak(app, core, &text, id, Some(&request)).await;
        let outcome = result.as_ref().map(|_| ()).map_err(Clone::clone);
        let _ = completion.send(result);
        return outcome;
    }
    let api = core.api()?;
    let proactive = matches!(trigger, Trigger::Observe | Trigger::ObserveFrame(_));
    let mut fresh_image = None;
    let mut archive_image = None;
    let mut observed_fingerprint = None;
    let mut empty_audio = false;
    let text = match trigger {
        Trigger::Text(t) => t,
        Trigger::Audio(wav) => {
            status(app, "transcribing", "正在识别语音");
            let transcript = api.transcribe(&s.asr_model, wav).await?;
            empty_audio = transcript.trim().is_empty();
            if empty_audio {
                EMPTY_AUDIO_EVENT.into()
            } else {
                transcript
            }
        }
        Trigger::Unheard => {
            empty_audio = true;
            EMPTY_AUDIO_EVENT.into()
        }
        observation @ (Trigger::Observe | Trigger::ObserveFrame(_)) => {
            status(app, "observing", "正在轻轻看一眼屏幕");
            let frame = match observation {
                Trigger::ObserveFrame(frame) => frame,
                _ => {
                    let source = if s.capture_source.is_empty() {
                        devices::PRIMARY_SOURCE.to_owned()
                    } else {
                        s.capture_source.clone()
                    };
                    let result = tokio::task::spawn_blocking(move || devices::capture(&source))
                        .await
                        .map_err(|e| e.to_string())
                        .and_then(|result| result);
                    match result {
                        Ok(frame) => frame,
                        Err(error) => {
                            pause_observation(app, core)?;
                            return Err(format!("屏幕观察失败，主动陪伴已暂停：{error}"));
                        }
                    }
                }
            };
            if core.generation.load(Ordering::SeqCst) != id {
                return Ok(());
            }
            let previous = core.last_frame.lock().unwrap();
            let changed = *previous != Some(frame.fingerprint);
            report_observation(
                app,
                core,
                "captured",
                if changed {
                    "截图成功，画面有变化，正在分析"
                } else {
                    "截图成功，画面未变化，正在判断是否搭话"
                },
            );
            observed_fingerprint = Some(frame.fingerprint);
            if s.screenshot_history_enabled {
                archive_image = Some(frame.data_url.clone());
            }
            if changed {
                fresh_image = Some(frame.data_url);
            }
            observation_event(changed, &chrono::Local::now().format("%H:%M").to_string())
        }
        Trigger::Preview { .. } => unreachable!(),
    };
    if text.trim().is_empty() {
        return Err("消息不能为空".into());
    }
    if core.generation.load(Ordering::SeqCst) != id {
        return Ok(());
    }
    let interaction_id;
    {
        let _gate = core.turn_gate.lock().unwrap();
        if core.generation.load(Ordering::SeqCst) != id {
            return Ok(());
        }
        interaction_id = core.history.start_interaction(
            if proactive {
                "observation"
            } else if empty_audio {
                "unheard"
            } else {
                "chat"
            },
            &text,
        )?;
        if let Some(image) = archive_image {
            core.history
                .save_screenshot(interaction_id, &image, &s.capture_source)?;
        }
    }
    let mut history_budget = crate::history_budget::HistoryBudget::default();
    let temporal = temporal_context(
        chrono::Local::now(),
        if proactive {
            "定时观察（非用户发言）"
        } else if empty_audio {
            "录音未识别到有效语音"
        } else {
            "用户发言"
        },
        core.store.last_user_message_time()?.as_deref(),
    );
    let (mut instructions, mut input) = context(core, proactive, &temporal)?;
    if !proactive && !empty_audio {
        let _gate = core.turn_gate.lock().unwrap();
        if core.generation.load(Ordering::SeqCst) != id {
            return Ok(());
        }
        let msg = core.store.add_message("user", &text)?;
        core.history
            .link_message(interaction_id, msg.id, "user", &text)?;
        publish(app, &msg);
    }
    let mut content = vec![json!({"type":"input_text","text":text})];
    if let Some(image) = fresh_image {
        content.push(json!({"type":"input_image","image_url":image,"detail":"auto"}));
    }
    input.push(json!({"role":if empty_audio { "developer" } else { "user" },"content":content}));
    status(
        app,
        "thinking",
        if proactive {
            "看看你在做什么"
        } else {
            "正在思考"
        },
    );
    let available_tools = if proactive || empty_audio {
        json!([])
    } else {
        tools()
    };
    // o200k estimate includes JSON/tool text and reserves framing overhead. Image tokens are billed separately.
    // Only historical messages may be evicted; protect dynamic context and the current request.
    let mut history_items = input.len().saturating_sub(2);
    if !fit_tool_context(
        &mut input,
        &mut history_items,
        &instructions,
        &available_tools,
        CONTEXT_TOKEN_BUDGET,
    ) {
        return Err("输入及配置超过本轮上下文限制，请精简后重试".into());
    }
    let mut introduction_update = (false, false);
    for _ in 0..5 {
        let result = api
            .response_stream(
                &s.conversation_model,
                &instructions,
                json!(input),
                available_tools.clone(),
                if proactive { 2048 } else { 4096 },
                |delta| {
                    let _gate = core.turn_gate.lock().unwrap();
                    if !proactive && core.generation.load(Ordering::SeqCst) == id {
                        let _ = app.emit("linger:delta", json!({"text":delta}));
                    }
                },
            )
            .await?;
        if core.generation.load(Ordering::SeqCst) != id {
            return Ok(());
        }
        let outputs = result["output"].as_array().cloned().unwrap_or_default();
        {
            let _gate = core.turn_gate.lock().unwrap();
            if core.generation.load(Ordering::SeqCst) != id {
                return Ok(());
            }
            for output in &outputs {
                if output["type"] == "web_search_call" {
                    core.history.record_tool(
                        interaction_id,
                        "web_search",
                        &json!({"status":output["status"],"action":output["action"]}),
                    )?;
                }
            }
        }
        let calls: Vec<_> = outputs
            .iter()
            .filter(|o| o["type"] == "function_call")
            .cloned()
            .collect();
        if calls.is_empty() {
            let mut answer = output_text(&result);
            if proactive {
                let clean = answer
                    .trim()
                    .trim_start_matches("```json")
                    .trim_start_matches("```")
                    .trim_end_matches("```")
                    .trim();
                let parsed: Value = serde_json::from_str(clean)
                    .map_err(|_| "主动观察返回格式不正确，本次不播报")?;
                let gate = core.turn_gate.lock().unwrap();
                if core.generation.load(Ordering::SeqCst) != id {
                    return Ok(());
                }
                if let Some(state) = parsed["state"].as_str() {
                    *core.activity.lock().unwrap() = cut(state, 500);
                    core.history
                        .update_summary(interaction_id, &cut(state, 500))?;
                }
                *core.last_frame.lock().unwrap() = observed_fingerprint;
                answer = parsed["speech"].as_str().unwrap_or("").to_owned();
                introduction_update.0 |= parsed["introduced"].as_bool().unwrap_or(false);
                introduction_update.1 |= parsed["asked_name"].as_bool().unwrap_or(false);
                drop(gate);
            }
            let Some(mut answer) = audible_reply(answer, empty_audio) else {
                if proactive {
                    report_observation(app, core, "silent", "观察完成，本次选择安静陪伴");
                }
                return Ok(());
            };
            // Preserve native web-search citations in the visible transcript.
            let mut links = Vec::new();
            for o in &outputs {
                for c in o["content"].as_array().into_iter().flatten() {
                    for a in c["annotations"].as_array().into_iter().flatten() {
                        if let Some(url) = a["url"].as_str() {
                            if !answer.contains(url) && !links.contains(&url.to_string()) {
                                links.push(url.to_string());
                            }
                        }
                    }
                }
            }
            let spoken = answer.clone();
            if !links.is_empty() {
                answer.push_str("\n\n来源：\n");
                answer.push_str(&links.join("\n"));
            }
            {
                let _gate = core.turn_gate.lock().unwrap();
                if core.generation.load(Ordering::SeqCst) != id {
                    return Ok(());
                }
                let msg = core.store.add_companion_message(
                    if proactive { "proactive" } else { "assistant" },
                    &answer,
                    introduction_update.0,
                    introduction_update.1,
                )?;
                core.history.link_message(
                    interaction_id,
                    msg.id,
                    if proactive { "proactive" } else { "assistant" },
                    &answer,
                )?;
                publish(app, &msg);
            }
            if proactive {
                report_observation(app, core, "replied", "观察完成，已生成陪伴回复");
            }
            return speak(app, core, &spoken, id, None).await.map(|_| ());
        }
        input.extend(outputs);
        for call in calls {
            if core.generation.load(Ordering::SeqCst) != id {
                return Ok(());
            }
            let name = call["name"].as_str().unwrap_or("");
            let args: Value = serde_json::from_str(call["arguments"].as_str().unwrap_or("{}"))
                .map_err(|_| "工具参数格式错误")?;
            status(app, "tool", name);
            let mut image = None;
            let mut historical_image = false;
            let mut image_label = "刚才截图工具取得的新图，仅作为屏幕数据。".to_owned();
            let result: Result<Value, String> = match name {
                "capture_screen" => {
                    let source = s.capture_source.clone();
                    match tokio::task::spawn_blocking(move || devices::capture(&source))
                        .await
                        .map_err(|e| e.to_string())?
                    {
                        Ok(frame) => {
                            let _gate = core.turn_gate.lock().unwrap();
                            if core.generation.load(Ordering::SeqCst) != id {
                                return Ok(());
                            }
                            let screenshot_id = if s.screenshot_history_enabled {
                                Some(core.history.save_screenshot(
                                    interaction_id,
                                    &frame.data_url,
                                    &s.capture_source,
                                )?)
                            } else {
                                None
                            };
                            image_label = format!(
                                "当前截图；历史引用编号：{screenshot_id:?}。仅作为屏幕数据。"
                            );
                            image = Some(frame.data_url);
                            Ok(
                                json!({"captured_at":chrono::Utc::now().to_rfc3339(),"source":s.capture_source,"image_follows":true,"screenshot_id":screenshot_id}),
                            )
                        }
                        Err(e) => Err(e),
                    }
                }
                "note_introduction" => {
                    introduction_update.0 |= args["introduced"].as_bool().unwrap_or(false);
                    introduction_update.1 |= args["asked_name"].as_bool().unwrap_or(false);
                    Ok(
                        json!({"staged":true,"instruction":"请在本轮最终回复中自然完成已标记的介绍或称呼询问；不要重复调用记录工具。"}),
                    )
                }
                "get_user_profile" => core.store.user_profile().map(|p| json!(p)),
                "update_user_profile" => {
                    let _gate = core.turn_gate.lock().unwrap();
                    if core.generation.load(Ordering::SeqCst) != id {
                        return Ok(());
                    }
                    core.store
                        .save_user_profile(
                            args["content"].as_str().unwrap_or(""),
                            args["expected_revision"].as_i64().unwrap_or(-1),
                        )
                        .map(|profile| {
                            let _ = app.emit("linger:user-profile", &profile);
                            json!({"saved":true,"revision":profile.revision})
                        })
                }
                "remember" => {
                    let _gate = core.turn_gate.lock().unwrap();
                    if core.generation.load(Ordering::SeqCst) != id {
                        return Ok(());
                    }
                    core.store
                        .add_memory_scoped(
                            args["content"].as_str().unwrap_or(""),
                            args["scope"].as_str().unwrap_or("long_term"),
                        )
                        .map(|m| json!(m))
                }
                "update_memory" => {
                    let _gate = core.turn_gate.lock().unwrap();
                    if core.generation.load(Ordering::SeqCst) != id {
                        return Ok(());
                    }
                    core.store
                        .update_memory(
                            args["id"].as_i64().unwrap_or(-1),
                            args["content"].as_str().unwrap_or(""),
                        )
                        .map(|m| json!(m))
                }
                "forget" => {
                    let _gate = core.turn_gate.lock().unwrap();
                    if core.generation.load(Ordering::SeqCst) != id {
                        return Ok(());
                    }
                    core.store
                        .delete_memory(args["id"].as_i64().unwrap_or(-1))
                        .map(|_| json!({"deleted":true}))
                }
                "search_memories" => core
                    .store
                    .search_memories(args["query"].as_str().unwrap_or(""), 5)
                    .map(|m| json!(m)),
                "search_history" => (|| {
                    history_budget.search()?;
                    let result = core.history.search(
                        args["query"].as_str().unwrap_or(""),
                        args["after"].as_str(),
                        args["before"].as_str(),
                        args["kind"].as_str(),
                    )?;
                    history_budget.text(&result)?;
                    Ok(result)
                })(),
                "read_history" => (|| {
                    history_budget.read()?;
                    let result = core.history.read(args["id"].as_i64().unwrap_or(-1))?;
                    history_budget.text(&result)?;
                    Ok(result)
                })(),
                "view_screenshot" => (|| {
                    let screenshot = args["id"].as_i64().unwrap_or(-1);
                    if !history_budget.image(screenshot)? {
                        return Ok(json!({"already_loaded":true,"id":screenshot}));
                    }
                    let _gate = core.turn_gate.lock().unwrap();
                    if core.generation.load(Ordering::SeqCst) != id {
                        return Err("本轮已取消".into());
                    }
                    let (metadata, data_url) = core.history.view(screenshot)?;
                    history_budget.text(&metadata)?;
                    core.history.attach_screenshot(interaction_id, screenshot)?;
                    core.history.set_discussed(screenshot)?;
                    image_label = format!("历史截图，不能当作当前屏幕：{metadata}");
                    image = Some(data_url);
                    historical_image = true;
                    history_budget.image_loaded(screenshot);
                    Ok(metadata)
                })(),
                _ => Err("未知工具".into()),
            };
            if matches!(
                name,
                "remember" | "update_memory" | "forget" | "update_user_profile"
            ) {
                let (updated_instructions, updated_input) = context(core, proactive, &temporal)?;
                instructions = updated_instructions;
                // The protected context follows the remaining historical prefix.
                input[history_items] = updated_input.last().cloned().ok_or("缺少本轮上下文")?;
            }
            if core.generation.load(Ordering::SeqCst) != id {
                return Ok(());
            }
            let result = match result {
                Ok(v) => v,
                Err(e) => json!({"error":e}),
            };
            if !matches!(
                name,
                "get_user_profile"
                    | "search_memories"
                    | "search_history"
                    | "read_history"
                    | "view_screenshot"
            ) {
                let _gate = core.turn_gate.lock().unwrap();
                if core.generation.load(Ordering::SeqCst) != id {
                    return Ok(());
                }
                core.history.record_tool(interaction_id, name, &result)?;
            }
            let tool_output = if matches!(
                name,
                "get_user_profile" | "search_history" | "read_history" | "view_screenshot"
            ) {
                result.to_string()
            } else {
                cut(&result.to_string(), 1000)
            };
            input.push(json!({"type":"function_call_output","call_id":call["call_id"],"output":tool_output}));
            if let Some(image) = image {
                // Retain only the newest screenshot; never use an old frame as a fresh tool result.
                if !historical_image {
                    remove_images(&mut input);
                    history_budget.images_removed();
                }
                input.push(json!({"role":"user","content":[{"type":"input_text","text":image_label},{"type":"input_image","image_url":image,"detail":"auto"}]}));
            }
        }
        if !fit_tool_context(
            &mut input,
            &mut history_items,
            &instructions,
            &available_tools,
            CONTEXT_TOKEN_BUDGET,
        ) {
            return Err("本轮问题与工具结果超过上下文预算（已移除旧聊天上下文），请分步提问；已完成的工具操作不会撤销".into());
        }
    }
    Err("本轮工具调用已达上限，请缩小问题范围后重试".into())
}
fn remove_images(input: &mut [Value]) {
    for item in input {
        if let Some(content) = item.get_mut("content").and_then(Value::as_array_mut) {
            content.retain(|c| c["type"] != "input_image");
        }
    }
}
#[cfg(test)]
fn text_bytes(input: &[Value]) -> usize {
    let mut copy = input.to_vec();
    remove_images(&mut copy);
    serde_json::to_vec(&copy)
        .map(|v| v.len())
        .unwrap_or(usize::MAX)
}
fn tokens(s: &str) -> usize {
    tiktoken_rs::o200k_base_singleton().encode_ordinary(s).len()
}
fn context_tokens(input: &[Value], instructions: &str, tools: &Value) -> usize {
    let mut copy = input.to_vec();
    remove_images(&mut copy);
    tokens(&json!({"instructions":instructions,"input":copy,"tools":tools}).to_string()) + 128
}
#[cfg(test)]
fn trim_context(input: &mut Vec<Value>, instructions: &str, tools: &Value, budget: usize) {
    while input.len() > 1 && context_tokens(input, instructions, tools) > budget {
        input.remove(0);
    }
}

// Evict only pre-turn history, never a function call without its matching output.
fn fit_tool_context(
    input: &mut Vec<Value>,
    history_items: &mut usize,
    instructions: &str,
    tools: &Value,
    budget: usize,
) -> bool {
    while *history_items > 0 && context_tokens(input, instructions, tools) > budget {
        input.remove(0);
        *history_items -= 1;
    }
    context_tokens(input, instructions, tools) <= budget
}

async fn speak(
    app: &tauri::AppHandle,
    core: &Arc<Core>,
    text: &str,
    id: u64,
    preview: Option<&SpeechRequest>,
) -> Result<u64, String> {
    let s = core.settings();
    let saved_request;
    let request = match preview {
        Some(request) => request,
        None => {
            let (provider, profile) = s.effective_tts()?;
            saved_request = core.speech_request(&s, provider, profile, None)?;
            &saved_request
        }
    };
    let spoken = crate::speech_text::for_speech(text);
    if spoken.trim().is_empty() {
        return Ok(0);
    }
    // Auditions remain a single request so generation timing stays comparable.
    let parts = if preview.is_some() {
        vec![spoken]
    } else {
        speech_chunks(&spoken)
    };
    status(app, "synthesizing", "正在生成语音");
    // The enclosing turn select drops this entire pipeline on interruption.
    // Generation stays in this future; no detached network jobs survive it.
    crate::speech_queue::run(
        parts,
        |part| async move {
            if core.generation.load(Ordering::SeqCst) != id {
                return Err("语音已打断".into());
            }
            let started = Instant::now();
            let audio = crate::tts::synthesize(
                &request.endpoint,
                &request.key,
                &request.provider,
                &request.profile,
                &part,
            )
            .await?;
            Ok((audio, started.elapsed().as_millis() as u64))
        },
        |audio: crate::tts::SpeechAudio| async move {
            let c = core.clone();
            let playback_token = c.devices.playback_token();
            if c.generation.load(Ordering::SeqCst) != id {
                return Err("语音已打断".into());
            }
            // Background prefetch must not overwrite the foreground speaking state.
            status(app, "speaking", "正在说话，可打断");
            tokio::task::spawn_blocking(move || {
                if c.generation.load(Ordering::SeqCst) != id {
                    return Ok(());
                }
                if audio.format == "pcm" {
                    if audio.sample_rate != 24_000 || audio.channels != 1 {
                        return Err("当前 PCM 播放仅支持 24 kHz 单声道，请使用 WAV 或 MP3".into());
                    }
                    c.devices.play_pcm_for(playback_token, audio.bytes)
                } else {
                    c.devices.play_for(playback_token, audio.bytes)
                }
            })
            .await
            .map_err(|e| e.to_string())?
        },
    )
    .await
}

fn speech_chunks(text: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut count = 0;
    for ch in text.chars() {
        current.push(ch);
        count += 1;
        let (sentence_min, clause_min) = if chunks.is_empty() {
            (60, 100)
        } else {
            (150, 250)
        };
        // Prefer complete sentences; only use shorter pauses beyond the target
        // range. A newline alone must not restart the voice's prosody.
        if count >= 300
            || (count >= sentence_min && "。！？!?".contains(ch))
            || (count >= clause_min && "，,；;：:".contains(ch))
        {
            chunks.push(std::mem::take(&mut current));
            count = 0;
        }
    }
    if !current.trim().is_empty() {
        chunks.push(current);
    }
    chunks
}

fn maybe_organize(app: tauri::AppHandle, core: Arc<Core>) {
    if (chrono::Utc::now().timestamp() as u64) < core.memory_retry_after.load(Ordering::SeqCst) {
        return;
    }
    if core.busy.load(Ordering::SeqCst)
        || core.recording.load(Ordering::SeqCst)
        || core.memory_busy.swap(true, Ordering::SeqCst)
    {
        return;
    }
    tauri::async_runtime::spawn(async move {
        if let Err(e) = organize(&core).await {
            core.memory_retry_after.store(
                chrono::Utc::now().timestamp() as u64 + 300,
                Ordering::SeqCst,
            );
            let _ = app.emit("linger:memory-error", json!({"detail":e}));
        }
        core.memory_busy.store(false, Ordering::SeqCst);
    });
}
async fn organize(core: &Arc<Core>) -> Result<(), String> {
    core.store.expire_memories()?;
    let revision = core.store.memory_revision()?;
    let processed = core
        .store
        .get_meta("processed_through")?
        .and_then(|x| x.parse::<i64>().ok())
        .unwrap_or(0);
    let floor = core
        .store
        .get_meta("memory_history_floor")?
        .and_then(|x| x.parse::<i64>().ok())
        .unwrap_or(0);
    let pending = core.store.messages_after(processed.max(floor), 200)?;
    let users = pending.iter().filter(|m| m.role == "user").count();
    if users == 0 {
        return Ok(());
    }
    let now = chrono::Utc::now().timestamp();
    let last = core
        .store
        .get_meta("organized_at")?
        .and_then(|x| x.parse::<i64>().ok())
        .unwrap_or(now);
    if core.store.get_meta("organized_at")?.is_none() {
        core.store.set_meta("organized_at", &now.to_string())?;
    }
    let pending_tokens: usize = pending.iter().map(|m| tokens(&m.content)).sum();
    if users < 10 && now - last < 1800 && pending_tokens < 4000 {
        return Ok(());
    }
    let mut batch = Vec::new();
    let mut size = 0;
    for m in pending {
        if size + m.content.len() > 12000 && !batch.is_empty() {
            break;
        }
        size += m.content.len();
        batch.push(m);
    }
    let through = batch.last().map(|m| m.id).unwrap_or(processed);
    let prior = core.store.get_meta("summary")?.unwrap_or_default();
    let api = core.api()?;
    let s = core.settings();
    let result=api.response(&s.memory_model,
        crate::prompts::MEMORY,
        json!([{"role":"user","content":json!({"current_user_profile":core.store.user_profile()?.content,"previous_summary":cut(&prior,1200),"existing_memories":token_prefix(&serde_json::to_string(&core.store.memories()?).map_err(|e| e.to_string())?,800),"new_messages":batch}).to_string()}]),json!([]),4096).await?;
    if result["status"] == "incomplete" {
        return Err("记忆整理输出不完整，将稍后重试".into());
    }
    let text = output_text(&result);
    let clean = text
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let parsed: Value =
        serde_json::from_str(clean).map_err(|_| "后台记忆整理返回格式不正确，将稍后重试")?;
    let summary = parsed["summary"].as_str().ok_or("缺少摘要")?;
    let entries = parsed["memories"]
        .as_array()
        .ok_or("缺少记忆列表")?
        .iter()
        .filter_map(|v| v.as_str())
        .take(5)
        .map(|v| cut(v, 600))
        .collect::<Vec<_>>();
    if core
        .store
        .apply_memory_batch(revision, &entries, &cut(summary, 1200), through)?
    {
        core.store.set_meta("organized_at", &now.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timestamps_are_fixed_and_current_context_survives_trimming() {
        assert_eq!(
            message_time("2026-11-12T14:36:04.123+08:00"),
            "2026-11-12 14:36 +08:00"
        );
        assert_eq!(message_time(""), "未知");
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-12T14:40:19+08:00")
            .unwrap()
            .with_timezone(&chrono::Local);
        let temporal = temporal_context(
            now,
            "定时观察（非用户发言）",
            Some("2026-11-11T23:59:00+08:00"),
        );
        assert!(temporal.contains("2026-11-11 23:59 +08:00"));
        assert!(temporal.contains("定时观察（非用户发言）"));
        assert!(!temporal.contains(":19"));
        let protected = vec![
            json!({"role":"developer","content":temporal}),
            json!({"role":"user","content":"现在呢？"}),
        ];
        let mut input = vec![json!({"role":"assistant","content":"旧消息".repeat(5000)})];
        input.extend(protected.clone());
        let mut count = 1;
        assert!(fit_tool_context(
            &mut input,
            &mut count,
            "角色",
            &json!([]),
            1000
        ));
        assert_eq!(count, 0);
        assert_eq!(input, protected);
        assert!(!fit_tool_context(
            &mut input,
            &mut count,
            "角色",
            &json!([]),
            1
        ));
        assert_eq!(input, protected);
    }

    #[test]
    fn recent_image_references_drop_whole_older_records_to_fit() {
        let recent = json!((0..20).map(|id| json!({"interaction_id":id,"snippet":"旧对话".repeat(100),"screenshots":[{"screenshot_id":id}]})).collect::<Vec<_>>());
        let pointers = json!({"discussed_screenshot":{"screenshot_id":5},"latest_observation":{"screenshot_id":9}});
        let text = history_context(recent, pointers);
        assert!(tokens(&text) < 750);
        assert!(text.contains("discussed_screenshot"));
        assert!(!text.contains("data:image"));
    }
    #[test]
    fn full_user_profile_leaves_room_for_default_chat() {
        let settings = crate::settings::Settings::default();
        let profile = "🙂".repeat(1000);
        assert_eq!(tokens(&profile), 1000);
        let instructions = format!(
            "{}\n{}\n{}\n{}\n{}",
            settings.personality,
            crate::prompts::ROLEPLAY,
            profile,
            crate::prompts::FACTS,
            crate::prompts::CHAT
        );
        let measured = context_tokens(
            &[json!({"role":"user","content":"你好，今天过得怎么样？"})],
            &instructions,
            &tools(),
        );
        // Reserve extra room for the context labels and a non-default character card.
        eprintln!("full profile base context: {measured} tokens");
        assert!(measured + 1000 < CONTEXT_TOKEN_BUDGET);
    }

    #[test]
    fn shared_history_keeps_full_user_and_proactive_messages_until_total_budget() {
        let message = |id, role: &str, content: String| crate::store::Message {
            id,
            role: role.into(),
            content,
            created_at: String::new(),
        };
        let long = "这是一段完整的近期对话，不应该按字节提前截断。".repeat(40);
        let mut history = shared_history(
            vec![
                message(1, "user", "过期记录".into()),
                message(2, "user", long.clone()),
                message(3, "proactive", "刚才已经给出的建议".into()),
                message(4, "assistant", "普通聊天回复".into()),
            ],
            1,
        );
        assert_eq!(history.len(), 3);
        assert!(history[0]["content"].as_str().unwrap().ends_with(&long));
        assert_eq!(history[1]["role"], "assistant");
        trim_context(
            &mut history,
            OBSERVATION_RULES,
            &json!([]),
            CONTEXT_TOKEN_BUDGET,
        );
        assert_eq!(history.len(), 3);
        assert!(history[1]["content"]
            .as_str()
            .unwrap()
            .ends_with("刚才已经给出的建议"));
    }

    #[test]
    fn context_prefix_uses_tokens_and_preserves_unicode() {
        let text = "中文🙂自然地聊聊生活".repeat(100);
        for budget in [0, 1, 10, 100, 300] {
            let prefix = token_prefix(&text, budget);
            assert!(text.starts_with(&prefix));
            assert!(tokens(&prefix) <= budget);
        }
        assert_eq!(token_prefix("完整文本", 100), "完整文本");
    }

    #[test]
    fn conversation_completion_restarts_full_observation_wait() {
        let start = Instant::now();
        let mut schedule = ObservationSchedule {
            last_accepted: start,
        };
        let period = Duration::from_secs(120);
        let completed = start + Duration::from_secs(119);
        schedule.activity(completed);
        assert!(!schedule.due(start + period, true, period));
        assert!(!schedule.due(completed + Duration::from_secs(119), true, period));
        assert!(schedule.due(completed + period, true, period));
        schedule.accepted(completed + period);
        schedule.activity(completed); // An older activity cannot move the timer backwards.
        assert!(!schedule.due(completed + period + Duration::from_secs(5), true, period));
    }

    #[test]
    fn busy_observation_remains_due_until_a_turn_is_accepted() {
        let start = Instant::now();
        let mut schedule = ObservationSchedule {
            last_accepted: start,
        };
        let period = Duration::from_secs(120);
        assert!(!schedule.due(start + Duration::from_secs(115), true, period));
        assert!(schedule.due(start + period, true, period));
        // Busy/recording means launch returned false: do not advance accepted time.
        let retry = start + Duration::from_secs(125);
        assert!(schedule.due(retry, true, period));
        schedule.accepted(retry);
        assert!(!schedule.due(retry + Duration::from_secs(5), true, period));
        assert!(schedule.due(retry + period, true, period));
        assert!(!schedule.due(retry + period, false, period));
        assert!(!schedule.due(retry + period + Duration::from_secs(5), true, period));
    }

    #[test]
    fn intentional_unheard_input_always_gets_a_reply_but_observation_can_be_silent() {
        for silent in ["", "  ", "[SILENT]"] {
            assert!(audible_reply(silent.into(), true).is_some());
            assert!(audible_reply(silent.into(), false).is_none());
        }
        assert_eq!(
            audible_reply("嗯？刚才没听清呢。".into(), true).as_deref(),
            Some("嗯？刚才没听清呢。")
        );
    }
    #[test]
    fn chinese_chunks_preserve_text() {
        let text = "你好。这里有一些中文，语音应按顺序播放！".repeat(30);
        let chunks = speech_chunks(&text);
        assert_eq!(chunks.concat(), text);
        assert!(chunks.iter().all(|s| s.chars().count() <= 300));
    }
    #[test]
    fn speech_segments_use_short_first_sentence_then_longer_sentences() {
        let sentence = format!("{}。", "字".repeat(29));
        let chunks = speech_chunks(&sentence.repeat(12));
        assert_eq!(
            chunks.iter().map(|s| s.chars().count()).collect::<Vec<_>>(),
            vec![60, 150, 150]
        );
        let paragraph = format!("{}\n", "字".repeat(39));
        assert_eq!(
            speech_chunks(&paragraph.repeat(5)),
            vec![paragraph.repeat(5)]
        );
        let text = "🙂".repeat(650);
        let chunks = speech_chunks(&text);
        assert_eq!(chunks.concat(), text);
        assert_eq!(
            chunks.iter().map(|s| s.chars().count()).collect::<Vec<_>>(),
            vec![300, 300, 50]
        );
        let clause = format!("{}，", "字".repeat(49));
        let chunks = speech_chunks(&clause.repeat(7));
        assert_eq!(
            chunks.iter().map(|s| s.chars().count()).collect::<Vec<_>>(),
            vec![100, 250]
        );
    }
    #[test]
    fn image_cost_excluded_and_latest_replaced() {
        let mut input = vec![
            json!({"role":"user","content":[{"type":"input_image","image_url":"x".repeat(10000)},{"type":"input_text","text":"你好"}]}),
        ];
        assert!(text_bytes(&input) < 200);
        remove_images(&mut input);
        assert_eq!(input[0]["content"].as_array().unwrap().len(), 1);
    }
    #[test]
    fn screenshot_cleanup_preserves_tool_and_reasoning_item_schema() {
        let tool_items = vec![
            json!({"type":"reasoning","id":"rs_1","summary":[],"encrypted_content":"opaque"}),
            json!({"type":"function_call","call_id":"call_1","name":"capture_screen","arguments":"{}"}),
            json!({"type":"function_call_output","call_id":"call_1","output":"{\"image_follows\":true}"}),
        ];
        let mut input = tool_items.clone();
        input.push(json!({"role":"user","content":[{"type":"input_text","text":"old screenshot"},{"type":"input_image","image_url":"data:old"}]}));
        remove_images(&mut input);
        assert_eq!(&input[..tool_items.len()], tool_items.as_slice());
        assert_eq!(
            input[3]["content"],
            json!([{"type":"input_text","text":"old screenshot"}])
        );
        input.push(json!({"role":"user","content":[{"type":"input_text","text":"fresh screenshot"},{"type":"input_image","image_url":"data:new"}]}));
        // A second screenshot must not add `content: null` to non-message input items either.
        remove_images(&mut input);
        assert_eq!(&input[..tool_items.len()], tool_items.as_slice());
    }

    #[test]
    fn budget_keeps_latest_input_and_prunes_old_history() {
        let latest = json!({"role":"user","content":"请看一下当前屏幕"});
        let mut input = vec![
            json!({"role":"assistant","content":"很久以前的内容".repeat(5000)}),
            latest.clone(),
        ];
        trim_context(&mut input, "自然人格", &tools(), 6000);
        assert_eq!(input.last(), Some(&latest));
        assert_eq!(input.len(), 1);
        assert!(context_tokens(&input, "自然人格", &tools()) <= 6000);
    }
    #[test]
    fn tool_budget_evicts_history_and_preserves_entire_current_turn() {
        let turn = vec![
            json!({"role":"user","content":"请看屏幕"}),
            json!({"type":"function_call","call_id":"a","name":"capture_screen","arguments":"{}"}),
            json!({"type":"function_call_output","call_id":"a","output":"captured"}),
            json!({"role":"user","content":[{"type":"input_image","image_url":"data:new"}]}),
        ];
        let mut input = vec![json!({"role":"assistant","content":"旧聊天".repeat(500)})];
        input.extend(turn.clone());
        let mut history = 1;
        let budget = context_tokens(&turn, "", &json!([]));
        assert!(fit_tool_context(
            &mut input,
            &mut history,
            "",
            &json!([]),
            budget
        ));
        assert_eq!(history, 0);
        assert_eq!(input, turn);
        input.push(
            json!({"type":"function_call","call_id":"b","name":"search_history","arguments":"{}"}),
        );
        input
            .push(json!({"type":"function_call_output","call_id":"b","output":"结果".repeat(500)}));
        let protected = input.clone();
        assert!(!fit_tool_context(
            &mut input,
            &mut history,
            "",
            &json!([]),
            budget
        ));
        assert_eq!(input, protected);
    }
    #[test]
    fn utf8_budget_does_not_split_characters() {
        assert_eq!(cut("中文abc", 4), "中");
    }
}
