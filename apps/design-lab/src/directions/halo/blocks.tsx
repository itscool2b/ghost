import { usePrefersReducedMotion } from "@ghost/ui";
import {
  ArrowRightLeft,
  Check,
  ChevronDown,
  Clock,
  FilePen,
  FilePlus2,
  FileText,
  Globe,
  type LucideIcon,
  ScanSearch,
  Search,
  ShieldAlert,
  Sparkles,
  SquareTerminal,
  Trash2,
  Wrench,
  X,
} from "lucide-react";
import {
  type CSSProperties,
  Fragment,
  memo,
  type ReactNode,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  type AnsiColor,
  type AnsiLine,
  type AnsiSpan,
  type Autonomy,
  basename,
  type DiffLine,
  dirname,
  type FileDiff,
  formatDuration,
  formatElapsed,
  type PermissionModel,
  type PermissionOptionKind,
  type Player,
  type ThoughtItem,
  type ToolCallModel,
  type ToolFamily,
  type ToolKind,
  type ToolRunItem,
  usePlayhead,
} from "../../kit";
import { Collapse, cx, Glass, Kbd, LineCounts, RADIUS, Spinner, useEntrance } from "./primitives";
import { CodeLine, hangOf, Markdown } from "./text";

/* ------------------------------------------------------------------------------- live time */

/** Seconds since `since` on the timeline, re-rendering once a second. */
export function LiveElapsed({
  player,
  since,
  className,
}: {
  readonly player: Player;
  readonly since: number;
  readonly className?: string;
}) {
  const now = usePlayhead(player, 1000);
  return <span className={cx("halo-num", className)}>{formatElapsed(now - since)}</span>;
}

/* --------------------------------------------------------------------------------- thought */

/** Codex opens its reasoning with a bold title ("**Planning the ACP client**"): lift it out. */
function splitThought(text: string): { readonly title: string | null; readonly body: string } {
  const match = /^\s*\*\*([^*\n]{1,80})\*\*[ \t]*(?:\n+|$)/.exec(text);
  if (!match) return { title: null, body: text };
  return { title: match[1]?.trim() ?? null, body: text.slice(match[0].length) };
}

/**
 * The thought pill. While the agent thinks it reads "Thinking" and the thought scrolls past
 * underneath, a few lines at a time (the turn's one clock is on the status line); when the thought
 * ends it folds into "Thought for 6s", which opens the whole thought on click.
 */
export const ThoughtBlock = memo(function ThoughtBlock({ item }: { readonly item: ThoughtItem }) {
  const thinking = item.durationMs === null;
  const [open, setOpen] = useState(thinking);
  const [wasThinking, setWasThinking] = useState(thinking);
  // Adjusted during render, so the frame that ends the thought already starts folding it.
  if (wasThinking !== thinking) {
    setWasThinking(thinking);
    setOpen(thinking);
  }
  const { title, body } = useMemo(() => splitThought(item.text), [item.text]);

  return (
    <div className="halo-thought" data-thinking={thinking ? "" : undefined}>
      <button
        type="button"
        className="halo-pill-button halo-thought-pill"
        data-halo-light=""
        aria-expanded={open}
        onClick={() => setOpen((value) => !value)}
      >
        <Sparkles size={14} className="halo-thought-icon" aria-hidden="true" />
        {thinking ? (
          <span className="halo-shimmer">Thinking</span>
        ) : (
          <span>Thought for {formatDuration(item.durationMs ?? 0)}</span>
        )}
        {title ? <span className="halo-thought-title">{title}</span> : null}
        <ChevronDown size={14} className="halo-chevron" aria-hidden="true" />
      </button>
      <Collapse open={open}>
        <div className="halo-thought-well">
          {thinking ? (
            <Teleprompter>
              <Markdown text={body} streaming={item.streaming} variant="thought" />
            </Teleprompter>
          ) : (
            <Markdown text={body} streaming={false} variant="thought" />
          )}
        </div>
      </Collapse>
    </div>
  );
});

/** Reads the vertical offset a running transform has reached, so a new glide continues from it. */
function currentOffsetY(element: HTMLElement): number {
  const transform = getComputedStyle(element).transform;
  if (!transform || transform === "none") return 0;
  const values = /matrix(?:3d)?\(([^)]+)\)/.exec(transform)?.[1]?.split(",") ?? [];
  const ty = Number(values.length === 16 ? values[13] : values[5]);
  return Number.isFinite(ty) ? ty : 0;
}

/**
 * A few lines of live text, newest at the bottom: when a new line wraps in, the text glides up
 * instead of jumping, continuing from wherever the last glide had got to. The top edge fades only
 * once earlier lines have scrolled out of the window, so the first line never reads as disabled.
 */
function Teleprompter({ children }: { readonly children: ReactNode }) {
  const outerRef = useRef<HTMLDivElement>(null);
  const innerRef = useRef<HTMLDivElement>(null);
  const reduced = usePrefersReducedMotion();
  useLayoutEffect(() => {
    const outer = outerRef.current;
    const inner = innerRef.current;
    if (!outer || !inner) return;
    // Written to the DOM straight away: a growing thought costs no React render.
    const mark = (): void => {
      outer.toggleAttribute("data-overflow", inner.offsetHeight > outer.clientHeight + 1);
    };
    mark();
    if (typeof ResizeObserver !== "function") return;
    let last = inner.offsetHeight;
    let glide: Animation | null = null;
    const observer = new ResizeObserver(() => {
      mark();
      const height = inner.offsetHeight;
      const delta = height - last;
      last = height;
      if (reduced || delta <= 0 || delta > 120 || typeof inner.animate !== "function") return;
      const from = currentOffsetY(inner) + delta;
      glide?.cancel();
      glide = inner.animate(
        [{ transform: `translateY(${from}px)` }, { transform: "translateY(0)" }],
        { duration: 520, easing: "cubic-bezier(0.16, 1, 0.3, 1)" },
      );
    });
    observer.observe(inner);
    return () => {
      observer.disconnect();
      glide?.cancel();
    };
  }, [reduced]);
  return (
    <div ref={outerRef} className="halo-teleprompter">
      <div ref={innerRef} className="halo-teleprompter-inner">
        {children}
      </div>
    </div>
  );
}

/* ------------------------------------------------------------------------------- tool runs */

const FAMILY_ICONS: Readonly<Record<ToolFamily, LucideIcon>> = {
  explore: ScanSearch,
  edit: FilePen,
  execute: SquareTerminal,
  other: Wrench,
};

const KIND_ICONS: Readonly<Record<ToolKind, LucideIcon>> = {
  read: FileText,
  edit: FilePen,
  delete: Trash2,
  move: ArrowRightLeft,
  search: Search,
  execute: SquareTerminal,
  think: Sparkles,
  fetch: Globe,
  switch_mode: ArrowRightLeft,
  other: Wrench,
};

const FILE_KINDS: ReadonlySet<ToolKind> = new Set(["read", "edit", "delete", "move"]);

/**
 * A group stays one row ("Edited pacing.ts, pacing.test.ts +59 −23") and opens on click. It opens
 * on its own only while there is something to see right now: a command's last lines as it runs, a
 * failure, or a call waiting on you. A command folds back to its result row when it finishes.
 */
function opensByDefault(item: ToolRunItem): boolean {
  if (item.status === "waiting" || item.status === "failed") return true;
  return item.family === "execute" && item.status === "running";
}

function RunStatus({ status }: { readonly status: ToolRunItem["status"] }) {
  switch (status) {
    case "running":
      return <Spinner className="halo-run-status" />;
    case "waiting":
      return <span className="halo-run-status halo-wait-dot" role="img" aria-label="Waiting" />;
    case "failed":
      return <X size={14} className="halo-run-status halo-status-fail" aria-label="Failed" />;
    default:
      return <Check size={14} className="halo-run-status halo-status-ok" aria-label="Done" />;
  }
}

const lineText = (line: AnsiLine): string => line.map((span) => span.text).join("");

/**
 * What a finished test command reported, from the end of its output: "19 passed", "2 failed,
 * 17 passed". Null when the output has no such line.
 */
export function testSummary(lines: readonly AnsiLine[]): string | null {
  for (let index = lines.length - 1; index >= Math.max(0, lines.length - 12); index -= 1) {
    const line = lines[index];
    if (!line) continue;
    const counts = [...lineText(line).matchAll(/\b(\d+)\s+(passed|failed)\b/g)]
      .filter((match) => !(match[2] === "failed" && match[1] === "0"))
      .map((match) => `${match[1]} ${match[2]}`);
    if (counts.length > 0) return counts.join(", ");
  }
  return null;
}

/** "19 passed" or "exit 1": how a finished command went, beside its row. */
function RunResult({ item }: { readonly item: ToolRunItem }) {
  if (item.family !== "execute" || item.status === "running" || item.status === "waiting") {
    return null;
  }
  const terminal = item.calls[item.calls.length - 1]?.terminal ?? null;
  const exit = terminal?.exitCode ?? null;
  const summary = terminal ? testSummary(terminal.lines) : null;
  const failed = item.status === "failed" || (exit !== null && exit !== 0);
  if (!summary && (exit === null || exit === 0)) return null;
  return (
    <span className="halo-run-result" data-failed={failed ? "" : undefined}>
      {summary ?? `exit ${exit}`}
    </span>
  );
}

/** The run's one-line title. A single command reads as a verb and the command in mono. */
function runTitle(item: ToolRunItem): ReactNode {
  const only = item.calls.length === 1 ? item.calls[0] : undefined;
  const live = item.status === "running" || item.status === "waiting";
  if (item.family === "execute" && only?.target) {
    const verb =
      item.status === "running"
        ? "Running"
        : item.status === "waiting"
          ? "Waiting to run"
          : item.status === "failed"
            ? "Failed to run"
            : "Ran";
    return (
      <>
        <span className="halo-run-verb">{verb}</span>
        <code className="halo-cmd">{only.target}</code>
      </>
    );
  }
  if (item.family === "edit" && item.files.length > 0) {
    return (
      <>
        <span className="halo-run-verb">{live ? "Editing" : "Edited"}</span>
        <span className="halo-run-files">
          {item.files.map((file) => basename(file)).join(", ")}
        </span>
      </>
    );
  }
  return <span className="halo-run-verb">{live ? item.label : item.summary}</span>;
}

/** Terminal lines shown while a command runs: just enough to see it move. */
const LIVE_TAIL = 3;

/** Output lines shown before "Show earlier lines": the tail, where the result usually is. */
const TERMINAL_TAIL = 10;

/**
 * A group of tool calls as one row: what happened, how much changed, and its status. Opens with a
 * spring to each call: files read, search results, diffs, terminal output.
 */
export const ToolRunBlock = memo(function ToolRunBlock({ item }: { readonly item: ToolRunItem }) {
  // Null until you open or close it yourself; until then it follows the run (see opensByDefault).
  const [chosen, setChosen] = useState<boolean | null>(null);
  const open = chosen ?? opensByDefault(item);
  const Icon = FAMILY_ICONS[item.family];
  const running = item.status === "running";
  const tail = running && chosen === null ? LIVE_TAIL : TERMINAL_TAIL;
  return (
    <div
      className="halo-run"
      data-family={item.family}
      data-status={item.status}
      data-open={open ? "" : undefined}
    >
      <button
        type="button"
        className="halo-run-head"
        data-halo-light=""
        aria-expanded={open}
        onClick={() => setChosen(!open)}
      >
        <span className="halo-run-icon">
          <Icon size={15} aria-hidden="true" />
        </span>
        <span className={cx("halo-run-title", running && "halo-shimmer")}>{runTitle(item)}</span>
        <RunResult item={item} />
        <LineCounts added={item.added} removed={item.removed} />
        <RunStatus status={item.status} />
        <ChevronDown size={15} className="halo-chevron" aria-hidden="true" />
      </button>
      <Collapse open={open}>
        <div className="halo-run-body" data-family={item.family}>
          {item.family === "execute" ? (
            item.calls.map((call) => <TerminalView key={call.id} call={call} tail={tail} />)
          ) : item.family === "edit" ? (
            item.calls.map((call) => <FileEdit key={call.id} call={call} />)
          ) : (
            <ul className="halo-calls">
              {item.calls.map((call) => (
                <CallRow key={call.id} call={call} />
              ))}
            </ul>
          )}
        </div>
      </Collapse>
    </div>
  );
});

/** "Read pacing.ts packages/ui/src/orb": the verb, the file in full strength, its folder dim. */
function CallLabel({ call, tense }: { readonly call: ToolCallModel; readonly tense: string }) {
  if (FILE_KINDS.has(call.kind) && call.target) {
    const file = basename(call.target);
    const verb = tense.endsWith(file) ? tense.slice(0, tense.length - file.length).trim() : tense;
    return (
      <>
        <span className="halo-call-verb">{verb}</span>
        <span className="halo-call-file">{file}</span>
        <span className="halo-call-dir">{dirname(call.target)}</span>
      </>
    );
  }
  return <span className="halo-call-verb">{tense}</span>;
}

function CallRow({ call }: { readonly call: ToolCallModel }) {
  const enter = useEntrance();
  const Icon = KIND_ICONS[call.kind] ?? Wrench;
  const matches = call.search?.matches ?? [];
  const [open, setOpen] = useState(matches.length > 0 && matches.length <= 6);
  const running = call.status === "in_progress" || call.status === "pending";
  const head = (
    <>
      <Icon size={14} className="halo-call-icon" aria-hidden="true" />
      <span className={cx("halo-call-label", running && "halo-shimmer")}>
        <CallLabel call={call} tense={call.label} />
      </span>
      {matches.length > 0 ? (
        <span className="halo-call-meta">
          {matches.length} {matches.length === 1 ? "result" : "results"}
        </span>
      ) : null}
    </>
  );
  return (
    <li className={cx("halo-call", enter && "halo-enter")}>
      {matches.length > 0 ? (
        <>
          <button
            type="button"
            className="halo-call-head"
            aria-expanded={open}
            onClick={() => setOpen((value) => !value)}
          >
            {head}
            <ChevronDown size={14} className="halo-chevron" aria-hidden="true" />
          </button>
          <Collapse open={open}>
            <SearchResults pattern={call.search?.pattern ?? null} matches={matches} />
          </Collapse>
        </>
      ) : (
        <div className="halo-call-head" data-static="">
          {head}
        </div>
      )}
    </li>
  );
}

function Marked({ text, pattern }: { readonly text: string; readonly pattern: string | null }) {
  if (!pattern) return <>{text}</>;
  const parts = text.split(pattern);
  return (
    <>
      {parts.map((part, index) => (
        // biome-ignore lint/suspicious/noArrayIndexKey: parts are positional.
        <Fragment key={index}>
          {index > 0 ? <mark className="halo-hit">{pattern}</mark> : null}
          {part}
        </Fragment>
      ))}
    </>
  );
}

function SearchResults({
  pattern,
  matches,
}: {
  readonly pattern: string | null;
  readonly matches: NonNullable<ToolCallModel["search"]>["matches"];
}) {
  return (
    <ol className="halo-results">
      {matches.map((match) => (
        <li key={`${match.path}:${match.line}`} className="halo-result">
          <span className="halo-result-where">
            <span className="halo-result-file">{basename(match.path)}</span>
            {match.line !== null ? <span className="halo-result-line">{match.line}</span> : null}
          </span>
          <code className="halo-result-text">
            <Marked text={match.text.trim()} pattern={pattern} />
          </code>
        </li>
      ))}
    </ol>
  );
}

/* ----------------------------------------------------------------------------------- diffs */

/** Lines of a diff shown before "Show more": enough to read the change, short enough to scan. */
const DIFF_PREVIEW_LINES = 10;

function FileEdit({ call }: { readonly call: ToolCallModel }) {
  const enter = useEntrance();
  const diff = call.diffs[0] ?? null;
  const path = diff?.path ?? call.target ?? "";
  const isNew = diff?.isNew ?? false;
  const Icon = isNew ? FilePlus2 : FilePen;
  const running = call.status === "in_progress" || call.status === "pending";
  return (
    <div className={cx("halo-file", enter && "halo-enter")}>
      <div className="halo-file-head">
        <Icon size={14} className="halo-file-icon" aria-hidden="true" />
        <span className="halo-file-name">{basename(path)}</span>
        <span className="halo-file-dir">{dirname(path)}</span>
        {isNew ? <span className="halo-tag">New</span> : null}
        {diff ? <LineCounts added={diff.added} removed={diff.removed} /> : null}
      </div>
      {diff ? (
        <DiffView diff={diff} />
      ) : running ? (
        <p className="halo-file-pending halo-shimmer">Editing</p>
      ) : null}
    </div>
  );
}

type DiffRow =
  | { readonly kind: "hunk"; readonly key: string; readonly header: string }
  | { readonly kind: "line"; readonly key: string; readonly line: DiffLine };

function diffRows(diff: FileDiff): DiffRow[] {
  const rows: DiffRow[] = [];
  diff.hunks.forEach((hunk, hunkIndex) => {
    // A new file's single hunk header says nothing the "New" tag does not.
    if (!(diff.isNew && diff.hunks.length === 1)) {
      rows.push({ kind: "hunk", key: `h${hunkIndex}`, header: hunk.header });
    }
    hunk.lines.forEach((line, lineIndex) => {
      rows.push({ kind: "line", key: `h${hunkIndex}.${lineIndex}`, line });
    });
  });
  return rows;
}

const SIGN: Readonly<Record<DiffLine["op"], string>> = { add: "+", remove: "−", context: "" };

/**
 * A diff with old and new line numbers. Long lines wrap rather than clip: the code is its own
 * column, and a wrapped line continues two characters in from its own indentation.
 */
function DiffView({ diff }: { readonly diff: FileDiff }) {
  const [all, setAll] = useState(false);
  const rows = useMemo(() => diffRows(diff), [diff]);
  const lineRows = rows.filter((row) => row.kind === "line").length;
  let budget = all ? Number.POSITIVE_INFINITY : DIFF_PREVIEW_LINES;
  const shown: DiffRow[] = [];
  for (const row of rows) {
    if (row.kind === "line") {
      if (budget <= 0) break;
      budget -= 1;
    }
    shown.push(row);
  }
  const hidden = lineRows - shown.filter((row) => row.kind === "line").length;
  return (
    <figure className="halo-diff" aria-label={`Changes to ${basename(diff.path)}`}>
      {shown.map((row) =>
        row.kind === "hunk" ? (
          <div key={row.key} className="halo-diff-hunk">
            {row.header}
          </div>
        ) : (
          <div key={row.key} className="halo-diff-line" data-op={row.line.op}>
            <span className="halo-diff-num" aria-hidden="true">
              {row.line.oldLine ?? ""}
            </span>
            <span className="halo-diff-num" aria-hidden="true">
              {row.line.newLine ?? ""}
            </span>
            <span className="halo-diff-sign">{SIGN[row.line.op]}</span>
            <span className="halo-diff-code">
              <CodeLine text={row.line.text} />
            </span>
          </div>
        ),
      )}
      {hidden > 0 ? (
        <button type="button" className="halo-more" onClick={() => setAll(true)}>
          Show {hidden} more {hidden === 1 ? "line" : "lines"}
        </button>
      ) : null}
      {diff.truncated ? <p className="halo-diff-note">Too large to show line by line.</p> : null}
    </figure>
  );
}

/* -------------------------------------------------------------------------------- terminal */

/** The 16 terminal colours in Halo's palette: soft, bright enough on dark glass. */
const ANSI_PALETTE: Readonly<Record<string, string>> = {
  black: "#2a2c36",
  red: "#ff8080",
  green: "#6fe3a9",
  yellow: "#ffd479",
  blue: "#7fb4ff",
  magenta: "#e4a8ff",
  cyan: "#6fd8f5",
  white: "#e9ebf2",
  brightBlack: "rgb(255 255 255 / 0.58)",
  brightRed: "#ffa3a3",
  brightGreen: "#92f2c4",
  brightYellow: "#ffe6a6",
  brightBlue: "#a8cbff",
  brightMagenta: "#f0c8ff",
  brightCyan: "#a6ecff",
  brightWhite: "#ffffff",
};

function ansiColor(color: AnsiColor | null): string | undefined {
  if (color === null) return undefined;
  return color.startsWith("#") ? color : ANSI_PALETTE[color];
}

function spanStyle(span: AnsiSpan): CSSProperties | undefined {
  let fg = ansiColor(span.fg);
  let bg = ansiColor(span.bg);
  if (span.inverse) [fg, bg] = [bg ?? "#0b0c10", fg ?? "#e9ebf2"];
  if (!fg && !bg && !span.bold && !span.dim && !span.italic && !span.underline) return undefined;
  return {
    color: bg && !fg ? "#0b0c10" : fg,
    background: bg,
    fontWeight: span.bold ? 600 : undefined,
    opacity: span.dim ? 0.66 : undefined,
    fontStyle: span.italic ? "italic" : undefined,
    textDecoration: span.underline ? "underline" : span.strikethrough ? "line-through" : undefined,
  };
}

function TerminalView({ call, tail }: { readonly call: ToolCallModel; readonly tail: number }) {
  const [all, setAll] = useState(false);
  const terminal = call.terminal;
  const lines = terminal?.lines ?? [];
  // Leading blank lines carry nothing; trailing ones neither.
  let first = 0;
  while (first < lines.length && (lines[first]?.length ?? 0) === 0) first += 1;
  let end = lines.length;
  while (end > first && (lines[end - 1]?.length ?? 0) === 0) end -= 1;
  const visible = lines.slice(first, end);
  const shownFrom = all ? 0 : Math.max(0, visible.length - tail);
  const shown = visible.slice(shownFrom);
  const running = call.status === "in_progress" || call.status === "pending";
  const exit = terminal?.exitCode ?? null;
  return (
    <div className="halo-term" data-running={running ? "" : undefined}>
      {shownFrom > 0 && !running ? (
        <button type="button" className="halo-more halo-term-more" onClick={() => setAll(true)}>
          Show {shownFrom} earlier {shownFrom === 1 ? "line" : "lines"}
        </button>
      ) : null}
      <div className="halo-term-body">
        {shown.length === 0 ? (
          <div className="halo-term-line halo-term-waiting">
            {call.awaitingPermission ? "Waiting for approval" : "Starting"}
            <span className="halo-dots" aria-hidden="true" />
          </div>
        ) : (
          shown.map((line, index) => (
            <TerminalLine
              // biome-ignore lint/suspicious/noArrayIndexKey: output lines only ever append.
              key={shownFrom + first + index}
              spans={line}
            />
          ))
        )}
      </div>
      {exit !== null || terminal?.truncated ? (
        <div className="halo-term-foot">
          {terminal?.truncated ? <span>Output shortened, the rest is on disk</span> : null}
          {exit !== null ? (
            <span className="halo-exit" data-ok={exit === 0 ? "" : undefined}>
              {exit === 0 ? (
                <Check size={12} aria-hidden="true" />
              ) : (
                <X size={12} aria-hidden="true" />
              )}
              exit {exit}
            </span>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

const TerminalLine = memo(function TerminalLine({ spans }: { readonly spans: AnsiLine }) {
  const enter = useEntrance();
  const hang = useMemo(() => hangOf(lineText(spans)), [spans]);
  return (
    <div
      className={cx("halo-term-line", enter && "halo-ink-line")}
      style={{ "--hang": hang } as CSSProperties}
    >
      {spans.length === 0
        ? "​"
        : spans.map((span, index) => {
            const style = spanStyle(span);
            return style ? (
              <span
                // biome-ignore lint/suspicious/noArrayIndexKey: spans are positional.
                key={index}
                className={span.bg ? "halo-ansi-badge" : undefined}
                style={style}
              >
                {span.text}
              </span>
            ) : (
              // biome-ignore lint/suspicious/noArrayIndexKey: spans are positional.
              <Fragment key={index}>{span.text}</Fragment>
            );
          })}
    </div>
  );
});

/* ------------------------------------------------------------------------------ permission */

const SHORTCUTS: Readonly<Record<PermissionOptionKind, string>> = {
  allow_once: "⏎",
  allow_always: "⌘⏎",
  reject_once: "esc",
  reject_always: "esc",
};

const ANSWERED: Readonly<Record<PermissionOptionKind, string>> = {
  allow_once: "Allowed once",
  allow_always: "Always allowed",
  reject_once: "Rejected",
  reject_always: "Always rejected",
};

/** The option a shortcut picks: the first option of its kind. */
export function optionForShortcut(
  permission: PermissionModel,
  kinds: readonly PermissionOptionKind[],
): string | null {
  return permission.options.find((option) => kinds.includes(option.kind))?.option_id ?? null;
}

function permissionLine(permission: PermissionModel, agent: string, autonomy: Autonomy): string {
  const command = permission.tool?.kind === "execute";
  const what = command ? `${agent} wants to run a command.` : `${agent} needs your approval.`;
  if (autonomy === "auto_edit") return `${what} Auto-edit asks before commands.`;
  if (autonomy === "ask") return `${what} Ask checks every step with you.`;
  return what;
}

/**
 * An inline permission request: the question, the exact command, and the agent's answers with
 * their shortcuts. It floats a little nearer than the card around it, in amber: the one thing on
 * screen waiting for you. Once answered it folds to a single line.
 */
export function PermissionCard({
  permission,
  agent,
  autonomy,
  answer,
  pressed,
  cancelled,
  onAnswer,
}: {
  readonly permission: PermissionModel;
  readonly agent: string;
  readonly autonomy: Autonomy;
  /** An answer given here in the lab, before the agent has reported it. */
  readonly answer: string | null;
  /** The option whose shortcut was just pressed, for the pressed look. */
  readonly pressed: string | null;
  /** The turn was stopped while the request waited. */
  readonly cancelled: boolean;
  readonly onAnswer: (optionId: string) => void;
}) {
  const enter = useEntrance();
  const chosen = answer ?? permission.selectedOptionId;
  const option = permission.options.find((candidate) => candidate.option_id === chosen) ?? null;
  const pending = permission.status === "pending" && answer === null && !cancelled;
  const tool = permission.tool;
  const command = tool?.terminal?.command ?? (tool?.kind === "execute" ? tool.target : null);
  const seen = new Set<PermissionOptionKind>();

  if (!pending) {
    const dropped = permission.status === "cancelled" || (cancelled && answer === null);
    const allowed = option
      ? option.kind === "allow_once" || option.kind === "allow_always"
      : permission.status === "allowed";
    const text = dropped
      ? "Request cancelled"
      : option
        ? ANSWERED[option.kind]
        : permission.status === "allowed"
          ? "Allowed"
          : "Rejected";
    return (
      <div
        className="halo-answered"
        data-allowed={allowed && !dropped ? "" : undefined}
        data-cancelled={dropped ? "" : undefined}
      >
        {allowed && !dropped ? (
          <Check size={14} aria-hidden="true" />
        ) : (
          <X size={14} aria-hidden="true" />
        )}
        <span className="halo-answered-text">{text}</span>
        <code className="halo-cmd halo-answered-cmd">{command ?? permission.title}</code>
      </div>
    );
  }

  return (
    <section className={cx("halo-permission", enter && "halo-pop")} aria-label={permission.title}>
      <Glass material="alert" radius={RADIUS.bubble} />
      <div className="halo-permission-main">
        <span className="halo-permission-icon">
          <ShieldAlert size={18} aria-hidden="true" />
        </span>
        <div className="halo-permission-copy">
          <p className="halo-permission-title">{permission.title}</p>
          <p className="halo-permission-sub">{permissionLine(permission, agent, autonomy)}</p>
        </div>
      </div>
      {command ? (
        <div className="halo-permission-cmd">
          <span className="halo-prompt" aria-hidden="true">
            $
          </span>
          <code>{command}</code>
        </div>
      ) : null}
      <div className="halo-permission-actions">
        {permission.options.map((choice) => {
          const shortcut = seen.has(choice.kind) ? null : SHORTCUTS[choice.kind];
          seen.add(choice.kind);
          return (
            <button
              key={choice.option_id}
              type="button"
              className="halo-perm-btn"
              data-halo-light=""
              data-kind={choice.kind}
              data-pressed={pressed === choice.option_id ? "" : undefined}
              onClick={() => onAnswer(choice.option_id)}
            >
              <span>{choice.label}</span>
              {shortcut ? <Kbd>{shortcut}</Kbd> : null}
            </button>
          );
        })}
      </div>
    </section>
  );
}

/* ----------------------------------------------------------------------------------- footer */

export function DurationMetric({ ms }: { readonly ms: number }) {
  return (
    <span className="halo-metric" title="Turn duration">
      <Clock size={13} aria-hidden="true" />
      {formatDuration(ms)}
    </span>
  );
}
