import { useCallback, useEffect, useRef, useState } from "react";
import type { FormEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./UserProfileEditor.css";

type UserProfile = { content: string; revision: number };
function message(error: unknown) { return error instanceof Error ? error.message : String(error); }

export function UserProfileEditor({ ready }: { ready: boolean }) {
  const [base, setBase] = useState<UserProfile | null>(null);
  const [latest, setLatest] = useState<UserProfile | null>(null);
  const [draft, setDraft] = useState("");
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const state = useRef<{ base: UserProfile | null; latest: UserProfile | null; draft: string }>({ base: null, latest: null, draft: "" });
  const mounted = useRef(false);
  const epoch = useRef(0);
  const saveInFlight = useRef(false);
  const count = Array.from(draft).length;
  const dirty = base !== null && draft !== base.content;
  const conflict = base !== null && latest !== null && latest.revision > base.revision;

  const replaceDraft = useCallback((profile: UserProfile) => {
    state.current.base = profile; state.current.draft = profile.content;
    setBase(profile); setDraft(profile.content);
  }, []);
  const receive = useCallback((profile: UserProfile) => {
    const current = state.current;
    if (current.latest && profile.revision < current.latest.revision) return;
    current.latest = profile; setLatest(profile);
    // Remote writes may arrive while a manual edit is unsaved. Never silently
    // advance its base revision: the next save must still detect that conflict.
    if (!current.base || current.draft === current.base.content) replaceDraft(profile);
  }, [replaceDraft]);

  useEffect(() => {
    mounted.current = true;
    const generation = ++epoch.current;
    let disposed = false;
    let stop: (() => void) | undefined;
    if (ready) {
      setLoading(true); setError("");
      void (async () => {
        try {
          const unlisten = await listen<UserProfile>("linger:user-profile", ({ payload }) => {
            if (!disposed) receive(payload);
          });
          if (disposed) { unlisten(); return; }
          stop = unlisten;
          const profile = await invoke<UserProfile>("get_user_profile");
          if (!disposed) receive(profile);
        } catch (cause) { if (!disposed) setError(message(cause)); }
        finally { if (!disposed) setLoading(false); }
      })();
    }
    return () => {
      disposed = true; stop?.();
      if (epoch.current === generation) mounted.current = false;
    };
  }, [ready, receive]);

  async function refresh() {
    if (!ready || loading || saveInFlight.current) return;
    const generation = epoch.current;
    setLoading(true); setError(""); setNotice("");
    try {
      const profile = await invoke<UserProfile>("get_user_profile");
      if (mounted.current && generation === epoch.current) receive(profile);
    } catch (cause) { if (mounted.current && generation === epoch.current) setError(message(cause)); }
    finally { if (mounted.current && generation === epoch.current) setLoading(false); }
  }

  async function save(event: FormEvent) {
    event.preventDefault();
    const current = state.current;
    if (!ready || !current.base || saveInFlight.current || count > 2000) return;
    const generation = epoch.current;
    saveInFlight.current = true; setSaving(true); setError(""); setNotice("");
    try {
      const saved = await invoke<UserProfile>("save_user_profile", {
        content: current.draft, expectedRevision: current.base.revision,
      });
      if (!mounted.current || generation !== epoch.current) return;
      replaceDraft(saved);
      if (!state.current.latest || saved.revision >= state.current.latest.revision) {
        state.current.latest = saved; setLatest(saved);
      }
      setNotice(saved.content ? "用户档案已保存，下次交流会使用。" : "用户档案已清空，其他记忆不受影响。");
    } catch (cause) {
      if (!mounted.current || generation !== epoch.current) return;
      setError(message(cause));
      // A missed cross-window event must still surface the newest version after
      // a revision conflict, without discarding the user's unsaved draft.
      try {
        const profile = await invoke<UserProfile>("get_user_profile");
        if (mounted.current && generation === epoch.current) receive(profile);
      } catch { /* Keep the original save error and draft. */ }
    } finally {
      saveInFlight.current = false;
      if (mounted.current && generation === epoch.current) setSaving(false);
    }
  }

  return <section className="user-profile-editor" aria-labelledby="user-profile-heading">
    <div className="user-profile-heading">
      <div><h2 id="user-profile-heading">用户档案</h2><p>告诉她如何与你相处。可直接编辑，也可以对她说“把这个偏好记入我的用户档案”。</p></div>
      <button type="button" className="text-button" disabled={!ready || loading || saving} onClick={() => void refresh()}>{loading ? "加载中…" : "刷新档案"}</button>
    </div>
    <form onSubmit={event => void save(event)}>
      <label htmlFor="user-profile-content">称呼、稳定兴趣与交流偏好（支持 Markdown）</label>
      <textarea id="user-profile-content" value={draft} rows={8} disabled={!ready || !base || saving}
        placeholder={"## 如何称呼我\n\n## 兴趣与偏好\n\n## 相处方式\n喜欢主动分享想法；没有回复通常是在忙，不代表不想听。"}
        onChange={event => { state.current.draft = event.target.value; setDraft(event.target.value); setNotice(""); }} />
      <p className="user-profile-help">每轮陪伴都会参考这份档案。“重新开始对话”会保留它；清空此处并保存可移除档案。最多 2000 字，且须在 1000 token 内，超出时保存会提示。</p>
      {conflict && <div className="user-profile-conflict" role="status">
        <p>档案已被 AI 或其他窗口更新，你的草稿仍保留。请先复制要保留的内容，再加载最新版本合并；直接保存不会覆盖新版本。</p>
        <button type="button" className="secondary-button" disabled={saving} onClick={() => {
          if (state.current.latest) replaceDraft(state.current.latest);
          setError(""); setNotice("已加载最新档案，原草稿已替换。");
        }}>放弃草稿并加载最新档案</button>
      </div>}
      {error && <p className="user-profile-error" role="alert">{error}</p>}
      {notice && <p className="user-profile-notice" role="status">{notice}</p>}
      <div className="user-profile-actions"><small className={count > 2000 ? "user-profile-error" : ""}>{count} / 2000 字{dirty ? " · 未保存" : ""}</small>
        <button className="primary-button" type="submit" disabled={!ready || !base || loading || saving || !dirty || count > 2000}>{saving ? "保存中…" : "保存用户档案"}</button>
      </div>
    </form>
  </section>;
}
