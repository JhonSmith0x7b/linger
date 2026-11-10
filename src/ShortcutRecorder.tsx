import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { shortcutFromEvent, shortcutLabel } from "./shortcut";

interface Props {
  value: string;
  disabled: boolean;
  onChange: (shortcut: string) => void;
  onBusy: (busy: boolean) => void;
  onError: (error: unknown) => void;
}
export function ShortcutRecorder(props: Props) {
  const callbacks = useRef(props);
  callbacks.current = props;
  const [phase, setPhase] = useState<
    "idle" | "starting" | "listening" | "restoring"
  >("idle");
  const [hint, setHint] = useState("");
  const mounted = useRef(false);
  const token = useRef<number | null>(null);
  const generation = useRef(0);
  const listening = useRef(false);
  const pendingChord = useRef<string | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const beginInFlight = useRef<Promise<number> | null>(null);
  const restoreInFlight = useRef<Promise<void> | null>(null);
  const finish = useRef<(chord?: string) => Promise<void>>(async () => {});
  finish.current = (chord?: string) => {
    if (restoreInFlight.current) return restoreInFlight.current;
    if (token.current === null && beginInFlight.current === null)
      return Promise.resolve();
    const attempt = ++generation.current;
    listening.current = false;
    pendingChord.current = null;
    if (timer.current) {
      clearTimeout(timer.current);
      timer.current = null;
    }
    const currentToken = token.current;
    token.current = null;
    const pendingBegin = beginInFlight.current;
    if (mounted.current) setPhase("restoring");
    const restore = (async () => {
      try {
        const acquired =
          currentToken ??
          (pendingBegin ? await pendingBegin.catch(() => null) : null);
        if (acquired !== null)
          await invoke("end_shortcut_capture", { token: acquired });
        if (generation.current === attempt && chord && mounted.current) {
          callbacks.current.onChange(chord);
          setHint("已录入，保存设置后生效。");
        }
      } catch (error) {
        if (generation.current === attempt && mounted.current)
          callbacks.current.onError(error);
      } finally {
        if (generation.current === attempt) {
          beginInFlight.current = null;
          restoreInFlight.current = null;
          if (mounted.current) setPhase("idle");
          callbacks.current.onBusy(false);
        }
      }
    })();
    restoreInFlight.current = restore;
    return restore;
  };
  useEffect(() => {
    mounted.current = true;
    const keydown = (event: KeyboardEvent) => {
      if (!listening.current) return;
      event.preventDefault();
      event.stopImmediatePropagation();
      if (event.repeat) return;
      if (event.code === "Escape") {
        setHint("已取消录入，原快捷键保持不变。");
        void finish.current();
        return;
      }
      if (pendingChord.current) return;
      try {
        const chord = shortcutFromEvent(event);
        if (chord) {
          pendingChord.current = chord;
          setHint(`${shortcutLabel(chord)} · 松开所有按键以完成`);
        }
      } catch (error) {
        setHint(error instanceof Error ? error.message : "请换一个按键组合。");
      }
    };
    const keyup = (event: KeyboardEvent) => {
      if (!listening.current) return;
      event.preventDefault();
      event.stopImmediatePropagation();
      if (
        pendingChord.current &&
        !event.ctrlKey &&
        !event.altKey &&
        !event.shiftKey &&
        !event.metaKey
      )
        void finish.current(pendingChord.current);
    };
    const blur = () => {
      if (listening.current || token.current !== null)
        setHint("已取消录入，原快捷键保持不变。");
      void finish.current();
    };
    window.addEventListener("keydown", keydown, true);
    window.addEventListener("keyup", keyup, true);
    window.addEventListener("blur", blur);
    return () => {
      mounted.current = false;
      window.removeEventListener("keydown", keydown, true);
      window.removeEventListener("keyup", keyup, true);
      window.removeEventListener("blur", blur);
      void finish.current();
    };
  }, []);
  async function start() {
    if (
      phase !== "idle" ||
      props.disabled ||
      beginInFlight.current ||
      restoreInFlight.current ||
      token.current !== null
    )
      return;
    const attempt = ++generation.current;
    setPhase("starting");
    setHint("正在暂停原快捷键…");
    callbacks.current.onBusy(true);
    try {
      beginInFlight.current = invoke<number>("begin_shortcut_capture");
      const acquired = await beginInFlight.current;
      if (!mounted.current || generation.current !== attempt) return;
      beginInFlight.current = null;
      token.current = acquired;
      listening.current = true;
      setPhase("listening");
      setHint("请按下想使用的组合键；Esc 取消。");
      timer.current = setTimeout(() => {
        setHint("录入已超时，原快捷键已恢复。");
        void finish.current();
      }, 29_000);
    } catch (error) {
      if (mounted.current && generation.current === attempt) {
        beginInFlight.current = null;
        setPhase("idle");
        callbacks.current.onError(error);
        callbacks.current.onBusy(false);
      }
    }
  }
  return (
    <div className="shortcut-recorder">
      <span className="shortcut-label">全局按键语音</span>
      <div
        className={`shortcut-recorder-controls ${phase === "listening" ? "listening" : ""}`}
      >
        <button
          type="button"
          className="shortcut-display"
          aria-label="全局按键语音，点击录入"
          disabled={props.disabled || phase !== "idle"}
          onClick={() => void start()}
        >
          <kbd>{shortcutLabel(props.value)}</kbd>
        </button>
        {phase === "idle" ? (
          <button
            type="button"
            className="secondary-button"
            disabled={props.disabled}
            onClick={() => void start()}
          >
            录入快捷键
          </button>
        ) : (
          <button
            type="button"
            className="secondary-button"
            disabled={phase === "restoring"}
            onClick={() => {
              setHint("已取消录入，原快捷键保持不变。");
              void finish.current();
            }}
          >
            {phase === "restoring" ? "正在恢复…" : "取消录入"}
          </button>
        )}
      </div>
      <p className="shortcut-hint" role="status">
        {hint ||
          "点击后直接按键录入。按住录音，松开提交；录入期间不会触发语音。"}
      </p>
    </div>
  );
}
