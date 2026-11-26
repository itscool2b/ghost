/**
 * The project memory Aura attaches to a message as reference context.
 *
 * `visible_context` in `crates/ghost-core/src/chatstore/memory.rs` appends it after everything the
 * person sent, on the first message of a session and after the memory changes:
 *
 *     \n\nProject memory (Aura reference context, revision <id>)\n
 *     [Last edited by an agent in <place>, not by the user. …\n]
 *     Use this as project background. …\n
 *     \n
 *     <Markdown>\n
 *
 * The saved message is that whole text, so the chat reads it back apart here: the person's words
 * stay the message, and the memory folds behind "Project memory".
 */
export interface MemoryContext {
  /** The revision that was attached, or `empty` when the project had none. */
  readonly revision: string;
  /** "Last edited by an agent in …." when an agent, not the person, wrote this revision. */
  readonly editedBy: string | null;
  /** The memory as the agent received it. */
  readonly markdown: string;
}

export interface SentText {
  /** What the person sent, without the memory attached after it. */
  readonly text: string;
  readonly memory: MemoryContext | null;
}

const MARK = "\n\nProject memory (Aura reference context, revision ";
const GUIDANCE = "Use this as project background.";
const EDITED = "Last edited by an agent in ";
const EDITED_END = ", not by the user.";

/**
 * Splits a sent message into the person's words and the memory attached after them. Only a block
 * with Aura's exact header counts, so a message that merely mentions project memory stays whole.
 */
export function splitMemoryContext(sent: string): SentText {
  for (let at = sent.indexOf(MARK); at >= 0; at = sent.indexOf(MARK, at + 1)) {
    const memory = parse(sent.slice(at + MARK.length));
    if (memory) return { text: sent.slice(0, at), memory };
  }
  return { text: sent, memory: null };
}

function parse(block: string): MemoryContext | null {
  const close = block.indexOf(")\n");
  if (close <= 0) return null;
  const revision = block.slice(0, close);
  if (/\s/.test(revision)) return null;
  const rest = block.slice(close + 2);
  const end = rest.indexOf("\n\n");
  if (end < 0) return null;
  const header = rest.slice(0, end).split("\n");
  const guidance = header.pop();
  if (!guidance?.startsWith(GUIDANCE) || header.length > 1) return null;
  const edited = header[0];
  if (edited !== undefined && !edited.startsWith(EDITED)) return null;
  // Only its start names who edited it; the rest tells the agent how to weigh it.
  const named = edited?.lastIndexOf(EDITED_END) ?? -1;
  const body = rest.slice(end + 2);
  return {
    revision,
    editedBy: edited === undefined ? null : named < 0 ? edited : `${edited.slice(0, named)}.`,
    markdown: body.endsWith("\n") ? body.slice(0, -1) : body,
  };
}
