import type { VoiceProfile } from "./types";
import presets from "./character-presets.json";

export interface CharacterPreset {
  id: string;
  name: string;
  romanized: string;
  image: string;
  tags: string[];
  description: string;
  inner: string;
  greeting: string;
  personality: string;
  instructions: string;
  voice: string;
  speed: number;
}

export const characters: CharacterPreset[] = presets;

export function characterFor(id?: string) {
  return characters.find((character) => character.id === id);
}

export function defaultVoiceProfile(id = "custom"): VoiceProfile {
  const character = characterFor(id);
  return {
    voice: character?.voice ?? "coral",
    instructions:
      character?.instructions ??
      "自然、温暖地说中文，像熟悉的朋友一样，不要夸张。",
    speed: character?.speed ?? 1,
  };
}
