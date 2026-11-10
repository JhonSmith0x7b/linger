export type ShortcutKeyEvent = Pick<
  KeyboardEvent,
  "code" | "ctrlKey" | "altKey" | "shiftKey" | "metaKey"
>;
const modifiers = new Set([
  "ControlLeft",
  "ControlRight",
  "AltLeft",
  "AltRight",
  "ShiftLeft",
  "ShiftRight",
  "MetaLeft",
  "MetaRight",
  "Fn",
  "FnLock",
]);
const supported = new Set([
  "Backquote",
  "Backslash",
  "BracketLeft",
  "BracketRight",
  "Comma",
  "Equal",
  "Minus",
  "Period",
  "Quote",
  "Semicolon",
  "Slash",
  "Space",
  "Enter",
  "Tab",
  "Backspace",
  "Delete",
  "CapsLock",
  "Home",
  "End",
  "PageUp",
  "PageDown",
  "Insert",
  "PrintScreen",
  "ScrollLock",
  "Pause",
  "ArrowUp",
  "ArrowDown",
  "ArrowLeft",
  "ArrowRight",
  "NumLock",
  "NumpadAdd",
  "NumpadDecimal",
  "NumpadDivide",
  "NumpadEnter",
  "NumpadEqual",
  "NumpadMultiply",
  "NumpadSubtract",
]);
/** Match global-hotkey's physical-key accelerator names, independent of keyboard layout. */
export function shortcutFromEvent(event: ShortcutKeyEvent): string | null {
  if (modifiers.has(event.code)) return null;
  if (
    !supported.has(event.code) &&
    !/^Key[A-Z]$/.test(event.code) &&
    !/^Digit[0-9]$/.test(event.code) &&
    !/^Numpad[0-9]$/.test(event.code) &&
    !/^F([1-9]|1[0-9]|2[0-4])$/.test(event.code)
  ) {
    throw new Error("这个按键暂不支持，请换一个组合。");
  }
  return [
    event.ctrlKey && "Control",
    event.altKey && "Alt",
    event.shiftKey && "Shift",
    event.metaKey && "Super",
    event.code,
  ]
    .filter(Boolean)
    .join("+");
}
export function shortcutLabel(shortcut: string): string {
  return shortcut
    .split("+")
    .map(
      (key) =>
        ({
          CommandOrControl: "⌘ / Ctrl",
          Control: "Ctrl",
          Super: "⌘ / Win",
          Space: "空格",
          Shift: "Shift",
          Alt: "Alt / ⌥",
        })[key] ?? key.replace(/^Key/, "").replace(/^Digit/, ""),
    )
    .join(" + ");
}
