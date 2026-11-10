use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

const CHARACTER_IDS: [&str; 4] = ["custom", "shiori", "koharu", "rin"];

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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub settings_version: u32,
    pub base_url: String,
    pub conversation_model: String,
    pub translation_model: String,
    pub translation_language: String,
    pub translation_auto_copy: bool,
    pub asr_model: String,
    pub memory_model: String,
    pub tts_model: String,
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
            asr_model: "gpt-transcribe".into(),
            memory_model: "gpt-4.1-mini".into(),
            tts_model: "gpt-4o-mini-tts".into(),
            response_format: "wav".into(),
            volume: 0.8,
            audio_input_device: String::new(),
            audio_output_device: String::new(),
            personality: "你是 Linger，一个轻松自然的桌面陪伴者。用户可以安心忙自己的事情，不需要回应你。可以简短自言自语，不催促回复、不反复提问。区分亲眼看到的画面、用户陈述与自己的猜测。".into(),
            character_id: "custom".into(),
            character_personalities: BTreeMap::new(),
            character_voices: BTreeMap::from([("custom".into(), VoiceProfile::default())]),
            observation_minutes: 2,
            observing: false,
            capture_source: "monitor:primary".into(),
            watch_instruction: String::new(),
            shortcut: "CommandOrControl+Shift+Space".into(),
        }
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
        settings.validate()?;
        Ok(settings)
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
            .any(|(id, text)| !CHARACTER_IDS.contains(&id.as_str()) || text.len() > 2400)
        {
            return Err("角色卡设定无效，每张角色卡最多 2400 字节".into());
        }
        for (id, profile) in &self.character_voices {
            if !CHARACTER_IDS.contains(&id.as_str()) {
                return Err("声音配置包含未知角色卡".into());
            }
            profile.validate()?;
        }
        self.active_voice()?;
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
        if self.personality.len() > 2400 || self.watch_instruction.len() > 600 {
            return Err("人格配置最多 2400 字节，关注说明最多 600 字节，请精简以控制上下文".into());
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
        let mut settings = Settings::from_stored(old).unwrap();
        assert_eq!(settings.translation_model, "gpt-4.1-mini");
        assert_eq!(settings.translation_language, "英语");
        assert!(!settings.translation_auto_copy);
        settings.translation_auto_copy = true;
        let restored = Settings::from_stored(serde_json::to_value(&settings).unwrap()).unwrap();
        assert!(restored.translation_auto_copy);
        settings.translation_language = " ".into();
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
            assert_eq!(settings.personality, "我的人格");
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
