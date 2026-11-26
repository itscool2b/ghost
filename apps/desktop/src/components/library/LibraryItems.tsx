import { useEffect, useRef, useState } from "react";
import { errorMessage } from "../../lib/errors";
import type { LibraryApi } from "../../lib/library/api";
import type {
  LibraryCopyPreview,
  LibraryCopyReceipt,
  LibraryItem,
  LibraryItemKind,
  LibraryItemPreview,
  LibrarySourceStatus,
} from "../../lib/library/bindings";
import { Modal } from "../chat/Modal";
import "./library.css";
import { Select } from "../Select";

const LABELS = { claude: "Claude Code", codex: "Codex", pi: "Pi", shared: "Shared" };
const KINDS = [
  { value: "all", label: "Everything" },
  { value: "skill", label: "Skills" },
  { value: "mcp_server", label: "MCP servers" },
  { value: "plugin", label: "Plugins" },
  { value: "package", label: "Packages" },
] as const;

const sourceLabel = (source: LibrarySourceStatus) =>
  `${LABELS[source.provider]} · ${source.scope === "user" ? "Personal" : "Project"}${source.id.endsWith(":claude-mcp") ? " MCP" : ""}`;
const kindLabel = (kind: LibraryItemKind) =>
  KINDS.find((entry) => entry.value === kind)?.label ?? kind;
const sourceName = (sources: readonly LibrarySourceStatus[], id: string) => {
  const source = sources.find((entry) => entry.id === id);
  return source ? sourceLabel(source) : id;
};

/**
 * One Library item's preview with its reviewed copy to another agent, and the copy history with
 * its reviewed restores. The Plugin library opens both in dialogs; nothing changes an agent's
 * files before its review.
 */
export function ItemDetail({
  api,
  folder,
  item,
  sources,
  complete,
  manage,
}: {
  readonly api: LibraryApi;
  readonly folder: string | null;
  readonly item: LibraryItem;
  readonly sources: readonly LibrarySourceStatus[];
  readonly complete: (message: string) => void;
  readonly manage: () => void;
}) {
  const [preview, setPreview] = useState<LibraryItemPreview | null>(null);
  const [plan, setPlan] = useState<LibraryCopyPreview | null>(null);
  const [sharing, setSharing] = useState(false);
  const [target, setTarget] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const alive = useRef(true);
  const pending = useRef<string | null>(null);
  useEffect(() => {
    alive.current = true;
    void api
      .preview(folder, item.item, item.revision)
      .then((value) => {
        if (alive.current) setPreview(value);
      })
      .catch((reason) => {
        if (alive.current) setError(errorMessage(reason));
      });
    return () => {
      alive.current = false;
      if (pending.current) void api.cancel(pending.current).catch(() => {});
    };
  }, [api, folder, item]);
  const cancel = () => {
    if (busy) return;
    const id = pending.current;
    pending.current = null;
    setPlan(null);
    if (id) void api.cancel(id).catch((reason) => setError(errorMessage(reason)));
  };
  const prepare = async () => {
    setBusy(true);
    setError(null);
    try {
      const value = await api.prepare(folder, item.item, item.revision, target);
      if (!alive.current) {
        await api.cancel(value.id);
        return;
      }
      pending.current = value.id;
      setPlan(value);
    } catch (reason) {
      if (alive.current) setError(errorMessage(reason));
    } finally {
      if (alive.current) setBusy(false);
    }
  };
  const apply = async () => {
    if (!plan) return;
    setBusy(true);
    setError(null);
    pending.current = null;
    try {
      const result = await api.apply(plan.id);
      if (alive.current)
        complete(
          result.recovery_required
            ? (result.message ?? "Copy interrupted. Open Copy history to recover.")
            : "Copy saved. Originals are available in Copy history.",
        );
    } catch (reason) {
      if (alive.current) {
        setPlan(null);
        setError(errorMessage(reason));
      }
    } finally {
      if (alive.current) setBusy(false);
    }
  };
  const blocked =
    item.copy_blocked ??
    (!["skill", "instructions", "mcp_server"].includes(item.item.kind)
      ? "Copying this item needs its agent's configuration workflow. You can read it here."
      : null);
  const destinations = sources.filter(
    (source) =>
      source.available &&
      source.id !== item.item.source_id &&
      (item.item.kind === "mcp_server"
        ? source.provider !== "claude" ||
          source.scope === "project" ||
          source.id.endsWith(":claude-mcp")
        : !source.id.endsWith(":claude-mcp")) &&
      (source.provider !== "shared" || item.item.kind === "skill"),
  );
  const managed =
    (item.item.kind === "plugin" || item.item.kind === "package") && item.provider !== "shared";
  // Shared MCP reaches every agent Ghost runs, where a copy reaches one agent's files.
  const shareable = item.item.kind === "mcp_server" && item.provider !== "shared";
  const share = async () => {
    setBusy(true);
    setError(null);
    try {
      await api.share(folder, item.item, item.revision);
      if (alive.current)
        complete(
          `Shared ${item.label} with every agent. Each gets it from its next session; one that already has a server named ${item.label} keeps its own.`,
        );
    } catch (reason) {
      if (alive.current) {
        setSharing(false);
        setError(errorMessage(reason));
      }
    } finally {
      if (alive.current) setBusy(false);
    }
  };
  return (
    <section className="library-detail" aria-label="Item preview">
      <header>
        <div>
          <h2>{item.label}</h2>
          <p>
            {LABELS[item.provider]} · {item.scope === "user" ? "Personal" : "Project"} ·{" "}
            {kindLabel(item.item.kind)}
          </p>
          <code>{item.relative_path}</code>
        </div>
        {managed && item.scope === "user" ? (
          <button type="button" className="g-button g-button-outline g-button-28" onClick={manage}>
            Manage with {LABELS[item.provider]}
          </button>
        ) : null}
      </header>
      {managed && item.scope !== "user" ? (
        <p className="library-detail-note">
          Manage this project integration through its vendor. Personal ones are in Manage
          integrations.
        </p>
      ) : null}
      {error ? (
        <p className="library-feedback" role="alert">
          {error}
        </p>
      ) : null}
      {preview ? (
        <>
          <div className="library-well">
            <div className="library-file-heading">
              <span>{item.relative_path.split("/").pop()}</span>
              <small>
                {preview.files.length > 1
                  ? `${preview.files.length} files`
                  : `${item.bytes.toLocaleString()} bytes`}
              </small>
            </div>
            <pre className="library-code">{preview.text}</pre>
          </div>
          {preview.files.length > 1 ? (
            <details className="library-file-list">
              <summary>Supporting files</summary>
              <ul>
                {preview.files.map((file) => (
                  <li key={file}>
                    <code>{file}</code>
                  </li>
                ))}
              </ul>
            </details>
          ) : null}
          {preview.notices.map((notice) => (
            <p key={notice} className="library-detail-note">
              {notice}
            </p>
          ))}
        </>
      ) : !error ? (
        <p role="status" className="library-detail-note">
          Reading item…
        </p>
      ) : null}
      <div className="library-copy-controls">
        {blocked ? (
          <p>{blocked}</p>
        ) : (
          <>
            <label>
              Copy to
              <Select
                className="g-select"
                aria-label="Copy destination"
                value={target}
                disabled={busy || !preview}
                onChange={(event) => setTarget(event.currentTarget.value)}
              >
                <option value="">Choose an agent and scope</option>
                {destinations.map((source) => (
                  <option key={source.id} value={source.id}>
                    {sourceLabel(source)}
                  </option>
                ))}
              </Select>
            </label>
            <button
              type="button"
              className="g-button g-button-primary g-button-36"
              disabled={busy || !target || !preview}
              onClick={() => void prepare()}
            >
              {busy ? "Preparing…" : "Review copy"}
            </button>
          </>
        )}
      </div>
      {shareable ? (
        <div className="library-copy-controls library-share">
          <p>Give this server to every agent that takes shared servers, in every project.</p>
          <button
            type="button"
            className="g-button g-button-outline g-button-36"
            disabled={busy || !preview}
            onClick={() => setSharing(true)}
          >
            Share with every agent
          </button>
        </div>
      ) : null}
      {sharing ? (
        <Modal
          title="Share with every agent"
          size="md"
          className="library-review"
          onClose={() => {
            if (!busy) setSharing(false);
          }}
        >
          <p className="library-review-intro">
            <strong>{item.label}</strong> from{" "}
            <strong>{sourceName(sources, item.item.source_id)}</strong> becomes a shared MCP server
            in every project for Claude Code, Codex, Pi, OpenCode, Cursor, Grok Build and Hermes.
            Antigravity and oh-my-pi, whose own servers Ghost cannot check, start without it and say
            so in their chats.
          </p>
          <p className="library-detail-note">
            The shared definition keeps its command and arguments, or its address, and the names of
            its environment variables and headers. Their values stay on this device as local
            credentials, never exported. {LABELS[item.provider]}'s own configuration is left as it
            is.
          </p>
          <div className="g-dialog-actions library-actions">
            <button
              type="button"
              className="g-button"
              disabled={busy}
              onClick={() => setSharing(false)}
            >
              Cancel
            </button>
            <button
              type="button"
              className="g-button g-button-primary"
              disabled={busy}
              onClick={() => void share()}
            >
              {busy ? "Sharing…" : "Share"}
            </button>
          </div>
        </Modal>
      ) : null}
      {plan ? (
        <Modal title="Review copy" size="lg" className="library-review" onClose={cancel}>
          <p className="library-review-intro">
            Copying <strong>{item.label}</strong> from{" "}
            <strong>{sourceName(sources, item.item.source_id)}</strong> to{" "}
            <strong>{sourceName(sources, plan.to_source)}</strong>.
          </p>
          <div className="library-comparison">
            <section>
              <h3>Before</h3>
              <pre>{plan.before_text ?? "New file"}</pre>
            </section>
            <section>
              <h3>After</h3>
              <pre>{plan.after_text}</pre>
            </section>
          </div>
          <details className="library-review-files">
            <summary>
              {plan.files.length} {plan.files.length === 1 ? "file" : "files"} in this copy
            </summary>
            <ul>
              {plan.files.map((file) => (
                <li key={file.path}>
                  <code>{file.path}</code>
                  <span>{file.before_revision ? "Replace" : "Create"}</span>
                </li>
              ))}
            </ul>
          </details>
          {plan.notices.map((notice) => (
            <p className="library-detail-note" key={notice}>
              {notice}
            </p>
          ))}
          <div className="g-dialog-actions library-actions">
            <button type="button" className="g-button" disabled={busy} onClick={cancel}>
              Cancel
            </button>
            <button
              type="button"
              className="g-button g-button-primary"
              disabled={busy}
              onClick={() => void apply()}
            >
              {busy ? "Saving…" : "Apply copy"}
            </button>
          </div>
        </Modal>
      ) : null}
    </section>
  );
}

export function HistoryView({
  api,
  folder,
  receipts,
  sources,
  complete,
}: {
  readonly api: LibraryApi;
  readonly folder: string | null;
  readonly receipts: readonly LibraryCopyReceipt[];
  readonly sources: readonly LibrarySourceStatus[];
  readonly complete: (message: string) => void;
}) {
  const [selected, setSelected] = useState<LibraryCopyReceipt | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const restore = async () => {
    if (!selected) return;
    setBusy(true);
    setError(null);
    try {
      const result = await api.rollback(folder, selected.source_id, selected.id);
      setSelected(null);
      complete(
        result.recovery_required
          ? (result.message ?? "Recovery needs another review.")
          : "Original files restored.",
      );
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  };
  return (
    <section className="library-history" aria-label="Copy history">
      <h2>Copy history</h2>
      <p>Restore the original files after a copy. Files edited since are left alone.</p>
      {receipts.length === 0 ? (
        <div className="library-empty">
          <h3>No copies yet</h3>
          <p>Copies you apply from the Library appear here.</p>
        </div>
      ) : (
        <ul>
          {receipts.map((receipt) => (
            <li key={`${receipt.source_id}:${receipt.id}`} data-state={receipt.state}>
              <div>
                <strong>{receipt.files.join(", ")}</strong>
                <small>
                  {sourceName(sources, receipt.source_id)} ·{" "}
                  <span className="library-receipt-state">
                    {receipt.state === "complete"
                      ? "Copied"
                      : receipt.state === "rolled_back"
                        ? "Restored"
                        : "Needs recovery"}
                  </span>
                </small>
              </div>
              <code title="Copy receipt">{receipt.id}</code>
              <button
                type="button"
                className="g-button g-button-outline g-button-28"
                disabled={receipt.state === "rolled_back"}
                onClick={() => setSelected(receipt)}
              >
                Review restore
              </button>
            </li>
          ))}
        </ul>
      )}
      {selected ? (
        <Modal
          title="Restore original files?"
          size="md"
          className="library-review"
          onClose={() => {
            if (!busy) setSelected(null);
          }}
        >
          <p className="library-review-intro">
            Restores the files kept from before this copy to{" "}
            <strong>{sourceName(sources, selected.source_id)}</strong>. If one was edited since,
            nothing changes.
          </p>
          <ul className="library-restore-files">
            {selected.files.map((file) => (
              <li key={file}>
                <code>{file}</code>
              </li>
            ))}
          </ul>
          {error ? (
            <p role="alert" className="library-dialog-error">
              {error}
            </p>
          ) : null}
          <div className="g-dialog-actions library-actions">
            <button
              type="button"
              className="g-button"
              disabled={busy}
              onClick={() => setSelected(null)}
            >
              Cancel
            </button>
            <button
              type="button"
              className="g-button g-button-primary"
              disabled={busy}
              onClick={() => void restore()}
            >
              {busy ? "Restoring…" : "Restore originals"}
            </button>
          </div>
        </Modal>
      ) : null}
    </section>
  );
}
