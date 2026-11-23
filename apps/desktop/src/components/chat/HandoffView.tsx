import {
  ChevronRight,
  FileDiff,
  FolderOpen,
  MessagesSquare,
  ScrollText,
  SquareTerminal,
} from "lucide-react";
import { type ReactNode, useMemo } from "react";
import { type HandoffCommand, type ParsedHandoff, parseHandoff } from "../../lib/chat/handoff";
import { CopyAction } from "./CopyAction";

const plural = (count: number, one: string, many = `${one}s`) =>
  `${count} ${count === 1 ? one : many}`;

/** "3 turns · 12 commands · 1 still failing": what a handoff holds, in one line. */
export function handoffSummary(parsed: ParsedHandoff): string {
  const commands = parsed.commands.length + parsed.earlier;
  return [
    parsed.turns.length ? plural(parsed.turns.length, "turn") : null,
    commands ? plural(commands, "command") : null,
    parsed.failure ? "1 still failing" : null,
  ]
    .filter(Boolean)
    .join(" · ");
}

/** The summary line for a handoff's text, or nothing when it holds none of those. */
export function HandoffCount({ text }: { readonly text: string }) {
  const summary = useMemo(() => {
    const parsed = parseHandoff(text);
    return parsed ? handoffSummary(parsed) : "";
  }, [text]);
  return summary ? <span className="chat-handoff-count">{summary}</span> : null;
}

function Dot({ state }: { readonly state: HandoffCommand["state"] }) {
  return <span className="handoff-dot" data-state={state} aria-hidden="true" />;
}

const STATE_WORDS: Record<HandoffCommand["state"], string> = {
  ok: "succeeded",
  failed: "failed",
  running: "was still running",
};

function CommandRow({ command }: { readonly command: HandoffCommand }) {
  return (
    <li className="handoff-command" data-state={command.state}>
      <Dot state={command.state} />
      <span className="handoff-command-body">
        <code className="handoff-command-text" title={command.command}>
          {command.command}
        </code>
        {command.cwd ? <span className="handoff-command-cwd">in {command.cwd}</span> : null}
      </span>
      <span className="handoff-command-meta">
        <span className="handoff-visually-hidden">{STATE_WORDS[command.state]}, </span>
        {command.outcome === "ok" ? "done" : command.outcome}
        <span className="handoff-command-turn">turn {command.turn}</span>
      </span>
    </li>
  );
}

/** One folded part of the view: a quiet row with its icon and words, which opens its content. */
function Part({
  icon,
  label,
  open = false,
  testId,
  children,
}: {
  readonly icon: ReactNode;
  readonly label: string;
  readonly open?: boolean;
  readonly testId: string;
  readonly children: ReactNode;
}) {
  return (
    <details className="handoff-part" open={open} data-testid={testId}>
      <summary>
        <span className="handoff-part-icon" aria-hidden="true">
          {icon}
        </span>
        <span className="handoff-part-label">{label}</span>
        <ChevronRight className="handoff-part-chevron" size={14} aria-hidden="true" />
      </summary>
      <div className="handoff-part-body">{children}</div>
    </details>
  );
}

/**
 * A handoff as the next agent receives it, laid out: the last command still failing and its
 * first error first (that is what the next agent most needs), then the commands that ran, the
 * working folder, the conversation and the changes, each folded to one row. "Exact text" opens
 * the text itself, which is what is sent, word for word. Text that does not read as a handoff
 * shows as it is.
 */
export function HandoffView({
  text,
  emptyText,
}: {
  readonly text: string;
  /** What to say when there is no handoff text at all. */
  readonly emptyText?: string;
}) {
  const parsed = useMemo(() => parseHandoff(text), [text]);
  if (!text)
    return <p className="handoff-empty">{emptyText ?? "No earlier context is included."}</p>;
  if (!parsed)
    return (
      // biome-ignore lint/a11y/noNoninteractiveTabindex: long context scrolls by keyboard.
      <pre className="handoff-raw" tabIndex={0}>
        {text}
      </pre>
    );
  const commands = parsed.commands.length + parsed.earlier;
  const failed = parsed.commands.filter((command) => command.state === "failed").length;
  return (
    <div className="handoff" data-testid="handoff-view">
      {parsed.preface ? <p className="handoff-preface">{parsed.preface}</p> : null}
      {parsed.folder ? (
        <p className="handoff-folder" data-testid="handoff-folder">
          <FolderOpen size={14} aria-hidden="true" />
          <span className="handoff-visually-hidden">Working folder: </span>
          <code title={parsed.folder}>{parsed.folder}</code>
        </p>
      ) : null}
      {parsed.failure ? (
        <section
          className="handoff-failure"
          data-testid="handoff-failure"
          aria-label="Still failing"
        >
          <div className="handoff-failure-head">
            <Dot state="failed" />
            <code className="handoff-command-text" title={parsed.failure.command.command}>
              {parsed.failure.command.command}
            </code>
            <span className="handoff-failure-meta">
              Still failing · {parsed.failure.command.outcome} · turn {parsed.failure.command.turn}
            </span>
          </div>
          {parsed.failure.error ? (
            // biome-ignore lint/a11y/noNoninteractiveTabindex: a long error scrolls by keyboard.
            <pre className="handoff-error" tabIndex={0} data-testid="handoff-error">
              {parsed.failure.error}
            </pre>
          ) : null}
        </section>
      ) : null}
      <div className="handoff-parts">
        {commands ? (
          <Part
            icon={<SquareTerminal size={14} />}
            label={`${plural(commands, "command")} ran${failed ? `, ${failed} failed` : ""}`}
            open={!parsed.failure && parsed.commands.length <= 6}
            testId="handoff-commands"
          >
            <ol className="handoff-commands">
              {parsed.commands.map((command) => (
                <CommandRow key={command.position} command={command} />
              ))}
            </ol>
            {parsed.earlier ? (
              <p className="handoff-note">
                {plural(parsed.earlier, "earlier command")} {parsed.earlier === 1 ? "is" : "are"}{" "}
                not listed.
              </p>
            ) : null}
          </Part>
        ) : null}
        {parsed.turns.length ? (
          <Part
            icon={<MessagesSquare size={14} />}
            label={`${plural(parsed.turns.length, "turn")} of conversation${parsed.omitted ? ", earlier ones shortened" : ""}`}
            testId="handoff-turns"
          >
            <ol className="handoff-turns">
              {parsed.turns.map((turn) => (
                <li key={turn.ordinal} className="handoff-turn">
                  <p className="handoff-turn-head">
                    Turn {turn.ordinal} · {turn.agent}
                    {turn.ended ? (
                      <span className="handoff-turn-ended"> · {turn.ended}</span>
                    ) : null}
                  </p>
                  <p className="handoff-turn-user">{turn.user}</p>
                  {turn.answer ? <p className="handoff-turn-answer">{turn.answer}</p> : null}
                </li>
              ))}
            </ol>
          </Part>
        ) : null}
        <Part icon={<FileDiff size={14} />} label="Changes in the folder" testId="handoff-changes">
          <pre className="handoff-well">{parsed.files || "None listed."}</pre>
          <pre className="handoff-well">{parsed.diffStat || "No diff summary."}</pre>
        </Part>
        <Part
          icon={<ScrollText size={14} />}
          label={`Exact text, ${(new TextEncoder().encode(text).length / 1024).toFixed(1)} KB`}
          testId="handoff-exact"
        >
          <div className="handoff-exact">
            {/* biome-ignore lint/a11y/noNoninteractiveTabindex: long context scrolls by keyboard. */}
            <pre className="handoff-well" tabIndex={0}>
              {text}
            </pre>
            <CopyAction text={text} label="Copy handoff" />
          </div>
        </Part>
      </div>
    </div>
  );
}
