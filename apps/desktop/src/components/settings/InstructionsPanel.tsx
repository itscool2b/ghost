import { AgentMark } from "@ghost/ui/graphite";
import { FolderOpen } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type {
  InstructionsAgent,
  InstructionsChange,
  InstructionsContent,
  InstructionsFile,
  InstructionsSnapshot,
  InstructionsTarget,
  SetupSnapshot,
} from "../../bindings";
import { errorMessage } from "../../lib/errors";
import {
  editableFiles,
  fileByKey,
  fileNote,
  GLOBAL,
  type InstructionsApi,
  instructionsApi,
  listNames,
  modeSentence,
  parseTargetKey,
  readersSentence,
  statusWord,
  targetKey,
  worktreeGap,
} from "../../lib/instructions";
import { type SetupApi, setupApi } from "../../lib/setup";
import { Modal } from "../chat/Modal";
import { type ReviewChoice, ReviewDialog } from "./InstructionsReview";
import { Row, Switch } from "./SettingsRow";
import "./instructions.css";
import { Select } from "../Select";

export interface InstructionsFolder {
  readonly id: string;
  readonly label: string;
}

/** The marks for instruction agents, by their catalog brand. */
const MARK: Record<string, string> = {
  claude: "claude-code",
  omp: "oh-my-pi",
};

type Pending =
  | { readonly kind: "change"; readonly change: InstructionsChange }
  | {
      readonly kind: "choose";
      readonly title: string;
      readonly sentence: string;
      readonly choices: readonly ReviewChoice[];
    };

type Confirm = { readonly kind: "include" | "commit"; readonly file: InstructionsFile };

/**
 * Instructions in Settings (visual-language 6.16a): where the agents' markdown instructions live
 * for one scope, who reads which file, one shared file or a file per agent, and the text itself.
 * Every write goes through a review of exact files and diffs that Rust prepared.
 */
export function InstructionsPanel({
  api = instructionsApi,
  setup = setupApi,
  initialTarget = GLOBAL,
  folders = [],
  chatLabel,
  openOnLoad,
}: {
  readonly api?: InstructionsApi;
  /** The profile's preferences, for the worktree switch. */
  readonly setup?: SetupApi;
  readonly initialTarget?: InstructionsTarget;
  readonly folders?: readonly InstructionsFolder[];
  /** The open chat, when the page was opened for one (its worktree). */
  readonly chatLabel?: string;
  /**
   * Development previews only: start the switch to one file, or review a seeded edit of the
   * first file, once the snapshot arrives. Nothing is applied without Apply.
   */
  readonly openOnLoad?:
    | { readonly kind: "switch" }
    | { readonly kind: "review"; readonly text: string };
}) {
  const [target, setTarget] = useState<InstructionsTarget>(initialTarget);
  const [snapshot, setSnapshot] = useState<InstructionsSnapshot | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [view, setView] = useState<"universal" | "per_agent">("universal");
  const [pending, setPending] = useState<Pending | null>(null);
  const [confirm, setConfirm] = useState<Confirm | null>(null);
  const [status, setStatus] = useState<{ text: string; error: boolean } | null>(null);
  // The profile's switch for copying Git-ignored files into new worktree chats; on until read.
  const [preferences, setPreferences] = useState<SetupSnapshot | null>(null);
  const [savingCopies, setSavingCopies] = useState(false);
  const [copiesError, setCopiesError] = useState<string | null>(null);
  const copiesIgnored = preferences?.preferences.general.copy_instructions_to_worktrees ?? true;
  useEffect(() => {
    let live = true;
    setup.snapshot().then(
      (next) => {
        if (live) setPreferences(next);
      },
      (failure) => {
        if (live) setCopiesError(errorMessage(failure));
      },
    );
    return () => {
      live = false;
    };
  }, [setup]);
  const setCopies = async (enabled: boolean) => {
    if (!preferences || savingCopies) return;
    setSavingCopies(true);
    setCopiesError(null);
    try {
      setPreferences(
        await setup.apply(preferences.preferences.revision, {
          type: "copy_instructions_to_worktrees",
          enabled,
        }),
      );
    } catch (failure) {
      setCopiesError(errorMessage(failure));
    } finally {
      setSavingCopies(false);
    }
  };
  // Drafts survive switching files and scopes while the page is open; keyed by scope and file.
  const drafts = useRef(new Map<string, { text: string; revision: string | null }>());
  const [, setDraftVersion] = useState(0);
  const key = targetKey(target);
  useEffect(() => {
    setTarget(initialTarget);
  }, [initialTarget]);

  const load = useCallback(
    async (quiet = false) => {
      if (!quiet) setLoadError(null);
      try {
        const next = await api.snapshot(target);
        setSnapshot(next);
        setLoadError(null);
        setView(next.mode === "per_agent" ? "per_agent" : "universal");
      } catch (error) {
        if (!quiet) {
          setSnapshot(null);
          setLoadError(errorMessage(error));
        }
      }
    },
    [api, target],
  );
  useEffect(() => {
    setSnapshot(null);
    setSelected(null);
    setStatus(null);
    void load();
  }, [load]);
  // An edit made in another editor shows up when the window comes back.
  useEffect(() => {
    const refresh = () => {
      if (document.visibilityState === "visible") void load(true);
    };
    window.addEventListener("focus", refresh);
    document.addEventListener("visibilitychange", refresh);
    return () => {
      window.removeEventListener("focus", refresh);
      document.removeEventListener("visibilitychange", refresh);
    };
  }, [load]);

  const files = useMemo(() => {
    if (!snapshot) return [];
    if (view === "universal" || snapshot.mode === "universal") return editableFiles(snapshot);
    return editableFiles({ ...snapshot, mode: "per_agent" });
  }, [snapshot, view]);
  const file =
    (snapshot && fileByKey(snapshot, selected)) ??
    files[0] ??
    (snapshot ? fileByKey(snapshot, snapshot.default_source) : undefined);
  const draftKey = file ? `${key}|${file.key}` : "";
  const draft = drafts.current.get(draftKey);
  const text = draft?.text ?? file?.text ?? "";
  const dirty = draft !== undefined && draft.text !== (file?.text ?? "");
  const changedOutside =
    dirty && draft !== undefined && draft.revision !== (file?.revision ?? null);

  const setDraft = (value: string) => {
    if (!file) return;
    const existing = drafts.current.get(draftKey);
    drafts.current.set(draftKey, {
      text: value,
      revision: existing?.revision ?? file.revision ?? null,
    });
    setDraftVersion((version) => version + 1);
  };
  const discard = () => {
    drafts.current.delete(draftKey);
    setDraftVersion((version) => version + 1);
  };

  const switchMode = (next: "universal" | "per_agent") => {
    if (!snapshot) return;
    if (snapshot.mode === "none") {
      setView(next);
      return;
    }
    if (next === snapshot.mode) return;
    if (next === "per_agent") {
      setPending({ kind: "change", change: { type: "per_agent" } });
      return;
    }
    reviewUniversal();
  };

  /**
   * Reviews one file for every agent: the switch itself, or bringing in an agent that reads
   * nothing yet. Different texts are chosen first.
   */
  const reviewUniversal = () => {
    if (!snapshot) return;
    const source = snapshot.default_source;
    if (!source) return;
    const universal = (content: InstructionsContent): InstructionsChange => ({
      type: "universal",
      source,
      content,
      remove: [],
      agents: null,
    });
    if (snapshot.conflict.length > 1) {
      const paths = snapshot.conflict.map((key) => fileByKey(snapshot, key)?.path ?? key);
      setPending({
        kind: "choose",
        title: "Choose the shared text",
        sentence: `${listNames(paths)} have different text. Pick what ${fileByKey(snapshot, source)?.path ?? "the shared file"} keeps; the rest is backed up.`,
        choices: [
          ...snapshot.conflict.map((key) => {
            const conflictFile = fileByKey(snapshot, key);
            return {
              id: key,
              label: `Keep the text of ${conflictFile?.path ?? key}`,
              detail: firstLine(conflictFile?.text),
              change: universal(key === source ? { type: "source" } : { type: "file", file: key }),
            };
          }),
          {
            id: "combine",
            label: "Combine them",
            detail: `${listNames(paths)}, in that order.`,
            change: universal({ type: "combine" }),
          },
        ],
      });
      return;
    }
    setPending({ kind: "change", change: universal({ type: "source" }) });
  };

  const resolveDrift = (agent: InstructionsAgent) => {
    if (!snapshot || !agent.file) return;
    const copy = fileByKey(snapshot, agent.file);
    const source = fileByKey(snapshot, agent.via);
    setPending({
      kind: "choose",
      title: `${copy?.path ?? "The copy"} was edited by hand`,
      sentence: `It no longer matches ${source?.path ?? "the shared file"}. Choose which text every agent reads.`,
      choices: [
        {
          id: "restore",
          label: `Restore the text of ${source?.path ?? "the shared file"}`,
          detail: "The edit is backed up.",
          change: { type: "restore", file: agent.file },
        },
        {
          id: "adopt",
          label: "Use this edit everywhere",
          detail: `${source?.path ?? "The shared file"} and every managed copy take it.`,
          change: { type: "adopt", file: agent.file },
        },
      ],
    });
  };

  const save = () => {
    if (!file) return;
    setPending({ kind: "change", change: { type: "save", file: file.key, text } });
  };

  const opened = useRef(false);
  // biome-ignore lint/correctness/useExhaustiveDependencies: Runs once, when the first snapshot arrives.
  useEffect(() => {
    if (!openOnLoad || !snapshot || opened.current) return;
    opened.current = true;
    if (openOnLoad.kind === "switch") switchMode("universal");
    else if (file) {
      drafts.current.set(`${key}|${file.key}`, { text: openOnLoad.text, revision: file.revision });
      setPending({
        kind: "change",
        change: { type: "save", file: file.key, text: openOnLoad.text },
      });
    }
  }, [snapshot]);

  const applied = (message: string, savedKey?: string) => {
    if (savedKey) drafts.current.delete(`${key}|${savedKey}`);
    setPending(null);
    setStatus({ text: message, error: false });
    void load(true);
  };

  const runConfirm = async () => {
    if (!confirm) return;
    const { kind, file: target_file } = confirm;
    try {
      if (kind === "include") {
        await api.includeInWorktrees(target, target_file.key.replace(/^project:/, ""));
        setStatus({
          text: `New worktree chats copy ${target_file.path} during setup.`,
          error: false,
        });
      } else {
        const head = await api.commit(target, target_file.key.replace(/^project:/, ""));
        setStatus({ text: `Committed ${target_file.path} as ${head}.`, error: false });
      }
      setConfirm(null);
      void load(true);
    } catch (error) {
      setConfirm(null);
      setStatus({ text: errorMessage(error), error: true });
    }
  };

  const scopeOptions = [
    { key: "global", label: "Your user files" },
    ...folders.map((folder) => ({ key: `folder:${folder.id}`, label: folder.label })),
    ...(target.type === "chat" ? [{ key, label: chatLabel ?? "This chat's folder" }] : []),
  ];
  const universal =
    snapshot?.mode === "universal" || (snapshot?.mode === "none" && view === "universal");
  const agents = snapshot?.agents ?? [];
  // Installed agents lead; the rest fold away, as on the Agents page.
  const shown = agents.filter((agent) => agent.installed);
  const absent = agents.filter((agent) => !agent.installed);

  return (
    <div className="instructions" data-testid="instructions">
      <section className="settings-section" aria-label="Scope and mode">
        <div className="settings-group">
          <Row
            title="Applies to"
            mark={<FolderOpen size={18} />}
            detail={
              snapshot?.folder ? (
                <span className="settings-mono">{snapshot.folder}</span>
              ) : (
                "Each agent's own file in your user folder."
              )
            }
          >
            <Select
              className="g-select settings-select settings-control"
              aria-label="Applies to"
              value={key}
              onChange={(event) => setTarget(parseTargetKey(event.currentTarget.value))}
            >
              {scopeOptions.map((option) => (
                <option key={option.key} value={option.key}>
                  {option.label}
                </option>
              ))}
            </Select>
          </Row>
          {snapshot?.agents
            .filter((agent) => agent.status === "drift" && agent.installed)
            .map((agent) => (
              <Row
                key={agent.id}
                title={`Edited copy for ${agent.name}`}
                detail={`${fileByKey(snapshot, agent.file)?.path ?? "A managed copy"} no longer matches ${fileByKey(snapshot, agent.via)?.path ?? "the shared file"}.`}
              >
                <button
                  className="g-button g-button-outline g-button-28"
                  type="button"
                  onClick={() => resolveDrift(agent)}
                >
                  Resolve
                </button>
              </Row>
            ))}
          {snapshot?.worktree_branch ? (
            <Row
              title="Worktree"
              detail={
                <>
                  Changes here stay on{" "}
                  <span className="settings-mono">{snapshot.worktree_branch}</span> until you merge
                  it.
                </>
              }
            />
          ) : null}
        </div>
        {snapshot?.notices.map((notice) => (
          <p className="settings-footnote" key={notice}>
            {notice}
          </p>
        ))}
        {loadError ? (
          <div className="settings-error" role="alert">
            <p>{loadError}</p>
            <button
              className="g-button g-button-outline g-button-28"
              type="button"
              onClick={() => void load()}
            >
              Try again
            </button>
          </div>
        ) : null}
      </section>

      {snapshot ? (
        <>
          <section className="settings-section" aria-labelledby="instructions-text">
            <h2 id="instructions-text">Text</h2>
            <p className="settings-lead">
              Agents read it when a session starts. Running chats keep what they started with.
            </p>
            <div className="settings-group instructions-editor">
              {!universal && files.length > 1 ? (
                <div className="instructions-files">
                  <div className="instructions-tabs" role="tablist" aria-label="Files">
                    {files.map((option) => (
                      <button
                        key={option.key}
                        type="button"
                        role="tab"
                        aria-selected={option.key === file?.key}
                        className="instructions-file-tab"
                        data-missing={option.kind === "missing" ? "" : undefined}
                        onClick={() => setSelected(option.key)}
                      >
                        {option.path}
                      </button>
                    ))}
                  </div>
                  {file ? <span className="instructions-meta">{fileMeta(file)}</span> : null}
                </div>
              ) : null}
              {file ? (
                <FileEditor
                  tabs={!universal && files.length > 1}
                  file={file}
                  copiesIgnored={copiesIgnored}
                  snapshot={snapshot}
                  text={text}
                  dirty={dirty}
                  changedOutside={changedOutside}
                  onChange={setDraft}
                  onDiscard={discard}
                  onReview={save}
                  onRename={() =>
                    setPending({ kind: "change", change: { type: "rename", file: file.key } })
                  }
                  onInclude={() => setConfirm({ kind: "include", file })}
                  onCommit={
                    target.type === "folder"
                      ? () => setConfirm({ kind: "commit", file })
                      : undefined
                  }
                />
              ) : (
                <div className="instructions-empty">
                  <strong>No file to edit here</strong>
                  <p>None of the agents has an instructions file at this scope.</p>
                </div>
              )}
            </div>
            {status ? (
              <p
                className={
                  status.error ? "settings-footnote instructions-error" : "settings-footnote"
                }
                role={status.error ? "alert" : "status"}
              >
                {status.text}
              </p>
            ) : null}
          </section>

          <section className="settings-section" aria-labelledby="instructions-agents">
            <h2 id="instructions-agents">Agents</h2>
            <ul className="settings-group instructions-agents">
              {shown.map((agent) => (
                <AgentRow
                  key={agent.id}
                  agent={agent}
                  snapshot={snapshot}
                  onSetUp={reviewUniversal}
                />
              ))}
            </ul>
            {absent.length > 0 ? (
              <details className="instructions-more">
                <summary>
                  {absent.length} {absent.length === 1 ? "agent" : "agents"} not installed
                </summary>
                <ul className="settings-group instructions-agents">
                  {absent.map((agent) => (
                    <AgentRow
                      key={agent.id}
                      agent={agent}
                      snapshot={snapshot}
                      onSetUp={reviewUniversal}
                    />
                  ))}
                </ul>
              </details>
            ) : null}
          </section>

          {snapshot.nested.length + snapshot.parents.length > 0 ? (
            <section className="settings-section" aria-label="Files in other folders">
              <details className="instructions-more">
                <summary>
                  {snapshot.nested.length + snapshot.parents.length}{" "}
                  {snapshot.nested.length + snapshot.parents.length === 1 ? "file" : "files"} in
                  other folders
                </summary>
                <ul className="settings-group">
                  {[...snapshot.parents, ...snapshot.nested].map((other) => (
                    <li className="settings-row" key={other.path}>
                      <div className="settings-row-text">
                        <div>
                          <span className="settings-row-label settings-mono">{other.path}</span>
                          <p>
                            {other.readers.length
                              ? `Read by ${listNames(other.readers.map((id) => agentNameOf(snapshot, id)))} when working there. Edit it in your editor.`
                              : "Edit it in your editor."}
                          </p>
                        </div>
                      </div>
                    </li>
                  ))}
                </ul>
              </details>
            </section>
          ) : null}
        </>
      ) : !loadError ? (
        <p className="settings-footnote" role="status">
          Reading instruction files…
        </p>
      ) : null}

      {pending ? (
        <ReviewDialog
          api={api}
          target={target}
          snapshot={snapshot}
          change={pending.kind === "change" ? pending.change : null}
          choose={pending.kind === "choose" ? pending : null}
          onClose={() => {
            setPending(null);
            void load(true);
          }}
          onApplied={applied}
        />
      ) : null}
      {confirm ? (
        <Modal
          title={
            confirm.kind === "include"
              ? `Copy ${confirm.file.path} into new worktree chats?`
              : `Commit ${confirm.file.path}?`
          }
          onClose={() => setConfirm(null)}
        >
          <p className="instructions-lead">
            {confirm.kind === "include" ? (
              <>
                New worktree chats from this folder copy <strong>{confirm.file.path}</strong> during
                their setup. Nothing is copied now.
              </>
            ) : (
              <>
                Ghost commits only <strong>{confirm.file.path}</strong> with the message{" "}
                <strong>Add {confirm.file.path}</strong>. Anything else you staged stays staged.
              </>
            )}
          </p>
          <div className="g-dialog-actions">
            <button className="g-button" type="button" onClick={() => setConfirm(null)}>
              Cancel
            </button>
            <button
              className="g-button g-button-primary"
              type="button"
              onClick={() => void runConfirm()}
            >
              {confirm.kind === "include" ? "Copy into new chats" : "Commit"}
            </button>
          </div>
        </Modal>
      ) : null}
      <section className="settings-section" aria-labelledby="instructions-sharing">
        <h2 id="instructions-sharing">Sharing</h2>
        <div className="settings-group">
          <div className="instructions-mode-choice">
            <h3>How instructions are shared</h3>
            <p>{snapshot ? modeSentence(snapshot) : "Reading files…"}</p>
            <fieldset
              className="instructions-mode-options"
              disabled={!snapshot || pending !== null}
              data-testid="instructions-mode"
            >
              <legend className="sr-only">How instructions are shared</legend>
              {(
                [
                  [
                    "universal",
                    "Shared instructions",
                    "Maintain one shared source. Ghost shows how each agent reads it.",
                  ],
                  [
                    "per_agent",
                    "Agent-specific files",
                    "Edit native files separately. Some agents may read the same files.",
                  ],
                ] as const
              ).map(([value, label, description]) => (
                <label key={value}>
                  <input
                    type="radio"
                    name="instructions-mode"
                    value={value}
                    checked={snapshot?.mode === "none" ? view === value : snapshot?.mode === value}
                    onChange={() => switchMode(value)}
                  />
                  <span>
                    <strong>{label}</strong>
                    <small>{description}</small>
                  </span>
                </label>
              ))}
            </fieldset>
          </div>
          <Row
            title="Copy into new worktree chats"
            detail={
              copiesIgnored
                ? "Instruction files Git ignores, like a local AGENTS.md or CLAUDE.md, are copied into each new worktree chat, so its agents read them too. Files already there are kept."
                : "New worktree chats get only the instruction files committed to Git."
            }
          >
            <Switch
              label="Copy instruction files into new worktree chats"
              testId="instructions-worktree-copies"
              checked={copiesIgnored}
              disabled={!preferences || savingCopies}
              onChange={(enabled) => void setCopies(enabled)}
            />
          </Row>
        </div>
        <p className="settings-footnote">
          Files that are new but not ignored are not copied, since an agent could commit them on the
          chat's branch. Commit them instead.
        </p>
        {copiesError ? (
          <p className="settings-footnote instructions-error" role="alert">
            {copiesError}
          </p>
        ) : null}
      </section>
    </div>
  );
}

function agentNameOf(snapshot: InstructionsSnapshot, id: string): string {
  return snapshot.agents.find((agent) => agent.id === id)?.name ?? id;
}

function firstLine(text: string | null | undefined): string {
  const line = text?.split("\n").find((value) => value.trim() !== "") ?? "";
  return line.length > 72 ? `${line.slice(0, 72)}…` : line || "Empty";
}

function AgentRow({
  agent,
  snapshot,
  onSetUp,
}: {
  readonly agent: InstructionsAgent;
  readonly snapshot: InstructionsSnapshot;
  readonly onSetUp: () => void;
}) {
  const { word, tone } = statusWord(agent);
  // One action at most, and only where something needs the person. An edited copy is resolved
  // from its own row at the top of the page.
  const action =
    agent.status === "not_set" &&
    agent.installed &&
    snapshot.mode === "universal" &&
    snapshot.default_source
      ? { label: "Set up", run: onSetUp }
      : null;
  return (
    <li
      className="settings-row instructions-agent"
      data-agent={agent.id}
      data-status={agent.status}
    >
      <div className="settings-row-text">
        <AgentMark agent={MARK[agent.id] ?? agent.id} size={24} ghost={!agent.installed} />
        <div>
          <strong>{agent.name}</strong>
          <p>{agent.detail}</p>
          {agent.warnings.map((warning) => (
            <p className="instructions-warning" key={warning}>
              {warning}
            </p>
          ))}
        </div>
      </div>
      <div className="instructions-agent-end">
        <span className="instructions-status" data-tone={tone}>
          <span className="instructions-dot" aria-hidden="true" />
          {word}
        </span>
        {action ? (
          <button
            className="g-button g-button-outline g-button-28"
            type="button"
            aria-label={`${action.label} ${agent.name}`}
            onClick={action.run}
          >
            {action.label}
          </button>
        ) : null}
      </div>
    </li>
  );
}

function FileEditor({
  file,
  snapshot,
  text,
  dirty,
  changedOutside,
  onChange,
  onDiscard,
  onReview,
  onRename,
  onInclude,
  onCommit,
  tabs,
  copiesIgnored,
}: {
  readonly file: InstructionsFile;
  /** Instructions' worktree switch: a file Git ignores is copied into new worktree chats. */
  readonly copiesIgnored: boolean;
  readonly snapshot: InstructionsSnapshot;
  readonly text: string;
  readonly dirty: boolean;
  readonly changedOutside: boolean;
  readonly onChange: (text: string) => void;
  readonly onDiscard: () => void;
  readonly onReview: () => void;
  readonly onRename: () => void;
  readonly onInclude: () => void;
  readonly onCommit?: () => void;
  /** The file tabs above already name the file. */
  readonly tabs: boolean;
}) {
  const gap = worktreeGap(file, copiesIgnored);
  const editable = file.editable;
  const notes: { text: string; volt: boolean }[] = [];
  const note = fileNote(file, copiesIgnored);
  if (note) notes.push({ text: note, volt: !editable || gap !== null || !!file.disk_name });
  if (file.managed)
    notes.push({
      text: file.managed.edited
        ? "This managed copy was edited by hand."
        : `A copy of ${fileByKey(snapshot, file.managed.source)?.path ?? "the shared file"} that Ghost keeps in sync.`,
      volt: file.managed.edited,
    });
  if (file.import_of)
    notes.push({
      text: `Only imports ${fileByKey(snapshot, file.import_of)?.path ?? "another file"}. Lines below the import are for Claude Code alone.`,
      volt: false,
    });
  // Size limits of the installed agents that read this file, next to the text they are about.
  for (const warning of new Set(
    snapshot.agents
      .filter(
        (agent) =>
          agent.installed &&
          (agent.file === file.key || agent.via === file.key || file.readers.includes(agent.id)),
      )
      .flatMap((agent) => agent.warnings),
  ))
    notes.push({ text: warning, volt: true });
  if (changedOutside)
    notes.push({
      text: `${file.path} changed on disk since you started editing. The review compares with the file as it is now.`,
      volt: true,
    });
  return (
    <div className="instructions-file">
      {/* With file tabs the tab row names the file and carries its size. */}
      {tabs ? null : (
        <div className="instructions-file-head">
          <span className="instructions-path settings-mono">{file.path}</span>
          <span className="instructions-meta">{fileMeta(file)}</span>
        </div>
      )}
      {notes.length > 0 ? (
        <ul className="instructions-notes" aria-label={`About ${file.path}`}>
          {notes.map((note) => (
            <li key={note.text} data-volt={note.volt ? "" : undefined}>
              <span className="instructions-bullet" aria-hidden="true" />
              {note.text}
            </li>
          ))}
        </ul>
      ) : null}
      <textarea
        className="g-textarea instructions-textarea settings-sheet-text"
        aria-label={`Text of ${file.path}`}
        spellCheck={false}
        readOnly={!editable}
        value={editable || file.text !== null ? text : ""}
        placeholder={
          editable
            ? file.kind === "missing"
              ? `Write the instructions for ${file.path}.`
              : "Empty. Agents skip an empty file."
            : undefined
        }
        onChange={(event) => onChange(event.currentTarget.value)}
      />
      <div className="instructions-foot">
        <p>{readersSentence(snapshot, file)}</p>
        <div className="settings-actions">
          {file.disk_name && editable ? (
            <button
              className="g-button g-button-outline g-button-28"
              type="button"
              onClick={onRename}
            >
              Rename to {file.path}
            </button>
          ) : null}
          {gap && !dirty ? (
            <button
              className="g-button g-button-outline g-button-28"
              type="button"
              onClick={onInclude}
            >
              Copy into new worktree chats
            </button>
          ) : null}
          {gap?.commit && onCommit && !dirty ? (
            <button
              className="g-button g-button-outline g-button-28"
              type="button"
              onClick={onCommit}
            >
              Commit
            </button>
          ) : null}
          {dirty ? (
            <button
              className="g-button g-button-ghost g-button-28"
              type="button"
              onClick={onDiscard}
            >
              Discard
            </button>
          ) : null}
          <button
            className="g-button g-button-primary g-button-28"
            type="button"
            disabled={!editable || !dirty}
            onClick={onReview}
          >
            Review changes
          </button>
        </div>
      </div>
    </div>
  );
}

function fileMeta(file: InstructionsFile): string {
  return file.kind === "missing"
    ? "Not created yet"
    : `${file.lines} ${file.lines === 1 ? "line" : "lines"} · ${formatBytes(file.bytes)}`;
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1).replace(/\.0$/, "")} KiB`;
  return `${(bytes / 1024 / 1024).toFixed(1).replace(/\.0$/, "")} MiB`;
}
