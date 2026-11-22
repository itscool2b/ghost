import { ArrowUp, Paperclip, PenTool, Square } from "lucide-react";
import { type ReactNode, useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import type { FileMention, SessionStatus, SlashCommand } from "../../bindings";
import { matchingCommands, slashQuery } from "../../lib/chat/present";
import { composerPlaceholder } from "../../lib/chat/queueWait";
import type { ComposerContext, ComposerTransport } from "../../lib/composer";
import { errorMessage } from "../../lib/errors";
import { fitTextarea } from "../../lib/fitTextarea";
import type { CanvasBridge } from "./CanvasPane";
import { ContextChips, MentionMenu, useComposerContext } from "./ComposerContext";
import {
  DictationBar,
  DictationButton,
  DictationGhost,
  revealDictation,
  useDictation,
} from "./Dictation";
import { SlashMenu } from "./SlashMenu";

/**
 * The message box: Enter sends, Shift+Enter starts a new line; Send, and Stop while a turn runs or
 * starts (Force stop while it cancels). A leading `/` opens the agent's slash commands, filtered by
 * what follows it; choosing one inserts `/name `. Text is sent exactly as typed, and stays when
 * sending fails.
 */
export function Composer({
  status,
  commands,
  error,
  onSend,
  onSteer,
  onStop,
  onForceStop,
  controls,
  agentName = "the agent",
  chatId,
  contextApi,
  blocked = false,
  runningTasks = 0,
  canvas,
}: {
  readonly status: SessionStatus;
  readonly commands: readonly SlashCommand[];
  readonly error: string | null;
  /** Answers whether the text was taken. */
  readonly onSend: (text: string, context?: ComposerContext) => Promise<boolean>;
  readonly onSteer?: (text: string, context?: ComposerContext) => Promise<boolean>;
  readonly onStop: () => void;
  readonly onForceStop: () => void;
  readonly controls?: ReactNode;
  readonly agentName?: string;
  readonly chatId?: string;
  readonly contextApi?: ComposerTransport;
  readonly blocked?: boolean;
  /** Background tasks running now: Stop ends them with the turn, and says so. */
  readonly runningTasks?: number;
  /** The chat's canvas: whether it is open, its toggle, and the link it adds to the draft through. */
  readonly canvas?: {
    readonly open: boolean;
    readonly onToggle: () => void;
    readonly bridge: CanvasBridge;
  };
}) {
  const [active, setActive] = useState(0);
  const [menuClosed, setMenuClosed] = useState(false);
  const [sending, setSending] = useState(false);
  const input = useRef<HTMLTextAreaElement>(null);
  const ghostNode = useRef<HTMLDivElement>(null);
  const context = useComposerContext(chatId, contextApi);
  const { text, setText } = context;
  // The canvas adds its drawing and its text through this draft.
  const contextRef = useRef(context);
  contextRef.current = context;
  const canvasBridge = canvas?.bridge;
  // Something the canvas added just arrived: the capsule's rim says so for a moment.
  const [received, setReceived] = useState(0);
  useEffect(() => {
    if (!received) return;
    const settle = window.setTimeout(() => setReceived(0), 1600);
    return () => window.clearTimeout(settle);
  }, [received]);
  useEffect(() => {
    if (!canvasBridge) return;
    const bridge = canvasBridge;
    bridge.composer = {
      addImage: (bytes, replace, name = "Canvas.png") =>
        contextRef.current.pasteImage(bytes, name, replace),
      putText: (value, previous) =>
        contextRef.current.setText((current) => {
          if (previous && current.includes(previous))
            return value ? current.replace(previous, value) : current.replace(previous, "").trim();
          if (!value) return current;
          return current.trim()
            ? `${current.trimEnd()}

${value}`
            : value;
        }),
      full: () => contextRef.current.attachments.length >= 8,
      holds: (image, value) =>
        (image === null ||
          contextRef.current.attachments.some((attachment) => attachment.id === image)) &&
        (value === null || contextRef.current.text.includes(value)),
      announce: () => setReceived((count) => count + 1),
    };
    bridge.changed();
    return () => {
      bridge.composer = null;
      bridge.changed();
    };
  }, [canvasBridge]);
  // The canvas reads whether the draft still holds what it added.
  useEffect(() => {
    void text;
    void context.attachments;
    canvasBridge?.changed();
  }, [canvasBridge, text, context.attachments]);
  const dictation = useDictation({
    api: contextApi?.dictation,
    input,
    text,
    setText,
    disabled: blocked || sending || !context.ready,
  });
  const dictating = dictation?.busy ?? false;
  const [mentionItems, setMentionItems] = useState<FileMention[]>([]);
  const [mentionError, setMentionError] = useState<string | null>(null);
  const mentionQuery =
    !menuClosed && contextApi ? text.match(/(?:^|\s)@([^\s]*)$/)?.[1] : undefined;
  useEffect(() => {
    setMentionItems([]);
    setMentionError(null);
    if (!contextApi || !chatId || mentionQuery === undefined) return;
    let disposed = false;
    const timer = window.setTimeout(() => {
      void contextApi
        .mentions(chatId, mentionQuery)
        .then((items) => {
          if (!disposed) setMentionItems(items);
        })
        .catch((error) => {
          if (!disposed) setMentionError(errorMessage(error));
        });
    }, 120);
    return () => {
      disposed = true;
      window.clearTimeout(timer);
    };
  }, [contextApi, chatId, mentionQuery]);
  // The box grows with the draft, and with dictated words still in its ghost, up to 240px; with
  // the canvas open, up to 120px, so a long draft scrolls instead of shrinking the drawing.
  const fitMax = canvas?.open ? 120 : 240;
  // biome-ignore lint/correctness/useExhaustiveDependencies: The value and the ghost change scrollHeight after React commits them.
  useLayoutEffect(() => {
    const field = input.current;
    if (!field) return;
    const ghost = ghostNode.current;
    fitTextarea(field, 48, fitMax, () => ghost?.scrollHeight ?? 0);
    if (ghost) revealDictation(ghost, field);
  }, [text, dictation?.layout, fitMax]);
  const fieldId = useId();
  const menuId = useId();
  const mentionId = useId();
  const query = slashQuery(text);
  const matches = query === null || menuClosed ? [] : matchingCommands(commands, query);
  const menuOpen = matches.length > 0;
  const mentionOpen = mentionItems.length > 0 && mentionQuery !== undefined;
  const empty = text.trim() === "" && !context.attachments.length && !context.mentions.length;
  const working = status === "running" || status === "starting";
  const steerable = onSteer !== undefined && status === "running";

  const send = async (steer = false): Promise<void> => {
    if (sending || dictating || context.busy || blocked || empty) return;
    if (steer && (!onSteer || status !== "running")) return;
    setSending(true);
    try {
      const sent = {
        text,
        attachments: context.attachments.map((item) => item.id),
        mentions: [...context.mentions],
      };
      const submit = steer && onSteer ? onSteer : onSend;
      const accepted =
        context.attachments.length || context.mentions.length
          ? await submit(text, sent)
          : await submit(text);
      if (accepted) {
        context.consume(sent, steer);
      }
    } finally {
      setSending(false);
    }
  };

  const choose = (command: SlashCommand): void => {
    setText(`/${command.name} `);
    setActive(0);
  };
  const chooseMention = (item: FileMention): void => {
    context.addMention(item.path);
    setText((value) => value.replace(/@[^\s]*$/, ""));
    setMentionItems([]);
    setActive(0);
    input.current?.focus();
  };

  return (
    <div
      className="chat-composer"
      data-tour="composer"
      data-working={working ? "" : undefined}
      data-received={received ? "" : undefined}
    >
      <ContextChips context={context} />
      {mentionOpen ? (
        <MentionMenu
          id={mentionId}
          items={mentionItems}
          active={Math.min(active, mentionItems.length - 1)}
          onChoose={chooseMention}
        />
      ) : null}
      {menuOpen ? (
        <SlashMenu
          id={menuId}
          commands={matches}
          active={Math.min(active, matches.length - 1)}
          onChoose={choose}
        />
      ) : null}
      <label htmlFor={fieldId} className="sr-only">
        Message
      </label>
      <div className="chat-composer-field" data-ghost={dictation?.ghost ? "" : undefined}>
        <textarea
          ref={input}
          id={fieldId}
          className="g-textarea"
          data-testid="composer-input"
          value={text}
          disabled={!context.ready}
          readOnly={dictating}
          onBlur={() => void context.flush()}
          placeholder={composerPlaceholder(status, agentName)}
          title="Enter sends, Shift+Enter adds a line, / lists commands, @ mentions a file"
          aria-controls={mentionOpen ? mentionId : menuOpen ? menuId : undefined}
          onPaste={(event) => {
            if (!contextApi) return;
            const image = Array.from(event.clipboardData.files).find((file) =>
              file.type.startsWith("image/"),
            );
            if (image) {
              event.preventDefault();
              void context.paste(image);
            }
          }}
          onChange={(event) => {
            dictation?.interrupt();
            setText(event.currentTarget.value);
            setMenuClosed(false);
            setActive(0);
          }}
          onKeyDown={(event) => {
            if (event.nativeEvent.isComposing) return;
            // While dictating, the draft is the dictation's: Enter finishes it, as on its button.
            if (dictating) {
              if (event.key === "Enter" && !event.shiftKey) {
                event.preventDefault();
                dictation?.finish();
              }
              return;
            }
            if (mentionOpen) {
              if (event.key === "ArrowDown" || event.key === "ArrowUp") {
                event.preventDefault();
                const step = event.key === "ArrowDown" ? 1 : -1;
                setActive((value) => (value + step + mentionItems.length) % mentionItems.length);
                return;
              }
              if (event.key === "Enter" || event.key === "Tab") {
                const item = mentionItems[Math.min(active, mentionItems.length - 1)];
                if (item) {
                  event.preventDefault();
                  chooseMention(item);
                  return;
                }
              }
              if (event.key === "Escape") {
                event.preventDefault();
                setMenuClosed(true);
                return;
              }
            }
            if (menuOpen) {
              if (event.key === "ArrowDown" || event.key === "ArrowUp") {
                event.preventDefault();
                const step = event.key === "ArrowDown" ? 1 : -1;
                setActive((current) => (current + step + matches.length) % matches.length);
                return;
              }
              if (event.key === "Enter" || event.key === "Tab") {
                const command = matches[Math.min(active, matches.length - 1)];
                if (command) {
                  event.preventDefault();
                  choose(command);
                  return;
                }
              }
              if (event.key === "Escape") {
                // Closes the menu; the Esc that stops a turn is the next one.
                event.preventDefault();
                setMenuClosed(true);
                return;
              }
            }
            if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
              event.preventDefault();
              void send();
            }
          }}
        />
        {dictation?.ghost ? (
          <DictationGhost
            key={dictation.ghost.key}
            ghost={dictation.ghost}
            node={ghostNode}
            onSettled={dictation.settled}
          />
        ) : null}
      </div>
      <div className="chat-composer-controls" hidden={dictation?.open}>
        {contextApi ? (
          <button
            type="button"
            className="chat-composer-attach"
            aria-label="Attach a file"
            title="Attach a file, drop one here, or paste an image. Type @ to mention a project file."
            disabled={context.busy || sending}
            onClick={() => void context.pick()}
          >
            <Paperclip size={16} />
          </button>
        ) : null}
        {canvas ? (
          <button
            type="button"
            className="chat-composer-attach chat-composer-canvas"
            aria-label="Canvas"
            aria-pressed={canvas.open}
            title={canvas.open ? "Close the canvas" : "Draw on a canvas and add it to this message"}
            onClick={canvas.onToggle}
          >
            <PenTool size={16} />
          </button>
        ) : null}
        {controls}
        <div className="chat-composer-actions" data-running={working ? "" : undefined}>
          {dictation ? (
            <DictationButton
              dictation={dictation}
              disabled={blocked || sending || !context.ready}
            />
          ) : null}
          <StopButton
            status={status}
            onStop={onStop}
            onForceStop={onForceStop}
            quiet={!empty && status !== "cancelling"}
            runningTasks={runningTasks}
          />
          {working && empty ? null : (
            <SendControls
              status={status}
              steerable={steerable}
              disabled={sending || dictating || context.busy || blocked || empty}
              agentName={agentName}
              onSend={() => void send()}
              onSteer={() => void send(true)}
            />
          )}
        </div>
      </div>
      {dictation?.open ? <DictationBar dictation={dictation} /> : null}
      {error !== null || context.error !== null || mentionError !== null ? (
        <span role="alert" className="ui-error" data-testid="composer-error">
          {context.error ?? mentionError ?? error}
        </span>
      ) : null}
      {context.saveError ? (
        <button
          type="button"
          className="g-button g-button-28 chat-composer-retry"
          onClick={() => void context.retrySave()}
        >
          Retry draft save
        </button>
      ) : null}
    </div>
  );
}

/**
 * The send control. Idle: the send disc. While a turn runs, a typed message goes in one pill: an
 * agent that steers (Codex) offers "Steer" beside the Queue disc; one that takes messages only
 * between turns (Claude Code, Pi) says "After this turn" there instead, calm text and no dead
 * control, and the Queue disc is what Enter does. While the agent starts, the same calm words say
 * the message goes once it has started, not that it is queued.
 */
export function SendControls({
  status,
  steerable,
  disabled,
  agentName,
  onSend,
  onSteer,
}: {
  readonly status: SessionStatus;
  readonly steerable: boolean;
  readonly disabled: boolean;
  readonly agentName: string;
  readonly onSend: () => void;
  readonly onSteer: () => void;
}) {
  const after = useId();
  const working = status === "running" || status === "starting";
  const afterTurn = status === "running" && !steerable;
  const afterStart = status === "starting";
  return (
    <span
      className="chat-send-group"
      data-split={steerable || afterTurn || afterStart ? "" : undefined}
    >
      {afterStart ? (
        <span
          id={after}
          className="chat-after-turn"
          data-testid="composer-after-start"
          title={`${agentName} is starting. Your message goes once it is ready.`}
        >
          Once started
        </span>
      ) : steerable ? (
        <button
          type="button"
          className="chat-steer"
          data-testid="composer-steer"
          disabled={disabled}
          onClick={onSteer}
          title="Send this message into the running turn"
        >
          Steer
        </button>
      ) : afterTurn ? (
        <span
          id={after}
          className="chat-after-turn"
          data-testid="composer-after-turn"
          title={`${agentName} takes messages after the current turn, not during it.`}
        >
          After this turn
        </span>
      ) : null}
      <button
        type="button"
        className="g-button g-button-primary g-button-icon chat-send"
        data-testid="composer-send"
        disabled={disabled}
        onClick={onSend}
        aria-label={afterStart ? "Send once started" : working ? "Queue message" : "Send message"}
        aria-describedby={afterTurn || afterStart ? after : undefined}
        title={
          afterStart
            ? `Send once ${agentName} has started (Enter)`
            : afterTurn
              ? `Queue for ${agentName} after this turn (Enter)`
              : working
                ? "Queue message (Enter)"
                : "Send message (Enter)"
        }
      >
        <ArrowUp size={16} strokeWidth={2.25} />
      </button>
    </span>
  );
}

/**
 * Stop while a turn runs or the agent starts; Force stop while the turn cancels. Stop is the white
 * circle while the message box is empty, and steps back to a soft one once there is a message to
 * queue, so the white circle is always the next thing Enter does.
 */
export function StopButton({
  status,
  onStop,
  onForceStop,
  quiet = false,
  runningTasks = 0,
}: {
  readonly status: SessionStatus;
  readonly onStop: () => void;
  readonly onForceStop: () => void;
  readonly quiet?: boolean;
  /** Claude Code ends its background tasks when its turn is stopped: the button says so. */
  readonly runningTasks?: number;
}) {
  const ends =
    runningTasks > 0
      ? `Also ends ${runningTasks} background ${runningTasks === 1 ? "task" : "tasks"}.`
      : undefined;
  if (status === "cancelling") {
    return (
      <button
        type="button"
        className="g-button g-button-danger chat-force-stop"
        data-testid="composer-force-stop"
        onClick={onForceStop}
        title="Force stop (Esc)"
      >
        Force stop
      </button>
    );
  }
  if (status === "running" || status === "starting") {
    return (
      <button
        type="button"
        className={`g-button g-button-icon chat-send ${quiet ? "" : "g-button-primary"}`}
        data-testid="composer-stop"
        data-quiet={quiet ? "" : undefined}
        onClick={onStop}
        aria-label="Stop"
        aria-description={ends}
        title={ends ? `Stop (Esc). ${ends}` : "Stop (Esc)"}
      >
        <Square size={10} fill="currentColor" strokeWidth={0} />
      </button>
    );
  }
  return null;
}
