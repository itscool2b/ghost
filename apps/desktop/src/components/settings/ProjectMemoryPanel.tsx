import { BookOpen } from "lucide-react";
import { type CSSProperties, useCallback, useEffect, useId, useRef, useState } from "react";
import type { ProjectMemory, SetupSnapshot } from "../../bindings";
import { errorMessage } from "../../lib/errors";
import {
  adoptsSaved,
  MAX_MEMORY_BYTES,
  memoryBytes,
  newerVersion,
  type ProjectMemoryApi,
  projectMemoryApi,
} from "../../lib/projectMemory";
import { type SetupApi, setupApi } from "../../lib/setup";
import { MemoryNotes } from "./MemoryNotes";
import { Row, Switch } from "./SettingsRow";
import "./settings.css";
import "./project-memory.css";
import { Select } from "../Select";

type Project = { readonly id: string; readonly name: string };

/** The reason an edit made here is saved with. Agents give their own. */
const EDITED_HERE = "Edited in Settings";

/**
 * Project memory: a short brief every chat in a project starts with, and notes that every chat
 * sees listed and reads when one fits its task (MemoryNotes). One switch says whether agents may
 * keep them up to date. The brief is typed and saved here; a note is opened from the list.
 *
 * There is no history or review queue here (owner, 10 November 2026). An agent's edit shows a
 * notice with Undo where it happens; a version saved elsewhere while this page holds a draft is
 * offered in place of the draft, never laid over it.
 */
export function ProjectMemoryPanel({
  projects,
  initialProjectId,
  api = projectMemoryApi,
  setup = setupApi,
  titled = true,
}: {
  readonly projects: readonly Project[];
  readonly initialProjectId?: string;
  readonly api?: ProjectMemoryApi;
  /** The profile's preferences, for the switch that lets agents edit memory. */
  readonly setup?: SetupApi;
  /** Settings names the page itself: the panel then keeps only its description. */
  readonly titled?: boolean;
}) {
  const [projectId, setProjectId] = useState(initialProjectId ?? projects[0]?.id ?? "");
  // The version the draft started from, and the newest one read since.
  const [memory, setMemory] = useState<ProjectMemory | null>(null);
  const [saved, setSaved] = useState<ProjectMemory | null>(null);
  const [markdown, setMarkdown] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // The profile's switch for agents' edits; on until read, as Rust's default is.
  const [preferences, setPreferences] = useState<SetupSnapshot | null>(null);
  const [savingPolicy, setSavingPolicy] = useState(false);
  const [policyError, setPolicyError] = useState<string | null>(null);
  const agentsEdit = preferences?.preferences.general.agent_memory_writes ?? true;
  const sequence = useRef(0);
  const refreshes = useRef(0);
  const size = useId();
  const bytes = memoryBytes(markdown);
  const dirty = memory !== null && markdown !== memory.markdown;
  const newer = newerVersion(memory, saved, markdown);

  // A project chosen before the list arrived, or one that has since gone.
  useEffect(() => {
    if (projects.length > 0 && !projects.some((project) => project.id === projectId))
      setProjectId(projects[0]?.id ?? "");
  }, [projects, projectId]);

  useEffect(() => {
    let live = true;
    setup.snapshot().then(
      (next) => {
        if (live) setPreferences(next);
      },
      (failure) => {
        if (live) setPolicyError(errorMessage(failure));
      },
    );
    return () => {
      live = false;
    };
  }, [setup]);

  const setAgentsEdit = async (enabled: boolean) => {
    if (!preferences || savingPolicy) return;
    setSavingPolicy(true);
    setPolicyError(null);
    try {
      setPreferences(
        await setup.apply(preferences.preferences.revision, {
          type: "agent_memory_writes",
          enabled,
        }),
      );
    } catch (failure) {
      // Another page may have saved a preference since this one read them: read and try again.
      try {
        const fresh = await setup.snapshot();
        setPreferences(
          fresh.preferences.general.agent_memory_writes === enabled
            ? fresh
            : await setup.apply(fresh.preferences.revision, {
                type: "agent_memory_writes",
                enabled,
              }),
        );
      } catch {
        setPolicyError(errorMessage(failure));
      }
    } finally {
      setSavingPolicy(false);
    }
  };

  const load = useCallback(async () => {
    const current = ++sequence.current;
    setNotice(null);
    if (!projectId) {
      setMemory(null);
      setSaved(null);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const next = await api.read(projectId);
      if (current !== sequence.current) return;
      setMemory(next);
      setSaved(next);
      setMarkdown(next.markdown);
    } catch (failure) {
      if (current === sequence.current) setError(errorMessage(failure));
    } finally {
      if (current === sequence.current) setBusy(false);
    }
  }, [api, projectId]);
  useEffect(() => {
    void load();
    return () => {
      sequence.current += 1;
    };
  }, [load]);

  // Memory another writer changed (an agent, another device): read it without touching the draft
  // or the busy state, and never after a newer full read.
  const refresh = useCallback(async () => {
    if (!projectId) return;
    const generation = sequence.current;
    const mine = ++refreshes.current;
    try {
      const next = await api.read(projectId);
      if (generation === sequence.current && mine === refreshes.current) setSaved(next);
    } catch {
      // What is shown stays; saving reports a failure.
    }
  }, [api, projectId]);
  useEffect(
    () =>
      api.subscribe((change) => {
        if (change.project_id === null || change.project_id === projectId) void refresh();
      }),
    [api, projectId, refresh],
  );
  useEffect(() => {
    const again = () => {
      if (document.visibilityState === "visible") void refresh();
    };
    window.addEventListener("focus", again);
    document.addEventListener("visibilitychange", again);
    return () => {
      window.removeEventListener("focus", again);
      document.removeEventListener("visibilitychange", again);
    };
  }, [refresh]);
  // An editor without a draft shows the newer version at once; a draft keeps what it started
  // from, and the notice below offers the newer version instead.
  useEffect(() => {
    if (adoptsSaved(memory, saved, markdown) && saved) {
      setMemory(saved);
      setMarkdown(saved.markdown);
    }
  }, [saved, memory, markdown]);

  /** Saves the draft over the newest version this page knows of. */
  const save = async () => {
    if (!memory || busy) return;
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      const next = await api.save({
        project_id: projectId,
        expected_revision: (saved ?? memory).revision,
        markdown,
        reason: EDITED_HERE,
      });
      setMemory(next);
      setSaved(next);
      setMarkdown(next.markdown);
      setNotice("Saved. Agents get it with their next message.");
    } catch (failure) {
      setError(errorMessage(failure));
      // Memory may have changed in the moment before the save: show that version beside the draft.
      await refresh();
    } finally {
      setBusy(false);
    }
  };

  const heading = titled ? <h2>Project memory</h2> : null;
  return (
    <section className="project-memory" aria-label="Project memory">
      <div className="settings-section-heading">
        {heading}
        <p>
          What every chat in a project starts with. The brief goes to every chat, so it is short:
          your standing rules. Notes hold the rest, and an agent reads one when it fits the task.
        </p>
      </div>
      <div className="settings-group">
        <Row
          title="Let agents edit memory"
          mark={<BookOpen size={18} />}
          detail={
            agentsEdit
              ? "Agents keep it up to date as they work. A notice with Undo follows each edit."
              : "Agents read it and cannot change it."
          }
        >
          <Switch
            label="Let agents edit memory"
            testId="project-memory-direct"
            checked={agentsEdit}
            disabled={!preferences || savingPolicy}
            onChange={(enabled) => void setAgentsEdit(enabled)}
          />
        </Row>
        {projects.length > 1 ? (
          <Row
            title="Project"
            detail={
              dirty ? "Save or discard your edit to switch." : "Each project has its own memory."
            }
          >
            <Select
              className="g-select settings-select settings-control"
              aria-label="Project"
              value={projectId}
              disabled={busy || dirty}
              onChange={(event) => setProjectId(event.currentTarget.value)}
            >
              {projects.map((project) => (
                <option key={project.id} value={project.id}>
                  {project.name}
                </option>
              ))}
            </Select>
          </Row>
        ) : null}
      </div>
      {policyError ? (
        <p role="alert" className="project-memory-error">
          {policyError}
        </p>
      ) : null}
      {!projects.length ? <p>Create a project to keep shared project memory.</p> : null}
      {memory ? (
        <div className="project-memory-editor">
          {newer ? (
            <div className="project-memory-newer" role="status">
              <span>
                Memory changed while you were editing. Saving replaces that version with yours.
              </span>
              <button
                type="button"
                className="g-button g-button-28"
                disabled={busy}
                onClick={() => {
                  setMemory(newer);
                  setMarkdown(newer.markdown);
                  setError(null);
                }}
              >
                Use the newer version
              </button>
            </div>
          ) : null}
          <div className="settings-sheet">
            <div className="settings-sheet-head">
              <strong>Brief</strong>
              <span
                id={size}
                className="settings-meter"
                data-over={bytes > MAX_MEMORY_BYTES ? "" : undefined}
                style={
                  {
                    "--settings-fill": `${Math.min(100, Math.round((bytes / MAX_MEMORY_BYTES) * 100))}%`,
                  } as CSSProperties
                }
              >
                {bytes > MAX_MEMORY_BYTES
                  ? `${(bytes - MAX_MEMORY_BYTES).toLocaleString()} bytes over the 4 KB limit. Move part of it into a note.`
                  : `${bytes.toLocaleString()} of ${MAX_MEMORY_BYTES.toLocaleString()} bytes`}
                <i aria-hidden="true" />
              </span>
            </div>
            <textarea
              className="g-textarea project-memory-text settings-sheet-text"
              aria-label="Markdown"
              aria-describedby={size}
              spellCheck={false}
              value={markdown}
              disabled={busy}
              rows={16}
              placeholder="What must every chat in this project know? Keep it short: the rest belongs in notes."
              onChange={(event) => {
                setMarkdown(event.currentTarget.value);
                setNotice(null);
              }}
            />
            <div className="settings-sheet-foot">
              <span className="project-memory-status" role="status">
                {notice ?? (dirty ? "Not saved yet." : "")}
              </span>
              <button
                type="button"
                className="g-button g-button-ghost g-button-28"
                disabled={busy || !dirty}
                onClick={() => {
                  setMarkdown((saved ?? memory).markdown);
                  setMemory(saved ?? memory);
                  setError(null);
                }}
              >
                Discard
              </button>
              <button
                type="button"
                className="g-button g-button-primary g-button-28"
                disabled={busy || !dirty || bytes > MAX_MEMORY_BYTES}
                onClick={() => void save()}
              >
                Save
              </button>
            </div>
          </div>
        </div>
      ) : null}
      {error ? (
        <p role="alert" className="project-memory-error">
          {error}
        </p>
      ) : null}
      {memory ? (
        <MemoryNotes
          projectId={projectId}
          notes={(saved ?? memory).notes ?? []}
          api={api}
          onChanged={() => void refresh()}
        />
      ) : null}
    </section>
  );
}
