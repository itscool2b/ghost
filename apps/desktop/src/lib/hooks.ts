import {
  type RefCallback,
  type RefObject,
  useCallback,
  useLayoutEffect,
  useRef,
  useSyncExternalStore,
} from "react";
import type { StreamSnapshot, StreamStore } from "./session";
import {
  isScrollUpKey,
  isScrollUpWheel,
  keyScrollsLog,
  pressTakesOver,
  StickToBottom,
} from "./stick";

const followVisibility = (changed: () => void): (() => void) => {
  if (typeof document === "undefined") return () => {};
  document.addEventListener("visibilitychange", changed);
  return () => document.removeEventListener("visibilitychange", changed);
};
const pageShown = (): boolean =>
  typeof document === "undefined" || document.visibilityState !== "hidden";

/** Whether the window is on screen. A start at sign-in keeps it hidden until it is opened. */
export function usePageVisible(): boolean {
  return useSyncExternalStore(followVisibility, pageShown, () => true);
}

/** The store's current snapshot; re-renders on every frame the stream applies. */
export function useStream(store: StreamStore): StreamSnapshot {
  return useSyncExternalStore(store.subscribe, store.getSnapshot, store.getSnapshot);
}

/**
 * Keeps a scroll container pinned to its bottom while content grows, unless the reader has
 * scrolled up; scrolling back down to the bottom re-pins it (see stick.ts for how a reader's
 * scroll is told apart from the follow). `content` is anything that changes when the content
 * does. Returns the ref for the scroll container.
 */
export function useStickToBottom<T extends HTMLElement>(
  content: unknown,
  shared?: RefObject<StickToBottom | null>,
  onPinnedChange?: (pinned: boolean) => void,
): RefCallback<T> {
  const local = useRef<StickToBottom | null>(null);
  const stick = shared ?? local;

  // biome-ignore lint/correctness/useExhaustiveDependencies: `content` is the trigger by design.
  useLayoutEffect(() => {
    stick.current?.follow();
  }, [content, stick]);

  return useCallback(
    (element: T | null) => {
      if (!element) return;
      const controller = new StickToBottom(element, onPinnedChange);
      stick.current = controller;
      controller.follow();

      const inLog = (target: EventTarget | null): boolean =>
        target instanceof Node && element.contains(target);
      let lastPressInLog = false;

      // Intent arrives before the scroll it causes: release on it, so the next commit cannot undo
      // the scroll before its event arrives.
      const onScroll = (): void => controller.onScroll();
      const onWheel = (event: WheelEvent): void => {
        if (isScrollUpWheel(event)) controller.release();
      };
      const onTouchMove = (): void => controller.release();
      // On the document: a key pressed with nothing focused scrolls what was last clicked, which
      // only a document listener sees.
      const onPointerDown = (event: PointerEvent): void => {
        lastPressInLog = inLog(event.target);
        const press = { onContainer: event.target === element, button: event.button };
        if (lastPressInLog && pressTakesOver(press)) controller.release();
      };
      const onKeyDown = (event: KeyboardEvent): void => {
        const target = event.target;
        const onPage =
          target === document || target === document.body || target === document.documentElement;
        if (
          isScrollUpKey(event) &&
          keyScrollsLog({ inLog: inLog(target), onPage, lastPressInLog })
        ) {
          controller.release();
        }
      };
      // Tabbing into the log scrolls the focused control into view: keep it there.
      const onFocusIn = (event: FocusEvent): void => {
        if (event.target instanceof Element && event.target.matches(":focus-visible")) {
          controller.release();
        }
      };

      element.addEventListener("scroll", onScroll, { passive: true });
      element.addEventListener("wheel", onWheel, { passive: true });
      element.addEventListener("touchmove", onTouchMove, { passive: true });
      element.addEventListener("focusin", onFocusIn);
      document.addEventListener("pointerdown", onPointerDown, true);
      document.addEventListener("keydown", onKeyDown, true);
      // A resized window reflows the text: a pinned view stays on the last line.
      const resize =
        typeof ResizeObserver === "undefined"
          ? null
          : new ResizeObserver(() => controller.follow());
      resize?.observe(element);

      return () => {
        element.removeEventListener("scroll", onScroll);
        element.removeEventListener("wheel", onWheel);
        element.removeEventListener("touchmove", onTouchMove);
        element.removeEventListener("focusin", onFocusIn);
        document.removeEventListener("pointerdown", onPointerDown, true);
        document.removeEventListener("keydown", onKeyDown, true);
        resize?.disconnect();
        if (stick.current === controller) stick.current = null;
      };
    },
    [stick, onPinnedChange],
  );
}

/** Whether the media query matches now, following its changes; `false` without a window. */
export function useMediaQuery(query: string): boolean {
  return useSyncExternalStore(
    (onChange) => {
      if (typeof window === "undefined" || typeof window.matchMedia !== "function") return () => {};
      const list = window.matchMedia(query);
      list.addEventListener("change", onChange);
      return () => list.removeEventListener("change", onChange);
    },
    () =>
      typeof window !== "undefined" &&
      typeof window.matchMedia === "function" &&
      window.matchMedia(query).matches,
    () => false,
  );
}
