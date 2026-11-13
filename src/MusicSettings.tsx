import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

type BridgeInfo = { enabled: boolean; connected: boolean; port: number; token?: string; busy?: boolean };
type Candidate = { video_id: string; title: string; artist?: string; detail?: string; duration?: string };
const errorText = (error: unknown) => error instanceof Error ? error.message : String(error);

export function MusicSettings({ desktop }: { desktop: boolean }) {
  const [info, setInfo] = useState<BridgeInfo>({ enabled: false, connected: false, port: 47831 });
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [busy, setBusy] = useState(false);
  const [query, setQuery] = useState("");
  const [candidates, setCandidates] = useState<Candidate[]>([]);
  const [result, setResult] = useState<unknown>(null);
  const refresh = useCallback(async () => {
    if (desktop) setInfo(await invoke<BridgeInfo>("get_music_bridge_info"));
  }, [desktop]);
  useEffect(() => {
    let alive = true;
    let pending = false;
    const tick = async () => {
      if (!desktop || pending) return;
      pending = true;
      try {
        const value = await invoke<BridgeInfo>("get_music_bridge_info");
        if (alive) setInfo(value);
      } catch (e) { if (alive) setError(errorText(e)); }
      finally { pending = false; }
    };
    void tick();
    const timer = window.setInterval(() => void tick(), 3000);
    return () => { alive = false; window.clearInterval(timer); };
  }, [desktop]);
  async function enable() {
    setBusy(true); setError(""); setNotice("");
    try {
      setInfo(await invoke<BridgeInfo>("set_music_bridge_enabled", { enabled: !info.enabled }));
      setCandidates([]); setResult(null);
    } catch (e) { setError(errorText(e)); }
    finally { setBusy(false); }
  }
  async function command(action: string, args: Record<string, string> = {}) {
    setBusy(true); setError(""); setNotice("");
    if (action === "search") setCandidates([]);
    try {
      const value = await invoke<Record<string, unknown>>("music_command", { action, args });
      setResult(value);
      if (action === "search") setCandidates((value.candidates || []) as Candidate[]);
      await refresh();
    } catch (e) { setError(errorText(e)); }
    finally { setBusy(false); }
  }
  return <section className="music-settings">
    <div className="section-intro"><h2>YouTube Music</h2><p>通过已登录的 Chrome / Edge 标签页播放音乐，不切换当前窗口。</p></div>
    <div className="music-controls">
      <button type="button" className="secondary-button" disabled={!desktop} onClick={() => void invoke("open_music_extension_folder").catch(e => setError(errorText(e)))}>打开扩展文件夹</button>
      <button type="button" className="secondary-button" disabled={!desktop || busy} onClick={() => void enable()}>{info.enabled ? "关闭音乐连接" : "启用音乐连接"}</button>
      <span role="status">{info.connected ? "浏览器已连接" : info.enabled ? "等待扩展连接" : "未启用"}</span>
    </div>
    <ol className="music-instructions">
      <li>点击“打开扩展文件夹”，在 Chrome / Edge 的扩展管理页开启开发者模式，用“加载已解压的扩展程序”选择这个文件夹。</li>
      <li>启用连接，复制配对码到扩展。打开并登录 YouTube Music，在扩展里选择需要控制的标签页。</li>
      <li>连接后即可说“播放一首王菲的歌”“暂停音乐”“下一首”。首次播放若被浏览器拦截，请在音乐网页手动播放一次。</li>
    </ol>
    {info.enabled && <>
      <div className="music-controls">
        <label>配对码 <input type="password" readOnly value={info.token || ""} autoComplete="off" aria-label="音乐扩展配对码" /></label>
        <button type="button" className="secondary-button" onClick={() => {
          if (!info.token) return;
          void invoke("copy_text", { text: info.token }).then(() => setNotice("配对码已复制")).catch(e => setError(errorText(e)));
        }}>复制配对码</button>
        <span>端口 {info.port}</span>
      </div>
      <p className="subtitle">配对保存在本机；关闭连接停止接收新指令，已发出的操作可能仍会完成。请保持绑定标签页打开，浏览器重启后在扩展中重新选择标签页。</p>
      <div className="music-controls">
        <input value={query} maxLength={200} onChange={e => setQuery(e.target.value)} placeholder="歌名 / 歌手" aria-label="搜索音乐" onKeyDown={e => { if (e.key === "Enter") { e.preventDefault(); if (!busy && info.connected && query.trim()) void command("search", { query: query.trim() }); } }} />
        <button type="button" className="secondary-button" disabled={busy || !info.connected || !query.trim()} onClick={() => void command("search", { query: query.trim() })}>搜索</button>
      </div>
      <div className="music-controls">{[["status", "播放状态"], ["pause", "暂停"], ["resume", "继续"], ["next", "下一首"]].map(([action, title]) => <button type="button" className="secondary-button" key={action} disabled={busy || !info.connected} onClick={() => void command(action)}>{title}</button>)}</div>
      {candidates.map(item => <div className="music-candidate" key={item.video_id}>
        <div><strong>{item.title}</strong><small>{item.artist || item.detail} {item.duration}</small></div>
        <button type="button" className="secondary-button" disabled={busy || !info.connected} onClick={() => void command("play", { video_id: item.video_id })}>播放</button>
        <button type="button" className="secondary-button" disabled={busy || !info.connected} onClick={() => void command("queue", { video_id: item.video_id })}>加入队列</button>
      </div>)}
      {result !== null && <details><summary>调试返回</summary><pre className="music-result">{JSON.stringify(result, null, 2)}</pre></details>}
    </>}
    {busy && <p role="status">正在等待浏览器确认…</p>}
    {notice && <p role="status">{notice}</p>}
    {error && <p role="alert">{error}</p>}
  </section>;
}
