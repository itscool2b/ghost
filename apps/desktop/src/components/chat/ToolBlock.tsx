import { prefersReducedMotion } from "@ghost/ui";
import { CopyButton, StatusDot } from "@ghost/ui/graphite";
import {
  ArrowRightLeft,
  Brain,
  ChevronRight,
  FileMinus,
  FilePenLine,
  FileText,
  Globe,
  Search,
  SlidersHorizontal,
  SquareTerminal,
  Wrench,
} from "lucide-react";
import { useContext, useEffect, useMemo, useRef, useState } from "react";
import type { ToolContent, ToolLocation, Truncation } from "../../bindings";
import type { BlobState } from "../../lib/chat/chatStore";
import {
  diffDelta,
  type InputField,
  prettyJson,
  type SearchLink,
  searchLinks,
  toolInput,
  withoutSearchLinks,
} from "../../lib/chat/toolDetails";
import { toolPhrase } from "../../lib/chat/toolWords";
import type { ToolDisplayStatus, ToolItem } from "../../lib/chat/transcript";
import { imageSource } from "../../lib/chatImages";
import { formatCount } from "../../lib/format";
import { type Inline, parseInline } from "../../lib/markdown";
import { ChatImageTile } from "./ChatImages";
import { DiffBlock } from "./DiffBlock";
import { useDisclosure } from "./DisclosureMemory";
import { FileLink } from "./FileLink";
import { TerminalOutput } from "./TerminalOutput";
import { TaskLinkContext } from "./taskLinks";
import { RevealContext } from "./VirtualRows";

/** What a block needs to page a truncated output in through `chat_blob`. */
export interface BlobProps {
  readonly blobs: ReadonlyMap<string, BlobState>;
  readonly onLoadBlob: (sha256: string) => void;
}

function OutputImages({ text }: { readonly text: string }) {
  const images: Extract<Inline, { kind: "image" }>[] = [];
  const visit = (nodes: readonly Inline[]) => {
    for (const node of nodes) {
      if (images.length >= 8) return;
      if (node.kind === "image" && !images.some((image) => image.path === node.path))
        images.push(node);
      else if ("children" in node) visit(node.children);
    }
  };
  visit(parseInline(text));
  return images.length ? (
    <ul className="chat-image-gallery" aria-label="Tool images">
      {images.map((image) => (
        <li key={image.path}>
          <ChatImageTile
            image={{
              name: image.alt || "Agent image",
              source: imageSource(image.path),
            }}
          />
        </li>
      ))}
    </ul>
  ) : null;
}

const STATUS_LABEL: Record<ToolDisplayStatus, string> = {
  pending: "Pending",
  in_progress: "Running",
  completed: "Done",
  failed: "Failed",
  stopped: "Stopped",
};

/** One outline glyph per kind, 14px on a 1.5 stroke, in the row's gutter. */
export const TOOL_ICON = {
  read: FileText,
  edit: FilePenLine,
  delete: FileMinus,
  move: ArrowRightLeft,
  search: Search,
  execute: SquareTerminal,
  think: Brain,
  fetch: Globe,
  switch_mode: SlidersHorizontal,
  other: Wrench,
} as const;

/** How long a disclosure's height glide may take before its row stops following it. */
const OPEN_MS = 300;
const CLOSE_MS = 240;

/** A call's line: the verb in the tense of its state in sans, then its object, in mono when code. */
export function ToolPhraseText({
  title,
  kind,
  display,
  sheen = false,
}: {
  readonly title: string;
  readonly kind: ToolItem["toolKind"];
  readonly display: ToolDisplayStatus;
  /** A sheen passes over it while the call runs. */
  readonly sheen?: boolean;
}) {
  const phrase = toolPhrase(title, kind, display);
  return (
    <span className="chat-tool-label" data-sheen={sheen ? "" : undefined}>
      {phrase.verb ? (
        <>
          <span className="chat-tool-verb">{phrase.verb}</span>{" "}
        </>
      ) : null}
      <span className="chat-tool-object" data-literal={phrase.literal ? "" : undefined}>
        {phrase.object}
      </span>
    </span>
  );
}

/** A call's tooltip: its full title, and the path it acts on when that says more. */
export function toolTooltip(tool: ToolItem): string {
  const title = tool.title || tool.toolName || "Tool";
  const path = tool.locations[0]?.path;
  return path && !title.includes(path) ? `${title}\n${path}` : title;
}

/**
 * A tool call: its line (kind icon, verb and object, status) over its output well. The well opens
 * and closes with a height glide; while it moves, the virtual rows below follow it frame by frame.
 * `bare`: the call is its stretch's only one, whose header already says it, so only the well shows.
 */
export function ToolBlock({
  tool,
  display,
  blobs,
  onLoadBlob,
  bare = false,
}: {
  readonly tool: ToolItem;
  readonly display: ToolDisplayStatus;
  readonly bare?: boolean;
} & BlobProps) {
  const running = display === "pending" || display === "in_progress";
  const Icon = TOOL_ICON[tool.toolKind];
  const reveal = useContext(RevealContext);
  // A call that started a background task opens it beside the chat.
  const task = useContext(TaskLinkContext)?.(tool.id) ?? null;
  // A call shown bare (its stretch's only one) stays open when a second call gives it a head.
  const wasBare = useRef(bare);
  if (bare) wasBare.current = true;
  const [expanded, setExpanded] = useDisclosure(
    tool.key,
    display,
    display === "failed" || wasBare.current,
  );
  useEffect(() => {
    // Opening a single-call group also opens its output. Keep that choice when a second call
    // joins, and when virtualization or collapsing the group unmounts this row.
    if (wasBare.current && expanded) setExpanded(true);
  }, [expanded, setExpanded]);
  const [outputFocused, setOutputFocused] = useState(false);
  const [moving, setMoving] = useState<"open" | "close" | null>(null);
  const open = bare || expanded || outputFocused || moving === "close";
  const title = tool.title || tool.toolName || "Tool";
  const input = useMemo(() => toolInput(tool), [tool]);
  const links = useMemo(() => searchLinks(tool), [tool]);
  // Once its links show as links, a search's text drops their JSON.
  const contents = useMemo(
    () =>
      links.length > 0
        ? tool.content.map((content) =>
            content.type === "text" && content.truncated === null
              ? { ...content, text: withoutSearchLinks(content.text) }
              : content,
          )
        : tool.content,
    [links, tool.content],
  );
  const delta = useMemo(() => diffDelta(tool), [tool]);
  useEffect(() => {
    if (moving === null) return;
    const timer = setTimeout(() => setMoving(null), moving === "open" ? OPEN_MS : CLOSE_MS);
    return () => clearTimeout(timer);
  }, [moving]);
  const toggle = () => {
    const closing = expanded || outputFocused;
    setExpanded(!closing);
    if (prefersReducedMotion()) return;
    setMoving(closing ? "close" : "open");
    reveal?.hold(closing ? CLOSE_MS : OPEN_MS);
  };
  return (
    <div
      className="chat-tool"
      data-kind="tool"
      data-bare={bare ? "" : undefined}
      data-task={task ? "" : undefined}
      data-testid="item"
      data-test-kind="tool"
      data-test-tool-kind={tool.toolKind}
      data-test-status={display}
      onFocusCapture={(event) =>
        setOutputFocused((event.target as Element).closest("summary") === null)
      }
      onBlurCapture={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget)) setOutputFocused(false);
      }}
    >
      <details
        data-testid="tool-output"
        data-reveal={moving ?? undefined}
        open={open}
        onToggle={(event) => {
          // The page itself may open a closed call (find in page); follow it.
          if (!bare && moving === null && event.currentTarget.open !== open)
            setExpanded(event.currentTarget.open);
        }}
      >
        {/* biome-ignore lint/a11y/noStaticElementInteractions: a summary is its details element's own button (Enter and Space click it); the click is taken over only so the well can glide shut before the details close. */}
        <summary
          className="chat-tool-head"
          title={toolTooltip(tool)}
          onClick={(event) => {
            event.preventDefault();
            toggle();
          }}
        >
          <span className="chat-gutter">
            {display === "failed" ? (
              <StatusDot tone="red" />
            ) : display === "stopped" ? (
              <StatusDot tone="neutral" />
            ) : (
              <Icon className="chat-tool-icon" size={14} strokeWidth={1.5} aria-hidden="true" />
            )}
          </span>
          <ToolPhraseText title={title} kind={tool.toolKind} display={display} sheen={running} />
          {delta ? (
            <span
              className="chat-diff-totals chat-tool-delta"
              title={`${delta.added} added, ${delta.removed} removed`}
            >
              <span data-op="add">+{delta.added}</span>{" "}
              <span data-op="remove">−{delta.removed}</span>
            </span>
          ) : null}
          <span className="chat-tool-status" data-status={display}>
            {STATUS_LABEL[display]}
          </span>
          <ChevronRight className="chat-tool-chevron" size={12} aria-hidden="true" />
        </summary>
        <div
          className="chat-reveal"
          onAnimationEnd={(event) => {
            if (event.target === event.currentTarget) setMoving(null);
          }}
        >
          <div className="chat-reveal-clip">
            <div className="chat-tool-well">
              {input.length > 0 ? <ToolInput fields={input} /> : null}
              <Locations locations={tool.locations} />
              {links.length > 0 ? <SearchLinks links={links} /> : null}
              {contents.length > 0 ? (
                <div className="chat-segment">
                  {contents.map((content, index) => (
                    <ToolContentView
                      // biome-ignore lint/suspicious/noArrayIndexKey: an update replaces content as a whole.
                      key={index}
                      content={content}
                      disclosureKey={`${tool.key}:${index}`}
                      running={running}
                      blobs={blobs}
                      onLoadBlob={onLoadBlob}
                    />
                  ))}
                </div>
              ) : (
                <p className="ui-muted">
                  {running ? "Waiting for output…" : "No output reported."}
                </p>
              )}
              {tool.truncated ? (
                <EventDetails truncation={tool.truncated} blobs={blobs} onLoadBlob={onLoadBlob} />
              ) : null}
            </div>
          </div>
        </div>
      </details>
      {task ? (
        <button
          type="button"
          className="g-button g-button-ghost g-button-24 chat-tool-task"
          data-testid="tool-open-task"
          aria-label={`Open task: ${task.title}`}
          title="Follow this task beside the chat"
          onClick={(event) => {
            // WebKit does not focus a clicked button; the panel returns focus to it when it closes.
            event.currentTarget.focus();
            task.open();
          }}
        >
          Open
        </button>
      ) : null}
    </div>
  );
}

/** What the call was given, each input by its name; long values set on their own lines. */
function ToolInput({ fields }: { readonly fields: readonly InputField[] }) {
  return (
    <dl className="chat-tool-input" data-testid="tool-input">
      {fields.map((field) => (
        <div key={field.key} data-block={field.block ? "" : undefined}>
          <dt>{field.key}</dt>
          <dd>
            {field.block ? (
              <pre className="ui-pre chat-tool-input-code">{field.value}</pre>
            ) : (
              field.value
            )}
          </dd>
        </div>
      ))}
    </dl>
  );
}

/** The pages a web search found, each opening in the browser. */
function SearchLinks({ links }: { readonly links: readonly SearchLink[] }) {
  return (
    <ul className="chat-tool-links" data-testid="tool-links">
      {links.map((link) => (
        <li key={link.url}>
          <a href={link.url} target="_blank" rel="noreferrer noopener" title={link.url}>
            {link.title}
          </a>
          <span>{link.host}</span>
        </li>
      ))}
    </ul>
  );
}

export function Locations({ locations }: { readonly locations: readonly ToolLocation[] }) {
  if (locations.length === 0) return null;
  return (
    <ul className="chat-locations" data-testid="tool-locations">
      {locations.map((location, index) => (
        // biome-ignore lint/suspicious/noArrayIndexKey: locations are positional.
        <li key={index}>
          <FileLink reference={location.path} line={location.line}>
            {location.line === null ? location.path : `${location.path}:${location.line}`}
          </FileLink>
        </li>
      ))}
    </ul>
  );
}

const UTF8 = new TextEncoder();
const UTF8_TEXT = new TextDecoder();

/** What a cut terminal output shows once pages of its full text arrive. */
export interface TerminalParts {
  /** The loaded start of the full output; `null` until a page arrives. */
  readonly head: string | null;
  /** Bytes between `head` and `tail` that are not shown. */
  readonly between: number;
  /** The end of the output still to show after `head`. */
  readonly tail: string;
}

/**
 * A cut terminal output keeps its end inline, since the end explains failures. While pages of the
 * full output load from the start, the loaded start shows, then how much lies between, then that
 * end, until the pages reach it. Pages and the inline end are both cut on character boundaries,
 * so a byte offset into the full output is one into its end as well. A full output keeps only its
 * first 64 MiB, so the pages may never reach the end.
 */
export function terminalParts(
  output: string,
  truncation: Truncation | null,
  blob: BlobState | undefined,
): TerminalParts {
  if (truncation === null || blob === undefined || blob.nextOffset === 0) {
    return { head: null, between: 0, tail: output };
  }
  const end = UTF8.encode(output);
  const endStart = Math.max(0, truncation.total_bytes - end.length);
  if (blob.nextOffset < endStart) {
    return { head: blob.text, between: endStart - blob.nextOffset, tail: output };
  }
  const rest = end.subarray(Math.min(end.length, blob.nextOffset - endStart));
  return { head: blob.text, between: 0, tail: UTF8_TEXT.decode(rest) };
}

/** A full output keeps only its first 64 MiB: what an output loaded to its last page lacks. */
function unkeptBytes(truncation: Truncation | null, blob: BlobState | undefined): number {
  if (truncation === null || !blob?.eof) return 0;
  return Math.max(0, truncation.total_bytes - blob.total);
}

/** One entry of a tool's (or a card's) content: text, a diff, or terminal output. */
export function ToolContentView({
  content,
  disclosureKey = "output",
  running = false,
  blobs,
  onLoadBlob,
}: {
  readonly content: ToolContent;
  readonly disclosureKey?: string;
  readonly running?: boolean;
} & BlobProps) {
  const blob = content.truncated?.blob ? blobs.get(content.truncated.blob) : undefined;
  switch (content.type) {
    case "text": {
      const unkept = unkeptBytes(content.truncated, blob);
      // JSON (an MCP server's reply, mostly) is set out over lines.
      const raw = blob?.text || content.text;
      const shown = prettyJson(raw) ?? raw;
      return (
        <div
          className="chat-command"
          data-testid="tool-content"
          data-test-type="text"
          data-test-truncated={String(content.truncated !== null)}
        >
          <pre className="ui-pre" data-testid="tool-text">
            {shown}
          </pre>
          <OutputImages text={raw} />
          <div className="chat-output-actions">
            <CopyButton text={shown} label="Copy output" caption="Copy output" />
          </div>
          {unkept > 0 ? (
            <p className="chat-marker">
              The remaining {formatCount(unkept)} bytes weren't kept: an output keeps its first 64
              MiB.
            </p>
          ) : null}
          <FullOutput truncation={content.truncated} blob={blob} onLoadBlob={onLoadBlob} />
        </div>
      );
    }
    case "terminal": {
      const { head, between, tail } = terminalParts(content.output, content.truncated, blob);
      const visible =
        head === null || between === 0
          ? (head ?? "") + tail
          : `${head}\n[${between} bytes omitted]\n${tail}`;
      return (
        <div
          className="chat-command"
          data-testid="tool-content"
          data-test-type="terminal"
          data-test-truncated={String(content.truncated !== null)}
        >
          <TerminalOutput text={visible} running={running} disclosureKey={disclosureKey}>
            {head === null || between === 0 ? (
              <pre className="ui-pre">{(head ?? "") + tail}</pre>
            ) : (
              <>
                <pre className="ui-pre">{head}</pre>
                <p className="chat-marker">
                  {blob?.eof
                    ? `${formatCount(between)} bytes in between weren't kept (an output keeps its first 64 MiB). The end of the output:`
                    : `${formatCount(between)} bytes not loaded yet. The end of the output:`}
                </p>
                <pre className="ui-pre">{tail}</pre>
              </>
            )}
          </TerminalOutput>
          {content.exit_code !== null ? (
            <span className="ui-muted ui-mono">exit {content.exit_code}</span>
          ) : null}
          <div className="chat-output-actions">
            <CopyButton text={visible} label="Copy output" caption="Copy output" />
          </div>
          <FullOutput truncation={content.truncated} blob={blob} onLoadBlob={onLoadBlob} />
        </div>
      );
    }
    case "diff":
      return (
        <div
          className="chat-command"
          data-testid="tool-content"
          data-test-type="diff"
          data-test-truncated={String(content.truncated !== null)}
        >
          <DiffView
            disclosureKey={disclosureKey}
            path={content.path}
            oldText={content.old_text}
            newText={content.new_text}
            blob={blob}
          />
          <FullOutput truncation={content.truncated} blob={blob} onLoadBlob={onLoadBlob} />
        </div>
      );
  }
}

/** A diff that did not fit inline keeps empty texts; its blob holds `{old_text, new_text}`. */
function fullDiff(blob: BlobState | undefined): { old: string | null; new: string } | null {
  if (!blob?.eof) return null;
  try {
    const parsed: unknown = JSON.parse(blob.text);
    if (
      typeof parsed !== "object" ||
      parsed === null ||
      !("new_text" in parsed) ||
      typeof parsed.new_text !== "string"
    )
      return null;
    const old = "old_text" in parsed ? parsed.old_text : null;
    if (old !== null && typeof old !== "string") return null;
    return { old, new: parsed.new_text };
  } catch {
    return null;
  }
}

function DiffView({
  disclosureKey,
  path,
  oldText,
  newText,
  blob,
}: {
  readonly disclosureKey: string;
  readonly path: string;
  readonly oldText: string | null;
  readonly newText: string;
  readonly blob: BlobState | undefined;
}) {
  const loaded = fullDiff(blob);
  const before = loaded ? loaded.old : oldText;
  const after = loaded ? loaded.new : newText;
  return <DiffBlock path={path} oldText={before} newText={after} disclosureKey={disclosureKey} />;
}

/**
 * "Load full output": an entry cut to fit the event budget pages its complete text in through
 * `chat_blob`, 256 KiB at a time, each page ending on a character boundary.
 */
export function FullOutput({
  truncation,
  blob,
  onLoadBlob,
}: {
  readonly truncation: Truncation | null;
  readonly blob: BlobState | undefined;
  readonly onLoadBlob: (sha256: string) => void;
}) {
  if (truncation === null) return null;
  const sha = truncation.blob;
  if (sha === null) {
    return (
      <span className="ui-muted">
        Cut to fit ({formatCount(truncation.total_bytes)} bytes in all).
      </span>
    );
  }
  if (blob?.eof) return null;
  const label = blob?.loading
    ? "Loading…"
    : blob && blob.nextOffset > 0
      ? `Load more (${formatCount(blob.nextOffset)} of ${formatCount(blob.total)} bytes)`
      : `Load full output (${formatCount(truncation.total_bytes)} bytes)`;
  return (
    <div className="ui-row">
      <button
        type="button"
        className="g-button g-button-ghost g-button-28"
        data-testid="load-full-output"
        disabled={blob?.loading === true}
        onClick={() => onLoadBlob(sha)}
      >
        {label}
      </button>
      {blob?.error ? (
        <span role="alert" className="ui-error">
          {blob.error}
        </span>
      ) : null}
    </div>
  );
}

/** "Show full details": the event's complete original JSON, when more than entry text was cut. */
export function EventDetails({
  truncation,
  blobs,
  onLoadBlob,
}: { readonly truncation: Truncation } & BlobProps) {
  const sha = truncation.blob;
  if (sha === null) return null;
  const blob = blobs.get(sha);
  if (blob?.eof) {
    let pretty = blob.text;
    try {
      pretty = JSON.stringify(JSON.parse(blob.text), null, 2);
    } catch {
      // Not JSON after all: shown as it is.
    }
    return <pre className="ui-pre">{pretty}</pre>;
  }
  return (
    <div className="ui-row">
      <button
        type="button"
        className="g-button g-button-ghost g-button-28"
        data-testid="show-full-details"
        disabled={blob?.loading === true}
        onClick={() => onLoadBlob(sha)}
      >
        {blob?.loading ? "Loading…" : blob ? "Show more details" : "Show full details"}
      </button>
      {blob?.error ? (
        <span role="alert" className="ui-error">
          {blob.error}
        </span>
      ) : null}
    </div>
  );
}
