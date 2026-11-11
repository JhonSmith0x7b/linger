import type { CharacterTts, Settings, TtsProfile, TtsProvider } from "./types";
import { defaultVoiceProfile } from "./characters";

export const ttsProviders: { id: TtsProvider; name: string }[] = [
  { id: "openai", name: "OpenAI" },
  { id: "cosyvoice", name: "阿里云 · CosyVoice" },
  { id: "minimax", name: "阿里云 · MiniMax" },
];
export const minimaxEndpoint = "https://dashscope.aliyuncs.com/api/v1/services/aigc/multimodal-generation/generation";
export const cosyEndpointHint = "https://你的业务空间ID.cn-beijing.maas.aliyuncs.com/api/v1/services/audio/tts/SpeechSynthesizer";
export function ttsEndpoint(settings: Settings, provider: TtsProvider) {
  return settings.tts_connections?.[provider]?.endpoint ?? (provider === "minimax" ? minimaxEndpoint : "");
}
export function defaultTtsProfile(provider: TtsProvider, settings: Settings, characterId: string): TtsProfile {
  const shared = { speed: 1, pitch: 0, emotion: "", instructions: "", format: "wav", sample_rate: 24000 };
  if (provider === "cosyvoice") return { ...shared, model: "cosyvoice-v3-flash", voice: "longanyang", pitch: 1 };
  if (provider === "minimax") return { ...shared, model: "MiniMax/speech-2.8-turbo", voice: "male-qn-qingse", format: "mp3", sample_rate: 32000 };
  const legacy = { ...shared, ...(settings.character_voices?.[characterId] ?? defaultVoiceProfile(characterId)), model: settings.tts_model, format: settings.response_format };
  // Older cards stored this field even for models that cannot accept it.
  if (["tts-1", "tts-1-hd"].includes(legacy.model)) legacy.instructions = "";
  return legacy;
}
export function characterTts(settings: Settings, characterId: string): CharacterTts {
  return settings.character_tts?.[characterId] ?? { provider: "openai", profiles: { openai: defaultTtsProfile("openai", settings, characterId) } };
}
export const ttsModels: Record<TtsProvider, string[]> = {
  openai: ["gpt-4o-mini-tts", "tts-1", "tts-1-hd"],
  cosyvoice: ["cosyvoice-v3-flash", "cosyvoice-v3-plus", "cosyvoice-v3.5-flash", "cosyvoice-v3.5-plus", "cosyvoice-v2"],
  minimax: ["MiniMax/speech-2.8-turbo", "MiniMax/speech-2.8-hd", "MiniMax/speech-02-turbo", "MiniMax/speech-02-hd"],
};
// Small, verified official presets. Custom IDs remain available for other system or cloned voices.
// https://help.aliyun.com/zh/model-studio/cosyvoice-voice-list
// https://help.aliyun.com/zh/model-studio/minimax-synchronous-speech-synthesis-api
// MiniMax system voices: https://platform.minimaxi.com/document/T2A%20V2?key=66719005a427f0c8a5701643
export function ttsVoices(provider: TtsProvider, model: string): { id: string; name: string }[] {
  if (provider === "openai" && ["tts-1", "tts-1-hd"].includes(model)) return ["alloy", "echo", "fable", "onyx", "nova", "shimmer"].map(id => ({ id, name: id }));
  if (provider === "openai") return ["alloy", "ash", "ballad", "coral", "echo", "fable", "onyx", "nova", "sage", "shimmer", "verse", "marin", "cedar"].map(id => ({ id, name: id }));
  if (provider === "minimax") return [
    { id: "male-qn-qingse", name: "青涩青年 · male-qn-qingse" },
    { id: "female-shaonv", name: "少女 · female-shaonv" },
    { id: "female-yujie", name: "御姐 · female-yujie" },
    { id: "female-chengshu", name: "成熟女性 · female-chengshu" },
    { id: "female-tianmei", name: "甜美女性 · female-tianmei" },
  ];
  if (model === "cosyvoice-v2") return [
    { id: "longanrou", name: "龙安柔 · 温柔闺蜜" },
    { id: "longanqin", name: "龙安亲 · 亲和活泼" },
    { id: "longanwen", name: "龙安温 · 优雅知性" },
    { id: "longanyun", name: "龙安昀 · 居家暖男" },
  ];
  if (model === "cosyvoice-v3-flash" || model === "cosyvoice-v3-plus") return [
    { id: "longanyang", name: "龙安洋 · 阳光男声" }, { id: "longanhuan", name: "龙安欢 · 元气女声" },
  ];
  return [];
}
export function supportsInstructions(provider: TtsProvider, profile: TtsProfile) {
  if (provider === "openai") return !["tts-1", "tts-1-hd"].includes(profile.model);
  if (provider === "minimax" || profile.model === "cosyvoice-v2") return false;
  if (profile.model === "cosyvoice-v3-plus") return !profile.voice.startsWith("cosyvoice-");
  return true;
}
