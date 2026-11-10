import type { CaptureSource } from "./types";

export function sourceLabel(source: string, sources: CaptureSource[]) {
  return sources.find(item => item.id === source)?.name || (source === "monitor:primary" || !source ? "主屏幕（默认）" : source.startsWith("region:") ? "已设置的屏幕区域" : source.startsWith("window:") ? "已选择的窗口（刷新查看名称）" : "已选择的显示器（刷新查看名称）");
}

export function ScreenSourcePicker({ value, sources, disabled, loading, onChange, onRefresh, onPermission, notice }: {
  value: string; sources: CaptureSource[]; disabled: boolean; loading: boolean;
  onChange: (source: string) => void; onRefresh: () => void; onPermission: () => void; notice: string;
}) {
  const selected = value || "monitor:primary";
  return <div className="screen-source-picker">
    <label>观察来源
      <select aria-label="观察来源" value={selected} disabled={disabled} onFocus={() => { if (!sources.length && !loading) onRefresh(); }} onChange={e => onChange(e.target.value)}>
        <option value="monitor:primary">{sourceLabel("monitor:primary", sources)}</option>
        {selected !== "monitor:primary" && !sources.some(item => item.id === selected) && <option value={selected}>{sourceLabel(selected, sources)}</option>}
        {sources.filter(item => item.id !== "monitor:primary").map(item => <option key={item.id} value={item.id}>{item.name}</option>)}
      </select>
    </label>
    <div className="source-actions"><button type="button" disabled={disabled || loading} onClick={onRefresh}>{loading ? "读取中…" : "刷新来源"}</button><button type="button" disabled={disabled} onClick={onPermission}>检查录屏权限</button></div>
    {notice && <small role="status">{notice}</small>}
  </div>;
}
