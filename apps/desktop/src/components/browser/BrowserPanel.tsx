import "./browser.css";
import { AgentMark } from "@ghost/ui/graphite";
import {
  AppWindow,
  ArrowLeft,
  ArrowRight,
  Code,
  Copy,
  Download,
  Ellipsis,
  ExternalLink,
  Globe,
  History as HistoryIcon,
  Maximize2,
  Minimize2,
  Plus,
  RotateCw,
  SquareDashedMousePointer,
  Trash2,
  TriangleAlert,
  X,
  ZoomIn,
  ZoomOut,
} from "lucide-react";
import {
  type CSSProperties,
  type ReactNode,
  useCallback,
  useEffect,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import type { BrowserEvent, BrowserRect, BrowserTab } from "../../bindings";
import { type Annotation, annotationImage, annotationText, marks } from "../../lib/annotation";
import {
  type BrowserStore,
  clampShare,
  type PanelPlacement,
  sameRect,
  shownAddress,
  tabName,
} from "../../lib/browser";
import { covered } from "../../lib/browserOcclusion";
import { errorMessage } from "../../lib/errors";
import { closePopover, Popover } from "../chat/Popover";
import { AddressBar, type AddressBarHandle } from "./AddressBar";
import { AnnotationComposer } from "./AnnotationComposer";
import { ClearDataDialog } from "./ClearDataDialog";
import { HistoryView } from "./HistoryView";
import { StartPage } from "./StartPage";

/** What the panel needs from the chat to send an annotation. */
export interface AnnotationTarget {
  /** The chat's agent, model and thinking picker (its compact form). */
  readonly controls?: ReactNode;
  /** Whether the agent sees images (`null` until its session says). */
  readonly images: boolean | null;
  /** Sends the note to the chat now (or queues it): answers why not, or null. */
  readonly send: (text: string, image: Uint8Array | null) => Promise<string | null>;
  /** Puts the note in the message box instead: answers why not, or null. */
  readonly add: (text: string, image: Uint8Array | null) => Promise<string | null>;
}

const ZOOMS = [0.5, 0.67, 0.75, 0.8, 0.9, 1, 1.1, 1.25, 1.5, 1.75, 2, 2.5, 3];

/** The last media notice the browser showed: each one is said once on this device. */
const MEDIA_NOTICE_KEY = "auracode.browser.media-notice";

function mediaNoticeSeen(notice: string): boolean {
  try {
    return localStorage.getItem(MEDIA_NOTICE_KEY) === notice;
  } catch {
    return false;
  }
}

function rememberMediaNotice(notice: string): void {
  try {
    localStorage.setItem(MEDIA_NOTICE_KEY, notice);
  } catch {
    // A blocked storage says it again next time.
  }
}

/** A colour from the theme as `rgb()`, for the page's highlight and the annotation's outline. */
function themeColor(token: string, fallback: string): string {
  if (typeof document === "undefined") return fallback;
  const probe = document.createElement("span");
  probe.style.color = `var(${token})`;
  probe.style.display = "none";
  document.body.append(probe);
  const value = getComputedStyle(probe).color;
  probe.remove();
  return /^rgba?\(/.test(value) ? value : fallback;
}

interface Notice {
  readonly key: number;
  readonly text: string;
  readonly url?: string;
  /** `outside`: another program's link to open there; `popup`: a window to open as a tab. */
  readonly action?: "outside" | "popup";
}

/**
 * The browser beside the chat (docs/superpowers/plans/2026-11-09-browser.md): the chat's tabs, a
 * toolbar with the address bar, Annotate and the menu, and the page itself, a native view Rust
 * places over the page area. Whatever opens over that area (a menu, a dialog, the address bar's
 * suggestions, an annotation) shows over the page's picture until it closes.
 */
export function BrowserPanel({
  chatId,
  store,
  placement,
  narrow,
  onPlacement,
  onClose,
  runUrl,
  agentId,
  agentName,
  annotation: target,
}: {
  readonly chatId: string;
  readonly store: BrowserStore;
  readonly placement: PanelPlacement;
  /** The window is too narrow for two panes: the browser covers the chat. */
  readonly narrow: boolean;
  readonly onPlacement: (change: Partial<PanelPlacement>) => void;
  readonly onClose: () => void;
  /** The chat's Run address, for the start page. */
  readonly runUrl: string | null;
  readonly agentId: string;
  readonly agentName: string;
  readonly annotation?: AnnotationTarget;
}) {
  useSyncExternalStore(store.subscribe, store.getVersion, store.getVersion);
  const chat = store.chat(chatId);
  const tab = store.active(chatId);
  const status = store.status();
  const agent = store.agent(chatId);
  const page = useRef<HTMLDivElement>(null);
  const address = useRef<AddressBarHandle>(null);
  const panel = useRef<HTMLElement>(null);
  const [view, setView] = useState<"page" | "history">("page");
  const [clearing, setClearing] = useState(false);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  // The page's picture, standing in while something opens over it or a note is written.
  const [picture, setPicture] = useState<string | null>(null);
  const [frozen, setFrozen] = useState(false);
  const [note, setNote] = useState<Annotation | null>(null);
  const [noteError, setNoteError] = useState<string | null>(null);
  const [sending, setSending] = useState(false);
  const noteRef = useRef(note);
  noteRef.current = note;
  const tabId = tab?.id ?? null;
  const live = Boolean(tab?.url) && !tab?.crashed;
  const annotating = tab?.annotating === true;

  const fail = useCallback((error: unknown) => setFailure(errorMessage(error)), []);
  useEffect(() => {
    if (!failure) return;
    const clear = window.setTimeout(() => setFailure(null), 8000);
    return () => window.clearTimeout(clear);
  }, [failure]);

  // Pages play no sound or video on this system (Linux without GStreamer's plugins): the panel
  // says so once, until it is dismissed or closes, and not again on this device.
  const mediaNotice = status?.media_notice ?? null;
  const [mediaShown, setMediaShown] = useState<string | null>(null);
  useEffect(() => {
    if (!mediaNotice || mediaNoticeSeen(mediaNotice)) return;
    rememberMediaNotice(mediaNotice);
    setMediaShown(mediaNotice);
  }, [mediaNotice]);

  // A chat whose browser has no tab yet opens one: its start page.
  const opening = useRef(false);
  useEffect(() => {
    if (!chat || chat.tabs.length > 0 || opening.current || !store.usable()) return;
    opening.current = true;
    store.api.open(chatId, null, true).then(
      () => {
        opening.current = false;
      },
      (error) => {
        opening.current = false;
        fail(error);
      },
    );
  }, [chat, chatId, store, fail]);

  // Another tab, or another page: the history view, a note and the page's picture go.
  // biome-ignore lint/correctness/useExhaustiveDependencies: the tab's identity is the trigger.
  useEffect(() => {
    setNote(null);
    setNoteError(null);
    setPicture(null);
    setFrozen(false);
  }, [tabId]);

  // ------------------------------------------------------------------ placement

  // What the page should do: Rust is told only when it changes.
  const shown = useRef<{ tab: string | null; rect: BrowserRect | null; visible: boolean }>({
    tab: null,
    rect: null,
    visible: false,
  });
  // Something open over the page: capture its picture, then hide it; show it again once clear.
  const cover = useRef<"clear" | "capturing" | "covered">("clear");
  const holdRef = useRef(false);
  // A note's picture once the note is gone: the page shows first, its picture goes a moment
  // later, so nothing flashes (unless something covers the page by then).
  const releasePicture = useCallback(() => {
    window.setTimeout(() => {
      if (!holdRef.current && cover.current === "clear") setPicture(null);
    }, 80);
  }, []);
  holdRef.current = frozen || note !== null;
  useEffect(() => {
    if (!tabId) return;
    let frame = 0;
    let stopped = false;
    const tick = () => {
      if (stopped) return;
      const node = page.current;
      let rect: BrowserRect | null = null;
      if (node && live && view === "page") {
        const box = node.getBoundingClientRect();
        const visible =
          box.width > 1 &&
          box.height > 1 &&
          document.visibilityState === "visible" &&
          (typeof node.checkVisibility !== "function" || node.checkVisibility());
        if (visible) rect = { x: box.left, y: box.top, width: box.width, height: box.height };
      }
      const over =
        rect !== null &&
        covered(
          {
            left: rect.x ?? 0,
            top: rect.y ?? 0,
            right: (rect.x ?? 0) + (rect.width ?? 0),
            bottom: (rect.y ?? 0) + (rect.height ?? 0),
          },
          document,
          page.current,
        );
      if (over && cover.current === "clear" && !holdRef.current) {
        cover.current = "capturing";
        const settle = (image: string | null) => {
          if (cover.current !== "capturing") return;
          cover.current = "covered";
          setPicture(image);
        };
        // Never wait long: past this the page hides without its picture.
        const fallback = window.setTimeout(() => settle(null), 220);
        store.api.capture(tabId).then(
          (image) => {
            window.clearTimeout(fallback);
            settle(image);
          },
          () => {
            window.clearTimeout(fallback);
            settle(null);
          },
        );
      } else if (!over && cover.current !== "clear" && !holdRef.current) {
        cover.current = "clear";
        // The page shows first; its picture goes a frame later, so nothing flashes.
        window.setTimeout(() => {
          if (cover.current === "clear" && !holdRef.current) setPicture(null);
        }, 60);
      }
      const visible = rect !== null && cover.current !== "covered" && !holdRef.current;
      const last = shown.current;
      if (last.tab !== tabId || !sameRect(last.rect, rect) || last.visible !== visible) {
        shown.current = { tab: tabId, rect, visible };
        store.api.place(tabId, rect, visible).catch(() => {
          // The next frame tries again.
          shown.current = { tab: null, rect: null, visible: false };
        });
      }
      frame = requestAnimationFrame(tick);
    };
    frame = requestAnimationFrame(tick);
    return () => {
      stopped = true;
      cancelAnimationFrame(frame);
      shown.current = { tab: null, rect: null, visible: false };
      cover.current = "clear";
      void store.api.place(tabId, null, false).catch(() => {});
    };
  }, [tabId, live, view, store]);

  // ------------------------------------------------------------------ actions

  const openInput = useCallback(
    (input: string) => {
      setView("page");
      if (!tab) {
        store.api.open(chatId, input, true).catch(fail);
        return;
      }
      store.api.navigate(tab.id, input).catch(fail);
    },
    [tab, chatId, store, fail],
  );

  const newTab = useCallback(() => {
    setView("page");
    store.api.open(chatId, null, true).then(() => {
      requestAnimationFrame(() => address.current?.focus());
    }, fail);
  }, [chatId, store, fail]);

  const closeTab = useCallback(
    (id: string) => {
      store.api.close(id).catch(fail);
    },
    [store, fail],
  );

  const cycle = useCallback(
    (step: number) => {
      if (!chat || chat.tabs.length < 2 || !tab) return;
      const index = chat.tabs.findIndex((entry) => entry.id === tab.id);
      const next = chat.tabs[(index + step + chat.tabs.length) % chat.tabs.length];
      if (next) store.api.activate(chatId, next.id).catch(fail);
    },
    [chat, tab, chatId, store, fail],
  );

  const toggleAnnotate = useCallback(() => {
    if (!tab || !live) return;
    setNote(null);
    setNoteError(null);
    setFrozen(false);
    store.api
      .annotate(
        tab.id,
        !annotating,
        themeColor("--g-volt", "rgb(59, 130, 246)"),
        themeColor("--g-bg", "rgb(255, 255, 255)"),
      )
      .catch(fail);
  }, [tab, live, annotating, store, fail]);

  // Leaving the panel (or the chat) ends Annotate.
  useEffect(
    () => () => {
      if (tabId && store.active(chatId)?.annotating)
        void store.api.annotate(tabId, false, "", "").catch(() => {});
    },
    [tabId, chatId, store],
  );

  // ------------------------------------------------------------------ events from the pages

  useEffect(
    () =>
      store.onEvent((event: BrowserEvent) => {
        if ("chat_id" in event && event.chat_id !== chatId) return;
        switch (event.type) {
          case "pick": {
            if (event.tab_id !== store.active(chatId)?.id) return;
            const current = noteRef.current;
            if (current && event.pick.add) {
              setNote({
                ...current,
                elements: [...current.elements, ...(event.pick.elements ?? [])],
              });
              return;
            }
            setNoteError(null);
            setPicture(event.pick.screenshot ?? null);
            setFrozen(true);
            setNote({ pick: event.pick, elements: event.pick.elements ?? [] });
            return;
          }
          case "annotate_ended":
            setNote(null);
            setFrozen(false);
            releasePicture();
            return;
          case "shortcut":
            switch (event.action) {
              case "address":
                address.current?.focus();
                return;
              case "new-tab":
                newTab();
                return;
              case "close-tab":
                closeTab(event.tab_id);
                return;
              case "toggle":
                onClose();
                return;
              case "annotate":
                toggleAnnotate();
                return;
              case "next-tab":
                cycle(1);
                return;
              case "previous-tab":
                cycle(-1);
                return;
              default:
                return;
            }
          case "outside":
            setNotice({
              key: Date.now(),
              text: `This page wants to open ${event.url.split(":")[0]}: ${event.url}`,
              url: event.url,
              action: "outside",
            });
            return;
          case "blocked":
            setNotice({
              key: Date.now(),
              text: `This page tried to open a window: ${shownAddress(event.url)}`,
              url: event.url,
              action: "popup",
            });
            return;
          case "download":
            setNotice({
              key: Date.now(),
              text:
                event.state === "started"
                  ? `Downloading ${event.file ?? "a file"} to your Downloads folder.`
                  : event.state === "done"
                    ? `Downloaded ${event.file?.split(/[\\/]/).pop() ?? "a file"} to your Downloads folder.`
                    : "A download failed.",
            });
            return;
          default:
            return;
        }
      }),
    [store, chatId, newTab, closeTab, onClose, toggleAnnotate, cycle, releasePicture],
  );

  useEffect(() => {
    // A choice waits for the person; news goes after a moment.
    if (!notice || notice.action) return;
    const clear = window.setTimeout(() => setNotice(null), 6000);
    return () => window.clearTimeout(clear);
  }, [notice]);

  // The browser's shortcuts while the panel is open and the window has the keyboard.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented || document.querySelector("dialog[open]")) return;
      const modifier = event.ctrlKey || event.metaKey;
      const key = event.key.toLowerCase();
      const inside = panel.current?.contains(document.activeElement) ?? false;
      if (modifier && !event.altKey && !event.shiftKey && key === "l") {
        event.preventDefault();
        address.current?.focus();
      } else if (modifier && !event.altKey && !event.shiftKey && key === "t" && inside) {
        event.preventDefault();
        newTab();
      } else if (modifier && !event.altKey && !event.shiftKey && key === "w" && inside && tab) {
        event.preventDefault();
        closeTab(tab.id);
      } else if (modifier && event.shiftKey && key === "d" && inside) {
        event.preventDefault();
        toggleAnnotate();
      } else if (inside && tab && ((modifier && key === "r") || event.key === "F5")) {
        event.preventDefault();
        store.api.go(tab.id, "reload").catch(fail);
      } else if (inside && tab && event.altKey && event.key === "ArrowLeft") {
        event.preventDefault();
        store.api.go(tab.id, "back").catch(fail);
      } else if (inside && tab && event.altKey && event.key === "ArrowRight") {
        event.preventDefault();
        store.api.go(tab.id, "forward").catch(fail);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [tab, newTab, closeTab, toggleAnnotate, store, fail]);

  // ------------------------------------------------------------------ the note

  const closeNote = useCallback(() => {
    setNote(null);
    setNoteError(null);
    setFrozen(false);
    releasePicture();
  }, [releasePicture]);

  const deliver = async (text: string, how: "send" | "add") => {
    const current = noteRef.current;
    if (!current || !target) return;
    setSending(true);
    setNoteError(null);
    try {
      const image =
        target.images === false
          ? null
          : await annotationImage(current, themeColor("--g-volt", "rgb(59, 130, 246)"));
      const message = annotationText(text, current, image !== null);
      const why = await (how === "send" ? target.send : target.add)(message, image);
      if (why) setNoteError(why);
      else closeNote();
    } catch (error) {
      setNoteError(errorMessage(error));
    } finally {
      setSending(false);
    }
  };

  // Shift-click on the page's picture adds the element under the pointer to the note.
  const pickOnPicture = (event: React.MouseEvent<HTMLDivElement>) => {
    if (!note || !tab || !event.shiftKey) return;
    const box = event.currentTarget.getBoundingClientRect();
    const scale = note.pick.viewport.width ? box.width / note.pick.viewport.width : 1;
    const x = (event.clientX - box.left) / scale;
    const y = (event.clientY - box.top) / scale;
    store.api.pickAt(tab.id, x, y).then((element) => {
      const current = noteRef.current;
      if (element && current) setNote({ ...current, elements: [...current.elements, element] });
    }, fail);
  };

  // ------------------------------------------------------------------ render

  const tabs = chat?.tabs ?? [];
  const zoom = tab?.zoom ?? 1;
  const setZoom = (next: number) => {
    if (tab) store.api.zoom(tab.id, next).catch(fail);
  };
  const pageSize = page.current
    ? { width: page.current.clientWidth, height: page.current.clientHeight }
    : { width: 800, height: 600 };
  // The note's picture is the page's view in the page's own pixels; it and its marks scale
  // together to the panel's width (the page may be zoomed, or the panel resized since).
  const pickView = note?.pick.viewport;
  const fit = pickView?.width ? pageSize.width / pickView.width : 1;
  const anchor = note
    ? (() => {
        const boxes = marks(note);
        if (!boxes.length) return { x: 0, y: 0, width: 0, height: 0 };
        const left = Math.min(...boxes.map((box) => box.x ?? 0));
        const top = Math.min(...boxes.map((box) => box.y ?? 0));
        const right = Math.max(...boxes.map((box) => (box.x ?? 0) + (box.width ?? 0)));
        const bottom = Math.max(...boxes.map((box) => (box.y ?? 0) + (box.height ?? 0)));
        return {
          x: left * fit,
          y: top * fit,
          width: (right - left) * fit,
          height: (bottom - top) * fit,
        };
      })()
    : null;
  const full = placement.full || narrow;
  return (
    <aside
      ref={panel}
      className="browser-panel"
      aria-label="Browser"
      data-testid="browser-panel"
      data-full={full ? "" : undefined}
      data-narrow={narrow ? "" : undefined}
      style={{ "--browser-share": String(placement.share) } as CSSProperties}
    >
      {!full ? (
        <hr
          className="browser-resizer"
          tabIndex={0}
          aria-label="Resize the browser"
          aria-orientation="vertical"
          aria-valuemin={28}
          aria-valuemax={78}
          aria-valuenow={Math.round(placement.share * 100)}
          onKeyDown={(event) => {
            if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
              event.preventDefault();
              onPlacement({
                share: clampShare(placement.share + (event.key === "ArrowLeft" ? 0.04 : -0.04)),
              });
            }
          }}
          onPointerDown={(event) => event.currentTarget.setPointerCapture(event.pointerId)}
          onPointerMove={(event) => {
            if (!event.currentTarget.hasPointerCapture(event.pointerId)) return;
            const area = panel.current?.parentElement?.getBoundingClientRect();
            const edge = panel.current?.getBoundingClientRect().right;
            if (area && edge && area.width > 0)
              onPlacement({ share: clampShare((edge - event.clientX) / area.width) });
          }}
          onPointerUp={(event) => event.currentTarget.releasePointerCapture(event.pointerId)}
        />
      ) : null}
      <div className="browser-tabs-row">
        <div className="browser-tabs" role="tablist" aria-label="Tabs">
          {tabs.map((entry) => (
            <TabButton
              key={entry.id}
              tab={entry}
              active={entry.id === tab?.id}
              onActivate={() => {
                setView("page");
                store.api.activate(chatId, entry.id).catch(fail);
              }}
              onClose={() => closeTab(entry.id)}
              onMove={(step) => cycle(step)}
            />
          ))}
        </div>
        <button
          type="button"
          className="g-button g-button-ghost g-button-icon g-button-28"
          aria-label="New tab"
          title="New tab (Ctrl+T)"
          onClick={newTab}
        >
          <Plus size={16} />
        </button>
        <span className="browser-tabs-end">
          {!narrow ? (
            <button
              type="button"
              className="g-button g-button-ghost g-button-icon g-button-28"
              aria-label={placement.full ? "Show the chat beside the browser" : "Full view"}
              title={placement.full ? "Show the chat beside the browser" : "Full view"}
              aria-pressed={placement.full}
              onClick={() => onPlacement({ full: !placement.full })}
            >
              {placement.full ? <Minimize2 size={15} /> : <Maximize2 size={15} />}
            </button>
          ) : null}
          <button
            type="button"
            className="g-button g-button-ghost g-button-icon g-button-28"
            aria-label="Close the browser"
            title="Close the browser (Ctrl+Shift+B)"
            onClick={onClose}
          >
            <X size={16} />
          </button>
        </span>
      </div>
      <div className="browser-toolbar" role="toolbar" aria-label="Page">
        <button
          type="button"
          className="g-button g-button-ghost g-button-icon g-button-28"
          aria-label="Back"
          title="Back (Alt+Left)"
          disabled={!tab?.can_go_back}
          onClick={() => tab && store.api.go(tab.id, "back").catch(fail)}
        >
          <ArrowLeft size={16} />
        </button>
        <button
          type="button"
          className="g-button g-button-ghost g-button-icon g-button-28"
          aria-label="Forward"
          title="Forward (Alt+Right)"
          disabled={!tab?.can_go_forward}
          onClick={() => tab && store.api.go(tab.id, "forward").catch(fail)}
        >
          <ArrowRight size={16} />
        </button>
        {tab?.loading ? (
          <button
            type="button"
            className="g-button g-button-ghost g-button-icon g-button-28"
            aria-label="Stop loading"
            title="Stop loading"
            onClick={() => store.api.go(tab.id, "stop").catch(fail)}
          >
            <X size={16} />
          </button>
        ) : (
          <button
            type="button"
            className="g-button g-button-ghost g-button-icon g-button-28"
            aria-label="Reload"
            title="Reload (Ctrl+R)"
            disabled={!tab?.url}
            onClick={() => tab && store.api.go(tab.id, "reload").catch(fail)}
          >
            <RotateCw size={15} />
          </button>
        )}
        <AddressBar
          ref={address}
          url={tab?.url ?? null}
          loading={tab?.loading ?? false}
          store={store}
          onOpen={openInput}
          onLeave={() => {
            if (tab?.url) void store.api.focus(tab.id).catch(() => {});
          }}
          autoFocus={tab !== null && tab.url === null}
        />
        {target ? (
          <button
            type="button"
            className="g-button g-button-ghost g-button-28 browser-annotate"
            aria-pressed={annotating}
            data-testid="browser-annotate"
            title={
              live
                ? annotating
                  ? "Stop annotating (Esc in the page)"
                  : "Annotate: pick part of the page and tell the agent what to change (Ctrl+Shift+D)"
                : "Open a page to annotate it"
            }
            disabled={!live}
            onClick={toggleAnnotate}
          >
            <SquareDashedMousePointer size={15} />
            <span className="browser-annotate-label">Annotate</span>
          </button>
        ) : null}
        <Popover
          label="Browser menu"
          tip="More"
          className="browser-menu"
          trigger={<Ellipsis size={16} />}
        >
          <div className="browser-menu-actions">
            <button
              type="button"
              className="g-menu-item"
              disabled={!tab?.url}
              onClick={(event) => {
                closePopover(event.currentTarget);
                if (tab?.url) store.api.openOutside(tab.url).catch(fail);
              }}
            >
              <ExternalLink size={14} aria-hidden="true" />
              Open in your browser
            </button>
            <button
              type="button"
              className="g-menu-item"
              disabled={!tab?.url}
              onClick={(event) => {
                closePopover(event.currentTarget);
                if (tab?.url) void navigator.clipboard?.writeText(tab.url).catch(() => {});
              }}
            >
              <Copy size={14} aria-hidden="true" />
              Copy address
            </button>
            <button
              type="button"
              className="g-menu-item"
              disabled={!live}
              onClick={(event) => {
                closePopover(event.currentTarget);
                if (tab) store.api.devtools(tab.id).catch(fail);
              }}
            >
              <Code size={14} aria-hidden="true" />
              Developer tools
            </button>
          </div>
          <fieldset className="browser-zoom" aria-label="Zoom" disabled={!live}>
            <button
              type="button"
              className="g-button g-button-ghost g-button-icon g-button-28"
              aria-label="Zoom out"
              onClick={() => setZoom(ZOOMS.filter((value) => value < zoom - 0.001).pop() ?? zoom)}
            >
              <ZoomOut size={15} />
            </button>
            <button
              type="button"
              className="g-button g-button-ghost g-button-28 browser-zoom-reset"
              title="Actual size"
              onClick={() => setZoom(1)}
            >
              {Math.round(zoom * 100)}%
            </button>
            <button
              type="button"
              className="g-button g-button-ghost g-button-icon g-button-28"
              aria-label="Zoom in"
              onClick={() => setZoom(ZOOMS.find((value) => value > zoom + 0.001) ?? zoom)}
            >
              <ZoomIn size={15} />
            </button>
          </fieldset>
          <div className="browser-menu-actions">
            <button
              type="button"
              className="g-menu-item"
              onClick={(event) => {
                closePopover(event.currentTarget);
                setView((current) => (current === "history" ? "page" : "history"));
              }}
            >
              <HistoryIcon size={14} aria-hidden="true" />
              {view === "history" ? "Back to the page" : "History"}
            </button>
            <button
              type="button"
              className="g-menu-item"
              onClick={(event) => {
                closePopover(event.currentTarget);
                setClearing(true);
              }}
            >
              <Trash2 size={14} aria-hidden="true" />
              Clear browsing data…
            </button>
          </div>
        </Popover>
      </div>
      {tab?.loading ? <div className="browser-progress" aria-hidden="true" /> : null}
      {agent ? (
        <output className="browser-agent" data-testid="browser-agent">
          <span className="browser-agent-mark" aria-hidden="true">
            <AgentMark agent={agentId} state="tool" size={14} />
          </span>
          <span className="browser-agent-text">
            {agentName} · {agent.label}
          </span>
        </output>
      ) : null}
      {mediaShown ? (
        <p className="browser-hint" role="status" data-testid="browser-media-notice">
          <span>{mediaShown}</span>
          <button
            type="button"
            className="g-button g-button-ghost g-button-icon g-button-24"
            aria-label="Dismiss"
            onClick={() => setMediaShown(null)}
          >
            <X size={14} />
          </button>
        </p>
      ) : null}
      {/* The hint stays while a note is open, so the page never moves under it. */}
      {annotating || note ? (
        <p
          className="browser-hint"
          data-testid={note ? "browser-note-hint" : "browser-annotate-hint"}
        >
          {note
            ? "Shift-click the page to add another element. Esc closes the note."
            : "Click part of the page, or drag over an area. Arrow Up and Down pick the parent or child; Esc stops."}
        </p>
      ) : null}
      {tab?.error && !tab.loading && view === "page" ? (
        <div className="browser-bar" role="alert">
          <TriangleAlert size={14} aria-hidden="true" />
          <span className="browser-bar-text">{tab.error}</span>
          <button
            type="button"
            className="g-button g-button-24"
            onClick={() => store.api.go(tab.id, "reload").catch(fail)}
          >
            Try again
          </button>
        </div>
      ) : null}
      {failure ? (
        <p className="browser-bar ui-error" role="alert">
          {failure}
        </p>
      ) : null}
      {/* biome-ignore lint/a11y/useKeyWithClickEvents: Shift-click adds the element under the pointer to an open note; keyboard users keep the note's own controls. */}
      {/* biome-ignore lint/a11y/noStaticElementInteractions: the page area only listens for that pointer gesture. */}
      <div
        ref={page}
        className="browser-page"
        data-testid="browser-page"
        data-frozen={picture !== null || frozen ? "" : undefined}
        onClick={pickOnPicture}
      >
        {view === "history" ? (
          <HistoryView
            api={store.api}
            onOpen={(url) => openInput(url)}
            actions={
              <>
                <button
                  type="button"
                  className="g-button g-button-28"
                  onClick={() => setClearing(true)}
                >
                  Clear browsing data…
                </button>
                <button
                  type="button"
                  className="g-button g-button-ghost g-button-icon g-button-28"
                  aria-label="Close history"
                  onClick={() => setView("page")}
                >
                  <X size={16} />
                </button>
              </>
            }
          />
        ) : !status?.enabled || status.unavailable ? (
          <div className="browser-empty" role="status">
            <Globe size={22} aria-hidden="true" />
            <p>
              {status?.unavailable ??
                "Browser use is turned off. Turn it on in Settings > Plugins & MCP > Universal plugins."}
            </p>
          </div>
        ) : !tab ? null : !tab.url ? (
          <StartPage store={store} runUrl={runUrl} onOpen={openInput} />
        ) : tab.crashed ? (
          <div className="browser-empty" role="alert">
            <TriangleAlert size={22} aria-hidden="true" />
            <p>This page stopped working.</p>
            {!tab.media && !status?.media_notice ? (
              <p>It stopped twice in a short time, so it reloads with audio and video off.</p>
            ) : null}
            <button
              type="button"
              className="g-button g-button-primary g-button-28"
              onClick={() => store.api.go(tab.id, "reload").catch(fail)}
            >
              Reload
            </button>
          </div>
        ) : picture !== null || frozen ? (
          <div className="browser-picture" aria-hidden={note ? undefined : true}>
            {note && pickView?.width && pickView.height ? (
              <div
                className="browser-picture-frame"
                style={
                  {
                    width: pickView.width,
                    height: pickView.height,
                    transform: `scale(${fit})`,
                  } as CSSProperties
                }
              >
                {picture ? <img src={picture} alt="" draggable={false} /> : null}
                {marks(note).map((box, index) => (
                  <span
                    // biome-ignore lint/suspicious/noArrayIndexKey: marks are positional
                    key={index}
                    className="browser-mark"
                    style={
                      {
                        left: box.x ?? 0,
                        top: box.y ?? 0,
                        width: box.width ?? 0,
                        height: box.height ?? 0,
                      } as CSSProperties
                    }
                  >
                    {marks(note).length > 1 ? (
                      <span className="browser-mark-number">{index + 1}</span>
                    ) : null}
                  </span>
                ))}
              </div>
            ) : picture ? (
              <img src={picture} alt="" draggable={false} />
            ) : null}
          </div>
        ) : tab.asleep ? (
          <div className="browser-empty" role="status">
            <p>Loading {shownAddress(tab.url)}…</p>
          </div>
        ) : null}
        {note && anchor && target ? (
          <AnnotationComposer
            annotation={note}
            anchor={anchor}
            page={pageSize}
            controls={target.controls}
            agentName={agentName}
            images={target.images}
            sending={sending}
            error={noteError}
            onSend={(text) => void deliver(text, "send")}
            onAdd={(text) => void deliver(text, "add")}
            onCancel={closeNote}
            onRemove={(index) =>
              setNote((current) => {
                if (!current) return current;
                const elements = current.elements.filter((_, at) => at !== index);
                return elements.length ? { ...current, elements } : null;
              })
            }
          />
        ) : null}
      </div>
      {notice ? (
        <div className="browser-notice" role="status" key={notice.key}>
          {notice.action === "outside" ? (
            <ExternalLink size={14} aria-hidden="true" />
          ) : notice.action === "popup" ? (
            <AppWindow size={14} aria-hidden="true" />
          ) : (
            <Download size={14} aria-hidden="true" />
          )}
          <span className="browser-notice-text" title={notice.text}>
            {notice.text}
          </span>
          {notice.action === "outside" && notice.url ? (
            <button
              type="button"
              className="g-button g-button-24"
              onClick={() => {
                const url = notice.url;
                setNotice(null);
                if (url) store.api.openOutside(url).catch(fail);
              }}
            >
              Open
            </button>
          ) : null}
          {notice.action === "popup" && notice.url ? (
            <button
              type="button"
              className="g-button g-button-24"
              onClick={() => {
                const url = notice.url;
                setNotice(null);
                if (url) store.api.open(chatId, url, true).catch(fail);
              }}
            >
              Open as a tab
            </button>
          ) : null}
          <button
            type="button"
            className="g-button g-button-ghost g-button-icon g-button-24"
            aria-label="Dismiss"
            onClick={() => setNotice(null)}
          >
            <X size={14} />
          </button>
        </div>
      ) : null}
      {clearing ? (
        <ClearDataDialog
          api={store.api}
          passwords={status?.passwords ?? false}
          onClose={() => setClearing(false)}
        />
      ) : null}
    </aside>
  );
}

function TabButton({
  tab,
  active,
  onActivate,
  onClose,
  onMove,
}: {
  readonly tab: BrowserTab;
  readonly active: boolean;
  readonly onActivate: () => void;
  readonly onClose: () => void;
  readonly onMove: (step: number) => void;
}) {
  const name = tabName(tab);
  return (
    <div className="browser-tab" data-active={active ? "" : undefined}>
      <button
        type="button"
        role="tab"
        className="browser-tab-button"
        aria-selected={active}
        aria-keyshortcuts="Delete"
        tabIndex={active ? 0 : -1}
        title={tab.url ? `${name}\n${tab.url}` : name}
        onClick={onActivate}
        onAuxClick={(event) => {
          if (event.button === 1) {
            event.preventDefault();
            onClose();
          }
        }}
        onKeyDown={(event) => {
          if (event.key === "ArrowRight" || event.key === "ArrowLeft") {
            event.preventDefault();
            onMove(event.key === "ArrowRight" ? 1 : -1);
            const strip = event.currentTarget.closest(".browser-tabs");
            requestAnimationFrame(() =>
              strip?.querySelector<HTMLElement>('[role="tab"][aria-selected="true"]')?.focus(),
            );
          } else if (event.key === "Delete") {
            event.preventDefault();
            onClose();
          }
        }}
      >
        <span className="browser-tab-mark" aria-hidden="true">
          {tab.loading ? <span className="browser-tab-spinner" /> : <Globe size={13} />}
        </span>
        <span className="browser-tab-name">{name}</span>
      </button>
      {/* The pointer's way to close; the tab list owns only tabs, which close with Delete. */}
      <button
        type="button"
        className="browser-tab-close"
        aria-hidden="true"
        title="Close tab (Ctrl+W)"
        tabIndex={-1}
        onClick={onClose}
      >
        <X size={12} />
      </button>
    </div>
  );
}
