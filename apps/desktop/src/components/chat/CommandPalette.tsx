import { Keycap } from "@ghost/ui/graphite";
import { File, Folder, MessageSquare, Search, Terminal } from "lucide-react";
import { useEffect, useId, useRef, useState } from "react";
import type { FileMention, SearchResults } from "../../bindings";
import type { Transport } from "../../lib/chat/transport";
import { composerDraft } from "../../lib/composerDraft";
import { errorMessage } from "../../lib/errors";
import { Modal } from "./Modal";

export interface PaletteCommand {
  readonly id: string;
  readonly title: string;
  readonly detail: string;
  readonly run: () => void;
}
type Scope = "All" | "Chats" | "Projects" | "Files" | "Commands";
type Result = {
  id: string;
  title: string;
  detail: string;
  kind: "chat" | "project" | "file" | "command";
  run: () => void;
};

export function CommandPalette({
  transport,
  chatId,
  commands,
  onChat,
  onProject,
  onClose,
}: {
  readonly transport: Transport;
  readonly chatId: string | null;
  readonly commands: readonly PaletteCommand[];
  readonly onChat: (id: string) => void;
  readonly onProject?: (id: string) => void;
  readonly onClose: () => void;
}) {
  const [query, setQuery] = useState("");
  const [scope, setScope] = useState<Scope>("All");
  const [archived, setArchived] = useState(false);
  const [data, setData] = useState<SearchResults>({ chats: [], projects: [] });
  const [files, setFiles] = useState<FileMention[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [active, setActive] = useState(0);
  const input = useRef<HTMLInputElement>(null);
  const listId = useId();
  useEffect(() => {
    input.current?.focus();
  }, []);
  useEffect(() => {
    let stale = false;
    setBusy(true);
    setError(null);
    setData({ chats: [], projects: [] });
    setFiles([]);
    const timer = setTimeout(() => {
      const work =
        scope === "Files"
          ? chatId && transport.composer
            ? transport.composer.mentions(chatId, query).then((value) => {
                if (!stale) setFiles(value);
              })
            : Promise.resolve()
          : scope === "Commands"
            ? Promise.resolve()
            : (transport.search?.search(query, archived).then((value) => {
                if (!stale) setData(value);
              }) ?? Promise.resolve());
      void work
        .catch((error) => {
          if (!stale) setError(errorMessage(error));
        })
        .finally(() => {
          if (!stale) setBusy(false);
        });
    }, 120);
    return () => {
      stale = true;
      clearTimeout(timer);
    };
  }, [query, scope, archived, chatId, transport]);
  const mention = (path: string) => {
    if (!chatId || !transport.composer) return;
    const draft = composerDraft(chatId, transport.composer);
    const current = draft.getSnapshot();
    if (!current.ready) {
      setError("The draft is still opening. Try again in a moment.");
      return;
    }
    if (current.mentions.length >= 32 && !current.mentions.includes(path)) {
      setError("This message already has 32 file mentions.");
      return;
    }
    draft.update({ mentions: [...new Set([...current.mentions, path])] });
    onClose();
  };
  const results: Result[] = [
    ...(scope === "All" || scope === "Chats"
      ? data.chats.map(
          (chat): Result => ({
            id: `chat:${chat.id}`,
            title: chat.title,
            detail: chat.snippet ?? `${chat.project}${chat.archived ? " · Archived" : ""}`,
            kind: "chat",
            run: () => {
              onChat(chat.id);
              onClose();
            },
          }),
        )
      : []),
    ...(onProject && (scope === "All" || scope === "Projects")
      ? data.projects.map(
          (project): Result => ({
            id: `project:${project.id}`,
            title: project.name,
            detail: "Open project settings",
            kind: "project",
            run: () => {
              onProject(project.id);
              onClose();
            },
          }),
        )
      : []),
    ...(scope === "Files"
      ? files.map(
          (file): Result => ({
            id: `file:${file.path}`,
            title: file.path,
            detail: "Add a file mention to the current draft",
            kind: "file",
            run: () => mention(file.path),
          }),
        )
      : []),
    ...(scope === "All" || scope === "Commands"
      ? commands
          .filter((command) =>
            `${command.title} ${command.detail}`.toLowerCase().includes(query.toLowerCase()),
          )
          .map(
            (command): Result => ({
              ...command,
              id: `command:${command.id}`,
              kind: "command",
              run: () => {
                onClose();
                command.run();
              },
            }),
          )
      : []),
  ];
  const current = Math.min(active, Math.max(0, results.length - 1));
  const rowId = (index: number) => `${listId}-${index}`;
  // biome-ignore lint/correctness/useExhaustiveDependencies: Scroll the active result when its position or count changes.
  useEffect(() => {
    document.getElementById(rowId(current))?.scrollIntoView({ block: "nearest" });
  }, [current, results.length]);
  const scopes: Scope[] = [
    "All",
    "Chats",
    ...(onProject ? ["Projects" as const] : []),
    ...(chatId && transport.composer ? ["Files" as const] : []),
    "Commands",
  ];
  return (
    <Modal title="Search and commands" size="lg" className="command-palette" onClose={onClose}>
      <div className="palette-input">
        <Search size={18} />
        <input
          ref={input}
          type="text"
          role="combobox"
          aria-label="Search and commands"
          aria-controls={listId}
          aria-expanded={true}
          aria-autocomplete="list"
          aria-activedescendant={results.length ? rowId(current) : undefined}
          placeholder="Find a chat, project, or command…"
          maxLength={128}
          value={query}
          onChange={(event) => {
            setQuery(event.currentTarget.value);
            setActive(0);
          }}
          onKeyDown={(event) => {
            if (event.nativeEvent.isComposing) return;
            if (event.key === "ArrowDown" || event.key === "ArrowUp") {
              event.preventDefault();
              if (results.length)
                setActive(
                  (current + (event.key === "ArrowDown" ? 1 : -1) + results.length) %
                    results.length,
                );
            } else if (event.key === "Enter") {
              event.preventDefault();
              results[current]?.run();
            }
          }}
        />
      </div>
      <section className="palette-scopes" aria-label="Search scope">
        {scopes.map((value) => (
          <button
            key={value}
            type="button"
            aria-pressed={scope === value}
            onClick={() => {
              setScope(value);
              setActive(0);
              input.current?.focus();
            }}
          >
            {value}
          </button>
        ))}
        {scope === "All" || scope === "Chats" ? (
          <label>
            <input
              type="checkbox"
              checked={archived}
              onChange={(event) => setArchived(event.currentTarget.checked)}
            />
            Archived
          </label>
        ) : null}
      </section>
      {error ? (
        <p className="ui-error palette-notice" role="alert">
          {error}
        </p>
      ) : null}
      <div
        className="palette-results"
        id={listId}
        role="listbox"
        aria-label="Search results"
        aria-busy={busy}
      >
        {results.map((result, index) => {
          const Icon = { chat: MessageSquare, project: Folder, file: File, command: Terminal }[
            result.kind
          ];
          return (
            <div
              id={rowId(index)}
              key={result.id}
              role="option"
              aria-selected={index === current}
              tabIndex={-1}
              onMouseMove={() => setActive(index)}
              onMouseDown={(event) => event.preventDefault()}
              onClick={result.run}
              onKeyDown={(event) => {
                if (event.key === "Enter") result.run();
              }}
            >
              <Icon size={16} />
              <span>
                <strong>{result.title}</strong>
                <small>{result.detail}</small>
              </span>
              {index === current ? <Keycap>↵</Keycap> : null}
            </div>
          );
        })}
      </div>
      {!results.length ? (
        <p className="palette-notice" role="status">
          {busy ? "Searching…" : "No matches. Try a different search."}
        </p>
      ) : null}
    </Modal>
  );
}
