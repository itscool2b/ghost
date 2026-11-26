import { Keycap } from "@ghost/ui/graphite";

/** The platform's command key as a keycap shows it: ⌘ on macOS, Ctrl on Windows. */
export function commandKey(): string {
  return typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.userAgent)
    ? "⌘"
    : "Ctrl";
}

/**
 * A shortcut as keycaps, one per key (4.4). Decorative: the control it sits in names the shortcut
 * itself through `aria-keyshortcuts`.
 */
export function Keys({ keys }: { readonly keys: readonly string[] }) {
  return (
    <span className="chat-keys" aria-hidden="true">
      {keys.map((key) => (
        <Keycap key={key}>{key}</Keycap>
      ))}
    </span>
  );
}
