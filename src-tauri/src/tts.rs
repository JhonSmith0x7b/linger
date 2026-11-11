//! Non-streaming speech adapters. Credentials belong only on the configured synthesis request.
use crate::settings::{validate_tts_endpoint, TtsProfile};
use reqwest::{Client, Response};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

const MAX_AUDIO_BYTES: usize = 32 * 1024 * 1024;
const MAX_JSON_BYTES: usize = MAX_AUDIO_BYTES * 2 + 64 * 1024;

#[derive(Debug)]
pub struct SpeechAudio {
    pub bytes: Vec<u8>,
    pub format: String,
    pub sample_rate: u32,
    pub channels: u16,
}

#[derive(Deserialize)]
struct CosyInstructionOptions {
    model: String,
    voice: String,
    emotions: Vec<String>,
    scenes: Vec<String>,
    #[serde(rename = "rolePrefix")]
    role_prefix: String,
    roles: Vec<String>,
    identities: Vec<String>,
    dialects: Vec<String>,
}

// One catalog supplies both the dropdowns and the preflight validation.
fn validate_cosy_instruction(profile: &TtsProfile) -> Result<(), String> {
    if profile.instructions.is_empty() {
        return Ok(());
    }
    static OPTIONS: std::sync::OnceLock<Vec<CosyInstructionOptions>> = std::sync::OnceLock::new();
    let options = OPTIONS.get_or_init(|| {
        serde_json::from_str(include_str!("../../src/cosy-instructions.json"))
            .expect("bundled CosyVoice instruction catalog")
    });
    let Some(option) = options
        .iter()
        .find(|option| option.model == profile.model && option.voice == profile.voice)
    else {
        // Custom voices and manually entered system IDs retain their own documented capabilities.
        return Ok(());
    };
    let instruction = &profile.instructions;
    let valid = option
        .dialects
        .iter()
        .any(|dialect| *instruction == format!("请用{dialect}表达。"))
        || option.emotions.iter().any(|emotion| {
            *instruction == format!("你说话的情感是{emotion}。")
                || option.scenes.iter().any(|scene| {
                    *instruction == format!("你正在进行{scene}，你说话的情感是{emotion}。")
                })
                || option.roles.iter().any(|role| {
                    *instruction
                        == format!("{}{role}，你说话的情感是{emotion}。", option.role_prefix)
                })
                || option.identities.iter().any(|identity| {
                    *instruction
                        == format!("你正在以一个{identity}的身份说话，你说话的情感是{emotion}。")
                })
        });
    if valid {
        Ok(())
    } else {
        Err("当前 CosyVoice 系统音色要求固定格式的指令，请在角色卡中重新选择情绪、场景或方言后再试听".into())
    }
}

fn request_body(provider: &str, profile: &TtsProfile, text: &str) -> Value {
    match provider {
        "openai" => {
            let mut body = json!({"model":profile.model, "voice":profile.voice,
                "speed":profile.speed, "response_format":profile.format, "input":text});
            if !profile.instructions.is_empty() {
                body["instructions"] = json!(profile.instructions);
            }
            body
        }
        "cosyvoice" => {
            let mut body = json!({"model":profile.model,"input":{
                "text":text,"voice":profile.voice,"format":profile.format,
                "sample_rate":profile.sample_rate,"rate":profile.speed,"pitch":profile.pitch
            }});
            if !profile.instructions.is_empty() {
                body["input"]["instruction"] = json!(profile.instructions);
            }
            body
        }
        _ => {
            let mut body = json!({"model":profile.model,"input":{
                "text":text,"output_format":"hex",
                "voice_setting":{"voice_id":profile.voice,"speed":profile.speed,"pitch":profile.pitch as i32},
                "audio_setting":{"format":profile.format,"sample_rate":profile.sample_rate,"channel":1}
            }});
            if !profile.emotion.is_empty() {
                body["input"]["voice_setting"]["emotion"] = json!(profile.emotion);
            }
            body
        }
    }
}

async fn read_bounded(mut response: Response, max: usize) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|length| length > max as u64)
    {
        return Err("语音接口响应过大，已停止下载".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "语音响应下载中断")? {
        if chunk.len() > max.saturating_sub(bytes.len()) {
            return Err("语音接口响应过大，已停止下载".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn checked_status(response: Response) -> Result<Response, String> {
    if !response.status().is_success() {
        // Do not echo provider-controlled bodies/URLs which can contain credentials or text.
        return Err(format!(
            "TTS API HTTP {}：请检查连接、Key、模型和音色配置",
            response.status().as_u16()
        ));
    }
    Ok(response)
}

fn check_provider_error(value: &Value, provider: &str) -> Result<(), String> {
    if value.get("code").is_some_and(|code| !code.is_null())
        || value.get("error").is_some_and(|e| !e.is_null())
    {
        let code = value["code"]
            .as_str()
            .or_else(|| value["error"]["code"].as_str())
            .unwrap_or("");
        let reason = match code {
            "InvalidApiKey" | "invalid_api_key" => "API Key 无效",
            "AccessDenied" | "Unauthorized" => "没有模型或业务空间的访问权限",
            "InvalidParameter" | "invalid_parameter" => "参数无效，请核对模型、音色和指令",
            "Throttling" | "Throttling.RateQuota" | "rate_limit_exceeded" => {
                "调用受到限流，请稍后重试"
            }
            "ModelNotFound" | "model_not_found" => "模型不存在或当前账号未开通",
            _ => "请检查 Key、模型权限、音色及参数",
        };
        return Err(format!("语音服务返回错误：{reason}"));
    }
    if provider == "minimax" {
        match value["output"]["base_resp"]["status_code"].as_i64() {
            Some(0) => (),
            Some(code) => {
                return Err(format!(
                    "MiniMax 合成失败（状态码 {code}），请检查 Key、音色和参数"
                ))
            }
            None => return Err("MiniMax 响应缺少合成状态码".into()),
        }
    }
    Ok(())
}

fn decode_hex(value: &str) -> Result<Vec<u8>, String> {
    if value.is_empty() || !value.len().is_multiple_of(2) || value.len() / 2 > MAX_AUDIO_BYTES {
        return Err("MiniMax 返回的音频数据为空、过大或格式无效".into());
    }
    value
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let digit = |b: u8| match b {
                b'0'..=b'9' => Some(b - b'0'),
                b'a'..=b'f' => Some(b - b'a' + 10),
                b'A'..=b'F' => Some(b - b'A' + 10),
                _ => None,
            };
            match (digit(pair[0]), digit(pair[1])) {
                (Some(hi), Some(lo)) => Ok(hi * 16 + lo),
                _ => Err("MiniMax 返回了无效的十六进制音频".into()),
            }
        })
        .collect()
}

pub async fn synthesize(
    endpoint: &str,
    key: &str,
    provider: &str,
    profile: &TtsProfile,
    text: &str,
) -> Result<SpeechAudio, String> {
    profile.validate(provider)?;
    if provider == "cosyvoice" {
        validate_cosy_instruction(profile)?;
    }
    validate_tts_endpoint(endpoint)?;
    if key.trim().is_empty() {
        return Err("请先保存当前 TTS 连接的 API Key".into());
    }
    if text.trim().is_empty() || text.chars().count() > 4096 {
        return Err("单次语音合成文本须为 1–4096 个字符".into());
    }
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(120))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "无法创建语音服务连接")?;
    let response = checked_status(
        client
            .post(endpoint.trim())
            .bearer_auth(key)
            .json(&request_body(provider, profile, text))
            .send()
            .await
            .map_err(|_| "语音服务连接失败或超时，请检查完整接口地址和网络")?,
    )?;
    let mut audio = SpeechAudio {
        bytes: Vec::new(),
        format: profile.format.clone(),
        sample_rate: profile.sample_rate,
        channels: 1,
    };
    if provider == "openai" {
        audio.bytes = read_bounded(response, MAX_AUDIO_BYTES).await?;
    } else {
        let value: Value = serde_json::from_slice(&read_bounded(response, MAX_JSON_BYTES).await?)
            .map_err(|_| "语音服务返回了无效的 JSON 响应")?;
        check_provider_error(&value, provider)?;
        if provider == "cosyvoice" {
            let url = value["output"]["audio"]["url"]
                .as_str()
                .ok_or("CosyVoice 未返回音频下载地址")?;
            let parsed = reqwest::Url::parse(url).map_err(|_| "音频下载地址无效")?;
            if !["http", "https"].contains(&parsed.scheme())
                || parsed.host_str().is_none()
                || !parsed.username().is_empty()
                || parsed.password().is_some()
                || parsed.fragment().is_some()
            {
                return Err("音频下载地址必须为无凭据的 HTTP(S) 地址".into());
            }
            // Signed storage URLs may contain query parameters. Never attach the synthesis key.
            let response = checked_status(
                client
                    .get(parsed)
                    .send()
                    .await
                    .map_err(|_| "音频下载失败或超时")?,
            )?;
            audio.bytes = read_bounded(response, MAX_AUDIO_BYTES).await?;
        } else {
            let hex = value["output"]["data"]["audio"]
                .as_str()
                .ok_or("MiniMax 未返回音频数据")?;
            audio.bytes = decode_hex(hex)?;
            let extra = &value["output"]["extra_info"];
            if let Some(format) = extra["audio_format"].as_str() {
                audio.format = format.to_owned();
            }
            if let Some(rate) = extra["audio_sample_rate"].as_u64() {
                audio.sample_rate = u32::try_from(rate).map_err(|_| "MiniMax 返回无效采样率")?;
            }
            if let Some(channels) = extra["audio_channel"].as_u64() {
                audio.channels = u16::try_from(channels).map_err(|_| "MiniMax 返回无效声道数")?;
            }
            if !["mp3", "wav"].contains(&audio.format.as_str())
                || !(8000..=48000).contains(&audio.sample_rate)
                || ![1, 2].contains(&audio.channels)
            {
                return Err("MiniMax 返回了不支持的音频格式或参数".into());
            }
        }
    }
    if audio.bytes.is_empty() {
        return Err("语音接口返回了空音频".into());
    }
    Ok(audio)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn profile(provider: &str) -> TtsProfile {
        TtsProfile {
            model: match provider {
                "cosyvoice" => "cosyvoice-v3-flash",
                "minimax" => "MiniMax/speech-2.8-turbo",
                _ => "gpt-4o-mini-tts",
            }
            .into(),
            voice: match provider {
                "cosyvoice" => "longanyang",
                "minimax" => "male-qn-qingse",
                _ => "coral",
            }
            .into(),
            instructions: String::new(),
            speed: 1.2,
            pitch: if provider == "cosyvoice" { 1.0 } else { 0.0 },
            emotion: String::new(),
            format: "wav".into(),
            sample_rate: 24000,
        }
    }
    fn mock(
        status: &str,
        headers: &str,
        body: Vec<u8>,
    ) -> (String, std::thread::JoinHandle<String>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let status = status.to_owned();
        let headers = headers.to_owned();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 4096];
            loop {
                let n = stream.read(&mut buffer).unwrap();
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..n]);
                if let Some(end) = request.windows(4).position(|v| v == b"\r\n\r\n") {
                    let header = String::from_utf8_lossy(&request[..end]);
                    let length: usize = header
                        .lines()
                        .find_map(|l| {
                            l.to_lowercase()
                                .strip_prefix("content-length:")
                                .map(|s| s.trim().parse().unwrap())
                        })
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            write!(
                stream,
                "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(&body).unwrap();
            String::from_utf8(request).unwrap()
        });
        (format!("http://{address}/tts"), handle)
    }
    fn body(request: &str) -> Value {
        serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap()
    }

    #[tokio::test]
    async fn openai_preserves_style_speed_and_binary_audio() {
        let (url, server) = mock(
            "200 OK",
            "Content-Type: audio/wav\r\n",
            b"RIFF-test".to_vec(),
        );
        let mut p = profile("openai");
        p.instructions = "温柔自然".into();
        let audio = synthesize(&url, "test-key", "openai", &p, "你好")
            .await
            .unwrap();
        assert_eq!(audio.bytes, b"RIFF-test");
        let request = server.join().unwrap();
        let body = body(&request);
        assert_eq!(body["instructions"], "温柔自然");
        assert_eq!(body["speed"], 1.2);
        assert_eq!(body["response_format"], "wav");
        assert_eq!(body["voice"], "coral");
        assert!(request
            .to_lowercase()
            .contains("authorization: bearer test-key"));
    }
    #[tokio::test]
    async fn cosyvoice_download_never_forwards_api_key() {
        let (audio_url, download) = mock(
            "200 OK",
            "Content-Type: audio/wav\r\n",
            b"RIFF-test".to_vec(),
        );
        let (url, server) = mock(
            "200 OK",
            "Content-Type: application/json\r\n",
            json!({"output":{"audio":{"url":format!("{audio_url}?signature=example")}}})
                .to_string()
                .into_bytes(),
        );
        let mut p = profile("cosyvoice");
        p.instructions = "你说话的情感是happy。".into();
        let audio = synthesize(&url, "test-key", "cosyvoice", &p, "你好")
            .await
            .unwrap();
        assert_eq!(audio.bytes, b"RIFF-test");
        let request = server.join().unwrap();
        let body = body(&request);
        assert_eq!(body["input"]["rate"], 1.2);
        assert_eq!(body["input"]["instruction"], p.instructions);
        let request = download.join().unwrap().to_lowercase();
        assert!(!request.contains("authorization"));
        assert!(!request.contains("test-key"));
    }
    #[tokio::test]
    async fn minimax_decodes_hex_and_obeys_actual_metadata() {
        let (url, server) = mock("200 OK", "Content-Type: application/json\r\n", json!({"output":{"base_resp":{"status_code":0},"data":{"audio":"52494646"},"extra_info":{"audio_format":"wav","audio_sample_rate":32000,"audio_channel":2}}}).to_string().into_bytes());
        let mut p = profile("minimax");
        p.emotion = "happy".into();
        p.pitch = -2.0;
        let audio = synthesize(&url, "test-key", "minimax", &p, "你好")
            .await
            .unwrap();
        assert_eq!(audio.bytes, b"RIFF");
        assert_eq!(audio.sample_rate, 32000);
        assert_eq!(audio.channels, 2);
        let request = server.join().unwrap();
        let body = body(&request);
        assert_eq!(body["input"]["output_format"], "hex");
        assert_eq!(body["input"]["voice_setting"]["pitch"], -2);
        assert_eq!(body["input"]["voice_setting"]["emotion"], "happy");
        assert!(body["input"].get("instructions").is_none());
    }
    #[tokio::test]
    async fn provider_failure_with_http_success_and_redirects_are_errors_without_secrets() {
        for (status, headers, body) in [
            (
                "200 OK",
                "",
                json!({"output":{"base_resp":{"status_code":1004,"status_msg":"test-key"}}})
                    .to_string(),
            ),
            (
                "302 Found",
                "Location: http://127.0.0.1:1/leak\r\n",
                "test-key".into(),
            ),
            ("401 Unauthorized", "", "test-key".into()),
        ] {
            let (url, server) = mock(status, headers, body.into_bytes());
            let error = synthesize(&url, "test-key", "minimax", &profile("minimax"), "你好")
                .await
                .unwrap_err();
            assert!(!error.contains("test-key"));
            server.join().unwrap();
        }
    }
    #[tokio::test]
    async fn audio_download_size_limit_is_enforced() {
        let (url, server) = mock("200 OK", "", vec![1; 16]);
        let response = Client::new().get(url).send().await.unwrap();
        assert!(read_bounded(response, 8)
            .await
            .unwrap_err()
            .contains("过大"));
        server.join().unwrap();
    }

    #[test]
    fn cosy_system_instructions_match_voice_and_model_specific_templates() {
        let mut p = profile("cosyvoice");
        for text in [
            "",
            "你说话的情感是neutral。",
            "你正在进行闲聊互动，你说话的情感是happy。",
            "你现在说话的角色是一个旁白，你说话的情感是sad。",
            "你正在以一个故事机的身份说话，你说话的情感是neutral。",
        ] {
            p.instructions = text.into();
            assert!(validate_cosy_instruction(&p).is_ok(), "{text}");
        }
        for text in [
            "冷静",
            "你说话的情感是calm。",
            "你说话的情感是happy",
            "你说话的角色是一个旁白，你说话的情感是sad。",
        ] {
            p.instructions = text.into();
            assert!(validate_cosy_instruction(&p).is_err(), "{text}");
        }
        p.voice = "longanhuan".into();
        p.instructions = "你正在进行剧情解说，你说话的情感是neutral。".into();
        assert!(validate_cosy_instruction(&p).is_ok());
        p.model = "cosyvoice-v3-plus".into();
        assert!(validate_cosy_instruction(&p).is_err());
        p.instructions = "你说话的角色是温和客服，你说话的情感是neutral。".into();
        assert!(validate_cosy_instruction(&p).is_ok());
        p.model = "cosyvoice-v3-flash".into();
        p.voice = "longanhuan_v3".into();
        assert!(validate_cosy_instruction(&p).is_err());
        p.instructions = "请用河南话表达。".into();
        assert!(validate_cosy_instruction(&p).is_ok());
        p.voice = "cosyvoice-v3-flash-linger-test".into();
        p.instructions = "用沉稳自然的语气说话".into();
        assert!(validate_cosy_instruction(&p).is_ok());
    }

    #[tokio::test]
    async fn invalid_system_instruction_fails_before_network_request() {
        let mut p = profile("cosyvoice");
        p.instructions = "冷静".into();
        let error = synthesize("http://127.0.0.1:1", "test-key", "cosyvoice", &p, "你好")
            .await
            .unwrap_err();
        assert!(error.contains("固定格式"));
    }

    #[test]
    fn rejects_malformed_audio_and_unsupported_controls() {
        for value in ["", "1", "zz", "你"] {
            assert!(decode_hex(value).is_err());
        }
        let mut p = profile("minimax");
        p.instructions = "温柔".into();
        assert!(p.validate("minimax").is_err());
        p.instructions.clear();
        p.emotion = "whisper".into();
        assert!(p.validate("minimax").is_err());
        let mut p = profile("cosyvoice");
        p.instructions = "汉".repeat(51);
        assert!(p.validate("cosyvoice").is_err());
        p.instructions.clear();
        p.model = "cosyvoice-v3.5-flash".into();
        assert!(p.validate("cosyvoice").is_err());
        p.voice = "cosyvoice-custom".into();
        assert!(p.validate("cosyvoice").is_ok());
    }
}
