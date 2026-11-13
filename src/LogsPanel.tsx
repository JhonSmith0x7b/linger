import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

const message = (error: unknown) => typeof error === "string" ? error : error instanceof Error ? error.message : String(error);

export function LogsPanel({ desktop }: { desktop: boolean }) {
  const [logs, setLogs] = useState("");
  const [autoRefresh, setAutoRefresh] = useState(true);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  const [clearing, setClearing] = useState(false);
  const [confirmClear, setConfirmClear] = useState(false);
  const [copying, setCopying] = useState(false);
  const [copied, setCopied] = useState(false);
  const consoleElement = useRef<HTMLPreElement>(null);
  const followTail = useRef(true);
  const alive = useRef(false);
  const epoch = useRef(0);
  const inFlight = useRef(false);
  const clearInFlight = useRef(false);
  const copyInFlight = useRef(false);
  const selectionInConsole = useCallback(() => {
    const selection = window.getSelection();
    return !!selection && !selection.isCollapsed && !!consoleElement.current &&
      (consoleElement.current.contains(selection.anchorNode) || consoleElement.current.contains(selection.focusNode));
  }, []);
  const refresh = useCallback(async () => {
    if (!desktop || !alive.current || inFlight.current || clearInFlight.current) return;
    inFlight.current = true;
    const revision = epoch.current;
    setLoading(true);
    try {
      const text = await invoke<string>("get_diagnostic_logs");
      if (alive.current && revision === epoch.current) {
        // Do not replace the text node while the user is selecting/copying a passage.
        if (!selectionInConsole()) {
          const element = consoleElement.current;
          followTail.current = !element || element.scrollHeight - element.scrollTop - element.clientHeight < 24;
          setLogs(text);
        }
        setError("");
      }
    } catch (e) {
      if (alive.current && revision === epoch.current) setError(message(e));
    } finally {
      inFlight.current = false;
      if (alive.current) setLoading(false);
    }
  }, [desktop, selectionInConsole]);
  useEffect(() => {
    alive.current = true;
    void refresh();
    return () => { alive.current = false; epoch.current += 1; };
  }, [refresh]);
  useEffect(() => {
    if (!autoRefresh) return;
    const timer = setInterval(() => { void refresh(); }, 2000);
    return () => clearInterval(timer);
  }, [autoRefresh, refresh]);
  useLayoutEffect(() => {
    const element = consoleElement.current;
    if (element && followTail.current && !selectionInConsole()) element.scrollTop = element.scrollHeight;
  }, [logs, selectionInConsole]);
  async function clear() {
    if (clearInFlight.current || !desktop) return;
    clearInFlight.current = true;
    epoch.current += 1;
    setClearing(true); setError(""); setCopied(false);
    try {
      await invoke("clear_diagnostic_logs");
      if (alive.current) { followTail.current = true; setLogs(""); setConfirmClear(false); }
    } catch (e) {
      if (alive.current) setError(message(e));
    } finally {
      clearInFlight.current = false;
      if (alive.current) { setClearing(false); void refresh(); }
    }
  }
  async function copy() {
    if (copyInFlight.current || !desktop) return;
    copyInFlight.current = true;
    const revision = epoch.current;
    setCopying(true); setCopied(false);
    try {
      await invoke("copy_diagnostic_logs");
      if (alive.current && revision === epoch.current) setCopied(true);
    } catch (e) {
      if (alive.current && revision === epoch.current) setError(message(e));
    } finally {
      copyInFlight.current = false;
      if (alive.current) setCopying(false);
    }
  }
  async function openFolder() {
    try { await invoke("open_diagnostic_log_folder"); }
    catch (e) { if (alive.current) setError(message(e)); }
  }
  return <section className="content-page logs-page">
    <div className="page-heading"><div><h1>调试日志</h1></div></div>
    <div className="logs-toolbar">
      <label><input type="checkbox" checked={autoRefresh} onChange={e => setAutoRefresh(e.target.checked)} />自动刷新（2 秒）</label>
      <button className="secondary-button" disabled={!desktop || loading || clearing} onClick={() => void refresh()}>刷新</button>
      <button className="secondary-button" disabled={!desktop || copying || clearing} onClick={() => void copy()}>{copied ? "已复制" : copying ? "复制中…" : "复制全部日志"}</button>
      <button className="secondary-button" disabled={!desktop} onClick={() => void openFolder()}>打开日志目录</button>
      <button className="secondary-button" disabled={!desktop || clearing} onClick={() => setConfirmClear(value => !value)}>清空</button>
    </div>
    {confirmClear && <div className="logs-confirm"><span>清空本机调试日志？</span><button className="secondary-button" disabled={clearing} onClick={() => void clear()}>{clearing ? "清空中…" : "确认清空"}</button><button className="secondary-button" disabled={clearing} onClick={() => setConfirmClear(false)}>取消</button></div>}
    {error && <pre className="logs-error" role="alert">{error}</pre>}
    <pre ref={consoleElement} className="logs-console" aria-label="原始调试日志" tabIndex={0} onScroll={() => {
      const element = consoleElement.current;
      if (element) followTail.current = element.scrollHeight - element.scrollTop - element.clientHeight < 24;
    }}>{logs || (!desktop ? "请在桌面应用中查看日志。" : "暂无调试日志。从本次更新后开始记录，旧版未保留的错误原文无法恢复。")}</pre>
  </section>;
}
