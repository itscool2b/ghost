import { type AgentBrandId, getBrand, Orb } from "@ghost/ui";
import { ChevronRight } from "lucide-react";
import type { Autonomy, FileChange, TranscriptModel } from "../../kit";
import { DiffStat } from "./ui";

/**
 * The handoff block: what auracode adds to the first message an agent gets in a chat another agent
 * worked in, and the only thing it ever adds. Visible before sending (in the composer) and after
 * (on the message it went with), so nothing reaches an agent that the user cannot read.
 */
export interface Handoff {
  readonly from: AgentBrandId;
  readonly to: AgentBrandId;
  /** The user's earlier messages, newest last. */
  readonly asked: readonly string[];
  /** Other agents' final answers, newest last. */
  readonly answers: readonly { readonly agent: AgentBrandId; readonly text: string }[];
  /** Files the chat changed so far, with line counts. */
  readonly files: readonly FileChange[];
}

/** A message the user sent from this window (the recording never sends), shown after its turns. */
export interface SentMessage {
  readonly id: string;
  readonly text: string;
  readonly agent: AgentBrandId;
  readonly model: string;
  readonly effort: string | null;
  readonly autonomy: Autonomy;
  readonly handoff: Handoff | null;
  /** The first message of a new chat: its orb flies in from the hero instead of fading in. */
  readonly fromHero: boolean;
}

const MAX_ASKED = 3;
const MAX_CHARS = 280;

/** The first paragraph of markdown as plain text, cut at a word near `max` characters. */
function brief(text: string, max = MAX_CHARS): string {
  const first =
    text
      .split(/\n{2,}/)
      .map((part) => part.trim())
      .find((part) => part !== "") ?? "";
  const plain = first
    .replace(/`([^`]*)`/g, "$1")
    .replace(/\*\*([^*]*)\*\*/g, "$1")
    .replace(/\s+/g, " ")
    .trim();
  if (plain.length <= max) return plain;
  const cut = plain.slice(0, max);
  const space = cut.lastIndexOf(" ");
  return `${(space > max * 0.6 ? cut.slice(0, space) : cut).replace(/[,.;:]$/, "")}…`;
}

/** The handoff `to` gets: the chat so far, as the other agents left it. */
export function buildHandoff(
  model: TranscriptModel,
  sent: readonly SentMessage[],
  from: AgentBrandId,
  to: AgentBrandId,
): Handoff {
  const asked = [
    ...model.turns.flatMap((turn) => (turn.user ? [turn.user.text] : [])),
    ...sent.map((message) => message.text),
  ]
    .slice(-MAX_ASKED)
    .map((text) => brief(text, 200));
  const answers = model.turns.flatMap((turn) => {
    if (turn.agent === null || turn.agent === to) return [];
    const last = [...turn.items].reverse().find((item) => item.kind === "message");
    return last && last.kind === "message" && last.text.trim() !== ""
      ? [{ agent: turn.agent, text: brief(last.text) }]
      : [];
  });
  const byPath = new Map<string, FileChange>();
  for (const turn of model.turns) {
    for (const file of turn.footer?.files ?? []) {
      const known = byPath.get(file.path);
      byPath.set(
        file.path,
        known
          ? {
              ...known,
              added: known.added + file.added,
              removed: known.removed + file.removed,
            }
          : file,
      );
    }
  }
  return { from, to, asked, answers, files: [...byPath.values()] };
}

/** Two small orbs, from one agent to the next. */
export function HandoffOrbs({
  from,
  to,
}: {
  readonly from: AgentBrandId;
  readonly to: AgentBrandId;
}) {
  return (
    <span className="au-strip-orbs" aria-hidden="true">
      <Orb brand={from} state="idle" size={14} />
      <ChevronRight size={11} strokeWidth={2} />
      <Orb brand={to} state="idle" size={14} />
    </span>
  );
}

/** The handoff's text, section by section, exactly as the next agent gets it. */
export function HandoffView({ handoff }: { readonly handoff: Handoff }) {
  return (
    <div className="au-handoff">
      {handoff.asked.length > 0 ? (
        <div className="au-handoff-section">
          <p className="au-handoff-label">
            {handoff.asked.length === 1 ? "You asked" : "You asked, in order"}
          </p>
          {handoff.asked.map((text, index) => (
            // biome-ignore lint/suspicious/noArrayIndexKey: messages are positional.
            <p key={index} className="au-handoff-text">
              {text}
            </p>
          ))}
        </div>
      ) : null}
      {handoff.answers.map((answer, index) => (
        // biome-ignore lint/suspicious/noArrayIndexKey: answers are positional.
        <div key={index} className="au-handoff-section">
          <p className="au-handoff-label">{getBrand(answer.agent).name} answered</p>
          <p className="au-handoff-text">{answer.text}</p>
        </div>
      ))}
      {handoff.files.length > 0 ? (
        <div className="au-handoff-section">
          <p className="au-handoff-label">Files changed</p>
          <ul className="au-handoff-files">
            {handoff.files.map((file) => (
              <li key={file.path}>
                <span className="au-handoff-path">{file.path}</span>
                <DiffStat added={file.added} removed={file.removed} />
              </li>
            ))}
          </ul>
        </div>
      ) : null}
    </div>
  );
}
