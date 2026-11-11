import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export interface LibraryVoice {
  voice_id: string;
  name: string;
  target_model: string;
  status: string;
  created_at: string;
  in_use_by: string[];
}
const characterNames: Record<string, string> = { custom: "自定义角色", haruhi: "凉宫春日", yukino: "雪之下雪乃", fujiko: "峰不二子", melody: "泽音", shiori: "栞", koharu: "小晴", rin: "凛" };
const statusNames: Record<string, string> = { OK: "可使用", DEPLOYING: "处理中", UNDEPLOYED: "审核未通过", UNKNOWN: "尚未查询" };
function errorText(error: unknown) { return error instanceof Error ? error.message : String(error); }

export function VoiceLibrary({ endpoint, targetModel, selectedVoice, ready, refreshKey, onApply, onDeleted, onRenamed }: {
  endpoint: string; targetModel: string; selectedVoice: string; ready: boolean; refreshKey: number;
  onApply: (voiceId: string) => void; onDeleted: (voiceId: string) => void; onRenamed: (voiceId: string, name: string) => void;
}) {
  const [voices, setVoices] = useState<LibraryVoice[]>([]);
  const [loading, setLoading] = useState(false);
  const [busyId, setBusyId] = useState("");
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [editingId, setEditingId] = useState("");
  const [name, setName] = useState("");
  const [deletingId, setDeletingId] = useState("");
  const callbacks = useRef({ onApply, onDeleted, onRenamed });
  callbacks.current = { onApply, onDeleted, onRenamed };
  const mounted = useRef(false);
  const inFlight = useRef(false);
  const listRequest = useRef(0);
  const selected = voices.find(voice => voice.voice_id === selectedVoice);

  const load = useCallback(async (refreshCloud = false) => {
    if (!ready || !endpoint.trim()) return;
    const request = ++listRequest.current;
    setLoading(true);
    try {
      const records = await invoke<LibraryVoice[]>("list_reference_voices", { endpoint, refreshCloud });
      if (mounted.current && request === listRequest.current) setVoices(records);
    } catch (cause) { if (mounted.current && request === listRequest.current) setError(errorText(cause)); }
    finally { if (mounted.current && request === listRequest.current) setLoading(false); }
  }, [endpoint, ready]);
  useEffect(() => {
    mounted.current = true;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen("linger:settings", () => { void load(); }).then(stop => {
      if (disposed) stop(); else unlisten = stop;
    }).catch(cause => { if (!disposed) setError(errorText(cause)); });
    return () => { disposed = true; mounted.current = false; listRequest.current += 1; unlisten?.(); };
  }, [load]);
  useEffect(() => { void load(); }, [load, refreshKey]);

  async function act(voice: LibraryVoice, action: "apply" | "query" | "rename" | "delete") {
    if (inFlight.current) return;
    if (action === "delete" && (voice.voice_id === selectedVoice || voice.in_use_by.length)) return;
    if (action === "apply" && voice.target_model !== targetModel) return;
    const nextName = name.trim();
    if (action === "rename" && (!nextName || Array.from(nextName).length > 60)) { setError("音色名称请填写 1–60 个字符。"); return; }
    inFlight.current = true; setBusyId(voice.voice_id); setError(""); setNotice("");
    // Discard an older list request that could overwrite a successful mutation.
    listRequest.current += 1; setLoading(false);
    try {
      if (action === "rename") {
        await invoke("rename_reference_voice", { endpoint, voiceId: voice.voice_id, name: nextName });
        if (!mounted.current) return;
        setVoices(previous => previous.map(item => item.voice_id === voice.voice_id ? { ...item, name: nextName } : item));
        setEditingId(""); callbacks.current.onRenamed(voice.voice_id, nextName); setNotice("名称已保存到本地音色库。");
      } else if (action === "delete") {
        await invoke("delete_reference_voice", { endpoint, voiceId: voice.voice_id });
        if (!mounted.current) return;
        setVoices(previous => previous.filter(item => item.voice_id !== voice.voice_id));
        setDeletingId(""); callbacks.current.onDeleted(voice.voice_id); setNotice(`已删除音色“${voice.name}”。`);
      } else {
        const result = await invoke<{ voice_id: string; status: string; target_model: string | null }>("query_reference_voice", { endpoint, voiceId: voice.voice_id });
        if (!mounted.current) return;
        if (result.voice_id !== voice.voice_id || (result.target_model && result.target_model !== voice.target_model)) throw new Error("音色或模型信息不匹配，请刷新云端音色后重试。");
        setVoices(previous => previous.map(item => item.voice_id === voice.voice_id ? { ...item, status: result.status } : item));
        if (action === "apply") {
          if (result.status !== "OK") throw new Error(`音色暂不可使用：${statusNames[result.status] ?? result.status}。`);
          callbacks.current.onApply(voice.voice_id); setNotice(`已选择“${voice.name}”，可以试听，保存设置后用于正式对话。`);
        } else setNotice(`“${voice.name}”：${statusNames[result.status] ?? result.status}。`);
      }
      await load();
    } catch (cause) { if (mounted.current) setError(errorText(cause)); }
    finally { inFlight.current = false; if (mounted.current) setBusyId(""); }
  }

  return <section className="voice-library" aria-label="我的复刻音色">
    {selected && <p className="region-hint">当前复刻音色：<strong>{selected.name}</strong></p>}
    <details className="advanced">
      <summary>我的复刻音色{voices.length ? ` · ${voices.length}` : ""}</summary>
      <p className="region-hint">名称仅保存在本机。这里管理 Linger 创建或已记录的音色；刷新云端只同步 linger 前缀的音色，不代表账号全部音色。</p>
      <div className="voice-actions"><button type="button" className="secondary-button" disabled={!ready || !endpoint.trim() || loading || !!busyId} onClick={() => { setError(""); void load(true); }}>{loading ? "正在读取音色…" : "刷新云端音色（linger）"}</button></div>
      {!loading && voices.length === 0 && <p className="region-hint">暂无已记录的复刻音色。可以先上传创建，或刷新云端同步。</p>}
      <div className="voice-library-list">
        {voices.map(voice => {
          const inUse = voice.in_use_by.length > 0;
          const active = voice.voice_id === selectedVoice;
          const blockedDelete = active || inUse;
          return <article className="voice-library-item" key={voice.voice_id} aria-label={`音色 ${voice.name}`}>
            <div className="voice-library-title"><strong>{voice.name}</strong>{active && <span>当前选择</span>}</div>
            <p className="region-hint">{voice.target_model} · {statusNames[voice.status] ?? voice.status}</p>
            <details><summary>音色详情</summary><p className="voice-library-id">{voice.voice_id}</p>{voice.created_at && <p className="region-hint">创建于 {voice.created_at}</p>}</details>
            {editingId === voice.voice_id ? <div>
              <label className="field"><span>音色新名称</span><input aria-label={`音色新名称 ${voice.name}`} value={name} maxLength={60} disabled={!!busyId} onChange={event => setName(event.target.value)} /></label>
              <div className="voice-actions"><button type="button" className="secondary-button" disabled={!!busyId || !name.trim()} onClick={() => void act(voice, "rename")}>保存名称</button><button type="button" className="text-button" disabled={!!busyId} onClick={() => setEditingId("")}>取消重命名</button></div>
            </div> : <div className="voice-actions">
              <button type="button" className="secondary-button" disabled={!!busyId || loading || voice.target_model !== targetModel} onClick={() => void act(voice, "apply")}>{active ? "重新使用" : "使用此音色"}</button>
              <button type="button" className="text-button" disabled={!!busyId || loading} onClick={() => void act(voice, "query")}>查询音色状态</button>
              <button type="button" className="text-button" disabled={!!busyId || loading} onClick={() => { setEditingId(voice.voice_id); setName(voice.name); setDeletingId(""); }}>重命名</button>
              <button type="button" className="text-button voice-library-delete" disabled={!!busyId || loading || blockedDelete} onClick={() => setDeletingId(voice.voice_id)}>删除音色</button>
            </div>}
            {voice.target_model !== targetModel && <p className="region-hint">切换到 {voice.target_model} 后可使用。</p>}
            {blockedDelete && <p className="region-hint">{active ? "当前选择的音色不可删除。" : "已被角色卡引用，不可删除。"}{inUse ? `使用中的角色：${voice.in_use_by.map(id => characterNames[id] ?? id).join("、")}。请先更换这些角色卡的音色并保存设置。` : "请先选择其他音色。"}</p>}
            {deletingId === voice.voice_id && !blockedDelete && <div className="voice-library-confirm" role="alert">
              <p>永久删除云端音色“{voice.name}”？删除后无法撤销。</p><p className="voice-library-id">{voice.voice_id}</p>
              <div className="voice-actions"><button type="button" className="secondary-button" disabled={!!busyId || loading || blockedDelete} onClick={() => void act(voice, "delete")}>确认永久删除</button><button type="button" className="text-button" disabled={!!busyId} onClick={() => setDeletingId("")}>取消删除</button></div>
            </div>}
          </article>;
        })}
      </div>
      {error && <p className="model-feedback model-error" role="alert">{error}</p>}
      {notice && <p className="region-hint" role="status">{notice}</p>}
    </details>
  </section>;
}
