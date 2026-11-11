import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { VoiceLibrary } from "./VoiceLibrary";

type CloneJob = { voice_id: string; target_model: string; name?: string; status?: string };
type CloneStatus = { voice_id: string; target_model: string | null; status: string };
const MAX_AUDIO_BYTES = 10 * 1024 * 1024;
const POLL_MS = 4000;
const POLL_LIMIT_MS = 5 * 60 * 1000;

function audioType(file: File): string | null {
  const extension = file.name.split(".").pop()?.toLowerCase();
  return extension === "wav" ? "audio/wav" : extension === "mp3" ? "audio/mpeg" : extension === "m4a" ? "audio/mp4" : null;
}
function readAudio(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(new Error("无法读取参考音频，请重新选择文件。"));
    reader.onabort = () => reject(new Error("读取参考音频已取消。"));
    reader.onload = () => {
      const data = typeof reader.result === "string" ? reader.result.split(",")[1] : "";
      if (data) resolve(data); else reject(new Error("参考音频为空或无法读取。"));
    };
    reader.readAsDataURL(file);
  });
}
function errorText(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

/** The parent keys this component by endpoint + model. Jobs survive remounts in the backend. */
export function VoiceClone({ endpoint, targetModel, selectedVoice, ready, onApply }: {
  endpoint: string; targetModel: string; selectedVoice: string; ready: boolean; onApply: (voiceId: string) => void;
}) {
  const [file, setFile] = useState<File | null>(null);
  const [name, setName] = useState("");
  const nameEdited = useRef(false);
  const [libraryRevision, setLibraryRevision] = useState(0);
  const [job, setJob] = useState<CloneJob | null>(null);
  const [creating, setCreating] = useState(false);
  const [querying, setQuerying] = useState(false);
  const [loading, setLoading] = useState(true);
  const [polling, setPolling] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const mounted = useRef(false);
  const inFlight = useRef(false);
  const deadline = useRef(0);
  const revision = useRef(0);
  const fileInput = useRef<HTMLInputElement>(null);

  useEffect(() => {
    mounted.current = true;
    let current = true;
    if (!ready || !endpoint.trim()) { setLoading(false); return () => { current = false; mounted.current = false; }; }
    setLoading(true);
    void invoke<CloneJob | null>("get_reference_voice", { endpoint, targetModel }).then(saved => {
      if (current) setJob(saved);
    }).catch(cause => { if (current) setError(errorText(cause)); }).finally(() => { if (current) setLoading(false); });
    return () => { current = false; mounted.current = false; revision.current += 1; };
  }, [endpoint, targetModel, ready]);

  async function recover() {
    setLoading(true); setError(""); setPolling(false);
    const request = ++revision.current;
    try {
      const saved = await invoke<CloneJob | null>("get_reference_voice", { endpoint, targetModel });
      if (!mounted.current || request !== revision.current) return;
      setJob(saved); setNotice(saved ? "已找回音色 ID，点击“查询状态”继续。" : "此地址与模型还没有已保存的音色 ID。如果创建仍在进行，请稍后再次找回。");
    } catch (cause) { if (mounted.current && request === revision.current) setError(errorText(cause)); }
    finally { if (mounted.current && request === revision.current) setLoading(false); }
  }

  async function query(currentJob: CloneJob, automatic: boolean) {
    if (inFlight.current) return;
    inFlight.current = true; setQuerying(true); setError("");
    const request = revision.current;
    try {
      const result = await invoke<CloneStatus>("query_reference_voice", { endpoint, voiceId: currentJob.voice_id });
      if (!mounted.current || request !== revision.current) return;
      if (result.voice_id !== currentJob.voice_id || (result.target_model && result.target_model !== targetModel)) {
        setPolling(false); setJob({ ...currentJob, status: "INVALID" }); setError("返回的音色与当前目标模型不一致，不能应用。请切换到创建时使用的模型。"); return;
      }
      const updated = { ...currentJob, status: result.status, target_model: result.target_model ?? currentJob.target_model };
      setJob(previous => previous?.voice_id === updated.voice_id ? { ...updated, name: previous.name } : updated); setLibraryRevision(value => value + 1);
      if (result.status === "OK") { setPolling(false); setNotice("音色已就绪，点击“使用这个音色”后即可试听。"); }
      else if (result.status === "UNDEPLOYED") { setPolling(false); setError("音色审核未通过。请检查录音质量与内容后再创建。"); }
      else if (result.status === "DEPLOYING") {
        if (!automatic) { deadline.current = Date.now() + POLL_LIMIT_MS; setPolling(true); }
        setNotice("音色正在处理，每 4 秒查询一次，最多等待 5 分钟。");
      } else { setPolling(false); setNotice(`服务返回状态：${result.status}。可稍后手动查询。`); }
    } catch (cause) {
      if (mounted.current && request === revision.current) { setPolling(false); setError(`${errorText(cause)} 音色 ID 已保留，可点击“查询状态”重试。`); }
    } finally {
      inFlight.current = false;
      if (mounted.current && request === revision.current) setQuerying(false);
    }
  }
  useEffect(() => {
    if (!polling || !job || creating || querying) return;
    const timer = setTimeout(() => {
      if (Date.now() >= deadline.current) { setPolling(false); setNotice("等待已达 5 分钟，音色 ID 已保留。稍后点击“查询状态”即可继续，无需重新上传。"); }
      else void query(job, true);
    }, POLL_MS);
    return () => clearTimeout(timer);
    // query uses this render's immutable endpoint/model/job snapshot.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [polling, job, creating, querying]);

  async function create() {
    if (inFlight.current || !file) return;
    if (!name.trim() || Array.from(name.trim()).length > 60) { setError("音色名称请填写 1–60 个字符。"); return; }
    const mimeType = audioType(file);
    if (!mimeType || file.size === 0 || file.size > MAX_AUDIO_BYTES) { setError("请选择非空的 WAV、MP3 或 M4A 文件，大小不超过 10 MB。"); return; }
    inFlight.current = true; setCreating(true); setPolling(false); setError(""); setNotice("");
    const request = ++revision.current;
    try {
      const audioBase64 = await readAudio(file);
      if (!mounted.current || request !== revision.current) return;
      const result = await invoke<CloneJob>("create_reference_voice", { endpoint, targetModel, audioBase64, mimeType, name: name.trim() });
      if (!mounted.current || request !== revision.current) return;
      setJob({ ...result, status: "DEPLOYING" }); setLibraryRevision(value => value + 1);
      setFile(null); if (fileInput.current) fileInput.current.value = "";
      deadline.current = Date.now() + POLL_LIMIT_MS; setPolling(true);
      setNotice("音色已创建，正在等待处理。音色 ID 已保存，离开此页后可回来继续查询。");
    } catch (cause) {
      if (mounted.current && request === revision.current) setError(`${errorText(cause)} 未自动重试上传；若服务已受理，请先在百炼控制台确认结果。`);
    } finally {
      inFlight.current = false;
      if (mounted.current && request === revision.current) setCreating(false);
    }
  }

  const validFile = file && audioType(file) && file.size > 0 && file.size <= MAX_AUDIO_BYTES;
  const available = job?.status === "OK" && job.target_model === targetModel;
  return <><details className="advanced">
    <summary>上传参考音频 · 复刻声音</summary>
    <p className="region-hint">选择本人或已获授权的单人录音，建议 10–20 秒、无音乐和明显噪声。支持 WAV / MP3 / M4A，时长 5–60 秒、不超过 10 MB，采样率至少 16 kHz。</p>
    <p className="region-hint">点击创建后会上传至当前配置的阿里云语音服务。此音色绑定 {targetModel}，切换模型需使用对应音色。</p>
    <label className="field"><span>参考音频</span><input ref={fileInput} aria-label="参考音频" type="file" accept=".wav,.mp3,.m4a,audio/wav,audio/mpeg,audio/mp4" disabled={creating} onChange={event => {
      const selected = event.target.files?.[0] ?? null; setFile(selected); setError("");
      if (selected && !nameEdited.current) setName(Array.from(selected.name.replace(/\.[^.]+$/, "").trim()).slice(0, 60).join(""));
      if (selected && (!audioType(selected) || selected.size === 0 || selected.size > MAX_AUDIO_BYTES)) setError("请选择非空的 WAV、MP3 或 M4A 文件，大小不超过 10 MB。");
    }} /></label>
    <label className="field"><span>音色名称</span><input aria-label="音色名称" value={name} maxLength={60} disabled={creating} placeholder="例如：温柔女声、我的旁白" onChange={event => { nameEdited.current = true; setName(event.target.value); }} /><small>支持中文，1–60 个字符；名称仅保存在本机，可随时修改。</small></label>
    <div className="voice-actions">
      <button type="button" className="secondary-button" disabled={!ready || !endpoint.trim() || !validFile || !name.trim() || loading || creating || querying || polling} onClick={() => void create()}>{creating ? "正在上传并创建…" : "上传并创建音色"}</button>
      <button type="button" className="text-button" disabled={!ready || !endpoint.trim() || creating || querying || loading || polling} onClick={() => void recover()}>找回上次创建的音色</button>
      {polling && <button type="button" className="text-button" onClick={() => { setPolling(false); setNotice("已暂停自动查询，云端仍会继续处理。音色 ID 已保留。"); }}>暂停自动查询</button>}
    </div>
    {!endpoint.trim() && <p className="region-hint">请先在“语音连接”中填写 CosyVoice 完整 URI 并保存对应密钥。</p>}
    {loading && <p role="status">正在读取上次创建的音色…</p>}
    {job && <>
      {job.name && <p className="region-hint">上次创建：<strong>{job.name}</strong></p>}
      <label className="field"><span>上次创建的音色 ID · {job.target_model}</span><input aria-label="已创建的音色 ID" readOnly value={job.voice_id} onFocus={event => event.target.select()} /></label>
      <div className="voice-actions">
        <button type="button" className="secondary-button" disabled={!ready || creating || querying || polling || loading} onClick={() => void query(job, false)}>{querying ? "正在查询…" : "查询状态"}</button>
        <button type="button" className="secondary-button" disabled={!available || creating || querying} onClick={() => { onApply(job.voice_id); setNotice("已填入当前角色的音色配置，可先试听，再保存设置。"); }}>使用这个音色</button>
      </div>
    </>}
    {error && <p className="model-feedback model-error" role="alert">{error}</p>}
    {notice && <p className="region-hint" role="status">{notice}</p>}
  </details>
    <VoiceLibrary endpoint={endpoint} targetModel={targetModel} selectedVoice={selectedVoice} ready={ready} refreshKey={libraryRevision} onApply={onApply}
      onRenamed={(voiceId, nextName) => setJob(previous => previous?.voice_id === voiceId ? { ...previous, name: nextName } : previous)}
      onDeleted={voiceId => {
        if (job?.voice_id === voiceId) { revision.current += 1; setPolling(false); setQuerying(false); setJob(null); setNotice(""); }
      }} />
  </>;
}
