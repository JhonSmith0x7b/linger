import { translationStyles } from "./translationStyles";
import { useCallback, useEffect, useRef, useState } from "react";
import type { FormEvent, ReactNode } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  AudioDevices,
  CaptureSource,
  Memory,
  Message,
  ObservationStatus,
  Settings,
  Status,
  CharacterTts,
} from "./types";
import { characters, characterFor, defaultVoiceProfile } from "./characters";
import { MarkdownText } from "./MarkdownText";
import { FloatingPanel } from "./FloatingPanel";
import { UserProfileEditor } from "./UserProfileEditor";
import { ScreenSourcePicker, sourceLabel } from "./ScreenSourcePicker";
import { ShortcutRecorder } from "./ShortcutRecorder";
import { CharacterVoiceEditor, TtsConnections } from "./TtsSettings";
import { characterTts } from "./tts";

const defaults: Settings = {
  base_url: "https://api.openai.com/v1",
  conversation_model: "gpt-4.1-mini",
  asr_model: "gpt-transcribe",
  memory_model: "gpt-4.1-mini",
  tts_model: "gpt-4o-mini-tts",
  translation_model: "gpt-4.1-mini",
  translation_language: "英语",
  translation_auto_copy: false,
  translation_style: "casual",
  translation_custom_prompt: "",
  response_format: "pcm",
  volume: 0.8,
  audio_input_device: "",
  audio_output_device: "",
  personality:
    "你是一个温暖、自然的陪伴者。用户可以安心做自己的事情，无需回应每句话。不催促、不反复提问。",
  character_id: "custom",
  character_voices: { custom: defaultVoiceProfile() },
  observation_minutes: 2,
  observing: false,
  compact_visible: true,
  capture_source: "monitor:primary",
  watch_instruction: "",
  shortcut: "CommandOrControl+Shift+Space",
};
const phases: Record<string, string> = {
  idle: "在这里陪你",
  recording: "正在听你说",
  transcribing: "正在转写",
  thinking: "想一想",
  generating: "正在回应",
  speaking: "正在说话",
  playing: "正在说话",
  observing: "看看你的屏幕",
  tool: "正在使用工具",
  error: "需要留意",
  stopped: "已停止",
  synthesizing: "正在准备语音",
};
function observationTime(value: string) {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? "" : date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });
}
function Icon({ name, size = 20 }: { name: string; size?: number }) {
  const paths: Record<string, ReactNode> = {
    chat: (
      <>
        <path d="M21 11.5a8.4 8.4 0 0 1-.9 3.8 8.5 8.5 0 0 1-7.6 4.7 8.4 8.4 0 0 1-3.8-.9L3 21l1.9-5.7a8.4 8.4 0 0 1-.9-3.8 8.5 8.5 0 0 1 4.7-7.6 8.4 8.4 0 0 1 3.8-.9h.5a8.5 8.5 0 0 1 8 8v.5Z" />
      </>
    ),
    memory: (
      <>
        <path d="M12 3v18M8 4a3 3 0 0 0-5 3 4 4 0 0 0 0 7 3 3 0 0 0 5 5 3 3 0 0 0 4-3M16 4a3 3 0 0 1 5 3 4 4 0 0 1 0 7 3 3 0 0 1-5 5 3 3 0 0 1-4-3M5 10l3 1M19 10l-3 1" />
      </>
    ),
    settings: (
      <>
        <path d="M4 7h16M4 17h16" />
        <circle cx="9" cy="7" r="3" />
        <circle cx="15" cy="17" r="3" />
      </>
    ),
    mic: (
      <>
        <rect x="9" y="2" width="6" height="12" rx="3" />
        <path d="M5 10v2a7 7 0 0 0 14 0v-2M12 19v3M8 22h8" />
      </>
    ),
    screen: (
      <>
        <rect x="3" y="4" width="18" height="13" rx="2" />
        <path d="M8 21h8M12 17v4" />
      </>
    ),
    arrow: (
      <>
        <path d="m5 12 7-7 7 7M12 5v14" />
      </>
    ),
    stop: <rect x="6" y="6" width="12" height="12" rx="2" />,
    plus: <path d="M12 5v14M5 12h14" />,
    trash: (
      <>
        <path d="M3 6h18M9 6V3h6v3M5 6l1 15h12l1-15M10 10v7M14 10v7" />
      </>
    ),
    close: <path d="m6 6 12 12M6 18 18 6" />,
    refresh: (
      <>
        <path d="M20 7v5h-5M4 17v-5h5M6 7a7 7 0 0 1 12-1l2 3M4 15l2 3a7 7 0 0 0 12-1" />
      </>
    ),
  };
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.7"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {paths[name]}
    </svg>
  );
}
function Field({
  label,
  help,
  children,
}: {
  label: string;
  help?: string;
  children: ReactNode;
}) {
  return (
    <label className="field">
      <span>{label}</span>
      {children}
      {help && <small>{help}</small>}
    </label>
  );
}
function timestamp(value: string) {
  const date = new Date(value);
  return Number.isNaN(date.getTime())
    ? ""
    : date.toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit" });
}

export default function App() {
  const [page, setPage] = useState<"chat" | "memory" | "settings">("chat");
  const [settings, setSettings] = useState<Settings>(defaults);
  const [draft, setDraft] = useState<Settings>(defaults);
  const [settingsTab, setSettingsTab] = useState("connection");
  const [messages, setMessages] = useState<Message[]>([]);
  const [conversationEpoch, setConversationEpoch] = useState(0);
  const [confirmClear, setConfirmClear] = useState(false);
  const [clearingConversation, setClearingConversation] = useState(false);
  const clearingRef = useRef(false);
  const [historyLoading, setHistoryLoading] = useState(false);
  const [hasOlderMessages, setHasOlderMessages] = useState(false);
  const [browsingHistory, setBrowsingHistory] = useState(false);
  const historyMode = useRef(false);
  const historyRequest = useRef(0);
  const historyInFlight = useRef(false);
  const messageList = useRef<HTMLDivElement>(null);
  const [memories, setMemories] = useState<Memory[]>([]);
  const [sources, setSources] = useState<CaptureSource[]>([]);
  const [audioDevices, setAudioDevices] = useState<AudioDevices>({ inputs: [], outputs: [] });
  const [audioLoading, setAudioLoading] = useState(false);
  const [audioError, setAudioError] = useState("");
  const audioInFlight = useRef(false);
  const [sourcesLoading, setSourcesLoading] = useState(false);
  const [sourceBusy, setSourceBusy] = useState(false);
  const [observationBusy, setObservationBusy] = useState(false);
  const [sourceNotice, setSourceNotice] = useState("");
  const sourcesInFlight = useRef(false);
  const sourceInFlight = useRef(false);
  const observationInFlight = useRef(false);
  const savedSource = useRef("monitor:primary");
  const [status, setStatus] = useState<Status>({ phase: "idle", detail: "" });
  const [observationStatus, setObservationStatus] = useState<ObservationStatus | null>(null);
  const [input, setInput] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [hasKey, setHasKey] = useState(false);
  const [models, setModels] = useState<string[]>([]);
  const [modelsLoading, setModelsLoading] = useState(false);
  const [modelsLoaded, setModelsLoaded] = useState(false);
  const [modelsError, setModelsError] = useState("");
  const [modelQuery, setModelQuery] = useState("");
  const modelRequest = useRef(0);
  const roleDrafts = useRef<Record<string, string>>({});
  const ttsDrafts = useRef<Record<string, CharacterTts>>({});
  const [cardTts, setCardTts] = useState<CharacterTts>(() => characterTts(defaults, "custom"));
  const pendingVolume = useRef<number | null>(null);
  const volumeInFlight = useRef(false);
  const volumeRef = useRef(defaults.volume);
  const [capturingShortcut, setCapturingShortcut] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [saving, setSaving] = useState(false);
  const [autoCopySaving, setAutoCopySaving] = useState(false);
  const autoCopySavingRef = useRef(false);
  const [ready, setReady] = useState(false);
  const [memoryInput, setMemoryInput] = useState("");
  const [memoryScope, setMemoryScope] = useState("long_term");
  const [editingMemory, setEditingMemory] = useState<number | null>(null);
  const [held, setHeld] = useState(false);
  const [streamText, setStreamText] = useState("");
  const compact =
    new URLSearchParams(window.location.search).get("compact") === "true";
  const regionParts = draft.capture_source.startsWith("region:")
    ? draft.capture_source.split(":")
    : null;
  const holding = useRef(false);
  const recordingStart = useRef<Promise<unknown> | null>(null);
  const bottom = useRef<HTMLDivElement>(null);
  const desktop = isTauri();
  const report = useCallback(
    (e: unknown) =>
      setError(
        typeof e === "string" ? e : e instanceof Error ? e.message : String(e),
      ),
    [],
  );
  const run = useCallback(
    async (command: string, args?: Record<string, unknown>) => {
      setError("");
      try {
        return await invoke(command, args);
      } catch (e) {
        report(e);
        return undefined;
      }
    },
    [report],
  );
  const refreshMemories = useCallback(async () => {
    try {
      setMemories(await invoke<Memory[]>("list_memories"));
    } catch (e) {
      report(e);
    }
  }, [report]);
  const refreshSources = useCallback(async () => {
    if (sourcesInFlight.current) return;
    sourcesInFlight.current = true;
    setSourcesLoading(true);
    try {
      setSources(await invoke<CaptureSource[]>("list_sources"));
    } catch (e) {
      report(e);
    } finally {
      sourcesInFlight.current = false;
      setSourcesLoading(false);
    }
  }, [report]);
  const refreshAudioDevices = useCallback(async () => {
    if (audioInFlight.current) return;
    audioInFlight.current = true;
    setAudioLoading(true); setAudioError("");
    try { setAudioDevices(await invoke<AudioDevices>("list_audio_devices")); }
    catch (e) { setAudioError(String(e)); }
    finally { audioInFlight.current = false; setAudioLoading(false); }
  }, []);
  useEffect(() => {
    if (desktop && ready && page === "settings" && settingsTab === "audio") {
      void refreshAudioDevices();
    }
  }, [desktop, ready, page, settingsTab, refreshAudioDevices]);
  async function selectSource(source: string) {
    if (sourceInFlight.current || observationInFlight.current) return;
    sourceInFlight.current = true; setSourceBusy(true); setError("");
    try { await invoke("set_capture_source", { source }); }
    catch (e) { report(e); }
    finally { sourceInFlight.current = false; setSourceBusy(false); }
  }
  async function checkScreenPermission() {
    if (sourceInFlight.current || observationInFlight.current) return;
    sourceInFlight.current = true; setSourceBusy(true); setError(""); setSourceNotice("");
    try {
      const allowed = await invoke<boolean>("request_screen_permission");
      setSourceNotice(allowed ? "录屏权限已可用，可以开启陪伴。" : "尚未获得录屏权限。请在系统设置中允许 Linger；若已允许，请退出并重新打开应用后再试。");
      if (allowed) await refreshSources();
    } catch (e) { report(e); }
    finally { sourceInFlight.current = false; setSourceBusy(false); }
  }

  const invalidateModels = useCallback(() => {
    modelRequest.current += 1;
    setModels([]);
    setModelsLoaded(false);
    setModelsLoading(false);
    setModelsError("");
  }, []);
  const loadModels = useCallback(async (baseUrl: string, key?: string) => {
    const request = ++modelRequest.current;
    setModelsLoading(true);
    setModelsError("");
    setModels([]);
    setModelsLoaded(false);
    try {
      const ids = await invoke<string[]>("list_models", {
        baseUrl,
        ...(key?.trim() ? { key: key.trim() } : {}),
      });
      if (request !== modelRequest.current) return;
      setModels(Array.from(new Set(ids)).sort());
      setModelsLoaded(true);
    } catch (e) {
      if (request !== modelRequest.current) return;
      setModelsError(
        typeof e === "string" ? e : "读取模型失败，请检查连接后重试。",
      );
    } finally {
      if (request === modelRequest.current) setModelsLoading(false);
    }
  }, []);

  useEffect(() => {
    if (!desktop) return;
    let disposed = false;
    const cleanups: (() => void)[] = [];
    async function init() {
      try {
        const stopMessage = await listen<Message>(
          "linger:message",
          ({ payload }) => {
            setStreamText("");
            if (payload.role === "user" && historyMode.current) resumeLatestHistory();
            if (!historyMode.current) setMessages((old) => {
              const updated = [...old.filter((m) => m.id !== payload.id), payload].sort((a, b) => a.id - b.id);
              return updated.slice(-(compact ? 30 : 50));
            });
          },
        );
        if (disposed) {
          stopMessage();
          return;
        }
        cleanups.push(stopMessage);
        let receivedStatus = false;
        const stopStatus = await listen<Status>(
          "linger:status",
          ({ payload }) => {
            receivedStatus = true;
            if (payload.phase === "recording" && historyMode.current) resumeLatestHistory();
            setStatus(payload);
            if (
              ["idle", "error", "stopped", "recording"].includes(payload.phase)
            )
              setStreamText("");
            if (payload.phase === "error") setError(payload.detail);
            else if (payload.phase === "recording") setError("");
          },
        );
        if (disposed) {
          stopStatus();
          return;
        }
        cleanups.push(stopStatus);
        const initialStatus = await invoke<Status>("get_status");
        if (disposed) return;
        if (!receivedStatus) {
          setStatus(initialStatus);
          if (initialStatus.phase === "error") setError(initialStatus.detail);
        }
        let receivedObservation = false;
        const stopObservation = await listen<ObservationStatus>(
          "linger:observation",
          ({ payload }) => {
            receivedObservation = true;
            setObservationStatus(payload);
          },
        );
        if (disposed) { stopObservation(); return; }
        cleanups.push(stopObservation);
        const initialObservation = await invoke<ObservationStatus>("get_observation_status");
        if (disposed) return;
        if (!receivedObservation) setObservationStatus(initialObservation);
        const stopOpenSettings = await listen("linger:open-settings", () => {
          if (!compact) { setPage("settings"); setSettingsTab("connection"); }
        });
        if (disposed) { stopOpenSettings(); return; }
        cleanups.push(stopOpenSettings);
        const stopClear = await listen("linger:conversation-cleared", resetConversationView);
        if (disposed) { stopClear(); return; }
        cleanups.push(stopClear);
        const stopDelta = await listen<{ text: string }>(
          "linger:delta",
          ({ payload }) => setStreamText((old) => old + payload.text),
        );
        if (disposed) {
          stopDelta();
          return;
        }
        cleanups.push(stopDelta);
        const stopSettings = await listen<Settings>(
          "linger:settings",
          ({ payload }) => {
            const volume = volumeInFlight.current
              ? volumeRef.current
              : payload.volume;
            if (!volumeInFlight.current) volumeRef.current = volume;
            const priorSource = savedSource.current;
            savedSource.current = payload.capture_source;
            setSettings({ ...payload, volume });
            setDraft((previous) => ({
              ...previous,
              observing: payload.observing,
              compact_visible: payload.compact_visible,
              translation_auto_copy: payload.translation_auto_copy,
              capture_source: previous.capture_source === priorSource ? payload.capture_source : previous.capture_source,
              volume,
            }));
          },
        );
        if (disposed) {
          stopSettings();
          return;
        }
        cleanups.push(stopSettings);
        const initialHistoryRequest = historyRequest.current;
        const [initialSettings, initialMessages, keyExists] = await Promise.all(
          [
            invoke<Settings>("get_settings"),
            invoke<Message[]>("list_messages", { limit: compact ? 30 : 50 }),
            invoke<boolean>("key_status"),
          ],
        );
        if (disposed) return;
        savedSource.current = initialSettings.capture_source;
        setSettings(initialSettings);
        setDraft(initialSettings);
        volumeRef.current = initialSettings.volume;
        roleDrafts.current = {
          ...initialSettings.character_personalities,
          [initialSettings.character_id || "custom"]:
            initialSettings.personality,
        };
        ttsDrafts.current = { ...initialSettings.character_tts };
        const initialCharacter = initialSettings.character_id || "custom";
        setCardTts(characterTts(initialSettings, initialCharacter));
        setHasKey(keyExists);
        if (initialHistoryRequest === historyRequest.current) {
        setMessages((old) =>
          Array.from(
            new Map(
              [...initialMessages, ...old].map((m) => [m.id, m]),
            ).values(),
          ).sort((a, b) => a.id - b.id).slice(-(compact ? 30 : 50)),
        );
        setHasOlderMessages(initialMessages.length >= 50);
        }
        setReady(true);
        if (keyExists && !compact) void loadModels(initialSettings.base_url);
      } catch (e) {
        if (!disposed) report(e);
      }
    }
    void init();
    return () => {
      disposed = true;
      modelRequest.current += 1;
      cleanups.forEach((fn) => fn());
    };
  }, [desktop, report, loadModels, compact]);
  useEffect(() => {
    if (browsingHistory) {
      if (messageList.current) messageList.current.scrollTop = 0;
    } else bottom.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages, page, browsingHistory]);
  useEffect(() => {
    if (!compact && !browsingHistory && messages.length >= 50) setHasOlderMessages(true);
  }, [compact, browsingHistory, messages.length]);
  function resetConversationView() {
    historyRequest.current += 1; historyInFlight.current = false; historyMode.current = false;
    setMessages([]); setStreamText(""); setInput(""); setError("");
    setHistoryLoading(false); setHasOlderMessages(false); setBrowsingHistory(false);
    setConfirmClear(false); setObservationStatus(null);
    holding.current = false; setHeld(false);
    setConversationEpoch(epoch => epoch + 1);
  }
  async function clearConversation() {
    if (clearingRef.current) return;
    clearingRef.current = true; setClearingConversation(true); setError("");
    try {
      await invoke("clear_conversation");
      // Native success emits the reset event to every window.
      setConfirmClear(false);
      setNotice("对话与近期摘要已清空，角色设定、用户档案和记忆已保留。");
    } catch (error) { report(error); }
    finally { clearingRef.current = false; setClearingConversation(false); }
  }
  function resumeLatestHistory() {
    if (!historyMode.current) return;
    historyMode.current = false;
    setBrowsingHistory(false); setMessages([]);
    void loadHistory(undefined, true);
  }
  async function loadHistory(beforeId?: number, supersede = false) {
    if (historyInFlight.current && !supersede) return;
    historyInFlight.current = true; setHistoryLoading(true);
    const request = ++historyRequest.current;
    const wasHistorical = historyMode.current;
    // Keep live events from changing the page while an older snapshot is fetched.
    historyMode.current = beforeId !== undefined;
    try {
      const batch = await invoke<Message[]>("list_messages", { beforeId, limit: 50 });
      if (request !== historyRequest.current) return;
      if (beforeId !== undefined && batch.length === 0) {
        historyMode.current = wasHistorical;
        setHasOlderMessages(false);
        return;
      }
      setMessages(old => beforeId !== undefined ? batch.slice(-50) :
        Array.from(new Map([...old, ...batch].map(m => [m.id, m])).values()).sort((a, b) => a.id - b.id).slice(-50));
      setBrowsingHistory(beforeId !== undefined);
      setHasOlderMessages(batch.length >= 50);
    } catch (error) {
      if (request === historyRequest.current) { historyMode.current = wasHistorical; report(error); }
    } finally {
      if (request === historyRequest.current) { historyInFlight.current = false; setHistoryLoading(false); }
    }
  }
  useEffect(() => {
    if (page === "memory" && ready) void refreshMemories();
    if (page === "settings") setNotice("");
    // Keep unsaved character and connection drafts when navigating between pages.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [page, ready]);
  useEffect(() => {
    const stop = () => {
      if (!holding.current) return;
      holding.current = false;
      setHeld(false);
      void recordingStart.current
        ?.then(() => invoke("stop_recording"))
        .catch(report);
    };
    window.addEventListener("blur", stop);
    return () => window.removeEventListener("blur", stop);
  }, [report]);

  const change = <K extends keyof Settings>(key: K, value: Settings[K]) => {
    if (key === "base_url") {
      invalidateModels();
      setHasKey(false);
    }
    setDraft((prev) => ({ ...prev, [key]: value }));
  };
  function selectCharacter(id: string) {
    roleDrafts.current[draft.character_id || "custom"] = draft.personality;
    ttsDrafts.current[draft.character_id || "custom"] = cardTts;
    const preset = characterFor(id);
    // A newly added Melody card starts with the user's existing voice.
    // Saved profiles and unsaved edits always take precedence.
    const inheritVoice = id === "melody" && !draft.character_tts?.[id] && !draft.character_voices?.[id];
    setCardTts(ttsDrafts.current[id] ?? (inheritVoice ? structuredClone(cardTts) : characterTts(draft, id)));
    setDraft((prev) => ({
      ...prev,
      character_id: id,
      personality:
        roleDrafts.current[id] ?? preset?.personality ?? defaults.personality,
    }));
  }
  async function saveSettings(e: FormEvent) {
    e.preventDefault();
    if (capturingShortcut) return;
    setSaving(true);
    setError("");
    setNotice("");
    try {
      const savedSettings = {
        ...draft,
        character_personalities: {
          ...roleDrafts.current,
          [draft.character_id || "custom"]: draft.personality,
        },
        character_tts: {
          ...ttsDrafts.current,
          [draft.character_id || "custom"]: cardTts,
        },
      };
      await invoke("save_settings", { settings: savedSettings });
      setSettings(savedSettings);
      setDraft((previous) => ({
        ...previous,
        character_personalities: savedSettings.character_personalities,
        character_tts: savedSettings.character_tts,
      }));
      if (apiKey.trim()) {
        await invoke("set_api_key", { key: apiKey.trim() });
        setApiKey("");
        setHasKey(true);
      }
      const keyExists = await invoke<boolean>("key_status");
      setHasKey(keyExists);
      roleDrafts.current[draft.character_id || "custom"] = draft.personality;
      ttsDrafts.current[draft.character_id || "custom"] = cardTts;
      setNotice("设置已保存，下次请求使用新配置。");
      if (keyExists) void loadModels(draft.base_url);
    } catch (e) {
      report(e);
    } finally {
      setSaving(false);
    }
  }
  async function setTranslationAutoCopy(enabled: boolean) {
    if (autoCopySavingRef.current) return;
    autoCopySavingRef.current = true; setAutoCopySaving(true);
    try {
      await invoke("set_translation_auto_copy", { enabled });
      setSettings(previous => ({ ...previous, translation_auto_copy: enabled }));
      setDraft(previous => ({ ...previous, translation_auto_copy: enabled }));
    } finally { autoCopySavingRef.current = false; setAutoCopySaving(false); }
  }
  async function changeVolume(volume: number) {
    volumeRef.current = volume;
    pendingVolume.current = volume;
    setSettings((previous) => ({ ...previous, volume }));
    setDraft((previous) => ({ ...previous, volume }));
    if (volumeInFlight.current) return;
    volumeInFlight.current = true;
    try {
      while (pendingVolume.current !== null) {
        const next = pendingVolume.current;
        pendingVolume.current = null;
        try {
          await invoke("set_volume", { volume: next });
        } catch (error) {
          report(error);
          if (pendingVolume.current === null) {
            try {
              const saved = await invoke<Settings>("get_settings");
              if (
                pendingVolume.current === null &&
                volumeRef.current === next
              ) {
                volumeRef.current = saved.volume;
                setSettings((previous) => ({
                  ...previous,
                  volume: saved.volume,
                }));
                setDraft((previous) => ({ ...previous, volume: saved.volume }));
              }
            } catch {
              /* Keep the explicit error; no full settings reset. */
            }
          }
        }
      }
    } finally {
      volumeInFlight.current = false;
    }
  }

  async function send(e: FormEvent) {
    e.preventDefault();
    const text = input.trim();
    if (!text || !ready) return;
    resumeLatestHistory();
    setInput("");
    setError("");
    try {
      await invoke("send_text", { text });
    } catch (e) {
      report(e);
      setInput((old) => old || text);
    }
  }
  function startRecording() {
    if (holding.current || !ready) return;
    resumeLatestHistory();
    holding.current = true;
    setHeld(true);
    setError("");
    recordingStart.current = invoke("start_recording");
    void recordingStart.current.catch((e) => {
      holding.current = false;
      setHeld(false);
      report(e);
    });
  }
  function stopRecording() {
    if (!holding.current) return;
    holding.current = false;
    setHeld(false);
    void recordingStart.current
      ?.then(() => invoke("stop_recording"))
      .catch(report);
  }
  async function toggleObserving() {
    if (observationInFlight.current || sourceInFlight.current) return;
    observationInFlight.current = true; setObservationBusy(true);
    setError("");
    const enabled = !settings.observing;
    try {
      await invoke("set_observing", { enabled });
      // The backend emits the authoritative state after the first capture succeeds.
    } catch (e) {
      report(e);
    } finally {
      observationInFlight.current = false; setObservationBusy(false);
    }
  }
  async function saveMemory(e: FormEvent) {
    e.preventDefault();
    if (!memoryInput.trim()) return;
    try {
      if (editingMemory !== null)
        await invoke("update_memory", {
          id: editingMemory,
          content: memoryInput.trim(),
        });
      else
        await invoke("save_memory", {
          content: memoryInput.trim(),
          scope: memoryScope,
        });
      setMemoryInput("");
      setEditingMemory(null);
      await refreshMemories();
    } catch (e) {
      report(e);
    }
  }
  const activeCharacter = characterFor(settings.character_id);
  const selectedCharacter = characterFor(draft.character_id);
  const filteredModels = models.filter((model) =>
    model.toLowerCase().includes(modelQuery.toLowerCase().trim()),
  );
  const phaseLabel = phases[status.phase] || status.phase;
  const busy = !["idle", "error", "stopped"].includes(status.phase);

  const sourceControls = <>
    <ScreenSourcePicker value={settings.capture_source} sources={sources} disabled={!ready || sourceBusy || observationBusy} loading={sourcesLoading} onChange={source => void selectSource(source)} onRefresh={() => void refreshSources()} onPermission={() => void checkScreenPermission()} notice={sourceNotice} />
    {observationStatus && <small className="observation-feedback" data-phase={observationStatus.phase} role="status">
      {observationStatus.at && <time dateTime={observationStatus.at}>{observationTime(observationStatus.at)} · </time>}
      {observationStatus.detail}
      {observationStatus.captured_at && <> · 上次截图 <time dateTime={observationStatus.captured_at}>{observationTime(observationStatus.captured_at)}</time></>}
    </small>}
  </>;

  if (compact)
    return <FloatingPanel key={conversationEpoch} settings={settings} messages={messages} streamText={streamText} ready={ready} status={status} error={error} onStartRecording={startRecording} onStopRecording={stopRecording} onToggleObserving={() => void toggleObserving()} observationBusy={observationBusy || sourceBusy} sourceControls={sourceControls} onSetTranslationAutoCopy={setTranslationAutoCopy} />;

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="brand">
          <div className="brand-mark">
            l<span>·</span>
          </div>
          <div>
            Linger<small>留一点陪伴在身边</small>
          </div>
        </div>
        <nav aria-label="主导航">
          {(
            [
              { key: "chat", label: "陪伴", icon: "chat" },
              { key: "memory", label: "记忆", icon: "memory" },
              { key: "settings", label: "设置", icon: "settings" },
            ] as const
          ).map((item) => (
            <button
              key={item.key}
              className={`nav-item ${page === item.key ? "active" : ""}`}
              onClick={() => setPage(item.key)}
            >
              <Icon name={item.icon} />
              {item.label}
              {page === item.key && <span className="nav-dot" />}
            </button>
          ))}
        </nav>
        <button
          className="active-character"
          onClick={() => {
            setPage("settings");
            setSettingsTab("character");
          }}
          aria-label="选择陪伴角色"
        >
          {activeCharacter ? (
            <img src={activeCharacter.image} alt="" />
          ) : (
            <span className="custom-avatar">L</span>
          )}
          <span>
            <strong>{activeCharacter?.name || "自定义角色"}</strong>
            <small>
              {activeCharacter ? activeCharacter.tags[0] : "按你的方式陪伴"}
            </small>
          </span>
          <span className="character-arrow">↗</span>
        </button>
        <div className="sidebar-bottom">
          <div className={`connection-dot ${ready ? "connected" : ""}`} />
          <span>
            {desktop
              ? ready
                ? "本地运行 · 云端模型"
                : "正在连接本地服务"
              : "界面预览 · 请通过桌面应用使用"}
          </span>
          <small>你的节奏，你的空间。</small>
        </div>
      </aside>
      <main>
        <header className="topbar">
          <div className="breadcrumb">
            你的日常空间 <span>/</span>{" "}
            {page === "chat" ? "陪伴" : page === "memory" ? "记忆" : "设置"}
          </div>
          <div className="privacy-label">
            <span className="tiny-dot" /> 记录保存在本机
          </div>
        </header>
        {error && (
          <div role="alert" className="banner error">
            <span>{error}</span>
            <button aria-label="关闭错误" onClick={() => setError("")}>
              <Icon name="close" size={16} />
            </button>
          </div>
        )}
        {!desktop && (
          <div className="banner info">
            当前为浏览器界面预览。请运行 <code>npm run tauri dev</code>{" "}
            使用录音、截图和模型服务。
          </div>
        )}
        {page === "chat" && (
          <section className="chat-page">
            <div className="page-heading">
              <div>
                <p className="eyebrow">
                  {activeCharacter
                    ? `${activeCharacter.romanized} · 在你身边`
                    : "A LITTLE COMPANY"}
                </p>
                <h1>安心做你的事。</h1>
                <p className="subtitle">
                  {activeCharacter?.greeting ||
                    "想说话时，我就在这里。不说话也没关系。"}
                </p>
              </div>
              <div className="status-pill">
                <span className={busy ? "pulse-dot" : "tiny-dot"} />
                {phaseLabel}
              </div>
            </div>
            <div className="observation-bar">
              <div className="observation-icon">
                <Icon name="screen" />
              </div>
              <div>
                <strong>
                  {settings.observing ? "正在陪你看屏幕" : "屏幕观察已暂停"}
                </strong>
                <small>
                  {observationBusy ? "正在检查权限与首次截图…" : `${sourceLabel(settings.capture_source, sources)} · 每 ${settings.observation_minutes} 分钟观察一次`}
                </small>
              </div>
              <button
                className="text-button"
                onClick={() => {
                  setSettingsTab("companion");
                  setPage("settings");
                }}
              >
                调整
              </button>
              <button
                className={`switch ${settings.observing ? "on" : ""}`}
                role="switch"
                aria-checked={settings.observing}
                aria-label="自动观察屏幕"
                disabled={
                  !ready || observationBusy || sourceBusy
                }
                onClick={() => void toggleObserving()}
              >
                <span />
              </button>
            </div>
            <div className="observation-source-row">{sourceControls}</div>
            <div className="conversation-reset">
              {confirmClear ? <div role="group" aria-label="确认重新开始对话">
                <p>清空全部聊天、主动陪伴记录、近期摘要和活动状态，并停止当前回复。角色设定、用户档案和记忆会保留。删除后无法恢复。</p>
                <button type="button" className="text-button" disabled={clearingConversation} onClick={() => void clearConversation()}>{clearingConversation ? "正在清空…" : "确认清空对话"}</button>
                <button type="button" className="text-button" disabled={clearingConversation} onClick={() => setConfirmClear(false)}>取消</button>
              </div> : <button type="button" className="text-button" disabled={!ready} onClick={() => setConfirmClear(true)}>重新开始对话</button>}
            </div>
            <div ref={messageList} className="messages" aria-live="polite" aria-label="对话记录">
              {messages.length > 0 && <div className="translation-actions" aria-label="对话分页">
                <button type="button" className="text-button" disabled={historyLoading || !hasOlderMessages}
                  onClick={() => void loadHistory(messages[0]?.id)}>{historyLoading ? "加载中…" : hasOlderMessages ? "上一页 · 更早的对话" : "已到最早记录"}</button>
                <small>每页最多 50 条</small>
                {browsingHistory && <button type="button" className="text-button" disabled={historyLoading}
                  onClick={() => void loadHistory()}>返回最新对话</button>}
              </div>}
              {messages.length === 0 && (
                <div className="empty-chat">
                  <div className="quiet-art">
                    <span />
                    <span />
                    <span />
                  </div>
                  <h2>从一句随意的话开始</h2>
                  <p>
                    按住下方按钮说话，或者写点什么。
                    <br />
                    你也可以让我安静地陪着你。
                  </p>
                  {!hasKey && (
                    <button
                      className="secondary-button"
                      onClick={() => {
                        setSettingsTab("connection");
                        setPage("settings");
                      }}
                    >
                      设置 API 连接 <span>↗</span>
                    </button>
                  )}
                  <div className="suggestions">
                    {["今天陪我一起工作吧", "记住，我喜欢自然、有来有往的聊天"].map(
                      (s) => (
                        <button key={s} onClick={() => setInput(s)}>
                          {s} <span>↗</span>
                        </button>
                      ),
                    )}
                  </div>
                </div>
              )}
              {messages.map((message) => (
                <article
                  key={message.id}
                  className={`message ${message.role === "user" ? "user" : "assistant"}`}
                >
                  <div className="message-meta">
                    <span>
                      {message.role === "user"
                        ? "你"
                        : message.role === "assistant"
                          ? "Linger"
                          : message.role}
                    </span>
                    <time>{timestamp(message.created_at)}</time>
                  </div>
                  <div className="message-body">{message.role === "user" ? message.content : <MarkdownText text={message.content} />}</div>
                </article>
              ))}
              {!browsingHistory && streamText && (
                <article className="message assistant">
                  <div className="message-meta">
                    <span>Linger</span>
                    <span>正在回应</span>
                  </div>
                  <div className="message-body"><MarkdownText text={streamText} /></div>
                </article>
              )}
              {!browsingHistory && busy && (
                <div className="working-indicator">
                  <span className="pulse-dot" />
                  {status.detail || phaseLabel}
                </div>
              )}
              <div ref={bottom} />
            </div>
            <div className="composer-area">
              <form className="composer" onSubmit={send}>
                <textarea
                  aria-label="消息"
                  placeholder="说点什么，或者让我现在看看屏幕…"
                  value={input}
                  onChange={(e) => setInput(e.target.value)}
                  rows={2}
                  onKeyDown={(e) => {
                    if (
                      e.key === "Enter" &&
                      !e.shiftKey &&
                      !e.nativeEvent.isComposing
                    ) {
                      e.preventDefault();
                      e.currentTarget.form?.requestSubmit();
                    }
                  }}
                />
                <div className="composer-controls">
                  <span>Enter 发送 · Shift + Enter 换行</span>
                  <button
                    className="send-button"
                    disabled={!ready || !input.trim()}
                    aria-label="发送消息"
                  >
                    <Icon name="arrow" size={18} />
                  </button>
                </div>
              </form>
              <div className="playback-controls">
                <label className="playback-volume">
                  <span>播放音量</span>
                  <input
                    type="range"
                    aria-label="播放音量"
                    min="0"
                    max="1"
                    step="0.01"
                    value={settings.volume}
                    disabled={!ready}
                    onChange={(e) => void changeVolume(Number(e.target.value))}
                  />
                  <output>{Math.round(settings.volume * 100)}%</output>
                </label>
                <small>立即调整本机播放</small>
              </div>
              <div className="voice-row">
                <button
                  className="text-button floating-button"
                  title={settings.compact_visible ? "隐藏悬浮球，下次启动保持隐藏" : "显示悬浮球：对话记录、语音、快捷翻译"}
                  disabled={!ready}
                  onClick={() => void run("toggle_compact")}
                >
                  {settings.compact_visible ? "隐藏悬浮球" : "显示悬浮球 ↗"}
                </button>
                <div className="shortcut-hint">
                  <kbd>
                    {settings.shortcut
                      .replace("CommandOrControl", "⌘ / Ctrl")
                      .replaceAll("+", " + ")}
                  </kbd>
                  <span>全局按住说话</span>
                </div>
                <button
                  className={`ptt-button ${held || status.phase === "recording" ? "recording" : ""}`}
                  disabled={!ready}
                  onPointerDown={(e) => {
                    if (e.button !== 0) return;
                    e.preventDefault();
                    e.currentTarget.setPointerCapture(e.pointerId);
                    startRecording();
                  }}
                  onPointerUp={stopRecording}
                  onPointerCancel={stopRecording}
                  onLostPointerCapture={stopRecording}
                  onContextMenu={(e) => e.preventDefault()}
                  onKeyDown={(e) => {
                    if ((e.key === " " || e.key === "Enter") && !e.repeat) {
                      e.preventDefault();
                      startRecording();
                    }
                  }}
                  onKeyUp={(e) => {
                    if (e.key === " " || e.key === "Enter") {
                      e.preventDefault();
                      stopRecording();
                    }
                  }}
                  onBlur={stopRecording}
                >
                  <Icon name="mic" size={18} />
                  {held || status.phase === "recording"
                    ? "松开，发送语音"
                    : "按住说话"}
                </button>
                <button
                  className="icon-button stop-button"
                  title="停止当前回复和播报"
                  aria-label="停止当前回复和播报"
                  disabled={!ready}
                  onClick={() => void run("interrupt")}
                >
                  <Icon name="stop" size={17} />
                </button>
              </div>
            </div>
          </section>
        )}
        {page === "memory" && (
          <section className="content-page">
            <div className="page-heading">
              <div>
                <p className="eyebrow">THINGS THAT STAY</p>
                <h1>我记得的事</h1>
                <p className="subtitle">你可以随时补充、修改，或让我忘记。</p>
              </div>
              <button
                className="icon-button"
                aria-label="刷新记忆"
                disabled={!ready}
                onClick={() => void refreshMemories()}
              >
                <Icon name="refresh" />
              </button>
            </div>
            <UserProfileEditor ready={ready} />
            <form className="memory-composer" onSubmit={saveMemory}>
              <label htmlFor="memory-input">
                {editingMemory !== null ? "修改这条记忆" : "想让我记住什么？"}
              </label>
              <textarea
                id="memory-input"
                value={memoryInput}
                onChange={(e) => setMemoryInput(e.target.value)}
                placeholder="例如：工作时少提问，聊天时可以分享自己的想法。"
                rows={3}
              />
              <div>
                {editingMemory === null && (
                  <select
                    aria-label="记忆有效期"
                    className="memory-scope"
                    value={memoryScope}
                    onChange={(e) => setMemoryScope(e.target.value)}
                  >
                    <option value="long_term">长期记住</option>
                    <option value="today">仅限今天</option>
                    <option value="session">本次使用</option>
                  </select>
                )}
                {editingMemory !== null && (
                  <button
                    type="button"
                    className="text-button"
                    onClick={() => {
                      setEditingMemory(null);
                      setMemoryInput("");
                    }}
                  >
                    取消修改
                  </button>
                )}
                <button
                  className="primary-button"
                  disabled={!ready || !memoryInput.trim()}
                >
                  <Icon name="plus" size={16} />
                  {editingMemory !== null ? "保存修改" : "记住这件事"}
                </button>
              </div>
            </form>
            <div className="section-label">
              长期记忆 <span>{memories.length} 条</span>
            </div>
            {memories.length === 0 && (
              <div className="empty-memory">
                <Icon name="memory" size={32} />
                <p>还没有长期记忆</p>
                <small>聊天时说“帮我记住…”，也可以在这里添加。</small>
              </div>
            )}
            <div className="memory-grid">
              {memories.map((memory) => (
                <article className="memory-card" key={memory.id}>
                  <p>{memory.content}</p>
                  <footer>
                    <small>
                      {memory.updated_at.slice(0, 10)} ·{" "}
                      {memory.scope === "today"
                        ? "今天"
                        : memory.scope === "session"
                          ? "本次使用"
                          : "长期"}
                    </small>
                    <button
                      className="text-button"
                      onClick={() => {
                        setEditingMemory(memory.id);
                        setMemoryInput(memory.content);
                      }}
                    >
                      修改
                    </button>
                    <button
                      className="icon-button"
                      aria-label={`忘记：${memory.content}`}
                      onClick={async () => {
                        try {
                          await invoke("delete_memory", { id: memory.id });
                          if (editingMemory === memory.id) {
                            setEditingMemory(null);
                            setMemoryInput("");
                          }
                          await refreshMemories();
                        } catch (e) {
                          report(e);
                        }
                      }}
                    >
                      <Icon name="trash" size={16} />
                    </button>
                  </footer>
                </article>
              ))}
            </div>
          </section>
        )}
        {page === "settings" && (
          <section className="content-page settings-page">
            <div className="page-heading">
              <div>
                <p className="eyebrow">MAKE IT YOURS</p>
                <h1>舒服的陪伴方式</h1>
                <p className="subtitle">连接、声音与性格，都由你决定。</p>
              </div>
            </div>
            <div className="settings-tabs" role="tablist" aria-label="设置分类">
              {[
                { id: "connection", label: "API 与模型" },
                { id: "character", label: "角色卡" },
                { id: "tts", label: "语音连接" },
                { id: "audio", label: "音频设备" },
                { id: "companion", label: "陪伴与屏幕" },
              ].map((tab) => (
                <button
                  key={tab.id}
                  type="button"
                  role="tab"
                  aria-selected={settingsTab === tab.id}
                  className={settingsTab === tab.id ? "selected" : ""}
                  onClick={() => setSettingsTab(tab.id)}
                >
                  {tab.label}
                </button>
              ))}
            </div>
            <form onSubmit={saveSettings} className="settings-form">
              {settingsTab === "connection" && (
                <>
                  <div className="section-intro">
                    <h2>你的 API 连接</h2>
                    <p>默认连接 OpenAI，也可以使用局域网 NewAPI 中转。</p>
                  </div>
                  <Field
                    label="Base URL"
                    help="填写包含 /v1 的 API 根地址。保存后，模型请求会发送至这个地址。"
                  >
                    <input
                      required
                      type="url"
                      disabled={saving}
                      value={draft.base_url}
                      onChange={(e) => change("base_url", e.target.value)}
                      placeholder="https://api.openai.com/v1"
                    />
                  </Field>
                  <Field
                    label={`API Key ${hasKey ? "· 已保存" : "· 尚未设置"}`}
                    help="密钥保存在系统凭据存储中。留空保留现有密钥；切换中转地址时请填写对应密钥。"
                  >
                    <input
                      type="password"
                      disabled={saving}
                      autoComplete="off"
                      value={apiKey}
                      onChange={(e) => {
                        invalidateModels();
                        setApiKey(e.target.value);
                      }}
                      placeholder={
                        hasKey ? "留空保留已保存的 Key" : "输入你的 API Key"
                      }
                    />
                  </Field>
                  <div className="model-connection-actions">
                    <button
                      type="button"
                      className="secondary-button"
                      disabled={!ready || modelsLoading || saving}
                      onClick={() => void loadModels(draft.base_url, apiKey)}
                    >
                      <Icon name="refresh" size={16} />
                      {modelsLoading ? "正在读取…" : "读取可用模型"}
                    </button>
                    <small>可先读取再保存；保存 Key 后也会自动读取。</small>
                  </div>
                  <div className="section-intro separated">
                    <h2>模型分工</h2>
                    <p>从当前连接返回的列表中选择，无需手动填写模型名。</p>
                  </div>
                  {modelsError && (
                    <div className="model-feedback model-error" role="alert">
                      {modelsError}
                    </div>
                  )}
                  {modelsLoading && (
                    <div className="model-feedback" role="status">
                      正在读取当前连接的模型列表…
                    </div>
                  )}
                  {!modelsLoading && modelsLoaded && (
                    <div className="model-feedback" role="status">
                      {models.length
                        ? `已读取 ${models.length} 个模型。列表不包含能力信息，请按用途选择。`
                        : "服务返回了空模型列表，请检查当前 Key 的模型权限。"}
                    </div>
                  )}
                  {!modelsLoaded && !modelsLoading && !modelsError && (
                    <div className="model-feedback">
                      填写 Key 后点击“读取可用模型”，或保存连接后自动读取。
                    </div>
                  )}
                  <Field label="翻译目标语言" help="悬浮面板中的中文会翻译成此语言；保存后生效。">
                    <input list="translation-languages" value={draft.translation_language} onChange={(e) => change("translation_language", e.target.value)} placeholder="例如：英语、日语、韩语" maxLength={80} />
                    <datalist id="translation-languages">{["英语", "日语", "韩语", "法语", "德语", "西班牙语"].map(language => <option key={language} value={language} />)}</datalist>
                  </Field>
                  <Field label="默认翻译风格" help="保存后生效。保留原意、只输出译文；悬浮翻译框可临时切换风格。">
                    <select value={draft.translation_style} onChange={e => change("translation_style", e.target.value as Settings["translation_style"])}>
                      {translationStyles.map(style => <option key={style.value} value={style.value}>{style.label}</option>)}
                    </select>
                  </Field>
                  <Field label="自定义翻译要求" help="选择自定义风格时使用，最多 1000 字。用于控制语气、措辞和术语，不改变原意。">
                    <textarea value={draft.translation_custom_prompt} onChange={e => change("translation_custom_prompt", e.target.value)} maxLength={1000} rows={3} placeholder="例如：像游戏队友聊天，简短随意，保留游戏术语。" />
                  </Field>
                  <label className="checkbox-label">
                    <input type="checkbox" checked={settings.translation_auto_copy || false} disabled={!ready || autoCopySaving || saving}
                      onChange={(e) => { void setTranslationAutoCopy(e.target.checked).catch(report); }} />
                    翻译完成后自动复制到剪贴板
                  </label>
                  <p className="translation-help">立即保存生效，会替换剪贴板原有内容；仍可手动复制译文。</p>
                  <Field label="搜索模型">
                    <input
                      type="search"
                      value={modelQuery}
                      onChange={(e) => setModelQuery(e.target.value)}
                      placeholder="按模型名称筛选，例如 gpt、transcribe、tts"
                      disabled={!models.length}
                    />
                  </Field>
                  {models.length > 0 && !filteredModels.length && (
                    <p className="model-feedback">
                      没有匹配的模型，试试其他关键词。当前选择仍会保留。
                    </p>
                  )}
                  <div className="field-grid">
                    {(
                      [
                        ["conversation_model", "对话与视觉"],
                        ["asr_model", "语音识别 ASR"],
                        ["memory_model", "摘要与记忆整理"],
                        ["translation_model", "快捷翻译"],
                      ] as const
                    ).map(([field, label]) => (
                      <Field key={field} label={label}>
                        <select
                          aria-label={label}
                          value={draft[field]}
                          onChange={(e) => change(field, e.target.value)}
                          disabled={!models.length || modelsLoading}
                        >
                          {!filteredModels.includes(draft[field]) && (
                            <option value={draft[field]}>
                              {draft[field]}（
                              {modelsLoaded && !models.includes(draft[field])
                                ? "未在当前列表"
                                : "当前配置"}
                              ）
                            </option>
                          )}
                          {filteredModels.map((model) => (
                            <option key={model} value={model}>
                              {model}
                            </option>
                          ))}
                        </select>
                      </Field>
                    ))}
                  </div>
                </>
              )}
              {settingsTab === "character" && (
                <>
                  <div className="section-intro character-intro">
                    <p className="eyebrow">MEET YOUR COMPANION</p>
                    <h2>一种性格，一种陪伴的距离。</h2>
                    <p>
                      先从一张角色卡认识她，再把相处方式调整成你喜欢的样子。
                    </p>
                  </div>
                  <div className="character-grid" aria-label="陪伴角色卡">
                    {characters.map((character) => (
                      <button
                        type="button"
                        key={character.id}
                        className={`character-card ${draft.character_id === character.id ? "selected" : ""}`}
                        aria-pressed={draft.character_id === character.id}
                        onClick={() => selectCharacter(character.id)}
                      >
                        <div className="character-art">
                          <img
                            src={character.image}
                            alt={`${character.name}的角色插画`}
                          />
                          <span className="character-selection">
                            {draft.character_id === character.id
                              ? "已选择 ✓"
                              : "选择角色"}
                          </span>
                          <div className="character-name">
                            <small>{character.romanized}</small>
                            <strong>{character.name}</strong>
                          </div>
                        </div>
                        <div className="character-copy">
                          <div className="character-tags">
                            {character.tags.map((tag) => (
                              <span key={tag}>{tag}</span>
                            ))}
                          </div>
                          <p>{character.description}</p>
                          <small className="character-inner">
                            {character.inner}
                          </small>
                        </div>
                      </button>
                    ))}
                  </div>
                  <button
                    type="button"
                    className={`custom-character-option ${!selectedCharacter ? "selected" : ""}`}
                    aria-pressed={!selectedCharacter}
                    onClick={() => selectCharacter("custom")}
                  >
                    <span className="custom-avatar">L</span>
                    <span>
                      <strong>保留我的自定义角色</strong>
                      <small>沿用原来的设定，或从空白灵感自由调整。</small>
                    </span>
                    <span>{!selectedCharacter ? "已选择 ✓" : "选择 ↗"}</span>
                  </button>
                  <div className="character-editor">
                    <div className="section-intro">
                      <h2>
                        {selectedCharacter
                          ? `${selectedCharacter.name} · 相处方式`
                          : "自定义 · 相处方式"}
                      </h2>
                      <p>
                        图片传达气质，下面的设定决定实际表达。保存后生效，聊天与记忆会保留。
                      </p>
                    </div>
                    <Field label="角色设定">
                      <textarea
                        aria-label="角色设定"
                        rows={6}
                        value={draft.personality}
                        onChange={(e) => change("personality", e.target.value)}
                      />
                    </Field>
                    {selectedCharacter && (
                      <button
                        type="button"
                        className="text-button"
                        onClick={() =>
                          change("personality", selectedCharacter.personality)
                        }
                      >
                        恢复这张卡的默认设定
                      </button>
                    )}
                    <CharacterVoiceEditor key={draft.character_id || "custom"} value={cardTts} onChange={setCardTts}
                      settings={draft} characterId={draft.character_id || "custom"} characterName={selectedCharacter?.name || "自定义角色"}
                      models={models} ready={ready} report={report} />
                  </div>
                </>
              )}
              {settingsTab === "tts" && <TtsConnections settings={draft} onChange={connections => change("tts_connections", connections)} ready={ready} report={report} />}
              {settingsTab === "audio" && (
                <>
                  <div className="section-intro">
                    <h2>麦克风与扬声器</h2>
                    <p>保存后，从下一次录音或播放开始使用。角色的音色与说话方式仍在角色卡中设置。</p>
                  </div>
                  {([
                    ["audio_input_device", "录音输入 · 麦克风", audioDevices.inputs],
                    ["audio_output_device", "播放输出 · 扬声器 / 耳机", audioDevices.outputs],
                  ] as const).map(([key, label, options]) => (
                    <Field key={key} label={label} help="选定设备断开时会提示错误；选择系统默认则跟随系统切换。">
                      <select aria-label={label} value={draft[key] || ""} disabled={!ready || audioLoading}
                        onChange={(e) => change(key, e.target.value)}>
                        <option value="">跟随系统默认设备</option>
                        {draft[key] && !options.some((device) => device.id === draft[key]) && (
                          <option value={draft[key]}>{draft[key]} · 暂不可用{audioLoading ? "（正在读取）" : ""}</option>
                        )}
                        {options.map((device) => <option key={device.id} value={device.id}>{device.name}</option>)}
                      </select>
                    </Field>
                  ))}
                  <button type="button" className="secondary-button" disabled={!ready || audioLoading}
                    onClick={() => void refreshAudioDevices()}>
                    {audioLoading ? "正在读取设备…" : "刷新音频设备"}
                  </button>
                  {audioError && <p role="alert">读取音频设备失败：{audioError}</p>}
                </>
              )}
              {settingsTab === "companion" && (
                <>
                  <div className="section-intro">
                    <h2>陪伴节奏</h2>
                    <p>无需回应每句话，也能自然地陪在身边。</p>
                  </div>
                  <ShortcutRecorder
                    value={draft.shortcut}
                    disabled={!ready || saving || autoCopySaving || capturingShortcut}
                    onChange={(shortcut) => change("shortcut", shortcut)}
                    onBusy={setCapturingShortcut}
                    onError={report}
                  />
                  <div className="section-intro separated">
                    <h2>屏幕观察</h2>
                    <p>仅观察你选择的来源，截图默认不长期保存。</p>
                  </div>
                  <Field label="屏幕或窗口">
                    <div className="inline-input">
                      <select
                        value={draft.capture_source}
                        onChange={(e) =>
                          change("capture_source", e.target.value)
                        }
                        onFocus={() => {
                          if (sources.length === 0 && ready)
                            void refreshSources();
                        }}
                      >
                        <option value="monitor:primary">{sourceLabel("monitor:primary", sources)}</option>
                        {draft.capture_source && draft.capture_source !== "monitor:primary" &&
                          !sources.some(
                            (s) => s.id === draft.capture_source,
                          ) && (
                            <option value={draft.capture_source}>
                              {sourceLabel(draft.capture_source, sources)}
                            </option>
                          )}
                        {sources.filter(source => source.id !== "monitor:primary").map((source) => (
                          <option key={source.id} value={source.id}>
                            {source.name}
                          </option>
                        ))}
                      </select>
                      <button
                        type="button"
                        className="icon-button"
                        disabled={!ready}
                        aria-label="刷新屏幕与窗口"
                        onClick={() => void refreshSources()}
                      >
                        <Icon name="refresh" />
                      </button>
                    </div>
                  </Field>
                  <div className="region-settings">
                    <label className="checkbox-label">
                      <input
                        type="checkbox"
                        checked={!!regionParts}
                        disabled={
                          draft.capture_source === "monitor:primary" || (!draft.capture_source.startsWith("monitor:") &&
                          !regionParts)
                        }
                        onChange={(e) =>
                          change(
                            "capture_source",
                            e.target.checked
                              ? `region:${draft.capture_source.split(":")[1]}:0:0:800:600`
                              : `monitor:${regionParts?.[1] || ""}`,
                          )
                        }
                      />
                      仅观察屏幕中的指定区域
                    </label>
                    {draft.capture_source === "monitor:primary" && <p className="region-hint">需要区域裁剪时，请先选择列表中的具体显示器。</p>}
                    {regionParts && (
                      <>
                        <p className="region-hint">
                          以所选屏幕左上角为原点，使用原生像素坐标。范围超出屏幕时会报错。
                        </p>
                        <div className="region-fields">
                          {["X", "Y", "宽度", "高度"].map((label, index) => (
                            <Field key={label} label={label}>
                              <input
                                required
                                type="number"
                                min={index > 1 ? 1 : 0}
                                step="1"
                                value={regionParts[index + 2]}
                                onChange={(e) => {
                                  const next = [...regionParts];
                                  next[index + 2] = e.target.value;
                                  change("capture_source", next.join(":"));
                                }}
                              />
                            </Field>
                          ))}
                        </div>
                      </>
                    )}
                  </div>
                  <div className="field-grid">
                    <Field label="观察间隔">
                      <select
                        value={draft.observation_minutes}
                        onChange={(e) =>
                          change("observation_minutes", Number(e.target.value))
                        }
                      >
                        {[1, 2, 3, 5].map((v) => (
                          <option key={v} value={v}>
                            每 {v} 分钟
                          </option>
                        ))}
                      </select>
                    </Field>
                    <Field label="自动观察">
                      <select
                        value={String(draft.observing)}
                        onChange={(e) =>
                          change("observing", e.target.value === "true")
                        }
                      >
                        <option value="false">暂停</option>
                        <option value="true">开启</option>
                      </select>
                    </Field>
                  </div>
                  <Field
                    label="特别关注什么"
                    help="例如：重点看右侧的任务提示。此说明指导理解，不会自动改变实际截图范围。"
                  >
                    <textarea
                      rows={3}
                      value={draft.watch_instruction}
                      onChange={(e) =>
                        change("watch_instruction", e.target.value)
                      }
                      placeholder="可以留空，按照当前活动自然陪伴。"
                    />
                  </Field>
                </>
              )}
              <div className="settings-footer">
                <span role="status">{notice || "设置保存在本机"}</span>
                <button
                  className="primary-button"
                  disabled={!ready || saving || autoCopySaving || capturingShortcut}
                >
                  {saving ? "正在保存…" : "保存设置"}
                </button>
              </div>
            </form>
          </section>
        )}
      </main>
    </div>
  );
}
