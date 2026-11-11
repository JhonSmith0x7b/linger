import catalog from "./cosy-instructions.json";

// Official per-model capabilities, shared with Rust request validation.
// https://help.aliyun.com/zh/model-studio/cosyvoice-voice-list
export interface CosyInstructionSpec {
  model: string;
  voice: string;
  emotions: string[];
  scenes: string[];
  rolePrefix: string;
  roles: string[];
  identities: string[];
  dialects: string[];
}
export type CosyInstructionMode = "none" | "emotion" | "scene" | "role" | "identity" | "dialect";
export interface CosyInstructionChoice { mode: CosyInstructionMode; emotion: string; detail: string }
export const emotionNames: Record<string, string> = {
  neutral: "自然", fearful: "害怕", angry: "生气", sad: "难过", surprised: "惊讶", happy: "开心", disgusted: "厌恶",
};
export function cosyInstructionSpec(model: string, voice: string): CosyInstructionSpec | undefined {
  return catalog.find(spec => spec.model === model && spec.voice === voice);
}
export function cosyInstructionDetails(spec: CosyInstructionSpec, mode: CosyInstructionMode): string[] {
  return mode === "scene" ? spec.scenes : mode === "role" ? spec.roles : mode === "identity" ? spec.identities : mode === "dialect" ? spec.dialects : [];
}
export function buildCosyInstruction(spec: CosyInstructionSpec, choice: CosyInstructionChoice): string | null {
  const { mode, emotion, detail } = choice;
  if (mode === "none") return "";
  if (mode === "dialect") return spec.dialects.includes(detail) ? `请用${detail}表达。` : null;
  if (!spec.emotions.includes(emotion)) return null;
  if (mode === "emotion") return `你说话的情感是${emotion}。`;
  if (!cosyInstructionDetails(spec, mode).includes(detail)) return null;
  if (mode === "scene") return `你正在进行${detail}，你说话的情感是${emotion}。`;
  if (mode === "role") return `${spec.rolePrefix}${detail}，你说话的情感是${emotion}。`;
  if (mode === "identity") return `你正在以一个${detail}的身份说话，你说话的情感是${emotion}。`;
  return null;
}
export function parseCosyInstruction(spec: CosyInstructionSpec, text: string): CosyInstructionChoice | null {
  if (text === "") return { mode: "none", emotion: "neutral", detail: "" };
  for (const emotion of spec.emotions) {
    const choice: CosyInstructionChoice = { mode: "emotion", emotion, detail: "" };
    if (buildCosyInstruction(spec, choice) === text) return choice;
    for (const mode of ["scene", "role", "identity"] as const) {
      for (const detail of cosyInstructionDetails(spec, mode)) {
        const option = { mode, emotion, detail };
        if (buildCosyInstruction(spec, option) === text) return option;
      }
    }
  }
  for (const detail of spec.dialects) {
    const choice: CosyInstructionChoice = { mode: "dialect", emotion: "neutral", detail };
    if (buildCosyInstruction(spec, choice) === text) return choice;
  }
  return null;
}
