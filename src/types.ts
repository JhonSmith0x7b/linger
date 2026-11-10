export interface VoiceProfile {
  voice: string;
  instructions: string;
  speed: number;
}
export interface Settings {
  base_url: string;
  conversation_model: string;
  asr_model: string;
  memory_model: string;
  tts_model: string;
  translation_model: string;
  translation_language: string;
  response_format: string;
  volume: number;
  audio_input_device: string;
  audio_output_device: string;
  personality: string;
  character_id?: string;
  character_personalities?: Record<string, string>;
  character_voices?: Record<string, VoiceProfile>;
  observation_minutes: number;
  observing: boolean;
  capture_source: string;
  watch_instruction: string;
  shortcut: string;
}
export interface Message {
  id: number;
  role: string;
  content: string;
  created_at: string;
}
export interface Memory {
  id: number;
  content: string;
  updated_at: string;
  scope?: string;
  expires_at?: string | null;
}
export interface CaptureSource {
  id: string;
  name: string;
}
export interface Status {
  phase: string;
  detail: string;
}

export interface ObservationStatus {
  phase: "idle" | "waiting" | "started" | "captured" | "silent" | "replied" | "error" | "paused";
  detail: string;
  at: string | null;
  captured_at: string | null;
}

export interface AudioDevices {
  inputs: { id: string; name: string }[];
  outputs: { id: string; name: string }[];
}
