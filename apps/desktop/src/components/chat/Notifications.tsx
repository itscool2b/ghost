import { Bell } from "lucide-react";
import { Fragment, type ReactNode, useEffect, useRef, useState } from "react";
import { Popover } from "./Popover";

/** One notice in the panel: a list item (a toast's row, an agent's update, the repair notice). */
export interface NoticeItem {
  readonly key: string;
  /** A failure or a warning keeps the bell marked until it is settled; other notices are news. */
  readonly level?: "warning" | "error";
  readonly node: ReactNode;
}

/** What the bell's dot says: a failure, a warning, news not yet looked at, or nothing. */
export type BellAttention = "error" | "warning" | "new" | null;

export function bellAttention(
  items: readonly NoticeItem[],
  seen: ReadonlySet<string>,
): BellAttention {
  if (items.some((item) => item.level === "error")) return "error";
  if (items.some((item) => item.level === "warning")) return "warning";
  if (items.some((item) => !seen.has(item.key))) return "new";
  return null;
}

/**
 * The number on the bell: the notices that fail or warn while one does, else the ones not yet seen.
 * Nothing while there is nothing to look at.
 */
export function bellCount(
  items: readonly NoticeItem[],
  seen: ReadonlySet<string>,
  attention: BellAttention,
): string | null {
  if (attention === null) return null;
  const count =
    attention === "new"
      ? items.filter((item) => !seen.has(item.key)).length
      : items.filter((item) => item.level === attention).length;
  return count > 9 ? "9+" : String(count);
}

const ATTENTION_WORD: Record<Exclude<BellAttention, null>, string> = {
  error: "one needs attention",
  warning: "one needs attention",
  new: "new",
};

/**
 * The window's notifications, behind a bell at the foot of the sidebar beside Settings (owner,
 * 7 November 2026, of the toasts that floated over the pane: "the notifications thing still
 * sucks"). Agents' new releases, startup notices, the database repair notice and agents' memory
 * edits wait here with their actions, each until its X or until it no longer applies. While
 * something is new the bell lights up with a count on its shoulder and swings once as each notice
 * arrives (owner, 27 November 2026: notifications such as agents' updates "should be more
 * obvious"); the count turns amber or red while one needs attention. Only what cannot wait also
 * shows over the pane (`Banners`). Opening the panel counts its notices as seen.
 */
export function Notifications({ items }: { readonly items: readonly NoticeItem[] }) {
  const [seen, setSeen] = useState<ReadonlySet<string>>(new Set());
  const [open, setOpen] = useState(false);
  // While the panel is open, whatever arrives is seen as it arrives.
  const keys = items.map((item) => item.key).join("\n");
  useEffect(() => {
    if (!open || keys === "") return;
    setSeen((current) => {
      const next = new Set(current);
      for (const key of keys.split("\n")) next.add(key);
      return next.size === current.size ? current : next;
    });
  }, [open, keys]);
  const attention = bellAttention(items, seen);
  const fresh = items.filter((item) => !seen.has(item.key)).length;
  const count = bellCount(items, seen, attention);
  // Each arrival swings the bell once: the icon is drawn anew, which restarts its animation.
  const [rings, setRings] = useState(0);
  const before = useRef(fresh);
  useEffect(() => {
    if (fresh > before.current && !open) setRings((value) => value + 1);
    before.current = fresh;
  }, [fresh, open]);
  const label =
    items.length === 0
      ? "Notifications, none"
      : `Notifications, ${items.length}${
          attention === "error" || attention === "warning"
            ? `, ${ATTENTION_WORD[attention]}`
            : fresh > 0
              ? `, ${fresh} new`
              : ""
        }`;
  return (
    <Popover
      className="chat-inbox"
      label="Notifications"
      triggerLabel={label}
      tip="Notifications"
      onOpenChange={setOpen}
      trigger={
        <span
          className="chat-inbox-bell"
          data-testid="notifications-bell"
          data-tour="notifications"
          data-attention={attention ?? undefined}
        >
          <Bell key={rings} size={16} aria-hidden="true" data-ring={rings > 0 ? "" : undefined} />
          {count !== null ? (
            <span className="chat-inbox-count" aria-hidden="true">
              {count}
            </span>
          ) : null}
        </span>
      }
    >
      <h2 className="chat-inbox-title">Notifications</h2>
      {items.length > 0 ? (
        <ul className="chat-inbox-list" data-testid="notifications">
          {items.map((item) => (
            <Fragment key={item.key}>{item.node}</Fragment>
          ))}
        </ul>
      ) : (
        <p className="chat-inbox-empty">No notifications.</p>
      )}
    </Popover>
  );
}
