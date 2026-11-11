import { buildCosyInstruction, cosyInstructionDetails, emotionNames, parseCosyInstruction } from "./cosyInstructionOptions";
import type { CosyInstructionChoice, CosyInstructionMode, CosyInstructionSpec } from "./cosyInstructionOptions";

export function CosyInstructions({ spec, instructions, onChange }: {
  spec: CosyInstructionSpec; instructions: string; onChange: (value: string) => void;
}) {
  const choice = parseCosyInstruction(spec, instructions);
  const modes: { id: CosyInstructionMode; name: string }[] = [
    { id: "none", name: "不指定" },
    ...(spec.emotions.length ? [{ id: "emotion" as const, name: "情绪" }] : []),
    ...(spec.scenes.length ? [{ id: "scene" as const, name: "场景 + 情绪" }] : []),
    ...(spec.roles.length ? [{ id: "role" as const, name: "角色 + 情绪" }] : []),
    ...(spec.identities.length ? [{ id: "identity" as const, name: "身份 + 情绪" }] : []),
    ...(spec.dialects.length ? [{ id: "dialect" as const, name: "方言" }] : []),
  ];
  const detailLabel = choice?.mode === "scene" ? "场景" : choice?.mode === "role" ? "角色" : choice?.mode === "identity" ? "身份" : "方言";
  function update(next: CosyInstructionChoice) {
    const text = buildCosyInstruction(spec, next);
    if (text !== null) onChange(text);
  }
  return <div className="cosy-instructions">
    <div className="section-intro"><h3>说话风格</h3><p>选好情绪和表达方式，会自动生成这个音色支持的指令。</p></div>
    {!choice && <div className="model-feedback model-error" role="alert">
      <p>原有声音指令不符合当前模型与音色的格式，已原样保留。请重新选择说话风格后再试听。</p>
      <label className="field"><span>保留的旧声音指令</span><textarea aria-label="保留的旧声音指令" readOnly rows={3} value={instructions} /></label>
    </div>}
    <div className="field-grid">
      <label className="field"><span>风格模式</span><select aria-label="风格模式" required value={choice?.mode ?? ""} onChange={event => {
        const mode = event.target.value as CosyInstructionMode;
        update({ mode, emotion: choice?.emotion || "neutral", detail: cosyInstructionDetails(spec, mode)[0] ?? "" });
      }}>
        {!choice && <option value="" disabled>请选择有效的风格模式</option>}
        {modes.map(mode => <option key={mode.id} value={mode.id}>{mode.name}</option>)}
      </select></label>
      {choice && !["none", "emotion"].includes(choice.mode) && <label className="field"><span>{detailLabel}</span><select aria-label={`风格${detailLabel}`} value={choice.detail} onChange={event => update({ ...choice, detail: event.target.value })}>
        {cosyInstructionDetails(spec, choice.mode).map(detail => <option key={detail}>{detail}</option>)}
      </select></label>}
      {choice && !["none", "dialect"].includes(choice.mode) && <label className="field"><span>情绪</span><select aria-label="风格情绪" value={choice.emotion} onChange={event => update({ ...choice, emotion: event.target.value })}>
        {spec.emotions.map(emotion => <option key={emotion} value={emotion}>{emotionNames[emotion] ?? emotion}</option>)}
      </select></label>}
    </div>
    {choice && <label className="field"><span>生成的声音指令</span><textarea aria-label="生成的声音指令" readOnly rows={2} value={instructions} placeholder="不指定额外的声音风格" /></label>}
  </div>;
}
