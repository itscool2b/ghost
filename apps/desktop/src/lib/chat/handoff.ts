/**
 * A handoff as the window shows it. The text is what the next agent receives, written by Rust
 * (`crates/ghost-core/src/conversation.rs`, `handoff`); this reads it back into its parts so the view
 * shows exactly that text, arranged: the conversation, the working folder, the commands that ran,
 * the last one still failing with its first error, and the changes. Anything that does not read
 * as a handoff is shown as its own plain text instead (`parseHandoff` answers `null`).
 */

/** The line Rust puts between a handoff and the person's own words in the prompt it sends. */
export const PROMPT_MARK = "\n\nCurrent user prompt\n";
const HEADER = "Visible conversation handoff\n";
const OMITTED = "[Some earlier context is omitted to fit the handoff budget.]";

export interface HandoffTurn {
  readonly ordinal: number;
  readonly agent: string;
  /** How a turn that did not finish ended ("interrupted", "failed"); `null` when it finished. */
  readonly ended: string | null;
  readonly user: string;
  readonly answer: string;
}

export interface HandoffCommand {
  /** Its place in the list the handoff sent (the same command may run more than once). */
  readonly position: number;
  readonly turn: number;
  /** `exit 101`, `failed`, `ok` or `running`. */
  readonly outcome: string;
  readonly command: string;
  /** Where it ran, when that was not the working folder. */
  readonly cwd: string | null;
  readonly state: "ok" | "failed" | "running";
}

export interface ParsedHandoff {
  /** A folder move written before the handoff ("Working folder changes from … to …"). */
  readonly preface: string | null;
  readonly omitted: boolean;
  readonly turns: readonly HandoffTurn[];
  readonly folder: string | null;
  readonly commands: readonly HandoffCommand[];
  /** Commands that ran before the listed ones. */
  readonly earlier: number;
  readonly failure: { readonly command: HandoffCommand; readonly error: string } | null;
  readonly files: string;
  readonly diffStat: string;
}

function state(outcome: string): HandoffCommand["state"] {
  if (outcome === "running") return "running";
  if (outcome === "ok" || outcome === "exit 0") return "ok";
  return "failed";
}

function command(line: string, position: number): HandoffCommand | null {
  const match = /^\[turn (\d+)\] (exit -?\d+|failed|ok|running) \| (.*)$/.exec(line);
  if (!match) return null;
  return {
    position,
    turn: Number(match[1]),
    outcome: match[2] ?? "",
    command: match[3] ?? "",
    cwd: null,
    state: state(match[2] ?? ""),
  };
}

function turns(block: string): HandoffTurn[] {
  const found: HandoffTurn[] = [];
  const pattern = /\nTurn (\d+) \(([^)\n]*)\)\nUser:\n/g;
  const heads = [...block.matchAll(pattern)];
  heads.forEach((head, index) => {
    const start = (head.index ?? 0) + head[0].length;
    const end = index + 1 < heads.length ? (heads[index + 1]?.index ?? block.length) : block.length;
    const body = block.slice(start, end);
    const split = body.lastIndexOf("\nFinal answer:\n");
    const [agent, ...rest] = (head[2] ?? "").split(", ");
    found.push({
      ordinal: Number(head[1]),
      agent: agent ?? "",
      ended: rest.length ? rest.join(", ") : null,
      user: (split >= 0 ? body.slice(0, split) : body).trim(),
      answer: split >= 0 ? body.slice(split + "\nFinal answer:\n".length).trim() : "",
    });
  });
  return found;
}

/** The parts of a handoff, or `null` when the text is not one this window can lay out. */
export function parseHandoff(text: string): ParsedHandoff | null {
  const at = text.indexOf(HEADER);
  if (at < 0) return null;
  const preface = text.slice(0, at).trim() || null;
  const changesAt = text.lastIndexOf("\nCurrent file changes\n");
  const statAt = text.lastIndexOf("\nGit diff --stat\n");
  if (changesAt < 0 || statAt < changesAt) return null;
  const body = text.slice(at + HEADER.length, changesAt);
  // The header's second line, then the conversation, then the work (newer handoffs only).
  const workAt = body.lastIndexOf("\nWorking folder\n");
  const conversation = workAt >= 0 ? body.slice(0, workAt) : body;
  const work = workAt >= 0 ? body.slice(workAt + "\nWorking folder\n".length) : null;
  let folder: string | null = null;
  const commands: HandoffCommand[] = [];
  let earlier = 0;
  let failure: ParsedHandoff["failure"] = null;
  if (work !== null) {
    const [first, ...lines] = work.split("\n");
    folder = first ?? null;
    let section: "none" | "commands" | "failing" | "error" = "none";
    let failing: HandoffCommand | null = null;
    const error: string[] = [];
    for (const line of lines) {
      if (section === "error") {
        error.push(line);
        continue;
      }
      if (line === "Commands run (newest last)") section = "commands";
      else if (line === "Last failing command") section = "failing";
      else if (line === "First error" && section === "failing") section = "error";
      else if (section === "commands") {
        const hidden = /^\[(\d+) earlier commands are not listed\.\]$/.exec(line);
        const cwd = /^ {2}in (.*)$/.exec(line);
        const last = commands.at(-1);
        if (hidden) earlier = Number(hidden[1]);
        else if (cwd && last) commands[commands.length - 1] = { ...last, cwd: cwd[1] ?? null };
        else {
          const read = command(line, commands.length);
          if (read) commands.push(read);
        }
      } else if (section === "failing" && !failing) failing = command(line, -1);
    }
    if (failing) failure = { command: failing, error: error.join("\n").trim() };
  }
  const files = text.slice(changesAt + "\nCurrent file changes\n".length, statAt);
  const diffStat = text
    .slice(statAt + "\nGit diff --stat\n".length)
    .replace(/\nEnd of handoff\s*$/, "");
  return {
    preface,
    omitted: conversation.includes(OMITTED),
    turns: turns(conversation),
    folder,
    commands,
    earlier,
    failure,
    files: files.trim(),
    diffStat: diffStat.trim(),
  };
}

/**
 * A sent message whose prompt began with a handoff: the handoff, and the words the person typed.
 * `null` for any other message.
 */
export function splitHandoff(sent: string): { handoff: string; text: string } | null {
  if (!sent.includes(HEADER)) return null;
  const mark = sent.indexOf(PROMPT_MARK);
  if (mark < 0 || !sent.slice(0, mark).includes("\nEnd of handoff")) return null;
  return { handoff: sent.slice(0, mark), text: sent.slice(mark + PROMPT_MARK.length) };
}
