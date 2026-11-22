import { type CSSProperties, useEffect, useId, useRef, useState } from "react";
import type { MemoryNote, MemoryNoteSummary } from "../../bindings";
import { errorMessage } from "../../lib/errors";
import {
  MAX_NOTE_BYTES,
  MAX_NOTE_DESCRIPTION,
  MAX_NOTES,
  memoryBytes,
  NOTE_NAME,
  type ProjectMemoryApi,
} from "../../lib/projectMemory";
import { Row } from "./SettingsRow";

/** The reason a note changed here is saved with. Agents give their own. */
const EDITED_HERE = "Edited in Settings";

/** Who saved a note last, as its row says it. */
function savedBy(by: string): string {
  return by === "agent" ? "an agent" : by === "bot" ? "a linked assistant" : "you";
}

/** How long ago a note was saved, in days: what tells an old note from a fresh one. */
export function noteAge(updatedAt: number, now: number): string {
  const days = Math.floor(Math.max(0, now - updatedAt) / 86_400_000);
  return days === 0 ? "today" : days === 1 ? "yesterday" : `${days} days ago`;
}

/** The note that is open: one that exists, as it was read, or a new one. */
type Open = { readonly kind: "new" } | { readonly kind: "note"; readonly read: MemoryNote };

/**
 * The notes of a project's memory. Every chat in the project sees this list (each note's name and
 * its one line) and its agent reads a note when that line fits the task, so the line is what
 * decides whether a note is ever used.
 *
 * One note is open at a time, to read, change or remove. A save names the revision that was read:
 * a note an agent changed meanwhile is not overwritten, and the refusal says so. There is no
 * history here, as on the rest of the page.
 */
export function MemoryNotes({
  projectId,
  notes,
  api,
  onChanged,
}: {
  readonly projectId: string;
  readonly notes: readonly MemoryNoteSummary[];
  readonly api: ProjectMemoryApi;
  /** A note was saved or removed here: the page reads the memory again. */
  readonly onChanged: () => void;
}) {
  const [open, setOpen] = useState<Open | null>(null);
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [markdown, setMarkdown] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const size = useId();
  const nameHint = useId();
  const sequence = useRef(0);

  // Another project's notes are another list: nothing of this one stays open.
  // biome-ignore lint/correctness/useExhaustiveDependencies: Closing follows the project alone.
  useEffect(() => {
    sequence.current += 1;
    setOpen(null);
    setError(null);
    setStatus(null);
  }, [projectId]);

  const close = () => {
    sequence.current += 1;
    setOpen(null);
    setError(null);
  };

  const start = () => {
    sequence.current += 1;
    setOpen({ kind: "new" });
    setName("");
    setDescription("");
    setMarkdown("");
    setError(null);
    setStatus(null);
  };

  const read = async (note: MemoryNoteSummary) => {
    const current = ++sequence.current;
    setBusy(true);
    setError(null);
    setStatus(null);
    try {
      const next = await api.readNote(projectId, note.name);
      if (current !== sequence.current) return;
      setOpen({ kind: "note", read: next });
      setName(next.name);
      setDescription(next.description);
      setMarkdown(next.markdown);
    } catch (failure) {
      if (current === sequence.current) setError(errorMessage(failure));
    } finally {
      if (current === sequence.current) setBusy(false);
    }
  };

  const existing = open?.kind === "note" ? open.read : null;
  const bytes = memoryBytes(markdown);
  const named = NOTE_NAME.test(name);
  const taken = open?.kind === "new" && notes.some((note) => note.name === name);
  const described = description.trim() !== "" && description.trim().length <= MAX_NOTE_DESCRIPTION;
  const dirty =
    open?.kind === "new"
      ? name !== "" || description !== "" || markdown !== ""
      : existing !== null &&
        (description.trim() !== existing.description || markdown !== existing.markdown);
  const ready = named && !taken && described && bytes <= MAX_NOTE_BYTES && dirty;

  const save = async () => {
    if (open === null || busy || !ready) return;
    setBusy(true);
    setError(null);
    try {
      const saved = await api.saveNote({
        project_id: projectId,
        name,
        expected_revision: existing?.revision ?? null,
        description: description.trim(),
        markdown,
        reason: EDITED_HERE,
      });
      setOpen({ kind: "note", read: saved });
      setDescription(saved.description);
      setMarkdown(saved.markdown);
      setStatus(`Saved ${saved.name}. Agents see it in the list with their next message.`);
      onChanged();
    } catch (failure) {
      setError(errorMessage(failure));
    } finally {
      setBusy(false);
    }
  };

  const remove = async () => {
    if (existing === null || busy) return;
    setBusy(true);
    setError(null);
    try {
      await api.deleteNote({
        project_id: projectId,
        name: existing.name,
        expected_revision: existing.revision,
        reason: EDITED_HERE,
      });
      setOpen(null);
      setStatus(`Deleted ${existing.name}.`);
      onChanged();
    } catch (failure) {
      setError(errorMessage(failure));
    } finally {
      setBusy(false);
    }
  };

  const now = Date.now();
  const full = notes.length >= MAX_NOTES;
  return (
    <div className="project-memory-notes" data-testid="memory-notes">
      <div className="settings-section-heading project-memory-notes-heading">
        <div>
          <h3>Notes</h3>
          <p>
            What a chat needs only sometimes: how one area is built, a decision and its reason, a
            trap. Every chat sees this list, and its agent reads a note when the line fits.
          </p>
        </div>
        <button
          type="button"
          className="g-button g-button-28"
          disabled={busy || full || open?.kind === "new"}
          title={full ? `A project has at most ${MAX_NOTES} notes.` : undefined}
          onClick={start}
        >
          New note
        </button>
      </div>
      {notes.length > 0 ? (
        <ul className="settings-group project-memory-note-list" aria-label="Notes">
          {notes.map((note) => (
            <li key={note.name}>
              <Row
                title={note.name}
                setting={`note:${note.name}`}
                detail={
                  <>
                    {note.description}
                    <span className="project-memory-note-meta">
                      {" "}
                      Saved by {savedBy(note.by)}, {noteAge(note.updated_at, now)}.
                    </span>
                  </>
                }
              >
                <button
                  type="button"
                  className="g-button g-button-ghost g-button-28"
                  aria-label={`Open ${note.name}`}
                  aria-pressed={existing?.name === note.name}
                  disabled={busy}
                  onClick={() => void read(note)}
                >
                  Open
                </button>
              </Row>
            </li>
          ))}
        </ul>
      ) : open === null ? (
        <p className="project-memory-notes-empty">
          No notes yet. Agents add them as they learn how this project works, and so can you.
        </p>
      ) : null}
      {open !== null ? (
        <div className="settings-sheet project-memory-note" data-testid="memory-note-editor">
          <div className="settings-sheet-head">
            <strong>{existing ? existing.name : "New note"}</strong>
            <span
              id={size}
              className="settings-meter"
              data-over={bytes > MAX_NOTE_BYTES ? "" : undefined}
              style={
                {
                  "--settings-fill": `${Math.min(100, Math.round((bytes / MAX_NOTE_BYTES) * 100))}%`,
                } as CSSProperties
              }
            >
              {bytes > MAX_NOTE_BYTES
                ? `${(bytes - MAX_NOTE_BYTES).toLocaleString()} bytes over the 16 KB limit`
                : `${bytes.toLocaleString()} of ${MAX_NOTE_BYTES.toLocaleString()} bytes`}
              <i aria-hidden="true" />
            </span>
          </div>
          <div className="project-memory-note-fields">
            {existing ? null : (
              <label>
                <span>Name</span>
                <input
                  className="g-input"
                  aria-describedby={nameHint}
                  aria-invalid={name !== "" && (!named || taken) ? true : undefined}
                  spellCheck={false}
                  autoComplete="off"
                  maxLength={40}
                  placeholder="windows-build"
                  value={name}
                  disabled={busy}
                  onChange={(event) => setName(event.currentTarget.value)}
                />
                <small id={nameHint}>
                  {taken
                    ? "A note has this name already."
                    : "Lowercase letters, digits and dashes. A note keeps its name."}
                </small>
              </label>
            )}
            <label>
              <span>When to read it</span>
              <input
                className="g-input"
                maxLength={MAX_NOTE_DESCRIPTION}
                placeholder="Read before building the app on Windows"
                value={description}
                disabled={busy}
                onChange={(event) => setDescription(event.currentTarget.value)}
              />
            </label>
          </div>
          <textarea
            className="g-textarea project-memory-text settings-sheet-text"
            aria-label={existing ? `${existing.name} Markdown` : "Note Markdown"}
            aria-describedby={size}
            spellCheck={false}
            value={markdown}
            disabled={busy}
            rows={12}
            placeholder="What should a chat working on this know?"
            onChange={(event) => setMarkdown(event.currentTarget.value)}
          />
          <div className="settings-sheet-foot">
            <span className="project-memory-status" role="status">
              {dirty ? "Not saved yet." : (status ?? "")}
            </span>
            {existing ? (
              <button
                type="button"
                className="g-button g-button-ghost g-button-28 project-memory-note-delete"
                disabled={busy}
                onClick={() => void remove()}
              >
                Delete note
              </button>
            ) : null}
            <button
              type="button"
              className="g-button g-button-ghost g-button-28"
              disabled={busy}
              onClick={close}
            >
              {dirty ? "Discard" : "Close"}
            </button>
            <button
              type="button"
              className="g-button g-button-primary g-button-28"
              disabled={busy || !ready}
              onClick={() => void save()}
            >
              Save note
            </button>
          </div>
        </div>
      ) : null}
      {status && open === null ? (
        <p className="project-memory-status" role="status">
          {status}
        </p>
      ) : null}
      {error ? (
        <p role="alert" className="project-memory-error">
          {error}
        </p>
      ) : null}
    </div>
  );
}
