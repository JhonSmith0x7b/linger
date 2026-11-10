import { useCallback, useEffect, useRef, useState } from "react";
import type { CSSProperties, PointerEvent, ReactNode } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { MarkdownText } from "./MarkdownText";
import type { Message, Settings, Status } from "./types";

type Mode = "orb" | "tools" | "chat" | "translate" | "more" | "bubble";
const phaseNames: Record<string, string> = { idle: "在这里陪你", recording: "正在听 · 再点麦克风发送", transcribing: "正在识别语音", thinking: "正在想一想", generating: "正在回应", observing: "看看你的屏幕", playing: "正在说话", speaking: "正在说话", synthesizing: "准备说给你听", stopped: "已停止", error: "有件事需要留意", tool: "正在使用工具" };

export function FloatingPanel({ settings, messages, streamText, ready, status, error: companionError, onStartRecording, onStopRecording, onToggleObserving, sourceControls, observationBusy, onSetTranslationAutoCopy }: {
  settings: Settings; messages: Message[]; streamText: string; ready: boolean; status: Status; error: string;
  onSetTranslationAutoCopy: (enabled: boolean) => Promise<void>; onStartRecording: () => void; onStopRecording: () => void; onToggleObserving: () => void; sourceControls: ReactNode; observationBusy: boolean;
}) {
  const [mode, setMode] = useState<Mode>("orb");
  const layer = useRef<HTMLDivElement>(null);
  const modeRef = useRef<Mode>("orb");
  const [bubble, setBubble] = useState("");
  const [level, setLevel] = useState(0);
  const [observingPulse, setObservingPulse] = useState(false);
  const observeTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const [text, setText] = useState("");
  const [chatText, setChatText] = useState("");
  const [sending, setSending] = useState(false);
  const chatInput = useRef<HTMLTextAreaElement>(null);
  const chatRevision = useRef(0);
  const sendingRef = useRef(false);
  const [result, setResult] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");
  const [copied, setCopied] = useState(false);
  const request = useRef(0);
  const displayRevision = useRef(0);
  const autoCopy = useRef(settings.translation_auto_copy);
  autoCopy.current = settings.translation_auto_copy;
  const inFlight = useRef(false);
  const output = useRef<HTMLTextAreaElement>(null);
  const translationInput = useRef<HTMLTextAreaElement>(null);
  const bottom = useRef<HTMLDivElement>(null);
  const hideTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const voiceActive = useRef(false);
  const [voiceRecording, setVoiceRecording] = useState(false);
  const [autoCopySaving, setAutoCopySaving] = useState(false);
  const layoutQueue = useRef(Promise.resolve());
  const press = useRef<{ x: number; y: number; dragging: boolean } | null>(null);
  const previousMessage = useRef<number | undefined>(undefined);
  const startRef = useRef(onStartRecording); startRef.current = onStartRecording;
  const stopRef = useRef(onStopRecording); stopRef.current = onStopRecording;
  const length = Array.from(text).length;
  const speaking = ["speaking", "playing", "synthesizing"].includes(status.phase);
  const phase = ["thinking", "generating", "tool"].includes(status.phase) ? "thinking" : speaking ? "speaking" : status.phase;
  const label = phaseNames[status.phase] || status.detail;

  const changeMode = useCallback((next: Mode) => {
    clearTimeout(hideTimer.current);
    if (next === "orb") {
      displayRevision.current += 1; inFlight.current = false;
      setText(""); setResult(""); setCopied(false); setError(""); setPending(false);
      chatRevision.current += 1; setChatText("");
    }
    if (next !== "tools" && voiceActive.current) {
      voiceActive.current = false; setVoiceRecording(false); stopRef.current();
    }
    modeRef.current = next; setMode(next);
    layoutQueue.current = layoutQueue.current.catch(() => {}).then(async () => {
      // Skip queued intermediate hover layouts when a card has already been opened.
      if (modeRef.current !== next) return;
      if (isTauri()) await invoke("compact_set_layout", { mode: next });
    }).catch(() => setError("悬浮界面尺寸调整失败，请重新打开悬浮球。"));
  }, []);
  useEffect(() => {
    if (mode !== "translate" && mode !== "chat" && mode !== "more") return;
    let cancelled = false;
    void layoutQueue.current.then(async () => {
      if (cancelled) return;
      if (isTauri()) await invoke("compact_focus");
      if (!cancelled && mode !== "more") (mode === "chat" ? chatInput : translationInput).current?.focus();
    }).catch(e => setError(String(e)));
    return () => { cancelled = true; };
  }, [mode]);
  useEffect(() => {
    if (!isTauri() || !layer.current) return;
    let frame = 0;
    let disposed = false;
    const publishRegions = () => {
      if (disposed) return;
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        const regions = Array.from(layer.current?.querySelectorAll<HTMLElement>("[data-hit-region]") ?? []).map(element => {
          const rect = element.getBoundingClientRect();
          return { x: rect.x, y: rect.y, width: rect.width, height: rect.height,
            radius: Math.min(parseFloat(getComputedStyle(element).borderRadius) || 0, rect.width / 2, rect.height / 2),
            orb: element.classList.contains("companion-orb") };
        }).filter(region => region.width > 0 && region.height > 0);
        void invoke("compact_set_hit_regions", { regions: { width: innerWidth, height: innerHeight, regions } }).catch(() => {
          if (!disposed) setError("悬浮界面点击区域同步失败，请重新打开悬浮球。");
        });
      });
    };
    const observer = new ResizeObserver(publishRegions);
    observer.observe(layer.current);
    window.addEventListener("resize", publishRegions);
    publishRegions();
    void layoutQueue.current.then(publishRegions);
    return () => { disposed = true; observer.disconnect(); window.removeEventListener("resize", publishRegions); cancelAnimationFrame(frame); };
  }, [mode, speaking]);
  const showBubble = useCallback((message: string) => {
    setBubble(message);
    if (["orb", "tools", "bubble"].includes(modeRef.current) && !press.current && !voiceActive.current) {
      changeMode("bubble");
      hideTimer.current = setTimeout(() => changeMode("orb"), 8000);
    }
  }, [changeMode]);
  const command = async (name: string, args?: Record<string, unknown>) => {
    try { await invoke(name, args); } catch (e) { setError(String(e)); showBubble(String(e)); }
  };
  useEffect(() => {
    document.documentElement.classList.add("orb-document");
    return () => document.documentElement.classList.remove("orb-document");
  }, []);
  useEffect(() => {
    let disposed = false;
    let cleanup: (() => void) | undefined;
    if (isTauri()) void listen<{ level: number }>("linger:audio-level", ({ payload }) => setLevel(Math.max(0, Math.min(1, payload.level)))).then(unlisten => { if (disposed) unlisten(); else cleanup = unlisten; });
    return () => { disposed = true; cleanup?.(); };
  }, []);
  useEffect(() => {
    let disposed = false;
    let cleanup: (() => void) | undefined;
    if (isTauri()) void listen<{ orb: boolean; inside: boolean }>("linger:overlay-hover", ({ payload }) => {
      if (press.current) return;
      if (payload.orb && modeRef.current === "orb") changeMode("tools");
      if (modeRef.current === "tools") {
        clearTimeout(hideTimer.current);
        if (!payload.inside && !voiceActive.current) hideTimer.current = setTimeout(() => changeMode("orb"), 650);
      }
    }).then(unlisten => { if (disposed) unlisten(); else cleanup = unlisten; });
    return () => { disposed = true; cleanup?.(); };
  }, [changeMode]);
  useEffect(() => {
    if (status.phase !== "observing") return;
    setObservingPulse(true);
    clearTimeout(observeTimer.current);
    observeTimer.current = setTimeout(() => setObservingPulse(false), 1300);
  }, [status.phase]);
  useEffect(() => {
    if (status.phase !== "recording") setLevel(0);
    if (["error", "idle", "stopped", "transcribing", "thinking"].includes(status.phase)) {
      voiceActive.current = false; setVoiceRecording(false);
    }
  }, [status.phase]);
  useEffect(() => {
    if (companionError) showBubble(companionError);
  }, [companionError, showBubble]);
  useEffect(() => {
    if (!ready) return;
    const last = messages.at(-1);
    if (previousMessage.current === undefined) { previousMessage.current = last?.id ?? 0; return; }
    if (last && last.id > previousMessage.current) {
      previousMessage.current = last.id;
      if (last.role !== "user") showBubble(last.content);
    }
  }, [messages, ready, showBubble]);
  useEffect(() => {
    request.current += 1; inFlight.current = false; setPending(false); setResult(""); setError(""); setCopied(false);
  }, [settings.translation_model, settings.translation_language]);
  useEffect(() => () => {
    request.current += 1; clearTimeout(hideTimer.current); clearTimeout(observeTimer.current);
    if (voiceActive.current) stopRef.current();
    if (isTauri()) void invoke("compact_set_interacting", { active: false });
  }, []);
  useEffect(() => { if (mode === "chat") bottom.current?.scrollIntoView({ block: "end" }); }, [messages, streamText, mode]);
  useEffect(() => {
    const escape = (event: KeyboardEvent) => { if (event.key === "Escape") { endPress(false); stopRef.current(); changeMode("orb"); } };
    window.addEventListener("keydown", escape);
    return () => window.removeEventListener("keydown", escape);
  }, [changeMode]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    const collapse = () => {
      endPress(false);
      stopRef.current();
      changeMode("orb");
    };
    window.addEventListener("blur", collapse);
    if (isTauri()) void getCurrentWindow().onFocusChanged(({ payload }) => {
      if (!payload) collapse();
    }).then(cleanup => { if (disposed) cleanup(); else unlisten = cleanup; });
    return () => { disposed = true; window.removeEventListener("blur", collapse); unlisten?.(); };
  }, [changeMode]);

  async function sendChat() {
    const submitted = chatText.trim();
    if (!ready || !submitted || sendingRef.current) return;
    const revision = chatRevision.current;
    sendingRef.current = true; setSending(true); setError("");
    try {
      await invoke("send_text", { text: submitted });
      if (chatRevision.current === revision) setChatText("");
    } catch (e) {
      if (modeRef.current === "chat" && chatRevision.current === revision) setError(String(e));
    } finally { sendingRef.current = false; setSending(false); }
  }

  async function translate() {
    if (!ready || autoCopySaving || inFlight.current || !text.trim() || length > 4000) return;
    inFlight.current = true; const id = ++request.current;
    const copyOnComplete = autoCopy.current;
    const display = displayRevision.current;
    setPending(true); setError(""); setResult(""); setCopied(false);
    try {
      const translated = await invoke<string>("translate_text", { text: text.trim() });
      if (id !== request.current) return;
      if (display === displayRevision.current) setResult(translated);
      if (copyOnComplete && autoCopy.current && translated.trim()) {
        try {
          await invoke("copy_text", { text: translated });
          if (id === request.current && display === displayRevision.current) setCopied(true);
        } catch {
          if (id === request.current && display === displayRevision.current) setError("翻译已完成，但自动复制失败，请点击下方复制译文。");
        }
      }
    }
    catch (e) { if (id === request.current && display === displayRevision.current) setError(String(e)); }
    finally { if (id === request.current) { inFlight.current = false; setPending(false); } }
  }
  async function copy() {
    try {
      if (isTauri()) await invoke("copy_text", { text: result }); else await navigator.clipboard.writeText(result);
      setCopied(true); setError("");
    } catch { output.current?.focus(); output.current?.select(); setError("复制未成功，已选中译文，请按 ⌘C / Ctrl+C。"); }
  }
  function endPress(click: boolean) {
    const active = press.current; press.current = null;
    if (active && !active.dragging) void command("compact_set_interacting", { active: false });
    if (click && active && !active.dragging) {
      if (modeRef.current === "chat") { void command("compact_focus").then(() => chatInput.current?.focus()); }
      else changeMode("chat");
    }
  }
  function beginPress(event: PointerEvent<HTMLButtonElement>) {
    if (event.button !== 0 || !ready) return;
    event.preventDefault(); event.currentTarget.setPointerCapture(event.pointerId);
    clearTimeout(hideTimer.current);
    press.current = { x: event.screenX, y: event.screenY, dragging: false };
    void command("compact_set_interacting", { active: true });
  }
  function movePress(event: PointerEvent<HTMLButtonElement>) {
    const active = press.current;
    if (!active || active.dragging || Math.hypot(event.screenX - active.x, event.screenY - active.y) < 6) return;
    active.dragging = true;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
    void command("compact_drag").finally(() => { press.current = null; void command("compact_set_interacting", { active: false }); });
  }
  function leave() {
    if (modeRef.current === "tools" && !press.current && !voiceActive.current) hideTimer.current = setTimeout(() => changeMode("orb"), 650);
  }
  function toggleVoice() {
    clearTimeout(hideTimer.current);
    if (voiceActive.current || status.phase === "recording") {
      if (voiceActive.current) stopRef.current(); else void command("stop_recording");
      voiceActive.current = false; setVoiceRecording(false);
    } else {
      voiceActive.current = true; setVoiceRecording(true);
      startRef.current();
    }
  }
  const card = ["chat", "translate", "more"].includes(mode);
  return <div ref={layer} className={`companion-layer mode-${mode}`} onPointerEnter={() => { if (modeRef.current === "tools") clearTimeout(hideTimer.current); }} onPointerLeave={leave}>
    <button data-hit-region className="orb-translate-shortcut" title="快捷翻译" aria-label="快捷翻译" onClick={() => { if (modeRef.current === "translate") { void command("compact_focus").then(() => translationInput.current?.focus()); } else changeMode("translate"); }}>译</button>
    {mode === "tools" && <span className="orb-status-text">{label}</span>}
    {mode === "tools" && <div data-hit-region className="orb-tools" aria-label="陪伴球操作">

      <button title="最近对话" aria-label="最近对话" onClick={() => changeMode("chat")}><span aria-hidden="true">☰</span></button>
      <button title="更多操作" aria-label="更多操作" onClick={() => changeMode("more")}>···</button>
      <button className={voiceRecording || status.phase === "recording" ? "orb-voice active" : "orb-voice"} disabled={!ready} title={voiceRecording || status.phase === "recording" ? "停止录音并发送" : "开始录音"} aria-label={voiceRecording || status.phase === "recording" ? "停止录音并发送" : "开始录音"} aria-pressed={voiceRecording || status.phase === "recording"} onClick={toggleVoice}>
        {voiceRecording || status.phase === "recording" ? "■" : <svg aria-hidden="true" viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" strokeWidth="1.8"><rect x="9" y="3" width="6" height="12" rx="3" /><path d="M6 11v1a6 6 0 0 0 12 0v-1M12 18v3M9 21h6" /></svg>}
      </button>
      {speaking && <button title="停止播报" aria-label="停止播报" onClick={() => void command("interrupt")}>■</button>}
    </div>}
    {mode === "bubble" && <div data-hit-region className={`orb-bubble ${companionError ? "has-error" : ""}`} role={companionError ? "alert" : "status"}>
      <button className="orb-close" aria-label="收起提示" onClick={() => changeMode("orb")}>×</button><MarkdownText text={bubble || label} className="orb-bubble-text" />
    </div>}
    {card && <section data-hit-region className="orb-card" aria-label={mode === "translate" ? "快捷翻译" : mode === "chat" ? "最近对话" : "更多操作"}>
      <header><strong>{mode === "translate" ? "快捷翻译" : mode === "chat" ? "最近对话" : "陪伴设置"}</strong><button className="orb-close" aria-label="收起面板" onClick={() => changeMode("orb")}>×</button></header>
      {error && mode !== "translate" && <p className="translation-error orb-operation-error" role="alert">{error}</p>}
      {mode === "chat" ? <div className="orb-card-content floating-chat">
        {!messages.length && !streamText && <div className="orb-empty">点击麦克风按钮说话。<small>你也可以安心忙自己的事情。</small></div>}
        {messages.slice(-30).map(m => <article key={m.id} className={`floating-message ${m.role === "user" ? "from-user" : ""}`}><small>{m.role === "user" ? "你" : m.role === "proactive" ? "主动陪伴" : "Linger"}</small>{m.role === "user" ? <p>{m.content}</p> : <MarkdownText text={m.content} />}</article>)}
        {streamText && <article className="floating-message"><small>正在回应</small><MarkdownText text={streamText} /></article>}<div ref={bottom} />
      </div> : mode === "translate" ? <div className="orb-card-content floating-translation">
        <div className="translation-direction"><strong>中文 <span>→</span> {settings.translation_language}</strong><small>{settings.translation_model}</small></div>
        <label className="checkbox-label translation-auto-copy"><input type="checkbox" checked={settings.translation_auto_copy || false} disabled={!ready || autoCopySaving} onChange={e => {
          setAutoCopySaving(true); setError("");
          void onSetTranslationAutoCopy(e.target.checked).catch(e => setError(String(e))).finally(() => setAutoCopySaving(false));
        }} />自动复制译文</label>
        <label htmlFor="translation-input">想表达什么？</label>
        <textarea ref={translationInput} id="translation-input" value={text} placeholder="输入中文…" onKeyDown={e => { if (e.key === "Enter" && !e.shiftKey && !e.repeat && !e.nativeEvent.isComposing && e.nativeEvent.keyCode !== 229) { e.preventDefault(); void translate(); } }} onChange={e => { request.current += 1; inFlight.current = false; setPending(false); setText(e.target.value); setResult(""); setError(""); setCopied(false); }} />
        <div className="translation-actions"><small className={length > 4000 ? "over-limit" : ""}>{length} / 4000</small><button className="primary-button" disabled={!ready || autoCopySaving || pending || !text.trim() || length > 4000} onClick={() => void translate()}>{pending ? "翻译中…" : "翻译 ↵"}</button></div>
        {error && <p className="translation-error" role="alert">{error}</p>}
        {result && <div className="translation-result"><div><label htmlFor="translation-output">译文</label></div><textarea ref={output} id="translation-output" readOnly value={result} /></div>}
        <p className="translation-help">Enter 翻译 · Shift+Enter 换行 · Esc 收起<br />{settings.translation_auto_copy ? "已开启自动复制，译文会替换剪贴板内容。" : "目标语言、模型与自动复制可在主窗口设置。"}</p>
      </div> : <div className="orb-card-content orb-menu">
        {sourceControls}
        <button disabled={observationBusy} onClick={onToggleObserving}>{observationBusy ? "正在检查…" : settings.observing ? "暂停主动陪伴" : "开启主动陪伴"}<span>{settings.observing ? "正在陪伴" : "已暂停"}</span></button>
        <button onClick={() => void command("compact_open_main", { settings: false })}>打开完整对话<span>查看历史</span></button>
        <button onClick={() => void command("compact_open_main", { settings: true })}>打开设置<span>模型 / 角色 / 翻译</span></button>
        <button onClick={() => { changeMode("orb"); void command("toggle_compact"); }}>隐藏陪伴球<span>可从托盘再次打开</span></button>
        <p>点击麦克风录音 · 拖动小球移位置<br />靠近小球，展开常用操作</p>
      </div>}
    {mode === "chat" && <form className="orb-chat-compose" onSubmit={e => { e.preventDefault(); void sendChat(); }}>
      <label htmlFor="orb-chat-input">发条消息</label>
      <textarea ref={chatInput} id="orb-chat-input" value={chatText} placeholder="输入消息，Enter 发送…" onChange={e => { chatRevision.current += 1; setChatText(e.target.value); }} onKeyDown={e => { if (e.key === "Enter" && !e.shiftKey && !e.repeat && !e.nativeEvent.isComposing && e.nativeEvent.keyCode !== 229) { e.preventDefault(); void sendChat(); } }} />
      <div><small>Shift+Enter 换行 · 失焦清空</small><button className="primary-button" disabled={!ready || sending || !chatText.trim()} type="submit">{sending ? "发送中…" : "发送"}</button></div>
    </form>}
    {mode === "translate" && result && <button className="orb-copy-button" onClick={() => void copy()}>{copied ? "✓ 已复制，可以粘贴了" : "复制译文"}</button>}
    </section>}
    <button data-hit-region className={`companion-orb phase-${phase} ${settings.observing ? "observation-enabled" : ""} ${observingPulse ? "observing-pulse" : ""} ${companionError || error ? "with-error" : ""}`} aria-label={`陪伴球 · ${settings.observing ? "读屏陪伴已开启" : "读屏陪伴已暂停"} · ${label}，单击打字，拖动移动`} title={`${companionError || error || label} · ${settings.observing ? "读屏陪伴已开启" : "读屏陪伴已暂停"} · 单击打字 / 拖动移动`} disabled={!ready}
      style={{ "--mic-level": level } as CSSProperties}
      onPointerEnter={() => { clearTimeout(hideTimer.current); if (["orb", "bubble"].includes(modeRef.current) && !press.current) changeMode("tools"); }}
      onPointerDown={beginPress} onPointerMove={movePress} onPointerUp={() => endPress(true)} onPointerCancel={() => endPress(false)} onLostPointerCapture={() => { if (!press.current?.dragging) endPress(false); }} onContextMenu={e => { e.preventDefault(); changeMode("more"); }}
      onKeyDown={e => { if ((e.key === "Enter" || e.key === " ") && !e.repeat) { e.preventDefault(); changeMode("chat"); } }} onBlur={() => endPress(false)}>
      {settings.observing && <span className="orb-observation-halo" aria-hidden="true" />}<span className="orb-ring" /><span className="orb-face" aria-hidden="true"><i /><i /></span>{(companionError || error) && <span className="orb-error-dot" />}
    </button>
  </div>;
}
