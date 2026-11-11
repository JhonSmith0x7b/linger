//! CosyVoice enrollment. Reference audio is sent once in-memory, never written or logged.
use base64::{engine::general_purpose::STANDARD, Engine};
use reqwest::Url;
use rodio::Source;
use serde::Serialize;
use serde_json::{json, Value};
use std::{io::Cursor, time::Duration};

const MAX_AUDIO_BYTES: usize = 10 * 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 64 * 1024;
const MODELS: [&str; 5] = [
    "cosyvoice-v3.5-plus",
    "cosyvoice-v3.5-flash",
    "cosyvoice-v3-plus",
    "cosyvoice-v3-flash",
    "cosyvoice-v2",
];

#[derive(Debug, Serialize)]
pub struct CloneStatus {
    pub voice_id: String,
    pub status: String,
    pub target_model: Option<String>,
}

pub fn customization_endpoint(endpoint: &str) -> Result<Url, String> {
    crate::settings::validate_tts_endpoint(endpoint)?;
    let mut url = Url::parse(endpoint.trim()).map_err(|_| "CosyVoice 接口地址无效")?;
    if url.path() != "/api/v1/services/audio/tts/SpeechSynthesizer" {
        return Err(
            "音色复刻需要标准 CosyVoice SpeechSynthesizer 完整接口地址，当前路径无法确定复刻接口"
                .into(),
        );
    }
    url.set_path("/api/v1/services/audio/tts/customization");
    Ok(url)
}

fn validate_voice_id(voice_id: &str) -> Result<(), String> {
    if !voice_id.starts_with("cosyvoice-")
        || voice_id.len() > 256
        || !voice_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
    {
        return Err("CosyVoice 音色 ID 格式无效".into());
    }
    Ok(())
}

fn prepare_audio(encoded: &str, mime_type: &str) -> Result<String, String> {
    if encoded.is_empty() || encoded.len() > MAX_AUDIO_BYTES.div_ceil(3) * 4 {
        return Err("参考音频不能为空，且文件不能超过 10 MB".into());
    }
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|_| "参考音频 Base64 数据无效")?;
    if bytes.is_empty() || bytes.len() > MAX_AUDIO_BYTES {
        return Err("参考音频不能为空，且文件不能超过 10 MB".into());
    }
    let mime = match mime_type {
        "audio/wav" | "audio/x-wav" | "audio/wave" => "audio/wav",
        "audio/mpeg" | "audio/mp3" => "audio/mpeg",
        "audio/mp4" | "audio/x-m4a" | "audio/m4a" => "audio/mp4",
        _ => return Err("参考音频仅支持 WAV、MP3 或 M4A".into()),
    };
    let signature_matches = match mime {
        "audio/wav" => bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WAVE"),
        "audio/mpeg" => {
            bytes.starts_with(b"ID3")
                || (bytes.len() > 2 && bytes[0] == 0xff && bytes[1] & 0xe0 == 0xe0)
        }
        "audio/mp4" => bytes.get(4..8) == Some(b"ftyp"),
        _ => false,
    };
    if !signature_matches {
        return Err("参考音频内容与文件格式不符，请重新导出 WAV、MP3 或 M4A".into());
    }
    if mime == "audio/wav" {
        let reader = hound::WavReader::new(Cursor::new(&bytes)).map_err(|_| "参考 WAV 文件无效")?;
        let spec = reader.spec();
        if spec.bits_per_sample != 16 || spec.sample_format != hound::SampleFormat::Int {
            return Err("参考 WAV 必须为 16-bit PCM 格式".into());
        }
    }
    let decoder = rodio::Decoder::new(Cursor::new(bytes))
        .map_err(|_| "无法解码参考音频，请重新导出音频文件")?;
    let channels = decoder.channels();
    let sample_rate = decoder.sample_rate();
    if ![1, 2].contains(&channels) || sample_rate < 16000 {
        return Err("参考音频需要至少 16 kHz 采样率，且为单声道或双声道".into());
    }
    // Count decoded samples so compressed files with absent/inaccurate duration metadata are checked too.
    let max_samples = u64::from(sample_rate) * u64::from(channels) * 60;
    let mut samples = 0_u64;
    for _ in decoder {
        samples += 1;
        if samples > max_samples {
            return Err("参考音频最长为 60 秒，请截取清晰人声片段".into());
        }
    }
    if samples < u64::from(sample_rate) * u64::from(channels) * 5 {
        return Err("参考音频至少需要 5 秒人声，建议使用 10–20 秒的清晰朗读".into());
    }
    Ok(format!("data:{mime};base64,{encoded}"))
}

fn provider_error(value: &Value) -> Option<String> {
    if value.get("code").is_none_or(Value::is_null) && value.get("error").is_none_or(Value::is_null)
    {
        return None;
    }
    let code = value["code"]
        .as_str()
        .or_else(|| value["error"]["code"].as_str())
        .unwrap_or("");
    let reason = match code {
        "InvalidApiKey" | "invalid_api_key" => "API Key 无效",
        "AccessDenied" | "Unauthorized" => "没有模型或业务空间的访问权限",
        "InvalidParameter" | "InvalidParameter.Audio" => "请检查音频规格和目标模型",
        "Throttling" | "Throttling.RateQuota" => "调用受到限流，请稍后查询",
        "QuotaExceeded" => "音色数量或调用配额已用尽",
        _ => "请检查音频、模型权限和连接设置",
    };
    Some(format!("音色复刻服务返回错误：{reason}"))
}

async fn request(endpoint: &str, key: &str, body: Value) -> Result<Value, String> {
    let url = customization_endpoint(endpoint)?;
    if key.trim().is_empty() {
        return Err("请先保存 CosyVoice 连接的 API Key".into());
    }
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(120))
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .build()
        .map_err(|_| "无法创建音色复刻连接")?;
    // Creating a voice is not retried: a lost response may still represent a successful enrollment.
    let mut response = client
        .post(url)
        .bearer_auth(key)
        .json(&body)
        .send()
        .await
        .map_err(|_| "音色服务请求连接失败或超时，未自动重试，请查询确认云端状态")?;
    let status = response.status();
    if response
        .content_length()
        .is_some_and(|n| n > MAX_RESPONSE_BYTES as u64)
    {
        return Err("音色复刻接口响应过大".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "音色复刻响应中断，未自动重试")?
    {
        if chunk.len() > MAX_RESPONSE_BYTES.saturating_sub(bytes.len()) {
            return Err("音色复刻接口响应过大".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    if !status.is_success() {
        let detail =
            provider_error(&value).unwrap_or_else(|| "请检查 Key、接口地址和模型权限".into());
        return Err(format!("音色复刻 API HTTP {}：{detail}", status.as_u16()));
    }
    if let Some(error) = provider_error(&value) {
        return Err(error);
    }
    if !value.is_object() {
        return Err("音色复刻服务返回了无效响应".into());
    }
    Ok(value)
}

pub async fn create(
    endpoint: &str,
    key: &str,
    target_model: &str,
    audio_base64: &str,
    mime_type: &str,
) -> Result<String, String> {
    if !MODELS.contains(&target_model) {
        return Err("请选择支持复刻的 CosyVoice 模型".into());
    }
    customization_endpoint(endpoint)?;
    if audio_base64.is_empty() || audio_base64.len() > MAX_AUDIO_BYTES.div_ceil(3) * 4 {
        return Err("参考音频不能为空，且文件不能超过 10 MB".into());
    }
    let encoded = audio_base64.to_owned();
    let mime = mime_type.to_owned();
    let audio = tokio::task::spawn_blocking(move || prepare_audio(&encoded, &mime))
        .await
        .map_err(|_| "参考音频校验任务失败")??;
    let value = request(
        endpoint,
        key,
        json!({"model":"voice-enrollment","input":{
            "action":"create_voice","target_model":target_model,"prefix":"linger","audio":audio
        }}),
    )
    .await?;
    let voice_id = value["output"]["voice_id"]
        .as_str()
        .ok_or("音色复刻未返回音色 ID，请在百炼控制台查询创建结果")?;
    validate_voice_id(voice_id)?;
    Ok(voice_id.to_owned())
}

pub async fn query(endpoint: &str, key: &str, voice_id: &str) -> Result<CloneStatus, String> {
    validate_voice_id(voice_id)?;
    let value = request(
        endpoint,
        key,
        json!({"model":"voice-enrollment","input":{
            "action":"query_voice","voice_id":voice_id
        }}),
    )
    .await?;
    let status = value["output"]["status"]
        .as_str()
        .ok_or("音色查询响应缺少状态")?;
    if !["DEPLOYING", "OK", "UNDEPLOYED"].contains(&status) {
        return Err("音色查询返回未知状态，请稍后重新查询".into());
    }
    let target_model = match value["output"].get("target_model") {
        None | Some(Value::Null) => None,
        Some(Value::String(model)) if MODELS.contains(&model.as_str()) => Some(model.clone()),
        _ => return Err("查询的音色不属于受支持的 CosyVoice 模型".into()),
    };
    Ok(CloneStatus {
        voice_id: voice_id.to_owned(),
        status: status.to_owned(),
        target_model,
    })
}

#[derive(Debug, Clone)]
pub struct CloudVoice {
    pub voice_id: String,
    pub target_model: String,
    pub status: String,
    pub created_at: String,
}

pub fn model_for_voice(voice_id: &str) -> Option<String> {
    MODELS
        .iter()
        .filter(|model| {
            voice_id
                .strip_prefix(**model)
                .is_some_and(|rest| rest.starts_with('-'))
        })
        .max_by_key(|model| model.len())
        .map(|model| (*model).to_owned())
}

fn parse_voice_page(value: &Value) -> Result<(Vec<CloudVoice>, usize), String> {
    let entries = value["output"]["voice_list"]
        .as_array()
        .ok_or("云端音色列表响应无效")?;
    if entries.len() > 100 {
        return Err("云端音色列表超过分页上限".into());
    }
    let mut voices = Vec::new();
    for entry in entries {
        let Some(id) = entry["voice_id"].as_str() else {
            return Err("云端音色列表缺少音色 ID".into());
        };
        let Some(model) = model_for_voice(id) else {
            continue;
        };
        validate_voice_id(id)?;
        let status = entry["status"]
            .as_str()
            .filter(|s| ["OK", "DEPLOYING", "UNDEPLOYED"].contains(s))
            .unwrap_or("UNKNOWN");
        let created = entry["gmt_create"]
            .as_str()
            .filter(|s| {
                s.len() <= 40
                    && s.chars()
                        .all(|c| c.is_ascii_digit() || "-: T+.Z".contains(c))
            })
            .unwrap_or("");
        voices.push(CloudVoice {
            voice_id: id.to_owned(),
            target_model: model,
            status: status.into(),
            created_at: created.into(),
        });
    }
    Ok((voices, entries.len()))
}

pub async fn list(endpoint: &str, key: &str) -> Result<Vec<CloudVoice>, String> {
    let mut voices = std::collections::BTreeMap::new();
    for page_index in 0..10 {
        let value = request(
            endpoint,
            key,
            json!({"model":"voice-enrollment","input":{
                "action":"list_voice","prefix":"linger","page_size":100,"page_index":page_index
            }}),
        )
        .await?;
        let (page, count) = parse_voice_page(&value)?;
        for voice in page {
            voices.insert(voice.voice_id.clone(), voice);
        }
        if count < 100 {
            break;
        }
    }
    Ok(voices.into_values().collect())
}

pub async fn delete(endpoint: &str, key: &str, voice_id: &str) -> Result<(), String> {
    validate_voice_id(voice_id)?;
    let value = request(
        endpoint,
        key,
        json!({"model":"voice-enrollment","input":{
            "action":"delete_voice","voice_id":voice_id
        }}),
    )
    .await?;
    if !value["output"].is_object() {
        return Err("音色删除未返回成功结果，已保留本地记录，请查询后重试".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn wav(seconds: u32, rate: u32, bits: u16) -> Vec<u8> {
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut writer = hound::WavWriter::new(
                &mut cursor,
                hound::WavSpec {
                    channels: 1,
                    sample_rate: rate,
                    bits_per_sample: bits,
                    sample_format: hound::SampleFormat::Int,
                },
            )
            .unwrap();
            for _ in 0..seconds * rate {
                writer.write_sample(0_i16).unwrap();
            }
            writer.finalize().unwrap();
        }
        cursor.into_inner()
    }
    fn mock_many(
        status: &str,
        bodies: Vec<Value>,
    ) -> (String, std::thread::JoinHandle<Vec<String>>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let status = status.to_owned();
        let handle = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for body in bodies {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let mut request = Vec::new();
                let mut buffer = [0_u8; 8192];
                loop {
                    let n = stream.read(&mut buffer).unwrap();
                    if n == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..n]);
                    if let Some(end) = request.windows(4).position(|b| b == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&request[..end]);
                        let length: usize = headers
                            .lines()
                            .find_map(|line| {
                                line.to_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse().unwrap())
                            })
                            .unwrap_or(0);
                        if request.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                let body = body.to_string();
                write!(stream,"HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
                requests.push(String::from_utf8(request).unwrap());
            }
            requests
        });
        (
            format!("http://{address}/api/v1/services/audio/tts/SpeechSynthesizer"),
            handle,
        )
    }
    fn mock(status: &str, body: Value) -> (String, std::thread::JoinHandle<String>) {
        let (url, handle) = mock_many(status, vec![body]);
        (
            url,
            std::thread::spawn(move || handle.join().unwrap().remove(0)),
        )
    }
    #[tokio::test]
    async fn cloud_library_paginates_and_only_imports_cosyvoice_models() {
        let first: Vec<Value>=(0..100).map(|n|json!({"voice_id":format!("cosyvoice-v3-flash-linger-{n}"),"status":"OK","gmt_create":"2026-11-11 12:00:00"})).collect();
        let (endpoint, server) = mock_many(
            "200 OK",
            vec![
                json!({"output":{"voice_list":first}}),
                json!({"output":{"voice_list":[{"voice_id":"cosyvoice-v3.5-flash-linger-last","status":"DEPLOYING"},{"voice_id":"qwen-audio-3.1-tts-plus-linger-skip","status":"OK"}]}}),
            ],
        );
        let voices = list(&endpoint, "test-key").await.unwrap();
        assert_eq!(voices.len(), 101);
        assert_eq!(
            voices
                .iter()
                .find(|v| v.voice_id.ends_with("-last"))
                .unwrap()
                .target_model,
            "cosyvoice-v3.5-flash"
        );
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 2);
        for (index, request) in requests.iter().enumerate() {
            assert!(request
                .to_lowercase()
                .contains("authorization: bearer test-key"));
            let body: Value =
                serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
            assert_eq!(body["input"]["action"], "list_voice");
            assert_eq!(body["input"]["prefix"], "linger");
            assert_eq!(body["input"]["page_index"], index);
            assert_eq!(body["input"]["page_size"], 100);
        }
    }
    #[tokio::test]
    async fn deletion_targets_only_requested_id_and_requires_success_output() {
        let id = "cosyvoice-v3-flash-linger-one";
        let (endpoint, server) = mock("200 OK", json!({"output":{}}));
        delete(&endpoint, "test-key", id).await.unwrap();
        let request = server.join().unwrap();
        let body: Value = serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(
            body,
            json!({"model":"voice-enrollment","input":{"action":"delete_voice","voice_id":id}})
        );
        let (endpoint, server) = mock("200 OK", json!({}));
        assert!(delete(&endpoint, "test-key", id).await.is_err());
        server.join().unwrap();
        let (endpoint, server) = mock("302 Found", json!({"message":"test-key"}));
        let error = delete(&endpoint, "test-key", id).await.unwrap_err();
        assert!(error.contains("302"));
        assert!(!error.contains("test-key"));
        server.join().unwrap();
    }

    #[test]
    fn endpoint_derivation_preserves_destination_and_rejects_unknown_routes() {
        assert_eq!(customization_endpoint("https://workspace.cn-beijing.maas.aliyuncs.com/api/v1/services/audio/tts/SpeechSynthesizer").unwrap().as_str(), "https://workspace.cn-beijing.maas.aliyuncs.com/api/v1/services/audio/tts/customization");
        for endpoint in [
            "https://example.com/tts",
            "https://example.com/v1/audio/speech",
            "https://user:key@example.com/api/v1/services/audio/tts/SpeechSynthesizer",
        ] {
            assert!(customization_endpoint(endpoint).is_err());
        }
    }
    #[test]
    fn validates_reference_audio_before_upload() {
        let valid = STANDARD.encode(wav(5, 16000, 16));
        assert!(prepare_audio(&valid, "audio/wav")
            .unwrap()
            .starts_with("data:audio/wav;base64,"));
        assert!(prepare_audio(&valid, "audio/mp4").is_err());
        assert!(prepare_audio(&STANDARD.encode(wav(4, 16000, 16)), "audio/wav").is_err());
        assert!(prepare_audio(&STANDARD.encode(wav(5, 8000, 16)), "audio/wav").is_err());
        assert!(prepare_audio(&STANDARD.encode(wav(5, 16000, 24)), "audio/wav").is_err());
        assert!(prepare_audio(&STANDARD.encode(wav(60, 16000, 16)), "audio/wav").is_ok());
        assert!(prepare_audio(&STANDARD.encode(wav(61, 16000, 16)), "audio/wav").is_err());
        assert!(prepare_audio(
            &"a".repeat(MAX_AUDIO_BYTES.div_ceil(3) * 4 + 1),
            "audio/wav"
        )
        .is_err());
        assert!(prepare_audio("not base64", "audio/wav").is_err());
        assert!(prepare_audio(&STANDARD.encode(b"RIFFbad"), "audio/wav").is_err());
    }
    #[tokio::test]
    async fn creates_once_with_inline_audio_and_queries_status_without_audio() {
        let id = "cosyvoice-v3-flash-linger-example";
        let (endpoint, server) = mock("200 OK", json!({"output":{"voice_id":id}}));
        let encoded = STANDARD.encode(wav(5, 16000, 16));
        assert_eq!(
            create(
                &endpoint,
                "test-key",
                "cosyvoice-v3-flash",
                &encoded,
                "audio/wav"
            )
            .await
            .unwrap(),
            id
        );
        let request = server.join().unwrap();
        assert!(request.starts_with("POST /api/v1/services/audio/tts/customization "));
        assert!(request
            .to_lowercase()
            .contains("authorization: bearer test-key"));
        let body: Value = serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(body["model"], "voice-enrollment");
        assert_eq!(body["input"]["action"], "create_voice");
        assert_eq!(body["input"]["target_model"], "cosyvoice-v3-flash");
        assert_eq!(
            body["input"]["audio"],
            format!("data:audio/wav;base64,{encoded}")
        );
        let (endpoint, server) = mock(
            "200 OK",
            json!({"output":{"status":"OK","target_model":"cosyvoice-v3-flash"}}),
        );
        let result = query(&endpoint, "test-key", id).await.unwrap();
        assert_eq!(result.status, "OK");
        assert_eq!(result.voice_id, id);
        assert_eq!(result.target_model.as_deref(), Some("cosyvoice-v3-flash"));
        let request = server.join().unwrap();
        let body: Value = serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(body["input"]["action"], "query_voice");
        assert!(body["input"].get("audio").is_none());
    }
    #[tokio::test]
    async fn handles_pending_rejected_and_business_errors_without_echoing_secrets() {
        for status in ["DEPLOYING", "UNDEPLOYED"] {
            let (endpoint, server) = mock("200 OK", json!({"output":{"status":status}}));
            assert_eq!(
                query(&endpoint, "test-key", "cosyvoice-test")
                    .await
                    .unwrap()
                    .status,
                status
            );
            server.join().unwrap();
        }
        for status in ["200 OK", "400 Bad Request"] {
            let (endpoint, server) = mock(
                status,
                json!({"code":"InvalidParameter","message":"test-key PRIVATE_AUDIO"}),
            );
            let error = query(&endpoint, "test-key", "cosyvoice-test")
                .await
                .unwrap_err();
            assert!(!error.contains("test-key"));
            assert!(!error.contains("PRIVATE_AUDIO"));
            server.join().unwrap();
        }
    }
}
