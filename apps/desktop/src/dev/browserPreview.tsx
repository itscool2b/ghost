/**
 * Browser fixture (`?preview=browser`, docs/superpowers/plans/2026-11-09-browser.md): a chat with
 * its browser open, over an in-memory browser. No webview: a same-origin frame stands in for the
 * native page, positioned wherever the panel places it and drawn above every element, as the
 * native view is, so occlusion, Annotate and the note behave as in the app. Its "screenshots" are
 * drawn from the frame's boxes and text on a canvas.
 *
 * `window.browserFixture` drives it: `pick(selector, add)`, `agent(label, opened)`, `emit(event)`,
 * and records `placements`, `sent`, `pastes` and `outside`.
 */
import { StrictMode, useState } from "react";
import { createRoot } from "react-dom/client";
import type {
  BrowserEvent,
  BrowserPick,
  BrowserRect,
  BrowserStatus,
  BrowserTab,
  ChatBrowser,
  ComposerAttachment,
  ComposerInput,
  HistoryEntry,
  PickedElement,
} from "../bindings";
import { ChatView } from "../components/chat/ChatView";
import { type BrowserApi, BrowserStore, browserPanels } from "../lib/browser";
import {
  attachResult,
  chatSummary,
  FakeTransport,
  historyPage,
  lists,
  sessionInfo,
} from "../lib/chat/testing";
import { withPreviewContext } from "./previewContext";

interface FixtureControls {
  readonly placements: { tab: string | null; rect: BrowserRect | null; visible: boolean }[];
  readonly sent: { text: string; attachments: string[] }[];
  readonly pastes: { name: string | undefined; bytes: number; png: boolean }[];
  readonly outside: string[];
  readonly annotate: { tab: string; on: boolean }[];
  pick(selector: string, add?: boolean): Promise<void>;
  agent(label: string | null, opened?: boolean): void;
  emit(event: BrowserEvent): void;
  page(): Document | null;
}

declare global {
  interface Window {
    browserFixture?: FixtureControls;
  }
}

const CHAT = "browser-chat";

/** The page every address shows: a small product site with a hero, cards and a sign-in form. */
const PAGE = `<!doctype html><html><head><meta charset="utf-8"><title>Acme · Pricing</title><style>
  * { box-sizing: border-box; }
  body { margin: 0; font: 15px/1.5 system-ui, sans-serif; color: #18181b; background: #fafafa; }
  header { display: flex; align-items: center; justify-content: space-between; padding: 14px 28px; background: #ffffff; border-bottom: 1px solid #e4e4e7; }
  .logo { font-weight: 700; letter-spacing: -0.02em; }
  nav a { margin-left: 18px; color: #3f3f46; text-decoration: none; }
  .hero { padding: 48px 28px 28px; }
  .hero h1 { margin: 0 0 8px; font-size: 34px; letter-spacing: -0.03em; }
  .hero p { margin: 0 0 20px; color: #52525b; max-width: 520px; }
  .cta { display: inline-flex; padding: 10px 18px; border: 0; border-radius: 999px; background: #18181b; color: #ffffff; font: inherit; font-weight: 600; }
  .cards { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 16px; padding: 8px 28px 28px; }
  .card { padding: 18px; border-radius: 14px; background: #ffffff; border: 1px solid #e4e4e7; }
  .card h2 { margin: 0 0 4px; font-size: 17px; }
  .price { font-size: 26px; font-weight: 700; }
  form { display: flex; gap: 8px; padding: 0 28px 40px; }
  input { flex: 1; padding: 9px 12px; border: 1px solid #d4d4d8; border-radius: 10px; font: inherit; }
</style></head><body>
  <header><span class="logo">Acme</span><nav><a href="#">Product</a><a href="#">Pricing</a><a href="#">Docs</a></nav></header>
  <section class="hero" data-component="Hero"><h1>Simple pricing</h1><p>One plan for side projects, one for teams, and one for everyone else.</p><button class="cta" data-component="HeroButton">Start free</button></section>
  <section class="cards"><article class="card" data-component="PlanCard"><h2>Hobby</h2><div class="price">$0</div><p>For trying things out.</p></article><article class="card" data-component="PlanCard"><h2>Team</h2><div class="price">$24</div><p>Per seat, billed monthly.</p></article><article class="card" data-component="PlanCard"><h2>Scale</h2><div class="price">Talk to us</div><p>For larger companies.</p></article></section>
  <form><input type="email" placeholder="you@company.com" aria-label="Email"><input type="password" placeholder="Password" aria-label="Password"><button class="cta" type="button">Sign in</button></form>
</body></html>`;

const now = () => Date.now();

function title(url: string | null): string {
  if (!url) return "";
  if (url.includes("google.com/search")) return "Search results";
  return "Acme · Pricing";
}

function resolve(input: string): string {
  const text = input.trim();
  if (/^[a-z]+:\/\//i.test(text)) return text;
  if (/^(localhost|127\.0\.0\.1)(:\d+)?(\/.*)?$/i.test(text)) return `http://${text}`;
  if (/^[\w-]+(\.[\w-]+)+(:\d+)?(\/.*)?$/.test(text))
    return `https://${text}/`.replace(/\/\/$/, "/");
  return `https://www.google.com/search?q=${encodeURIComponent(text)}`;
}

/** The element as Ghost's page script describes it. */
function describe(element: Element): PickedElement {
  const rect = element.getBoundingClientRect();
  const style = element.ownerDocument.defaultView?.getComputedStyle(element);
  const component = element.closest("[data-component]")?.getAttribute("data-component") ?? null;
  return {
    tag: element.localName,
    id: element.id || null,
    classes: Array.from(element.classList),
    attributes: Object.fromEntries(
      ["type", "aria-label", "placeholder", "href"]
        .filter((name) => element.hasAttribute(name))
        .map((name) => [name, element.getAttribute(name) ?? ""]),
    ),
    selector: [
      element.parentElement?.localName,
      element.localName + (element.classList[0] ? `.${element.classList[0]}` : ""),
    ]
      .filter(Boolean)
      .join(" > "),
    text: (element.textContent ?? "").replace(/\s+/g, " ").trim().slice(0, 200),
    rect: { x: rect.x, y: rect.y, width: rect.width, height: rect.height },
    styles: style
      ? {
          color: style.color,
          "background-color": style.backgroundColor,
          "font-size": style.fontSize,
          display: style.display,
        }
      : {},
    component: component
      ? { name: component, stack: ["App", "Pricing", component], framework: "React" }
      : null,
    html: element.outerHTML.replace(/\s+/g, " ").slice(0, 600),
  };
}

/** The in-memory browser: tabs, history, and a frame per placed page. */
export class FixtureBrowser implements BrowserApi {
  readonly #listeners = new Set<(event: BrowserEvent) => void>();
  readonly #chats = new Map<string, ChatBrowser>();
  readonly #frames = new Map<string, HTMLIFrameElement>();
  readonly #history: HistoryEntry[] = [
    { url: "http://localhost:3104/", title: "Acme dev", visits: 12, last_visit: now() - 60_000 },
    {
      url: "https://github.com/acme/web",
      title: "acme/web",
      visits: 4,
      last_visit: now() - 3_600_000,
    },
    {
      url: "https://docs.example.com/guide",
      title: "Guide · Example docs",
      visits: 2,
      last_visit: now() - 90_000_000,
    },
  ];
  #next = 1;
  /** The fixture opened its first page. */
  opened = false;
  readonly controls: FixtureControls;
  readonly #status: BrowserStatus = {
    enabled: true,
    unavailable: null,
    passwords: true,
    // `?media=off`: Linux without GStreamer's plugins.
    media_notice:
      new URLSearchParams(location.search).get("media") === "off"
        ? "Audio and video are off here: GStreamer's base and good plugins are missing. Install gst-plugins-base and gst-plugins-good, then restart Ghost."
        : null,
  };

  constructor() {
    const placements: FixtureControls["placements"] = [];
    this.controls = {
      placements,
      sent: [],
      pastes: [],
      outside: [],
      annotate: [],
      pick: async (selector, add = false) => {
        const tab = this.#active(CHAT);
        const frame = tab ? this.#frames.get(tab.id) : null;
        const element = frame?.contentDocument?.querySelector(selector);
        if (!tab || !element) throw new Error(`nothing matches ${selector}`);
        const pick: BrowserPick = {
          mode: "element",
          url: tab.url ?? "",
          title: tab.title,
          add,
          viewport: {
            width: frame?.clientWidth ?? 800,
            height: frame?.clientHeight ?? 600,
            dpr: 1,
          },
          area: null,
          elements: [describe(element)],
          screenshot: await this.capture(tab.id),
        };
        this.#emit({ type: "pick", chat_id: CHAT, tab_id: tab.id, pick });
      },
      agent: (label, opened = false) =>
        this.#emit({
          type: "agent",
          chat_id: CHAT,
          tab_id: this.#active(CHAT)?.id ?? null,
          label,
          opened,
        }),
      emit: (event) => this.#emit(event),
      page: () => {
        const tab = this.#active(CHAT);
        return (tab && this.#frames.get(tab.id)?.contentDocument) || null;
      },
    };
  }

  #emit(event: BrowserEvent): void {
    for (const listener of [...this.#listeners]) listener(event);
  }

  #chat(chatId: string): ChatBrowser {
    let chat = this.#chats.get(chatId);
    if (!chat) {
      chat = { chat_id: chatId, tabs: [], active: null };
      this.#chats.set(chatId, chat);
    }
    return chat;
  }

  #active(chatId: string): BrowserTab | null {
    const chat = this.#chat(chatId);
    return chat.tabs.find((tab) => tab.id === chat.active) ?? null;
  }

  #update(chatId: string, change: (chat: ChatBrowser) => ChatBrowser): void {
    const next = change(this.#chat(chatId));
    this.#chats.set(chatId, next);
    this.#emit({ type: "chat", browser: next });
  }

  #patch(tabId: string, change: Partial<BrowserTab>): void {
    for (const [chatId, chat] of this.#chats) {
      if (chat.tabs.some((tab) => tab.id === tabId))
        this.#update(chatId, (current) => ({
          ...current,
          tabs: current.tabs.map((tab) => (tab.id === tabId ? { ...tab, ...change } : tab)),
        }));
    }
  }

  async status(): Promise<BrowserStatus> {
    return this.#status;
  }
  async chat(chatId: string): Promise<ChatBrowser> {
    return this.#chat(chatId);
  }
  async open(chatId: string, input: string | null, activate: boolean): Promise<BrowserTab> {
    const url = input ? resolve(input) : null;
    const tab: BrowserTab = {
      id: `fixture-tab-${this.#next++}`,
      url,
      title: title(url),
      loading: false,
      can_go_back: false,
      can_go_forward: false,
      crashed: false,
      error: null,
      zoom: 1,
      asleep: false,
      annotating: false,
      media: this.#status.media_notice === null,
    };
    this.#update(chatId, (chat) => ({
      ...chat,
      tabs: [...chat.tabs, tab],
      active: activate || !chat.active ? tab.id : chat.active,
    }));
    if (url) this.#visit(url);
    return tab;
  }
  async close(tabId: string): Promise<void> {
    this.#frames.get(tabId)?.remove();
    this.#frames.delete(tabId);
    for (const [chatId, chat] of this.#chats) {
      const index = chat.tabs.findIndex((tab) => tab.id === tabId);
      if (index < 0) continue;
      this.#update(chatId, (current) => {
        const tabs = current.tabs.filter((tab) => tab.id !== tabId);
        const active =
          current.active === tabId
            ? ((tabs[index] ?? tabs[index - 1] ?? null)?.id ?? null)
            : current.active;
        return { ...current, tabs, active };
      });
    }
  }
  async activate(chatId: string, tabId: string): Promise<void> {
    this.#update(chatId, (chat) => ({ ...chat, active: tabId }));
  }
  async move(): Promise<void> {}
  async navigate(tabId: string, input: string): Promise<string> {
    const url = resolve(input);
    this.#patch(tabId, { url, title: title(url), loading: true, can_go_back: true, error: null });
    this.#visit(url);
    window.setTimeout(() => this.#patch(tabId, { loading: false }), 400);
    return url;
  }
  async go(tabId: string, action: string): Promise<void> {
    if (action === "reload") {
      this.#patch(tabId, { loading: true });
      window.setTimeout(() => this.#patch(tabId, { loading: false }), 300);
    } else if (action === "back") this.#patch(tabId, { can_go_back: false, can_go_forward: true });
    else if (action === "forward") this.#patch(tabId, { can_go_back: true, can_go_forward: false });
    else if (action === "stop") this.#patch(tabId, { loading: false });
  }
  async place(tabId: string | null, rect: BrowserRect | null, visible: boolean): Promise<void> {
    this.controls.placements.push({ tab: tabId, rect, visible });
    for (const [id, frame] of this.#frames) if (id !== tabId) frame.style.display = "none";
    if (!tabId) return;
    let frame = this.#frames.get(tabId);
    if (!frame) {
      frame = document.createElement("iframe");
      frame.title = "Page";
      frame.srcdoc = PAGE;
      frame.setAttribute("data-fixture-page", tabId);
      // The native page is no stop in the window's own tab order; the panel hands focus to it.
      frame.tabIndex = -1;
      Object.assign(frame.style, {
        position: "fixed",
        zIndex: "2147483000",
        border: "0",
        background: "#fafafa",
      });
      document.body.append(frame);
      this.#frames.set(tabId, frame);
    }
    if (!rect || !visible) {
      frame.style.display = "none";
      return;
    }
    Object.assign(frame.style, {
      display: "block",
      left: `${rect.x}px`,
      top: `${rect.y}px`,
      width: `${rect.width}px`,
      height: `${rect.height}px`,
    });
  }
  async capture(tabId: string): Promise<string | null> {
    const frame = this.#frames.get(tabId);
    const doc = frame?.contentDocument;
    if (!frame || !doc?.body) return null;
    const width = Math.max(1, frame.clientWidth || Number.parseFloat(frame.style.width) || 800);
    const height = Math.max(1, frame.clientHeight || Number.parseFloat(frame.style.height) || 600);
    const canvas = document.createElement("canvas");
    canvas.width = width;
    canvas.height = height;
    const context = canvas.getContext("2d");
    if (!context) return null;
    const view = doc.defaultView;
    context.fillStyle = view?.getComputedStyle(doc.body).backgroundColor ?? "white";
    context.fillRect(0, 0, width, height);
    for (const element of doc.body.querySelectorAll("*")) {
      const style = view?.getComputedStyle(element);
      if (!style) continue;
      const box = element.getBoundingClientRect();
      if (style.backgroundColor && style.backgroundColor !== "rgba(0, 0, 0, 0)") {
        context.fillStyle = style.backgroundColor;
        context.beginPath();
        context.roundRect(
          box.x,
          box.y,
          box.width,
          box.height,
          Number.parseFloat(style.borderRadius) || 0,
        );
        context.fill();
      }
      for (const node of element.childNodes) {
        if (node.nodeType !== Node.TEXT_NODE || !node.textContent?.trim()) continue;
        const range = doc.createRange();
        range.selectNodeContents(node);
        const at = range.getBoundingClientRect();
        context.fillStyle = style.color;
        context.font = `${style.fontWeight} ${style.fontSize} ${style.fontFamily}`;
        context.textBaseline = "top";
        context.fillText(node.textContent.trim(), at.x, at.y + 2);
      }
    }
    return canvas.toDataURL("image/jpeg", 0.85);
  }
  async zoom(_tabId: string, zoom: number): Promise<number> {
    return zoom;
  }
  async devtools(): Promise<void> {}
  async focus(): Promise<void> {}
  async annotate(tabId: string, on: boolean): Promise<boolean> {
    this.controls.annotate.push({ tab: tabId, on });
    this.#patch(tabId, { annotating: on });
    return on;
  }
  async pickAt(tabId: string, x: number, y: number): Promise<PickedElement | null> {
    const element = this.#frames.get(tabId)?.contentDocument?.elementFromPoint(x, y);
    return element ? describe(element) : null;
  }
  async suggest(input: string): Promise<HistoryEntry[]> {
    const words = input.toLowerCase();
    return this.#history.filter(
      (entry) =>
        entry.url.toLowerCase().includes(words) || entry.title.toLowerCase().includes(words),
    );
  }
  async history(query: string): Promise<HistoryEntry[]> {
    return this.suggest(query);
  }
  async removeHistory(urls: string[]): Promise<number> {
    const before = this.#history.length;
    for (const url of urls) {
      const index = this.#history.findIndex((entry) => entry.url === url);
      if (index >= 0) this.#history.splice(index, 1);
    }
    return before - this.#history.length;
  }
  async clear(): Promise<void> {
    this.#history.length = 0;
  }
  async openOutside(url: string): Promise<void> {
    this.controls.outside.push(url);
  }
  listen(listener: (event: BrowserEvent) => void): () => void {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }

  #visit(url: string): void {
    const found = this.#history.find((entry) => entry.url === url);
    if (found) {
      found.visits += 1;
      found.last_visit = now();
    } else this.#history.unshift({ url, title: title(url), visits: 1, last_visit: now() });
  }
}

/** One in-memory browser for the page (StrictMode builds the state twice; the frames are global). */
let shared: FixtureBrowser | null = null;

function Preview() {
  const [fixture] = useState(() => {
    shared ??= new FixtureBrowser();
    const browser = shared;
    window.browserFixture = browser.controls;
    const chat = chatSummary(CHAT, {
      status: "ready",
      agent_id: "claude",
      agent_name: "Claude Code",
    });
    const transport = withPreviewContext(
      new FakeTransport({
        attach: async () =>
          attachResult(chat.id, 0, historyPage(chat.id, []), {
            chat,
            session: sessionInfo({ status: "ready" }),
          }),
        lists: async () =>
          lists(1, {
            capabilities: {
              images: true,
              steer: false,
              interrupt: true,
              resume: true,
              fork: false,
              rewind: false,
              set_model: true,
              set_effort: true,
              permissions: true,
              plans: false,
              usage: false,
              limits: false,
              slash_commands: false,
              add_dirs: false,
              plan_edits: false,
              autonomy_levels: ["full_auto"],
            },
          }),
        send: async (_chat, text) => {
          browser.controls.sent.push({ text, attachments: [] });
          return {
            prompt_id: `p${browser.controls.sent.length}`,
            queued: false,
            position: 0,
          } as never;
        },
      }),
    );
    const files: ComposerAttachment[] = [];
    let draft: ComposerInput = { text: "", attachments: [], mentions: [] };
    transport.composer = {
      ...transport.composer,
      draft: async () => ({
        input: draft,
        files: files.filter((file) => draft.attachments.includes(file.id)),
      }),
      saveDraft: async (_id, input) => {
        draft = input;
      },
      paste: async (_id, bytes, name) => {
        const png =
          bytes[0] === 0x89 && bytes[1] === 0x50 && bytes[2] === 0x4e && bytes[3] === 0x47;
        browser.controls.pastes.push({ name, bytes: bytes.length, png });
        const item: ComposerAttachment = {
          id: `paste-${browser.controls.pastes.length}`,
          name: name ?? "Pasted image.png",
          mime: "image/png",
          bytes: bytes.length,
          image: true,
        };
        files.push(item);
        return item;
      },
      send: async (_id, input) => {
        browser.controls.sent.push({ text: input.text, attachments: [...input.attachments] });
        return {
          prompt_id: `p${browser.controls.sent.length}`,
          queued: false,
          position: 0,
        } as never;
      },
    };
    const store = new BrowserStore(browser);
    const params = new URLSearchParams(location.search);
    browserPanels.set(CHAT, {
      open: params.get("closed") === null,
      full: params.get("full") !== null,
    });
    // A page is open unless the fixture asks for the start page.
    if (params.get("start") === null && !browser.opened) {
      browser.opened = true;
      void browser.open(CHAT, "localhost:3104/pricing", true);
    }
    return { chat, transport: Object.assign(transport, { browser: store }) };
  });
  return (
    <main className="chat-app" style={{ display: "flex", flexDirection: "column" }}>
      <h1 className="sr-only">Browser verification</h1>
      <div className="chat-main" style={{ flex: 1, minHeight: 0 }}>
        <ChatView
          chatId={fixture.chat.id}
          listed={fixture.chat}
          transport={fixture.transport}
          agents={[]}
          storage={null}
        />
      </div>
    </main>
  );
}

export function mountBrowserPreview(container: HTMLElement) {
  createRoot(container).render(
    <StrictMode>
      <Preview />
    </StrictMode>,
  );
}
