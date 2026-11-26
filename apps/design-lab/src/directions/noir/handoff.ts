import { getBrand } from "@ghost/ui";
import type { FileChange, MessageItem, TranscriptModel } from "../../kit";

/**
 * The handoff block: what auracode puts in front of the first prompt to a different agent, and the
 * only thing it ever adds (plan.md, principle 3). Earlier prompts, other agents' final answers and
 * the files changed, clipped to a budget, as plain text the user can read before it goes.
 */
export interface Handoff {
  /** The agent the chat has been with, and the one taking over. */
  readonly fromName: string;
  readonly toName: string;
  readonly prompts: number;
  readonly answers: number;
  readonly files: number;
  /** The block, exactly as the next prompt carries it. */
  readonly block: string;
}

/** Characters kept of each answer; the rest is marked as cut. */
const ANSWER_BUDGET = 900;
/** Width of the +/- bar in the files table, as `git diff --stat` draws it. */
const STAT_BAR = 24;

export function handoffFor(model: TranscriptModel, from: string, to: string): Handoff | null {
  if (model.turns.length === 0 || from === to) return null;
  const fromName = getBrand(from).name;
  const toName = getBrand(to).name;
  const sections: string[] = [];
  const files = new Map<string, FileChange>();
  let prompts = 0;
  let answers = 0;
  for (const turn of model.turns) {
    if (turn.user) {
      prompts += 1;
      sections.push(`User:\n${turn.user.text.trim()}`);
    }
    const final = turn.items.findLast((item): item is MessageItem => item.kind === "message");
    if (final && final.text.trim() !== "" && turn.agent !== to) {
      answers += 1;
      sections.push(`${getBrand(turn.agent ?? "generic").name}:\n${clip(final.text.trim())}`);
    }
    for (const file of turn.footer?.files ?? []) {
      const known = files.get(file.path);
      files.set(
        file.path,
        known
          ? { ...known, added: known.added + file.added, removed: known.removed + file.removed }
          : file,
      );
    }
  }
  const stat = diffStat([...files.values()]);
  const block = [
    `Handoff from ${fromName}. This is the chat so far.`,
    ...sections,
    ...(stat ? [`Files changed:\n${stat}`] : []),
  ].join("\n\n");
  return { fromName, toName, prompts, answers, files: files.size, block };
}

/** Cuts long text at a word boundary within the budget, and says so. */
export function clip(text: string, budget = ANSWER_BUDGET): string {
  if (text.length <= budget) return text;
  const cut = text.slice(0, budget);
  const space = cut.lastIndexOf(" ");
  return `${(space > budget * 0.6 ? cut.slice(0, space) : cut).trimEnd()} [cut]`;
}

/** The files as `git diff --stat` prints them: path, total, a scaled +/- bar, then a summary. */
export function diffStat(files: readonly FileChange[]): string {
  if (files.length === 0) return "";
  const width = Math.max(...files.map((file) => file.path.length));
  const largest = Math.max(...files.map((file) => file.added + file.removed), 1);
  const digits = String(largest).length;
  // git's own scaling (diff.c, show_stats): every non-zero count keeps at least one mark.
  const scaled = (count: number): number =>
    count > 0 ? 1 + Math.floor((count * (STAT_BAR - 1)) / largest) : 0;
  const rows = files.map((file) => {
    const total = file.added + file.removed;
    let plus = file.added;
    let minus = file.removed;
    if (largest > STAT_BAR) {
      const marks = Math.max(scaled(total), plus > 0 && minus > 0 ? 2 : 0);
      if (plus < minus) {
        plus = scaled(plus);
        minus = marks - plus;
      } else {
        minus = scaled(minus);
        plus = marks - minus;
      }
    }
    return ` ${file.path.padEnd(width)} | ${String(total).padStart(digits)} ${"+".repeat(plus)}${"-".repeat(minus)}`;
  });
  const added = files.reduce((sum, file) => sum + file.added, 0);
  const removed = files.reduce((sum, file) => sum + file.removed, 0);
  const summary = [
    `${files.length} ${files.length === 1 ? "file" : "files"} changed`,
    `${added} ${added === 1 ? "insertion" : "insertions"}(+)`,
    `${removed} ${removed === 1 ? "deletion" : "deletions"}(-)`,
  ].join(", ");
  return [...rows, ` ${summary}`].join("\n");
}
