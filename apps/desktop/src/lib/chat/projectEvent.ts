/**
 * A prompt Ghost itself sent to a chat for its coordinated project: the brief that starts a
 * coordinator, a worker's assignment, a worker's report on its way to the coordinator, a routine.
 *
 * `EVENT_PREFIX` in `crates/ghost-core/src/session/coordinator.rs` begins each one:
 *
 *     [auracode project event] <title>\n
 *     \n
 *     <what the agent was sent>
 *
 * The person did not type it, so the chat shows it as one quiet line that opens what was sent,
 * not as their message.
 */
export interface ProjectEvent {
  /** What happened, in a few words: "Worker report", "Routine: Watch pull requests". */
  readonly title: string;
  /** Everything the agent was sent after the title. */
  readonly body: string;
}

const PREFIX = "[auracode project event] ";

/** The event a sent message is, or null for a message the person wrote. */
export function projectEvent(sent: string): ProjectEvent | null {
  if (!sent.startsWith(PREFIX)) return null;
  const rest = sent.slice(PREFIX.length);
  const end = rest.indexOf("\n");
  const title = (end < 0 ? rest : rest.slice(0, end)).trim();
  if (!title) return null;
  return { title, body: end < 0 ? "" : rest.slice(end + 1).replace(/^\n+/u, "") };
}
