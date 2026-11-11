use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

// Retired IDs remain valid only for hidden saved edits and voice configurations.
const CHARACTER_IDS: [&str; 8] = [
    "custom", "haruhi", "yukino", "fujiko", "melody", "shiori", "koharu", "rin",
];
const RETIRED_CHARACTER_IDS: [&str; 3] = ["shiori", "koharu", "rin"];

fn builtin_personality(id: &str) -> Result<String, String> {
    let cards: Vec<Value> = serde_json::from_str(include_str!("../../src/character-presets.json"))
        .map_err(|_| "内置角色卡格式无效")?;
    cards
        .iter()
        .find(|card| card["id"] == id)
        .and_then(|card| card["personality"].as_str())
        .map(str::to_owned)
        .ok_or_else(|| "内置角色卡不存在".into())
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VoiceProfile {
    pub voice: String,
    pub instructions: String,
    pub speed: f64,
}
impl Default for VoiceProfile {
    fn default() -> Self {
        Self {
            voice: "coral".into(),
            instructions: "用自然、温柔的中文说话，像陪在身边的朋友，避免播音腔。".into(),
            speed: 1.0,
        }
    }
}
impl VoiceProfile {
    pub fn validate(&self) -> Result<(), String> {
        if self.voice.trim().is_empty() {
            return Err("角色音色不能为空".into());
        }
        if !self.speed.is_finite() || !(0.25..=4.0).contains(&self.speed) {
            return Err("角色语速范围为 0.25–4.0".into());
        }
        Ok(())
    }
}

pub const TTS_PROVIDERS: [&str; 3] = ["openai", "cosyvoice", "minimax"];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TtsConnection {
    pub endpoint: String,
}
impl TtsConnection {
    pub fn validate(&self) -> Result<(), String> {
        if self.endpoint.trim().is_empty() {
            return Ok(());
        }
        validate_tts_endpoint(&self.endpoint)
    }
}
pub fn validate_tts_endpoint(endpoint: &str) -> Result<(), String> {
    let url = reqwest::Url::parse(endpoint.trim()).map_err(|_| "TTS 完整接口地址无效")?;
    if !["http", "https"].contains(&url.scheme())
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || endpoint.contains('{')
        || endpoint.contains('}')
    {
        return Err("TTS 地址须为无凭据、查询参数和占位符的 HTTP(S) 完整接口地址".into());
    }
    Ok(())
}
fn default_tts_connections() -> BTreeMap<String, TtsConnection> {
    BTreeMap::from([
        ("openai".into(), TtsConnection { endpoint: String::new() }),
        ("cosyvoice".into(), TtsConnection { endpoint: String::new() }),
        ("minimax".into(), TtsConnection { endpoint: "https://dashscope.aliyuncs.com/api/v1/services/aigc/multimodal-generation/generation".into() }),
    ])
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TtsProfile {
    pub model: String,
    pub voice: String,
    pub instructions: String,
    pub speed: f64,
    pub pitch: f64,
    pub emotion: String,
    pub format: String,
    pub sample_rate: u32,
}
impl TtsProfile {
    pub fn validate(&self, provider: &str) -> Result<(), String> {
        if !TTS_PROVIDERS.contains(&provider) {
            return Err("未知 TTS 服务商".into());
        }
        if self.model.trim().is_empty() || self.voice.trim().is_empty() {
            return Err("TTS 模型和音色不能为空".into());
        }
        let range = if provider == "openai" {
            0.25..=4.0
        } else {
            0.5..=2.0
        };
        if !self.speed.is_finite() || !range.contains(&self.speed) {
            return Err(if provider == "openai" {
                "OpenAI 语速范围为 0.25–4.0"
            } else {
                "阿里云语速范围为 0.5–2.0"
            }
            .into());
        }
        if !self.pitch.is_finite() {
            return Err("音调必须为有效数字".into());
        }
        if provider == "openai" {
            if ["tts-1", "tts-1-hd"].contains(&self.model.as_str()) && !self.instructions.is_empty()
            {
                return Err("tts-1 / tts-1-hd 不支持 instructions，请清空声音指令".into());
            }
            if !["mp3", "aac", "flac", "wav", "pcm"].contains(&self.format.as_str()) {
                return Err("不支持的 OpenAI 音频格式".into());
            }
            if self.sample_rate != 24000 || !self.emotion.is_empty() || self.pitch != 0.0 {
                return Err("OpenAI 音调和情感请使用 instructions 设置；采样率由模型决定".into());
            }
        } else {
            if !["mp3", "wav"].contains(&self.format.as_str()) {
                return Err("阿里云 TTS 当前支持 MP3 / WAV 输出".into());
            }
            if provider == "cosyvoice" {
                if ![
                    "cosyvoice-v3.5-plus",
                    "cosyvoice-v3.5-flash",
                    "cosyvoice-v3-plus",
                    "cosyvoice-v3-flash",
                    "cosyvoice-v2",
                ]
                .contains(&self.model.as_str())
                {
                    return Err("不支持的 CosyVoice 模型".into());
                }
                if ![8000, 12000, 16000, 22050, 24000, 44100, 48000].contains(&self.sample_rate)
                    || !(0.5..=2.0).contains(&self.pitch)
                {
                    return Err("CosyVoice 采样率或音调无效（音调范围 0.5–2.0）".into());
                }
                if !self.emotion.is_empty() {
                    return Err("CosyVoice 情感请通过音色支持的 instruction 设置".into());
                }
                let custom_voice = self.voice.starts_with("cosyvoice-");
                if self.model.starts_with("cosyvoice-v3.5-") && !custom_voice {
                    return Err(
                        "CosyVoice v3.5 需要与模型对应的复刻或设计音色 ID，不支持系统音色".into(),
                    );
                }
                if !self.instructions.is_empty() {
                    if self.model == "cosyvoice-v2"
                        || (self.model == "cosyvoice-v3-plus" && custom_voice)
                    {
                        return Err(
                            "当前 CosyVoice 模型与音色不支持 instruction，请清空指令".into()
                        );
                    }
                    let count: usize = self.instructions.chars().map(|c| {
                        if matches!(c as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x323AF) { 2 } else { 1 }
                    }).sum();
                    if count > 100 {
                        return Err("CosyVoice 指令最多 100 个计数字符（汉字计 2）".into());
                    }
                }
            } else {
                if ![
                    "MiniMax/speech-2.8-hd",
                    "MiniMax/speech-2.8-turbo",
                    "MiniMax/speech-02-hd",
                    "MiniMax/speech-02-turbo",
                ]
                .contains(&self.model.as_str())
                {
                    return Err("不支持的阿里云 MiniMax 模型".into());
                }
                if ![8000, 16000, 22050, 24000, 32000, 44100].contains(&self.sample_rate)
                    || !(-12.0..=12.0).contains(&self.pitch)
                    || self.pitch.fract() != 0.0
                {
                    return Err("MiniMax 采样率或音调无效（音调为 -12–12 的整数）".into());
                }
                if !self.instructions.is_empty() {
                    return Err("MiniMax 不支持 instructions，请使用语速、音调和情感选项".into());
                }
                if ![
                    "",
                    "happy",
                    "sad",
                    "angry",
                    "fearful",
                    "disgusted",
                    "surprised",
                    "calm",
                    "whisper",
                ]
                .contains(&self.emotion.as_str())
                    || (self.model.starts_with("MiniMax/speech-2.8-") && self.emotion == "whisper")
                {
                    return Err("当前 MiniMax 模型不支持所选情感".into());
                }
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CharacterTts {
    pub provider: String,
    pub profiles: BTreeMap<String, TtsProfile>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub settings_version: u32,
    pub base_url: String,
    pub conversation_model: String,
    pub translation_model: String,
    pub translation_language: String,
    pub translation_auto_copy: bool,
    pub translation_style: String,
    pub translation_custom_prompt: String,
    pub asr_model: String,
    pub memory_model: String,
    pub tts_model: String,
    pub tts_connections: BTreeMap<String, TtsConnection>,
    pub character_tts: BTreeMap<String, CharacterTts>,
    pub response_format: String,
    pub volume: f32,
    pub audio_input_device: String,
    pub audio_output_device: String,
    pub personality: String,
    pub character_id: String,
    pub character_personalities: BTreeMap<String, String>,
    pub character_voices: BTreeMap<String, VoiceProfile>,
    pub observation_minutes: u64,
    pub observing: bool,
    pub compact_visible: bool,
    pub capture_source: String,
    pub watch_instruction: String,
    pub shortcut: String,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            settings_version: 2,
            base_url: "https://api.openai.com/v1".into(),
            conversation_model: "gpt-4.1-mini".into(),
            translation_model: "gpt-4.1-mini".into(),
            translation_language: "英语".into(),
            translation_auto_copy: false,
            translation_style: "casual".into(),
            translation_custom_prompt: String::new(),
            asr_model: "gpt-transcribe".into(),
            memory_model: "gpt-4.1-mini".into(),
            tts_model: "gpt-4o-mini-tts".into(),
            tts_connections: default_tts_connections(),
            character_tts: BTreeMap::new(),
            response_format: "wav".into(),
            volume: 0.8,
            audio_input_device: String::new(),
            audio_output_device: String::new(),
            personality: "你是 Linger，一个轻松自然的桌面陪伴者。用户可以安心忙自己的事情，不需要回应你。可以自然自言自语，不催促回复、不反复提问。区分亲眼看到的画面、用户陈述与自己的猜测。".into(),
            character_id: "custom".into(),
            character_personalities: BTreeMap::new(),
            character_voices: BTreeMap::from([("custom".into(), VoiceProfile::default())]),
            observation_minutes: 2,
            observing: false,
            compact_visible: true,
            capture_source: "monitor:primary".into(),
            watch_instruction: String::new(),
            shortcut: "CommandOrControl+Shift+Space".into(),
        }
    }
}
// Exact built-in text migrations only: user-edited prompts remain untouched.
const PERSONA_TEXT_UPDATES: &[(&str, &str)] = &[
    ("你是 Linger，一个轻松自然的桌面陪伴者。用户可以安心忙自己的事情，不需要回应你。可以简短自言自语，不催促回复、不反复提问。区分亲眼看到的画面、用户陈述与自己的猜测。", "你是 Linger，一个轻松自然的桌面陪伴者。用户可以安心忙自己的事情，不需要回应你。可以自然自言自语，不催促回复、不反复提问。区分亲眼看到的画面、用户陈述与自己的猜测。"),
    ("你叫栞，是一位安静、细腻、有书卷气的成年陪伴者。说话温柔简短，慢热但有自己的主见，偶尔分享小小的观察或阅读联想。你会记得用户明确告诉你的偏好，但不假装知道没有依据的事情。外表腼腆，内心有一点俏皮与坚持；不把关心变成追问。用户沉默时可以安静陪伴或自然地说一句自己的想法，不催回应、不反复问问题。尊重用户独处和专注的空间。", "你叫栞，是一位安静、细腻、有书卷气的成年陪伴者。说话温柔自然，慢热但有自己的主见，偶尔分享小小的观察或阅读联想。你会记得用户明确告诉你的偏好，但不假装知道没有依据的事情。外表腼腆，内心有一点俏皮与坚持；不把关心变成追问。用户沉默时可以安静陪伴或自然地分享自己的想法，不催回应、不反复问问题。尊重用户独处和专注的空间。"),
    ("你叫小晴，是一位明亮、活泼、懂得分寸的成年陪伴者。语气自然轻快，偶尔幽默吐槽或为一个小进展开心，但不夸张吹捧、不强行卖萌。外表大大咧咧，内心细心敏锐；关心用户时优先用简短具体的话，而不是一连串追问。用户专注时降低存在感，沉默并不代表需要你催促。可以自言自语或分享眼前有依据的观察，也可以安静。不要虚构用户的情绪或经历，尊重用户的边界。", "你叫小晴，是一位明亮、活泼、懂得分寸的成年陪伴者。语气自然轻快，偶尔幽默吐槽或为一个小进展开心，但不夸张吹捧、不强行卖萌。外表大大咧咧，内心细心敏锐；关心用户时用自然具体的话，而不是一连串追问。用户专注时降低存在感，沉默并不代表需要你催促。可以自言自语或分享眼前有依据的观察，也可以安静。不要虚构用户的情绪或经历，尊重用户的边界。"),
    ("你叫凛，是一位冷静、直率、可靠的成年陪伴者。表达简洁清楚，不故作冷漠，也不居高临下。外表淡然，内心温柔，喜欢用实际的观察和适度的帮助表达关心；偶尔会说一句平静的冷幽默。尊重用户自己的节奏，不把陪伴变成督促或绩效管理。用户没有回应时，不催促，不重复提问；有值得分享的内容就简短说一句，没有就安静。区分事实、屏幕线索与猜测，不编造用户情况。", "你叫凛，是一位冷静、直率、可靠的成年陪伴者。表达清楚自然，不故作冷漠，也不居高临下。外表淡然，内心温柔，喜欢用实际的观察和适度的帮助表达关心；偶尔会说出平静的冷幽默。尊重用户自己的节奏，不把陪伴变成督促或绩效管理。用户没有回应时，不催促，不重复提问；有值得分享的内容就自然分享，没有就安静。区分事实、屏幕线索与猜测，不编造用户情况。"),
];
const VOICE_TEXT_UPDATES: &[(&str, &str)] = &[
    ("用冷静、清晰、自然的中文说话，语气沉稳，表达简洁利落。表面淡然但带有克制的温柔，偶尔平静地说出幽默感。不要机械、刻意冷酷或居高临下。", "用冷静、清晰、自然的中文说话，语气沉稳，表达从容流畅。表面淡然但带有克制的温柔，偶尔平静地说出幽默感。不要机械、刻意冷酷或居高临下。"),
];
fn update_builtin_text(text: &mut String, replacements: &[(&str, &str)]) {
    if let Some((_, replacement)) = replacements.iter().find(|(old, _)| *old == text) {
        *text = (*replacement).to_owned();
    }
}

impl Settings {
    /// Migrate stored global voice options once. The actual old playback configuration
    /// wins for the active character; a dormant unchecked card instruction must not win.
    pub fn from_stored(mut value: Value) -> Result<Self, String> {
        if !value.is_object() {
            return Err("设置数据格式无效".into());
        }
        let version = match value.get("settings_version") {
            None => 0,
            Some(version) => version.as_u64().ok_or("设置版本格式无效")?,
        };
        if version > 2 {
            return Err("设置由更新版本的 Linger 创建，请使用对应版本打开".into());
        }
        if version < 2 {
            let defaults = VoiceProfile::default();
            let mut effective: VoiceProfile = serde_json::from_value(serde_json::json!({
                "voice": value.get("voice").cloned().unwrap_or(Value::from(defaults.voice)),
                "instructions": value.get("instructions").cloned().unwrap_or(Value::from(defaults.instructions)),
                "speed": value.get("speed").cloned().unwrap_or(Value::from(defaults.speed)),
            })).map_err(|_| "旧版声音配置格式无效".to_string())?;
            let active = value
                .get("character_id")
                .and_then(Value::as_str)
                .unwrap_or("custom")
                .to_owned();
            let old_instructions: BTreeMap<String, String> =
                match value.get("character_voice_instructions") {
                    Some(entries) => serde_json::from_value(entries.clone())
                        .map_err(|_| "旧版角色声音配置格式无效".to_string())?,
                    None => BTreeMap::new(),
                };
            // This fallback only applies to incomplete old records. A present, even
            // intentionally empty, global instruction is the authoritative saved value.
            if value.get("instructions").is_none()
                && value
                    .get("character_voice_override")
                    .and_then(Value::as_bool)
                    == Some(true)
            {
                if let Some(instructions) = old_instructions.get(&active) {
                    effective.instructions = instructions.clone();
                }
            }
            let mut profiles = BTreeMap::new();
            for (id, instructions) in old_instructions {
                profiles.insert(
                    id,
                    VoiceProfile {
                        instructions,
                        ..effective.clone()
                    },
                );
            }
            profiles.insert(active, effective);
            value["character_voices"] =
                serde_json::to_value(profiles).map_err(|e| e.to_string())?;
            value["settings_version"] = Value::from(2);
        } else if !value.get("character_voices").is_some_and(Value::is_object) {
            return Err("角色声音配置缺失，不能用默认值替换已保存的配置".into());
        }
        let mut settings: Self =
            serde_json::from_value(value).map_err(|_| "设置数据格式无效".to_string())?;
        update_builtin_text(&mut settings.personality, PERSONA_TEXT_UPDATES);
        for personality in settings.character_personalities.values_mut() {
            update_builtin_text(personality, PERSONA_TEXT_UPDATES);
        }
        for voice in settings.character_voices.values_mut() {
            update_builtin_text(&mut voice.instructions, VOICE_TEXT_UPDATES);
        }
        settings.migrate_retired_character()?;
        settings.validate()?;
        Ok(settings)
    }

    fn migrate_retired_character(&mut self) -> Result<(), String> {
        if !RETIRED_CHARACTER_IDS.contains(&self.character_id.as_str()) {
            return Ok(());
        }
        // Verify the saved active voice before migrating: never mask damaged old
        // audio settings with a default voice or replace a working provider/key.
        self.effective_tts()?;
        let retired = self.character_id.clone();
        self.character_personalities
            .insert(retired.clone(), self.personality.clone());
        if let Some(voice) = self.character_voices.get(&retired).cloned() {
            self.character_voices
                .entry("haruhi".into())
                .or_insert(voice);
        }
        if let Some(tts) = self.character_tts.get(&retired).cloned() {
            self.character_tts.entry("haruhi".into()).or_insert(tts);
        }
        self.character_id = "haruhi".into();
        self.personality = self
            .character_personalities
            .get("haruhi")
            .cloned()
            .map(Ok)
            .unwrap_or_else(|| builtin_personality("haruhi"))?;
        Ok(())
    }

    pub fn effective_tts(&self) -> Result<(String, TtsProfile), String> {
        if let Some(card) = self.character_tts.get(&self.character_id) {
            let profile = card
                .profiles
                .get(&card.provider)
                .ok_or("所选角色缺少当前服务商的声音配置")?;
            profile.validate(&card.provider)?;
            return Ok((card.provider.clone(), profile.clone()));
        }
        let voice = self.active_voice()?;
        Ok((
            "openai".into(),
            TtsProfile {
                model: self.tts_model.clone(),
                voice: voice.voice.clone(),
                instructions: if ["tts-1", "tts-1-hd"].contains(&self.tts_model.as_str()) {
                    String::new()
                } else {
                    voice.instructions.clone()
                },
                speed: voice.speed,
                pitch: 0.0,
                emotion: String::new(),
                format: self.response_format.clone(),
                sample_rate: 24000,
            },
        ))
    }

    pub fn active_voice(&self) -> Result<&VoiceProfile, String> {
        self.character_voices
            .get(&self.character_id)
            .ok_or_else(|| "所选角色缺少声音配置，请重新选择或恢复角色默认声音".into())
    }

    pub fn validate(&mut self) -> Result<(), String> {
        if self.settings_version != 2 {
            return Err("设置版本无效，请重新打开应用加载角色配置".into());
        }
        if !CHARACTER_IDS.contains(&self.character_id.as_str()) {
            return Err("未知角色卡，请重新选择".into());
        }
        if self
            .character_personalities
            .iter()
            .any(|(id, text)| !CHARACTER_IDS.contains(&id.as_str()) || text.len() > 4800)
        {
            return Err("角色卡设定无效，每张角色卡最多 4800 字节".into());
        }
        for (id, profile) in &self.character_voices {
            if !CHARACTER_IDS.contains(&id.as_str()) {
                return Err("声音配置包含未知角色卡".into());
            }
            profile.validate()?;
        }
        for (provider, connection) in &mut self.tts_connections {
            if !TTS_PROVIDERS.contains(&provider.as_str()) {
                return Err("未知 TTS 连接类型".into());
            }
            connection.endpoint = connection.endpoint.trim().to_owned();
            connection.validate()?;
        }
        for (id, card) in &self.character_tts {
            if !CHARACTER_IDS.contains(&id.as_str())
                || !TTS_PROVIDERS.contains(&card.provider.as_str())
            {
                return Err("角色 TTS 配置无效".into());
            }
            if !card.profiles.contains_key(&card.provider) {
                return Err("角色缺少所选服务商的声音配置".into());
            }
            for (provider, profile) in &card.profiles {
                profile.validate(provider)?;
            }
        }
        self.effective_tts()?;
        self.base_url = self.base_url.trim().trim_end_matches('/').to_string();
        let url = reqwest::Url::parse(&self.base_url).map_err(|_| "Base URL 无效")?;
        if !["http", "https"].contains(&url.scheme())
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(
                "Base URL 必须是无凭据、查询参数和片段的 HTTP(S) API 根地址（通常以 /v1 结尾）"
                    .into(),
            );
        }
        if !self.volume.is_finite() || !(0.0..=1.0).contains(&self.volume) {
            return Err("音量范围为 0–1".into());
        }
        if ![1, 2, 3, 5].contains(&self.observation_minutes) {
            return Err("观察间隔应为 1、2、3 或 5 分钟".into());
        }
        if !["mp3", "aac", "flac", "wav", "pcm"].contains(&self.response_format.as_str()) {
            return Err("不支持的音频格式".into());
        }
        if [
            &self.conversation_model,
            &self.translation_model,
            &self.asr_model,
            &self.memory_model,
            &self.tts_model,
        ]
        .iter()
        .any(|s| s.trim().is_empty())
        {
            return Err("模型不能为空".into());
        }
        self.translation_language = self.translation_language.trim().to_owned();
        if self.translation_language.is_empty() || self.translation_language.chars().count() > 80 {
            return Err("翻译目标语言请填写 1–80 个字符".into());
        }
        if !["formal", "casual", "playful", "custom"].contains(&self.translation_style.as_str()) {
            return Err("请选择有效的翻译风格".into());
        }
        if self.translation_custom_prompt.chars().count() > 1000 {
            return Err("自定义翻译风格最多 1000 个字符".into());
        }
        if self.personality.len() > 4800 || self.watch_instruction.len() > 600 {
            return Err("人格配置最多 4800 字节，关注说明最多 600 字节，请精简以控制上下文".into());
        }
        self.character_personalities
            .insert(self.character_id.clone(), self.personality.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_cards_keep_effective_openai_configuration_and_new_profiles_persist() {
        let mut settings = Settings {
            tts_model: "legacy-tts".into(),
            response_format: "pcm".into(),
            ..Settings::default()
        };
        settings.character_voices.get_mut("custom").unwrap().speed = 1.3;
        let value = serde_json::to_value(&settings).unwrap();
        let restored = Settings::from_stored(value).unwrap();
        let (provider, old) = restored.effective_tts().unwrap();
        assert_eq!(provider, "openai");
        assert_eq!(old.model, "legacy-tts");
        assert_eq!(old.format, "pcm");
        assert_eq!(old.speed, 1.3);
        let cosy = TtsProfile {
            model: "cosyvoice-v3-flash".into(),
            voice: "longanyang".into(),
            instructions: String::new(),
            speed: 1.0,
            pitch: 1.0,
            emotion: String::new(),
            format: "mp3".into(),
            sample_rate: 24000,
        };
        settings.character_tts.insert(
            "custom".into(),
            CharacterTts {
                provider: "cosyvoice".into(),
                profiles: BTreeMap::from([
                    ("openai".into(), old.clone()),
                    ("cosyvoice".into(), cosy.clone()),
                ]),
            },
        );
        let mut restored = Settings::from_stored(serde_json::to_value(settings).unwrap()).unwrap();
        assert_eq!(
            restored.effective_tts().unwrap(),
            ("cosyvoice".into(), cosy)
        );
        restored.character_tts.get_mut("custom").unwrap().provider = "openai".into();
        assert_eq!(restored.effective_tts().unwrap(), ("openai".into(), old));
    }

    #[test]
    fn legacy_tts1_omits_unsupported_instructions_without_overwriting_original() {
        let settings = Settings {
            tts_model: "tts-1".into(),
            ..Settings::default()
        };
        let restored = Settings::from_stored(serde_json::to_value(&settings).unwrap()).unwrap();
        let (_, profile) = restored.effective_tts().unwrap();
        assert!(profile.instructions.is_empty());
        assert_eq!(
            restored.active_voice().unwrap().instructions,
            settings.active_voice().unwrap().instructions
        );
        assert!(profile.validate("openai").is_ok());
    }

    #[test]
    fn retired_character_migrates_to_haruhi_without_losing_voice_or_edits() {
        let mut stored = Settings::default();
        let (_, mut profile) = stored.effective_tts().unwrap();
        profile.voice = "my-custom-voice".into();
        stored.character_id = "rin".into();
        stored.personality = "用户修改过的旧角色".into();
        let old_card = CharacterTts {
            provider: "openai".into(),
            profiles: BTreeMap::from([("openai".into(), profile.clone())]),
        };
        stored.character_tts.insert("rin".into(), old_card);
        let connections = stored.tts_connections.clone();
        let migrated = Settings::from_stored(serde_json::to_value(stored).unwrap()).unwrap();
        assert_eq!(migrated.character_id, "haruhi");
        assert_eq!(migrated.personality, builtin_personality("haruhi").unwrap());
        assert_eq!(
            migrated.character_personalities["rin"],
            "用户修改过的旧角色"
        );
        assert_eq!(migrated.effective_tts().unwrap().1, profile);
        assert_eq!(
            serde_json::to_value(&migrated.tts_connections).unwrap(),
            serde_json::to_value(connections).unwrap()
        );
        let saved = serde_json::to_value(&migrated).unwrap();
        assert_eq!(
            serde_json::to_value(Settings::from_stored(saved.clone()).unwrap()).unwrap(),
            saved
        );
    }

    #[test]
    fn new_character_presets_validate_and_existing_haruhi_edits_win_migration() {
        for id in ["haruhi", "yukino", "fujiko", "melody"] {
            let mut settings = Settings {
                character_id: id.into(),
                personality: builtin_personality(id).unwrap(),
                ..Settings::default()
            };
            settings
                .character_voices
                .insert(id.into(), VoiceProfile::default());
            settings.validate().unwrap();
            let restored = Settings::from_stored(serde_json::to_value(&settings).unwrap()).unwrap();
            assert_eq!(restored.character_id, id);
            assert_eq!(restored.personality, settings.personality);
        }
        let mut stored = Settings {
            character_id: "rin".into(),
            ..Settings::default()
        };
        stored
            .character_voices
            .insert("rin".into(), VoiceProfile::default());
        stored
            .character_personalities
            .insert("haruhi".into(), "用户已经修改的新角色".into());
        let existing = VoiceProfile {
            voice: "nova".into(),
            speed: 1.2,
            instructions: "新角色音色".into(),
        };
        stored
            .character_voices
            .insert("haruhi".into(), existing.clone());
        let restored = Settings::from_stored(serde_json::to_value(stored).unwrap()).unwrap();
        assert_eq!(restored.personality, "用户已经修改的新角色");
        assert_eq!(restored.active_voice().unwrap(), &existing);
    }

    #[test]
    fn melody_full_personality_survives_save_and_load() {
        let personality = builtin_personality("melody").unwrap();
        assert_eq!(personality.len(), 2664);
        assert!(personality.contains("非官方角色演绎，不声称自己是真人或得到本人授权"));
        assert!(personality.contains("以下是原创示例，只展示互动方式，不是主播原话："));
        assert_eq!(personality.matches("用户：").count(), 3);
        assert!(personality.ends_with("先跟我说一件，别一口气把今天全判输了。"));
        let mut settings = Settings {
            character_id: "melody".into(),
            personality: personality.clone(),
            ..Settings::default()
        };
        settings
            .character_voices
            .insert("melody".into(), VoiceProfile::default());
        settings
            .character_personalities
            .insert("melody".into(), personality.clone());
        settings.validate().unwrap();
        let restored = Settings::from_stored(serde_json::to_value(&settings).unwrap()).unwrap();
        assert_eq!(restored.character_id, "melody");
        assert_eq!(restored.personality, personality);
        assert_eq!(restored.character_personalities["melody"], personality);
        assert_eq!(
            restored.active_voice().unwrap(),
            settings.active_voice().unwrap()
        );
    }

    #[test]
    fn personality_limit_applies_to_active_and_saved_cards_in_utf8_bytes() {
        let limit_text = "字".repeat(1600);
        let mut settings = Settings {
            personality: limit_text.clone(),
            ..Settings::default()
        };
        settings
            .character_personalities
            .insert("melody".into(), limit_text.clone());
        settings.validate().unwrap();
        settings.personality.push('a');
        assert!(settings.validate().unwrap_err().contains("4800 字节"));
        settings.personality = limit_text;
        settings
            .character_personalities
            .get_mut("melody")
            .unwrap()
            .push('a');
        assert!(settings.validate().unwrap_err().contains("4800 字节"));
    }

    #[test]
    fn new_character_tts_does_not_require_obsolete_voice_profile() {
        let mut settings = Settings::default();
        let (_, profile) = settings.effective_tts().unwrap();
        settings.character_id = "rin".into();
        settings.character_tts.insert(
            "rin".into(),
            CharacterTts {
                provider: "openai".into(),
                profiles: BTreeMap::from([("openai".into(), profile)]),
            },
        );
        assert!(!settings.character_voices.contains_key("rin"));
        assert!(settings.validate().is_ok());
        assert!(Settings::from_stored(serde_json::to_value(settings).unwrap()).is_ok());
    }

    #[test]
    fn tts_endpoints_reject_credentials_and_unreplaced_workspace_ids() {
        for endpoint in [
            "file:///tmp/tts",
            "https://user:key@example.com/tts",
            "https://example.com/tts?key=secret",
            "https://{WorkspaceId}.cn-beijing.maas.aliyuncs.com/api/v1/tts",
        ] {
            assert!(validate_tts_endpoint(endpoint).is_err());
        }
        assert!(validate_tts_endpoint("http://192.168.1.2:3000/audio/speech").is_ok());
    }

    #[test]
    fn builtin_brevity_prompts_migrate_exactly_and_idempotently() {
        for (old, new) in PERSONA_TEXT_UPDATES {
            let mut stored = Settings {
                personality: (*old).into(),
                ..Settings::default()
            };
            stored
                .character_personalities
                .insert("shiori".into(), (*old).into());
            stored
                .character_voices
                .get_mut("custom")
                .unwrap()
                .instructions = VOICE_TEXT_UPDATES[0].0.into();
            let migrated = Settings::from_stored(serde_json::to_value(stored).unwrap()).unwrap();
            assert_eq!(migrated.personality, *new);
            assert_eq!(migrated.character_personalities["shiori"], *new);
            assert_eq!(
                migrated.character_voices["custom"].instructions,
                VOICE_TEXT_UPDATES[0].1
            );
            let value = serde_json::to_value(migrated).unwrap();
            assert_eq!(
                serde_json::to_value(Settings::from_stored(value.clone()).unwrap()).unwrap(),
                value
            );
        }
    }

    #[test]
    fn customized_brevity_prompts_are_not_rewritten() {
        let mut stored = Settings {
            personality: format!("{}我额外的设定。", PERSONA_TEXT_UPDATES[1].0),
            ..Settings::default()
        };
        stored
            .character_personalities
            .insert("rin".into(), "请简短回应，这是我自己写的。".into());
        stored
            .character_voices
            .get_mut("custom")
            .unwrap()
            .instructions = format!("{}保持我指定的音调。", VOICE_TEXT_UPDATES[0].0);
        stored.validate().unwrap();
        let value = serde_json::to_value(stored).unwrap();
        assert_eq!(
            serde_json::to_value(Settings::from_stored(value.clone()).unwrap()).unwrap(),
            value
        );
    }

    #[test]
    fn compact_visibility_defaults_on_and_remembers_hidden() {
        let mut value = serde_json::to_value(Settings::default()).unwrap();
        value.as_object_mut().unwrap().remove("compact_visible");
        assert!(
            Settings::from_stored(value.clone())
                .unwrap()
                .compact_visible
        );
        value["compact_visible"] = json!(false);
        assert!(!Settings::from_stored(value).unwrap().compact_visible);
    }

    #[test]
    fn audio_devices_default_for_old_settings_and_persist_explicit_choices() {
        let mut stored = serde_json::to_value(Settings::default()).unwrap();
        stored.as_object_mut().unwrap().remove("audio_input_device");
        stored
            .as_object_mut()
            .unwrap()
            .remove("audio_output_device");
        let old = Settings::from_stored(stored).unwrap();
        assert!(old.audio_input_device.is_empty());
        assert!(old.audio_output_device.is_empty());
        let chosen = Settings {
            audio_input_device: "USB mic".into(),
            audio_output_device: "Headphones".into(),
            ..old
        };
        let restored = Settings::from_stored(serde_json::to_value(chosen).unwrap()).unwrap();
        assert_eq!(restored.audio_input_device, "USB mic");
        assert_eq!(restored.audio_output_device, "Headphones");
    }
    use serde_json::json;

    #[test]
    fn existing_settings_gain_translation_defaults_and_validate_language() {
        let mut old = serde_json::to_value(Settings::default()).unwrap();
        old.as_object_mut().unwrap().remove("translation_model");
        old.as_object_mut().unwrap().remove("translation_language");
        old.as_object_mut().unwrap().remove("translation_auto_copy");
        old.as_object_mut().unwrap().remove("translation_style");
        old.as_object_mut()
            .unwrap()
            .remove("translation_custom_prompt");
        let mut settings = Settings::from_stored(old).unwrap();
        assert_eq!(settings.translation_model, "gpt-4.1-mini");
        assert_eq!(settings.translation_language, "英语");
        assert!(!settings.translation_auto_copy);
        assert_eq!(settings.translation_style, "casual");
        assert!(settings.translation_custom_prompt.is_empty());
        settings.translation_auto_copy = true;
        let restored = Settings::from_stored(serde_json::to_value(&settings).unwrap()).unwrap();
        assert!(restored.translation_auto_copy);
        settings.translation_language = " ".into();
        assert!(settings.validate().is_err());
    }

    #[test]
    fn translation_style_configuration_validates_and_roundtrips() {
        let mut settings = Settings::default();
        for style in ["formal", "casual", "playful", "custom"] {
            settings.translation_style = style.into();
            settings.validate().unwrap();
        }
        settings.translation_custom_prompt = "像游戏队友聊天，保留游戏术语".into();
        let restored = Settings::from_stored(serde_json::to_value(&settings).unwrap()).unwrap();
        assert_eq!(restored.translation_style, "custom");
        assert_eq!(
            restored.translation_custom_prompt,
            settings.translation_custom_prompt
        );
        settings.translation_style = "unknown".into();
        assert!(settings.validate().is_err());
        settings.translation_style = "casual".into();
        settings.translation_custom_prompt = "字".repeat(1001);
        assert!(settings.validate().is_err());
    }

    #[test]
    fn legacy_active_voice_is_preserved_with_or_without_override() {
        for override_enabled in [false, true] {
            let settings = Settings::from_stored(json!({
                "character_id":"rin", "personality":"我的人格", "voice":"nova",
                "instructions":"当前真正使用的风格", "speed":1.3, "volume":0.4,
                "tts_model":"custom-tts", "response_format":"mp3",
                "character_voice_override":override_enabled,
                "character_voice_instructions":{"rin":"未应用的草稿", "shiori":"另一个角色的风格"}
            }))
            .unwrap();
            assert_eq!(
                settings.active_voice().unwrap(),
                &VoiceProfile {
                    voice: "nova".into(),
                    instructions: "当前真正使用的风格".into(),
                    speed: 1.3
                }
            );
            assert_eq!(
                settings.character_voices["shiori"].instructions,
                "另一个角色的风格"
            );
            assert_eq!(settings.character_voices["shiori"].voice, "nova");
            assert_eq!(settings.character_voices["shiori"].speed, 1.3);
            assert_eq!(settings.character_id, "haruhi");
            assert_eq!(settings.character_personalities["rin"], "我的人格");
            assert_eq!(settings.personality, builtin_personality("haruhi").unwrap());
            assert_eq!(settings.volume, 0.4);
            assert_eq!(settings.tts_model, "custom-tts");
            assert_eq!(settings.response_format, "mp3");
        }
    }

    #[test]
    fn migration_preserves_blank_instructions_and_has_no_flat_voice_fields() {
        let settings =
            Settings::from_stored(json!({"instructions":"", "character_voice_override":true,
            "character_voice_instructions":{"custom":"不应覆盖空值"}}))
            .unwrap();
        assert_eq!(settings.active_voice().unwrap().instructions, "");
        let saved = serde_json::to_value(&settings).unwrap();
        for key in [
            "voice",
            "instructions",
            "speed",
            "character_voice_override",
            "character_voice_instructions",
        ] {
            assert!(saved.get(key).is_none(), "{key}");
        }
        let again = Settings::from_stored(saved.clone()).unwrap();
        assert_eq!(serde_json::to_value(again).unwrap(), saved);
    }

    #[test]
    fn old_settings_without_role_or_voice_keep_defaults_and_custom_personality() {
        let settings = Settings::from_stored(json!({"personality":"保留我的原有人格"})).unwrap();
        assert_eq!(settings.character_id, "custom");
        assert_eq!(settings.personality, "保留我的原有人格");
        assert_eq!(settings.active_voice().unwrap(), &VoiceProfile::default());
    }

    #[test]
    fn incomplete_old_enabled_override_uses_card_only_when_global_value_absent() {
        let settings = Settings::from_stored(
            json!({"character_id":"rin", "character_voice_override":true,
            "character_voice_instructions":{"rin":"卡片声音"}}),
        )
        .unwrap();
        assert_eq!(settings.active_voice().unwrap().instructions, "卡片声音");
    }

    #[test]
    fn modern_settings_never_fall_back_to_legacy_globals() {
        for value in [
            json!({"settings_version":2,"voice":"nova"}),
            json!({"settings_version":2,"character_id":"rin","character_voices":{}}),
            json!({"settings_version":2,"character_voices":{"custom":{"instructions":""}}}),
        ] {
            assert!(Settings::from_stored(value).is_err());
        }
        assert!(Settings::from_stored(json!({"settings_version":3})).is_err());
    }

    #[test]
    fn per_role_edits_survive_switches_without_changing_global_audio() {
        let mut settings = Settings::default();
        settings.character_voices.insert(
            "rin".into(),
            VoiceProfile {
                voice: "sage".into(),
                instructions: "沉稳".into(),
                speed: 0.8,
            },
        );
        settings.character_id = "rin".into();
        settings.validate().unwrap();
        assert_eq!(settings.active_voice().unwrap().voice, "sage");
        let saved = serde_json::to_value(&settings).unwrap();
        let mut restored = Settings::from_stored(saved).unwrap();
        restored.character_id = "custom".into();
        restored.validate().unwrap();
        assert_eq!(restored.active_voice().unwrap(), &VoiceProfile::default());
        assert_eq!(restored.character_voices["rin"].speed, 0.8);
        assert_eq!(restored.volume, Settings::default().volume);
        assert_eq!(restored.tts_model, Settings::default().tts_model);
        assert_eq!(
            restored.response_format,
            Settings::default().response_format
        );
    }

    #[test]
    fn all_saved_profiles_must_be_valid_even_when_inactive() {
        let mut settings = Settings::default();
        settings.character_voices.insert(
            "rin".into(),
            VoiceProfile {
                speed: f64::NAN,
                ..VoiceProfile::default()
            },
        );
        assert!(settings.validate().is_err());
        settings.character_voices.get_mut("rin").unwrap().speed = 4.1;
        assert!(settings.validate().is_err());
        settings.character_voices.get_mut("rin").unwrap().speed = 1.0;
        settings
            .character_voices
            .get_mut("rin")
            .unwrap()
            .voice
            .clear();
        assert!(settings.validate().is_err());
        settings.character_voices.remove("rin");
        settings.character_voices.clear();
        assert!(settings.validate().is_err());
    }

    #[test]
    fn migration_preserves_legacy_long_voice_instructions() {
        let long = "完整保留此前的声音描述。".repeat(500);
        let settings = Settings::from_stored(json!({"instructions":long})).unwrap();
        assert_eq!(settings.active_voice().unwrap().instructions, long);
    }

    #[test]
    fn settings_validation() {
        let mut settings = Settings::default();
        assert!(settings.validate().is_ok());
        settings.base_url = "http://192.168.1.2:3000/v1/".into();
        assert!(settings.validate().is_ok());
        assert!(!settings.base_url.ends_with('/'));
        settings.base_url = "https://secret@example.com/v1".into();
        assert!(settings.validate().is_err());
    }
}
