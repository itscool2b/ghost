import { ArrowLeft } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import type {
  WorktreeSetup,
  WorktreeSetupInput,
  WorktreeSetupPreview,
  WorktreeSetupResult,
} from "../../bindings";
import { errorMessage } from "../../lib/errors";
import { type WorktreeSetupApi, worktreeSetupApi } from "../../lib/worktreeSetup";
import { TerminalSheet } from "../TerminalSheet";

export function WorktreeSetupPanel({
  chatId,
  worktreeId,
  branch,
  onBack,
  api = worktreeSetupApi,
}: {
  readonly chatId: string;
  readonly worktreeId: string;
  readonly branch: string;
  readonly onBack: () => void;
  readonly api?: WorktreeSetupApi;
}) {
  const [saved, setSaved] = useState<WorktreeSetup | null>(null);
  const [files, setFiles] = useState("");
  const [program, setProgram] = useState("");
  const [args, setArgs] = useState("");
  const [runProgram, setRunProgram] = useState("");
  const [runArgs, setRunArgs] = useState("");
  const [review, setReview] = useState<WorktreeSetupPreview | null>(null);
  const [result, setResult] = useState<WorktreeSetupResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [terminalOpen, setTerminalOpen] = useState(false);
  const live = useRef(false);
  const acting = useRef(false);
  const transport = useMemo(
    () => (result?.command_token ? api.terminal(chatId, result.command_token) : null),
    [api, chatId, result?.command_token],
  );
  useEffect(() => {
    live.current = true;
    void api.read(chatId).then(
      (value) => {
        if (!live.current) return;
        setSaved(value);
        setFiles(value.input.files.join("\n"));
        setProgram(value.input.command?.program ?? "");
        setArgs(value.input.command?.args.join("\n") ?? "");
        setRunProgram(value.input.run?.program ?? "");
        setRunArgs(value.input.run?.args.join("\n") ?? "");
      },
      (failure) => {
        if (live.current) setError(errorMessage(failure));
      },
    );
    return () => {
      live.current = false;
    };
  }, [api, chatId]);
  const input: WorktreeSetupInput = {
    files: files
      .split(/\r?\n/)
      .map((value) => value.trim())
      .filter(Boolean),
    command: program.trim()
      ? { program: program.trim(), args: args.split(/\r?\n/).filter((value) => value.length > 0) }
      : null,
    run: runProgram.trim()
      ? {
          program: runProgram.trim(),
          args: runArgs.split(/\r?\n/).filter((value) => value.length > 0),
        }
      : null,
  };
  const dirty = JSON.stringify(saved?.input) !== JSON.stringify(input);
  const run = async (action: () => Promise<void>) => {
    if (acting.current) return;
    acting.current = true;
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      await action();
    } catch (failure) {
      if (live.current) {
        setError(errorMessage(failure));
        setReview(null);
      }
    } finally {
      acting.current = false;
      if (live.current) setBusy(false);
    }
  };
  const prepare = (commandOnly: boolean) =>
    run(async () => {
      const value = await api.preview(chatId, worktreeId, commandOnly);
      if (live.current) {
        setReview(value);
        setResult(null);
      }
    });
  return (
    <section className="git-section" aria-labelledby="worktree-setup-title">
      <button
        className="g-button g-button-ghost g-button-28 git-back"
        type="button"
        disabled={busy}
        onClick={onBack}
      >
        <ArrowLeft size={14} aria-hidden="true" /> Back to worktrees
      </button>
      <h3 id="worktree-setup-title">Setup for {branch}</h3>
      <p>
        Choose files to copy from the project folder and an optional command. This recipe belongs to
        this folder on this device.
      </p>
      {error ? (
        <p className="git-warning" role="alert">
          {error}
        </p>
      ) : null}
      {notice ? <p role="status">{notice}</p> : null}
      {!saved && !error ? <p role="status">Loading setup…</p> : null}
      {saved && !review ? (
        <form
          onSubmit={(event) => {
            event.preventDefault();
            void run(async () => {
              const value = await api.save(chatId, input);
              if (live.current) {
                setSaved(value);
                setResult(null);
                setNotice("Setup saved on this device.");
              }
            });
          }}
        >
          <p className="git-path">Source: {saved.source}</p>
          <label className="ui-field">
            Files to copy, one relative path per line
            <textarea
              className="g-textarea git-pr-body"
              value={files}
              disabled={busy}
              maxLength={32768}
              placeholder={".env.local\nconfig/development.json"}
              onChange={(event) => setFiles(event.currentTarget.value)}
            />
          </label>
          <p className="git-help">
            Only listed files are copied. Existing destination files are kept. File contents are
            never saved in this recipe.
          </p>
          <label className="ui-field">
            Setup program
            <input
              className="g-input"
              value={program}
              disabled={busy}
              maxLength={4096}
              placeholder="An executable name or full path"
              onChange={(event) => setProgram(event.currentTarget.value)}
            />
          </label>
          <label className="ui-field">
            Arguments, one per line
            <textarea
              className="g-textarea git-pr-body"
              value={args}
              disabled={busy}
              maxLength={32768}
              onChange={(event) => setArgs(event.currentTarget.value)}
            />
          </label>
          <p className="git-help">
            Each line is one argument. For a script, choose its interpreter as the program and its
            path as an argument.
          </p>
          <label className="ui-field">
            Run program
            <input
              className="g-input"
              value={runProgram}
              disabled={busy}
              maxLength={4096}
              placeholder="What Run starts, such as a dev server"
              onChange={(event) => setRunProgram(event.currentTarget.value)}
            />
          </label>
          <label className="ui-field">
            Run arguments, one per line
            <textarea
              className="g-textarea git-pr-body"
              value={runArgs}
              disabled={busy}
              maxLength={32768}
              onChange={(event) => setRunArgs(event.currentTarget.value)}
            />
          </label>
          <p className="git-help">
            Run starts this in the chat's worktree, in its terminal panel, until you stop it. Each
            worktree keeps ten local ports: the first is in GHOST_PORT for setup, Run and the
            worktree's terminals, and in PORT for Run. A server that reads neither can take it
            through a shell: cmd.exe with <code>/c</code> and{" "}
            <code>pnpm dev --port %GHOST_PORT%</code>, say.
          </p>
          <div className="git-actions">
            <button className="g-button" type="submit" disabled={busy || !dirty}>
              Save setup
            </button>
            <button
              className="g-button"
              type="button"
              disabled={busy || dirty || (!input.files.length && !input.command)}
              onClick={() => void prepare(false)}
            >
              Review setup
            </button>
            <button
              className="g-button"
              type="button"
              disabled={busy || dirty || !input.command}
              onClick={() => void prepare(true)}
            >
              Review command only
            </button>
          </div>
        </form>
      ) : null}
      {review ? (
        <section className="git-review" aria-label="Review worktree setup">
          <h4>Review setup</h4>
          <dl className="git-facts">
            <dt>From</dt>
            <dd>{review.source}</dd>
            <dt>To</dt>
            <dd>{review.destination}</dd>
          </dl>
          <ul className="git-files">
            {review.files.map((file) => (
              <li key={file.path}>
                <code>{file.path}</code>
                <span>{file.bytes.toLocaleString()} bytes</span>
              </li>
            ))}
          </ul>
          {review.command ? (
            <div>
              <h4>Command, started separately</h4>
              <code className="git-path">{review.command.program}</code>
              <ol className="git-files">
                {review.command.args.map((argument, index) => (
                  // biome-ignore lint/suspicious/noArrayIndexKey: Reviewed argv is immutable and duplicate arguments have distinct positions.
                  <li key={`${index}:${argument}`}>
                    <code>{argument}</code>
                  </li>
                ))}
              </ol>
            </div>
          ) : null}
          {review.warnings.map((warning) => (
            <p className="git-warning" key={warning}>
              {warning}
            </p>
          ))}
          <div className="git-actions">
            <button
              className="g-button"
              type="button"
              disabled={busy}
              onClick={() => {
                setReview(null);
                setResult(null);
              }}
            >
              Back to recipe
            </button>
            <button
              className="g-button g-button-primary"
              type="button"
              disabled={busy || result !== null}
              onClick={() =>
                void run(async () => {
                  const value = await api.apply(chatId, review.token);
                  if (live.current) {
                    setResult(value);
                    setNotice(
                      value.warning
                        ? null
                        : "Reviewed setup prepared. The command has not started.",
                    );
                  }
                })
              }
            >
              {review.files.length ? "Copy reviewed files" : "Prepare reviewed command"}
            </button>
          </div>
        </section>
      ) : null}
      {result ? (
        <div className="git-result" role="status">
          <p>
            {result.copied.length
              ? `Copied ${result.copied.length} ${result.copied.length === 1 ? "file" : "files"}.`
              : "No files were copied."}
          </p>
          {result.copied.length ? (
            <ul>
              {result.copied.map((path) => (
                <li key={path}>
                  <code>{path}</code>
                </li>
              ))}
            </ul>
          ) : null}
          {result.warning ? <p className="git-warning">{result.warning}</p> : null}
          {transport ? (
            <button className="g-button" type="button" onClick={() => setTerminalOpen(true)}>
              Open setup terminal
            </button>
          ) : null}
        </div>
      ) : null}
      {terminalOpen && transport ? (
        <TerminalSheet
          agentId="worktree-setup"
          agentName={branch}
          purpose="setup"
          transport={transport}
          review={
            <div>
              <p className="git-path">{review?.destination}</p>
              <code className="git-path">{review?.command?.program}</code>
              <ol>
                {review?.command?.args.map((argument, index) => (
                  // biome-ignore lint/suspicious/noArrayIndexKey: Reviewed argv is immutable and duplicate arguments have distinct positions.
                  <li key={`${index}:${argument}`}>
                    <code>{argument}</code>
                  </li>
                ))}
              </ol>
            </div>
          }
          onClose={() => {
            setTerminalOpen(false);
            setReview(null);
            setResult(null);
          }}
        />
      ) : null}
    </section>
  );
}
