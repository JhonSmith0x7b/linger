import { useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { VoiceClone } from "./VoiceClone";
import { CosyInstructions } from "./CosyInstructions";
import { cosyInstructionSpec, parseCosyInstruction } from "./cosyInstructionOptions";
import type { CharacterTts, Settings, TtsProfile, TtsProvider } from "./types";
import { cosyEndpointHint, defaultTtsProfile, minimaxEndpoint, supportsInstructions, ttsEndpoint, ttsModels, ttsProviders, ttsVoices } from "./tts";

function Field({ label, help, children }: { label: string; help?: string; children: ReactNode }) {
  return <label className="field"><span>{label}</span>{children}{help && <small>{help}</small>}</label>;
}

export function TtsConnections({ settings, onChange, ready, report }: {
  settings: Settings; onChange: (value: Settings["tts_connections"]) => void; ready: boolean; report: (error: unknown) => void;
}) {
  const [provider, setProvider] = useState<TtsProvider>("cosyvoice");
  const [key, setKey] = useState("");
  const [saved, setSaved] = useState(false);
  const [saving, setSaving] = useState(false);
  const endpoint = ttsEndpoint(settings, provider);
  const inherited = provider === "openai" && !endpoint.trim();
  useEffect(() => {
    let current = true;
    setSaved(false);
    let timer: ReturnType<typeof setTimeout> | undefined;
    if (ready && !inherited && endpoint.trim()) {
      try {
        const url = new URL(endpoint);
        if (["https:", "http:"].includes(url.protocol) && !url.username && !url.password && !url.search && !url.hash) {
          timer = setTimeout(() => {
            void invoke<boolean>("tts_key_status", { provider, endpoint }).then(value => { if (current) setSaved(value); }).catch(error => { if (current) report(error); });
          }, 400);
        }
      } catch { /* Incomplete URI while typing. Save and audition validate the full address. */ }
    }
    return () => { current = false; clearTimeout(timer); };
  }, [provider, endpoint, inherited, ready, report]);
  async function saveKey() {
    setSaving(true);
    try {
      await invoke("save_tts_key", { provider, endpoint, key: key.trim() });
      setKey(""); setSaved(true);
    } catch (error) { report(error); }
    finally { setSaving(false); }
  }
  return <>
    <div className="section-intro"><h2>独立的语音连接</h2><p>这里只配置地址与密钥。模型、音色和表达方式在角色卡里选择。</p></div>
    <Field label="语音服务"><select aria-label="语音服务" value={provider} disabled={saving} onChange={event => { setProvider(event.target.value as TtsProvider); setKey(""); }}>
      {ttsProviders.map(item => <option key={item.id} value={item.id}>{item.name}</option>)}
    </select></Field>
    <Field label="语音 API 完整 URI" help={provider === "openai" ? "留空沿用聊天连接与密钥；独立地址需填完整 /audio/speech 路径，并单独保存密钥。" : provider === "cosyvoice" ? "填写业务空间专属完整地址，并使用该地域的百炼 API Key；不要保留示例中的业务空间 ID。" : "这是阿里云百炼的 MiniMax 接口，使用百炼 API Key。"}>
      <input aria-label="语音 API 完整 URI" type="url" value={endpoint} disabled={saving} onChange={event => { onChange({ ...settings.tts_connections, [provider]: { endpoint: event.target.value } }); setKey(""); }} placeholder={provider === "cosyvoice" ? cosyEndpointHint : provider === "minimax" ? minimaxEndpoint : "留空沿用聊天连接"} />
    </Field>
    {!inherited && <>
      <Field label={`语音 API Key · ${saved ? "已保存" : "尚未设置"}`} help="密钥独立保存在系统凭据存储中，按服务和完整地址区分；不写入角色卡。">
        <input aria-label="语音 API Key" type="password" autoComplete="off" value={key} disabled={saving} onChange={event => setKey(event.target.value)} placeholder={saved ? "留空保留当前密钥" : "输入百炼或对应服务的 API Key"} />
      </Field>
      <button type="button" className="secondary-button" disabled={!ready || saving || !key.trim() || !endpoint.trim()} onClick={() => void saveKey()}>{saving ? "正在保存密钥…" : "保存此地址的密钥"}</button>
    </>}
    <p className="region-hint">密钥立即保存；连接地址点击下方“保存设置”后用于正式对话。在角色卡中试听可验证当前地址、密钥和模型是否可用。</p>
  </>;
}

export function CharacterVoiceEditor({ value, onChange, settings, characterId, characterName, models, ready, report }: {
  value: CharacterTts; onChange: (value: CharacterTts) => void; settings: Settings; characterId: string; characterName: string;
  models: string[]; ready: boolean; report: (error: unknown) => void;
}) {
  const provider = value.provider;
  const profile = value.profiles[provider] ?? defaultTtsProfile(provider, settings, characterId);
  const endpoint = ttsEndpoint(settings, provider);
  const voices = ttsVoices(provider, profile.model);
  const [customVoice, setCustomVoice] = useState(!voices.some(voice => voice.id === profile.voice));
  const [text, setText] = useState("今天过得怎么样？你忙你的，我会在这里陪着你。有什么想说的，随时告诉我。");
  const [testing, setTesting] = useState(false);
  const testInFlight = useRef(false);
  const [timing, setTiming] = useState<string>("");
  const modelOptions = Array.from(new Set([...ttsModels[provider], ...(provider === "openai" && !endpoint.trim() ? models.filter(model => /tts|speech/i.test(model)) : []), profile.model]));
  const instructionEnabled = supportsInstructions(provider, profile);
  const customCosy = profile.voice.startsWith("cosyvoice-");
  const instructionSpec = provider === "cosyvoice" ? cosyInstructionSpec(profile.model, profile.voice) : undefined;
  const invalidSystemInstructions = !!instructionSpec && !parseCosyInstruction(instructionSpec, profile.instructions);
  const change = (patch: Partial<TtsProfile>) => {
    setTiming(""); onChange({ ...value, profiles: { ...value.profiles, [provider]: { ...profile, ...patch } } });
  };
  function selectProvider(next: TtsProvider) {
    const nextProfile = value.profiles[next] ?? defaultTtsProfile(next, settings, characterId);
    setCustomVoice(!ttsVoices(next, nextProfile.model).some(voice => voice.id === nextProfile.voice));
    setTiming("");
    onChange({ provider: next, profiles: { ...value.profiles, [provider]: profile, [next]: nextProfile } });
  }
  function selectModel(model: string) {
    const available = ttsVoices(provider, model);
    let next = { ...profile, model };
    if (provider === "openai" && ["tts-1", "tts-1-hd"].includes(model) && !available.some(voice => voice.id === profile.voice)) next = { ...next, voice: available[0].id };
    // Cloned voices belong to their creation model and cannot follow a model switch.
    if (provider === "cosyvoice" && ((customCosy && model !== profile.model) || !available.some(voice => voice.id === profile.voice))) next = { ...next, voice: available[0]?.id ?? "", instructions: "" };
    if (!supportsInstructions(provider, next)) next.instructions = "";
    const nextSpec = provider === "cosyvoice" ? cosyInstructionSpec(next.model, next.voice) : undefined;
    if (nextSpec && !parseCosyInstruction(nextSpec, next.instructions)) next.instructions = "";
    if (provider === "minimax" && model.includes("2.8") && next.emotion === "whisper") next.emotion = "";
    setCustomVoice(!available.some(voice => voice.id === next.voice));
    change(next);
  }
  async function audition() {
    if (testInFlight.current) return;
    testInFlight.current = true; setTesting(true); setTiming("");
    try {
      const result = await invoke<{ generation_ms: number }>("test_voice", { ttsProvider: provider, ttsProfile: profile, ttsEndpoint: endpoint, text });
      setTiming(`${ttsProviders.find(item => item.id === provider)?.name} · ${profile.model} · 生成耗时 ${(result.generation_ms / 1000).toFixed(2)} 秒`);
    } catch (error) { report(error); }
    finally { testInFlight.current = false; setTesting(false); }
  }
  return <div className="character-voice-editor">
    <div className="section-intro"><h2>{characterName} · 声音</h2><p>每张角色卡分别保存各服务的声音方案，切换后仍会保留之前的调整。</p></div>
    <div className="field-grid">
      <Field label="角色语音服务"><select aria-label="角色语音服务" value={provider} disabled={testing} onChange={event => selectProvider(event.target.value as TtsProvider)}>{ttsProviders.map(item => <option key={item.id} value={item.id}>{item.name}</option>)}</select></Field>
      <Field label="语音模型"><select aria-label="语音模型" value={profile.model} onChange={event => selectModel(event.target.value)}>{modelOptions.map(model => <option key={model}>{model}</option>)}</select></Field>
    </div>
    <Field label="音色 voice" help={provider === "cosyvoice" && profile.model.startsWith("cosyvoice-v3.5") ? "v3.5 只支持复刻或设计音色，请填与模型匹配的音色 ID。" : "可选择内置音色，也可填写其他官方音色或已创建的复刻音色 ID。"}>
      <select aria-label="音色 voice" value={customVoice || !voices.some(voice => voice.id === profile.voice) ? "__custom" : profile.voice} onChange={event => {
        const custom = event.target.value === "__custom"; setCustomVoice(custom);
        if (!custom) change({ voice: event.target.value, instructions: provider === "cosyvoice" ? "" : profile.instructions });
      }}>
        {voices.map(voice => <option key={voice.id} value={voice.id}>{voice.name}</option>)}<option value="__custom">自定义音色 ID</option>
      </select>
    </Field>
    {(customVoice || !voices.some(voice => voice.id === profile.voice)) && <Field label="自定义音色 ID"><input aria-label="自定义音色 ID" required value={profile.voice} onChange={event => {
      const next = { ...profile, voice: event.target.value };
      change({ voice: event.target.value, ...(!supportsInstructions(provider, next) ? { instructions: "" } : {}) });
    }} placeholder="粘贴与当前模型匹配的音色 ID" /></Field>}
    {provider === "cosyvoice" && <VoiceClone key={`${endpoint}::${profile.model}`} endpoint={endpoint} targetModel={profile.model} selectedVoice={profile.voice} ready={ready} onApply={voiceId => {
      setCustomVoice(true); change({ voice: voiceId, instructions: "" });
    }} />}
    <div className="field-grid">
      <Field label="语速 speed" help={provider === "openai" ? "1.0 为正常语速，范围 0.25–4.0。" : "1.0 为正常语速，范围 0.5–2.0。"}><input aria-label="语速 speed" type="number" required min={provider === "openai" ? 0.25 : 0.5} max={provider === "openai" ? 4 : 2} step="0.05" value={profile.speed} onChange={event => change({ speed: Number(event.target.value) })} /></Field>
      {provider !== "openai" && <Field label="音调 pitch" help={provider === "cosyvoice" ? "1.0 为原始音调，范围 0.5–2.0。" : "0 为原始音调，范围 −12 至 12。"}><input aria-label="音调 pitch" type="number" required min={provider === "cosyvoice" ? 0.5 : -12} max={provider === "cosyvoice" ? 2 : 12} step={provider === "cosyvoice" ? 0.05 : 1} value={profile.pitch} onChange={event => change({ pitch: Number(event.target.value) })} /></Field>}
      {provider === "minimax" && <Field label="情绪 emotion"><select aria-label="情绪 emotion" value={profile.emotion} onChange={event => change({ emotion: event.target.value })}>{[["", "根据文本自动判断"], ["happy", "高兴"], ["sad", "悲伤"], ["angry", "愤怒"], ["fearful", "害怕"], ["disgusted", "厌恶"], ["surprised", "惊讶"], ["calm", "平静"], ...(!profile.model.includes("2.8") ? [["whisper", "低语"]] : [])].map(([id, name]) => <option key={id} value={id}>{name}</option>)}</select></Field>}
    </div>
    {instructionSpec ? <CosyInstructions spec={instructionSpec} instructions={profile.instructions} onChange={instructions => change({ instructions })} /> : instructionEnabled ? <Field label="角色声音风格 instructions" help={provider === "cosyvoice" ? customCosy ? "描述语气与表达方式；汉字按 2 个字符计数，总计不超过 100。" : "系统音色按官方固定格式填写，例如：你说话的情感是happy。汉字按 2 个字符计数，总计不超过 100。其他音色请核对其指令支持情况。" : "描述这个角色的语气、情绪和表达节奏。"}>
      <textarea aria-label="角色声音风格 instructions" rows={3} value={profile.instructions} onChange={event => change({ instructions: event.target.value })} />
    </Field> : <p className="region-hint">当前模型 / 音色不使用自然语言风格指令。{provider === "minimax" ? "请通过语速、音调和情绪调整表现。" : "可使用语速等参数调整表现。"}</p>}
    <details className="advanced"><summary>声音输出参数</summary><div className="field-grid">
      <Field label="音频格式"><select aria-label="音频格式" value={profile.format} onChange={event => change({ format: event.target.value })}>{(provider === "openai" ? ["wav", "mp3", "pcm", "aac", "flac"] : ["wav", "mp3"]).map(format => <option key={format}>{format}</option>)}</select></Field>
      {provider !== "openai" && <Field label="采样率"><select aria-label="采样率" value={profile.sample_rate} onChange={event => change({ sample_rate: Number(event.target.value) })}>{(provider === "cosyvoice" ? [8000, 12000, 16000, 22050, 24000, 44100, 48000] : [8000, 16000, 22050, 24000, 32000, 44100]).map(rate => <option key={rate} value={rate}>{rate} Hz</option>)}</select></Field>}
    </div></details>
    <Field label="试听文本"><textarea aria-label="试听文本" rows={3} maxLength={1000} value={text} onChange={event => setText(event.target.value)} /></Field>
    <div className="voice-actions">
      <button type="button" className="secondary-button" disabled={!ready || testing || !text.trim() || !profile.voice.trim() || invalidSystemInstructions} onClick={() => void audition()}>{testing ? "正在试听…" : "试听当前角色声音"}</button>
      {testing && <button type="button" className="text-button" onClick={() => void invoke("interrupt").catch(report)}>停止试听</button>}
      <button type="button" className="text-button" disabled={testing} onClick={() => {
        const next = defaultTtsProfile(provider, { ...settings, character_voices: {} }, characterId);
        change(next); setCustomVoice(!ttsVoices(provider, next.model).some(voice => voice.id === next.voice));
      }}>恢复此服务的默认声音</button>
    </div>
    {timing && <p className="tts-timing" role="status">{timing}</p>}
    <p className="region-hint">试听使用当前未保存的声音方案与连接地址，以及该地址已保存的密钥；不会更改当前角色。正式对话需点击“保存设置”后生效。</p>
  </div>;
}
