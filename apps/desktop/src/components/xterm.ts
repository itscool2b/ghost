import type { Terminal } from "@xterm/xterm";
import { subscribeAppearance, subscribeTheme } from "../lib/appearance";
import { type TerminalVariant, terminalFonts } from "../lib/terminalFont";
import {
  cancelsDefault,
  copiesSelection,
  focusBeyond,
  leavesTerminal,
  scrollsTerminal,
  typesText,
} from "./terminalKeys";

/** An opened xterm view in Graphite's terminal colours, fitted to its element. */
export interface XtermView {
  readonly term: Terminal;
  /** Fits rows and columns to the element; does nothing while it has no size. */
  fit(): void;
  dispose(): void;
}

export interface XtermOptions {
  readonly inputLabel: string;
  readonly onLink?: (url: string) => void;
  readonly keys?: (event: KeyboardEvent) => boolean;
  /**
   * A terminal chat runs an agent's whole interface: deep scrollback and the blinking cursor its
   * prompt expects. A shell in the docked panel and a tool's sheet keep the compact defaults. All
   * draw with the person's terminal font unless they chose Ghost's (lib/terminalFont.ts).
   */
  readonly variant?: TerminalVariant;
}

type XtermModules = {
  readonly Terminal: typeof import("@xterm/xterm").Terminal;
  readonly FitAddon: typeof import("@xterm/addon-fit").FitAddon;
  readonly WebglAddon: typeof import("@xterm/addon-webgl").WebglAddon;
};
let modules: Promise<XtermModules> | null = null;

/** xterm loads on first use, once; the chat screen does not pay for it until a terminal opens. */
export function loadXterm(): Promise<XtermModules> {
  modules ??= Promise.all([
    import("@xterm/xterm"),
    import("@xterm/addon-fit"),
    import("@xterm/addon-webgl"),
  ]).then(([xterm, fit, webgl]) => ({
    Terminal: xterm.Terminal,
    FitAddon: fit.FitAddon,
    WebglAddon: webgl.WebglAddon,
  }));
  return modules;
}

/** Loads xterm, then opens it in `element` (which must be in the page by then). */
export async function openXterm(element: HTMLElement, options: XtermOptions): Promise<XtermView> {
  return createXterm(await loadXterm(), element, options);
}

/**
 * Opens xterm in `element`, shared by the terminal sheet, the docked panel and terminal chats:
 * code text on surface-1 and the ANSI palette (visual-language 3.1), the Term type size, no
 * blinking cursor, a screen reader mode, and input disabled until the caller enables it. Output
 * can never set the OS clipboard (OSC 52 is swallowed); `keys` decides which key events xterm
 * handles at all, within the rules of terminalKeys.ts. xterm measures its cells here, so `element`
 * must be in the page.
 */
export function createXterm(
  { Terminal, FitAddon, WebglAddon }: XtermModules,
  element: HTMLElement,
  options: XtermOptions,
): XtermView {
  const styles = getComputedStyle(document.documentElement);
  const color = (name: string, fallback: string) =>
    styles.getPropertyValue(name).trim() || fallback;
  const variant = options.variant ?? "tool";
  const ownStack = color("--g-mono", '"Geist Mono Variable", monospace');
  const font = terminalFonts.resolve(variant, ownStack);
  const term = new Terminal({
    cols: 80,
    rows: 24,
    scrollback: variant === "chat" ? 10000 : 1000,
    fontSize: font.fontSize,
    lineHeight: font.lineHeight,
    fontFamily: font.fontFamily,
    cursorBlink: variant === "chat",
    screenReaderMode: true,
    disableStdin: true,
    logLevel: "off",
    theme: terminalTheme(),
    linkHandler: {
      activate: (_event, uri) => {
        try {
          const url = new URL(uri);
          if (url.protocol === "https:" || url.protocol === "http:") options.onLink?.(url.href);
        } catch {
          /* Malformed links remain terminal text. */
        }
      },
    },
  });
  const fitAddon = new FitAddon();
  term.loadAddon(fitAddon);
  term.attachCustomKeyEventHandler((event) => {
    const leaving = leavesTerminal(event);
    if (leaving !== null) {
      if (event.type === "keydown") {
        event.preventDefault();
        event.stopPropagation();
        focusBeyond(element, leaving);
      }
      return false;
    }
    if (copiesSelection(event)) {
      event.preventDefault();
      event.stopPropagation();
      if (term.hasSelection())
        void navigator.clipboard?.writeText(term.getSelection()).catch(() => {});
      return false;
    }
    // Nothing runs to take the keys: they work in the app, and Tab reaches what follows.
    if (term.options.disableStdin) return scrollsTerminal(event);
    if (options.keys && !options.keys(event)) return false;
    if (typesText(event)) event.stopPropagation();
    return true;
  });
  // In screen reader mode xterm leaves most keys it sends uncancelled, so the browser, the app
  // and the window would act on them too: Shift+Tab sent its code and moved focus out of the
  // terminal. Every key it sent but typed text is the program's alone (terminalKeys.ts).
  const held = term.onKey(({ domEvent }) => {
    domEvent.stopPropagation();
    if (cancelsDefault(domEvent)) domEvent.preventDefault();
  });
  term.open(element);
  term.textarea?.setAttribute("aria-label", options.inputLabel);
  term.textarea?.setAttribute("aria-description", "Ctrl+Tab leaves the terminal.");
  // The canvas renderer is optional; terminal IO stays usable if WebGL is unavailable or lost.
  try {
    const webgl = new WebglAddon();
    webgl.onContextLoss(() => webgl.dispose());
    term.loadAddon(webgl);
  } catch {
    /* xterm keeps its DOM renderer. */
  }
  // Output cannot set the OS clipboard. Copying is always the user's own action.
  const clipboard = term.parser.registerOscHandler(52, () => true);
  // A new appearance or theme repaints every terminal, detached and inactive tabs too. Their
  // buffers, selection and running PTY survive.
  const repaint = () => {
    term.options.theme = terminalTheme();
  };
  const unsubscribeAppearance = subscribeAppearance(repaint);
  const unsubscribeTheme = subscribeTheme(repaint);
  // A new font choice, or the system's font arriving after the first paint, reflows every
  // terminal in place: its buffer and running process stay.
  const refont = () => {
    const next = terminalFonts.resolve(variant, ownStack);
    if (
      term.options.fontFamily === next.fontFamily &&
      term.options.fontSize === next.fontSize &&
      term.options.lineHeight === next.lineHeight
    )
      return;
    term.options.fontFamily = next.fontFamily;
    term.options.fontSize = next.fontSize;
    term.options.lineHeight = next.lineHeight;
    fitView();
  };
  const unsubscribeFont = terminalFonts.subscribe(refont);
  void terminalFonts.load();
  const fitView = () => {
    const dimensions = fitAddon.proposeDimensions();
    if (!dimensions || dimensions.cols < 2 || dimensions.rows < 1) return;
    term.resize(Math.min(1000, dimensions.cols), Math.min(1000, dimensions.rows));
  };
  return {
    term,
    fit: fitView,
    dispose: () => {
      unsubscribeAppearance();
      unsubscribeTheme();
      unsubscribeFont();
      clipboard.dispose();
      held.dispose();
      term.dispose();
    },
  };
}

/** Resolve from the root so inactive terminal tabs do not lose their palette while detached. */
function terminalTheme(): NonNullable<Terminal["options"]["theme"]> {
  const styles = getComputedStyle(document.documentElement);
  const color = (name: string, fallback: string) =>
    styles.getPropertyValue(name).trim() || fallback;
  return {
    background: color("--g-terminal-bg", color("--g-surface-1", "#141516")),
    foreground: color("--g-terminal-fg", color("--g-code-fg", "#d4d4d4")),
    cursor: color("--g-terminal-cursor", color("--g-fg-1", "#fafafa")),
    cursorAccent: color("--g-terminal-cursor-text", color("--g-surface-1", "#141516")),
    selectionBackground: color("--g-terminal-selection", color("--g-selection", "#333333")),
    selectionForeground: color("--g-fg-1", "#fafafa"),
    black: color("--g-ansi-black", "#4d4d4d"),
    red: color("--g-ansi-red", "#ff6b6b"),
    green: color("--g-ansi-green", "#3dd68c"),
    yellow: color("--g-ansi-yellow", "#e6cc6e"),
    blue: color("--g-ansi-blue", "#7aa7ff"),
    magenta: color("--g-ansi-magenta", "#d69cff"),
    cyan: color("--g-ansi-cyan", "#5cd6d6"),
    white: color("--g-ansi-white", "#d4d4d4"),
    brightBlack: color("--g-ansi-bright-black", "#858585"),
    brightRed: color("--g-ansi-bright-red", "#ff8f8f"),
    brightGreen: color("--g-ansi-bright-green", "#6ee7a8"),
    brightYellow: color("--g-ansi-bright-yellow", "#f2dc8c"),
    brightBlue: color("--g-ansi-bright-blue", "#a3c2ff"),
    brightMagenta: color("--g-ansi-bright-magenta", "#e4bdff"),
    brightCyan: color("--g-ansi-bright-cyan", "#8ae6e6"),
    brightWhite: color("--g-ansi-bright-white", "#ededed"),
  };
}
