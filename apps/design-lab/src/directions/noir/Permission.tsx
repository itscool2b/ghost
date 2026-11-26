import { GlassPanel } from "@ghost/ui";
import { Check, ShieldAlert, X } from "lucide-react";
import { createContext, useContext, useEffect, useRef } from "react";
import {
  AUTONOMY_LABELS,
  type Autonomy,
  type PermissionModel,
  type PermissionOption,
  type PermissionOptionKind,
} from "../../kit";
import { centreInTranscript, Kbd, shareInView, useReduced } from "./ui";

/**
 * True when a key press belongs to whatever has focus: a text field or a menu. The composer's field
 * is the exception: while a request waits, its Esc and Cmd+Enter answer the request.
 */
export function ownsKeys(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target.matches("[data-composer-field]")) return false;
  return (
    target.isContentEditable ||
    ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName) ||
    target.closest("[role='menu'], [role='dialog']") !== null
  );
}

/**
 * Answers given in this window. The recording ends on the question, so the answer is kept here,
 * where the card and the composer can both give it.
 */
export interface PermissionAnswers {
  readonly answers: ReadonlyMap<string, PermissionOption>;
  readonly answer: (permission: PermissionModel, option: PermissionOption) => void;
}

const AnswersContext = createContext<PermissionAnswers>({ answers: new Map(), answer: () => {} });
export const PermissionAnswersProvider = AnswersContext.Provider;

/** The answer that turns a request down once, for the composer's "do this instead". */
export function rejectOption(permission: PermissionModel): PermissionOption | null {
  return (
    permission.options.find((option) => option.kind === "reject_once") ??
    permission.options.find((option) => option.kind === "reject_always") ??
    null
  );
}

type Shortcut = "mod-enter" | "escape";

const SHORTCUT_KEYS: Readonly<Record<Shortcut, string>> = {
  "mod-enter": "⌘⏎",
  escape: "Esc",
};

/**
 * Cmd+Enter allows once, Esc rejects. Always allowing is the broadest answer, so it has no key:
 * it takes a click.
 */
function shortcutFor(kind: PermissionOptionKind): Shortcut | null {
  if (kind === "allow_once") return "mod-enter";
  if (kind === "reject_once" || kind === "reject_always") return "escape";
  return null;
}

const RESOLVED: Readonly<Record<PermissionOptionKind, string>> = {
  allow_once: "Allowed once",
  allow_always: "Always allowed",
  reject_once: "Rejected",
  reject_always: "Always rejected",
};

const AUTONOMY_NOTE: Readonly<Record<Autonomy, string>> = {
  full_auto: "Full auto answers requests like this one for you.",
  auto_edit: "edits files on its own in Auto-edit, and asks before it runs commands.",
  ask: "asks before every tool in Ask mode.",
};

/** A key only answers when at least this much of the card shows; else it brings the card into view. */
const MIN_SHARE = 0.5;

/**
 * An inline permission card, where the agent asked. The amber rule and icon are the only colour:
 * it is the one thing in the window waiting on the user. Its keys work from anywhere but a text
 * field, and only while the card can be seen.
 */
export function PermissionCard({
  permission,
  agentName,
  autonomy,
}: {
  readonly permission: PermissionModel;
  readonly agentName: string;
  readonly autonomy: Autonomy;
}) {
  const reduced = useReduced();
  const { answers, answer } = useContext(AnswersContext);
  const local = answers.get(permission.requestId) ?? null;
  const pending = permission.status === "pending" && local === null;
  const cardRef = useRef<HTMLDivElement>(null);
  const latest = useRef({ permission, answer, reduced });
  latest.current = { permission, answer, reduced };

  useEffect(() => {
    if (!pending) return;
    const onKey = (event: KeyboardEvent): void => {
      if (event.defaultPrevented || ownsKeys(event.target) || event.altKey) return;
      const mod = event.metaKey || event.ctrlKey;
      const wanted: Shortcut | null =
        event.key === "Enter" && mod
          ? "mod-enter"
          : event.key === "Escape" && !mod
            ? "escape"
            : null;
      if (!wanted) return;
      const { permission: current, answer: give, reduced: still } = latest.current;
      const option = current.options.find((entry) => shortcutFor(entry.kind) === wanted);
      const card = cardRef.current;
      if (!option || !card) return;
      event.preventDefault();
      // A request scrolled out of sight is never answered blind: the first press shows it.
      if (shareInView(card) < MIN_SHARE) {
        centreInTranscript(card, !still);
        return;
      }
      give(current, option);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [pending]);

  const command = permission.tool?.terminal?.command ?? permission.tool?.target ?? null;
  const resolvedKind =
    local?.kind ??
    permission.options.find((option) => option.option_id === permission.selectedOptionId)?.kind ??
    null;

  if (!pending) {
    const allowed = resolvedKind === "allow_once" || resolvedKind === "allow_always";
    return (
      <div className="n-perm-done" data-allowed={allowed ? "" : undefined}>
        <span className="n-perm-done-glyph" aria-hidden="true">
          {allowed ? <Check size={14} strokeWidth={1.8} /> : <X size={14} strokeWidth={1.8} />}
        </span>
        <span className="n-perm-done-label">
          {resolvedKind ? RESOLVED[resolvedKind] : "Request closed"}
        </span>
        <span className="n-perm-done-what">{permission.title}</span>
      </div>
    );
  }

  return (
    <GlassPanel ref={cardRef} elevation={2} squircle radius={16} className="n-perm">
      <div className="n-perm-head">
        <ShieldAlert size={16} strokeWidth={1.7} aria-hidden="true" className="n-perm-icon" />
        <span className="n-perm-kicker">Approval needed</span>
        <span className="n-perm-mode">{AUTONOMY_LABELS[autonomy]}</span>
      </div>
      <p className="n-perm-title">{permission.title}</p>
      {command ? (
        <div className="n-perm-command">
          <span className="n-term-prompt" aria-hidden="true">
            $
          </span>
          <code>{command}</code>
        </div>
      ) : null}
      <div className="n-perm-actions">
        {permission.options.map((option, index) => {
          const shortcut = shortcutFor(option.kind);
          return (
            <button
              key={option.option_id}
              type="button"
              className="n-perm-btn"
              data-kind={option.kind}
              data-primary={index === 0 ? "" : undefined}
              aria-keyshortcuts={
                shortcut === "mod-enter"
                  ? "Meta+Enter"
                  : shortcut === "escape"
                    ? "Escape"
                    : undefined
              }
              onClick={() => answer(permission, option)}
            >
              {option.label}
              {shortcut ? <Kbd>{SHORTCUT_KEYS[shortcut]}</Kbd> : null}
            </button>
          );
        })}
      </div>
      <p className="n-perm-note">
        {autonomy === "full_auto"
          ? AUTONOMY_NOTE.full_auto
          : `${agentName} ${AUTONOMY_NOTE[autonomy]}`}
      </p>
    </GlassPanel>
  );
}
