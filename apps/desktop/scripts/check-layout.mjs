/**
 * The layout probe: walks every ?preview= mode in
 * Chromium and WebKit at two window sizes, plus the chat shell's floating layers opened as a person
 * opens them ("settled#picker": the picker, the palette, the chat menu, the Changes panel, a
 * typed message) and other screens' states ("plugins#plugin-menu", an agent's opened details),
 * and checks visual correctness, with optional grid diagnostics:
 *
 * 1. Horizontal overflow: the page scrolls sideways, or a visible box reaches past the window.
 * 2. Optional --strict-grid diagnostics: a padded block container whose offset from its padded
 *    parent, or whose width or height, is off the 4px spacing grid by more than half a pixel.
 * 3. Overlapping controls: two visible interactive elements in the same layer (the page, one
 *    popover, one dialog) that overlap, unless one holds the other.
 * 4. Missing focus: an element reached with Tab that draws no visible ring (an outline or a ring
 *    shadow) and no highlight (a fill or border change), which lists use in place of a ring (3.7).
 * 5. Low contrast: visible text under 4.5:1 against the opaque surface behind it. Disabled
 *    controls are exempt, as WCAG exempts them.
 *
 * Development-only fixtures; no agent process or prompt. Usage:
 *   node scripts/check-layout.mjs [--server URL] [--engines chromium,webkit]
 *     [--themes light,dark] [--modes settled,thinking] [--sizes 1440x900,1024x700]
 *     [--focus-steps 120] [--strict-grid]
 * The report is written to target/layout-browser/<engine>.json.
 */
import { spawn } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import { chromium, webkit } from "playwright";
import { choose } from "./choose.mjs";

/** Every ?preview= mode main.tsx routes, the chat harness's own and the other screens'. */
export const MODES = [
  "settled",
  // A call's input, a web search's links, JSON replies, edit counts, Claude's plan, compaction.
  "tool-details",
  // Code colours, inline and display math, and a Mermaid diagram in one answer.
  "rich-answer",
  "streaming",
  "thinking",
  "permission",
  // A plan waiting for review, with Accept plan and Add to the plan.
  "plan",
  "error",
  "new",
  // The start screen: its shortest piece, grown to fit, and its tallest, shrunk to fit.
  "launch&art=b-1b-lancer",
  "launch&art=sr-71-blackbird",
  // The sidebar while the app opens: its placeholders (the lists never answer here), and a fault
  // with Retry beside it.
  "opening&late=600000",
  "opening&fail&late=50",
  // The tour: its offer at the sidebar's foot, and a stop beside an open chat's header.
  "tour&offer",
  "tour&go&open",
  "history",
  "history-question",
  "partial-history",
  "partial-message",
  "partial-work",
  "large-turn",
  "long-prompt",
  "memory-prompt",
  "long-output",
  "focus-group",
  "diff",
  "recorded-thinking",
  "recorded-tools",
  "questions",
  "questions&case=mixed#request",
  "questions&case=group#question-step",
  "steering",
  "worktree-setup",
  "profiles",
  "project-tools",
  "git",
  "git&changes",
  "settings",
  // General with a default agent chosen: its Model row and the agent's reported models.
  "settings#defaults",
  // Settings' other pages (one screen since the owner's plan of 4 November 2026).
  "settings&page=appearance",
  "settings&page=chats",
  "settings&page=voice",
  "settings&page=agents",
  "settings&page=plugins",
  // Browser: Browser use on, with what agents get and its settings.
  "settings&page=browser",
  // The pages that share its look: the instructions sheet, the notes' sheet, and Remote access
  // off and empty, then on with a device and a bot.
  "settings&page=instructions",
  "settings&page=memory",
  "settings&page=memory&scope=folder",
  "settings&page=remote",
  "settings&page=remote&remote=on",
  "settings&page=privacy",
  "setup",
  "agents",
  // An agent's details opened on the Agents page, in each state they take: ready on its direct
  // connection, ready on its ACP adapter, a newer release with Update, not installed, and broken;
  // then the same details in setup's compact roster.
  "agents#codex",
  "agents#codex-adapter",
  "agents#claude-update",
  "agents#grok-missing",
  "agents&broken#cursor-broken",
  "setup#roster",
  // Added accounts: Claude Code's Accounts with one added and each account's usage, the composer's
  // account picker, a chat's usage-limit card, then the same without secure storage.
  "accounts",
  "accounts&accounts=unavailable",
  // The details' executable choice, opened on its own fixture page (the Agents preview has no
  // executables to list).
  "scripts/fixtures/agent-installations.html#executables",
  // New chat in a project folder: a Git folder's worktree chip, no commit yet, a plain folder.
  "new-worktree",
  "new-unavailable",
  "new-plain",
  "git-missing",
  "chat-options",
  "chat-options#busy-folder",
  // The Plugin library, one agent at a time: live listings, an agent without a plugin system, an
  // update with the reread after a version change, a failed CLI listing, a row menu open and an
  // item's preview.
  "plugins",
  "plugins-pi",
  "plugins-update",
  "plugins-failed",
  "plugins#plugin-menu",
  "plugins#plugin-item",
  "steering#queued",
  "steering#queue-edit",
  // A message typed during a turn: Steer beside the Queue disc, or, for an agent without
  // steering, "After this turn" there instead.
  "steering#typed",
  "steering&case=unsupported#typed",
  // The chat shell's floating layers, opened the way a person opens them.
  "settled#picker",
  "settled#palette",
  "settled#chat-menu",
  "settled#changes",
  // The quit question, in the window's own dialog over the chat.
  "settled&quit#quit",
  "thinking#typed",
  // The worktree chip, the new-chat setup choice, and the normal chat's setup review/terminal.
  "new-worktree#worktree",
  "new-worktree#local",
  "new-worktree#branch-error",
  "new-setup",
  "setup-recovery",
  "setup-recovery#setup",
  "setup-actions.html?recovery#clone-recovery",
  "git-missing#recreate",
  // The docked terminal panel: two tabs open, opened with Ctrl+`, its shell menu, and resized.
  "terminal-panel",
  "settled#terminal",
  "terminal-panel#shell-menu",
  "terminal-panel#resized",
  // A worktree chat's merge that conflicts: the files, the message for the agent, Open in.
  "git-conflict#conflict",
  // A chat whose worktree folder was deleted: its start error, and Recreate folder from it.
  "missing-worktree",
  "missing-worktree#recreate",
  // Background tasks: the dock's strip, a subagent's task panel, and the overflow pill's list.
  "tasks",
  "tasks#task-panel",
  "tasks-many",
  "tasks-many#task-menu",
  // Setup's clone: the form with its exact command, Git running, and a failed clone; the same
  // clone from New project's dialog; and an agent's reviewed install with npm running.
  "clone",
  "clone-running",
  "clone-error",
  "new-project",
  "new-project#clone-dialog",
  // A saved project's settings: its folders as rows, a worktree to review, and its removal.
  "project-settings",
  "agents-installing",
  // Instructions in Settings: one file, per agent with its file tabs, the choice between
  // different texts, a hand-edited managed copy, size warnings, a save's review, and the
  // worktree question for an untracked file.
  "instructions",
  "instructions-per-agent",
  "instructions-per-agent#include",
  "instructions-conflict",
  "instructions-conflict#combine",
  "instructions-drift",
  "instructions-large",
  "instructions-review",
  // Settings' "Delete all o7 data": the review, worktrees with unsaved work, and macOS
  // with the Trash.
  "delete-data",
  "delete-data-worktrees",
  "delete-data-macos",
  // The queue saying why the next message waits: the agent starting, working (and quiet for a
  // minute), finishing its turn, a permission to answer, held after a restart, a failed start and
  // a missing worktree folder, each with its one action.
  "queue-starting",
  "queue-working",
  "queue-stalled",
  "queue-finishing",
  "queue-answer",
  "queue-held",
  "queue-failed",
  "queue-missing",
  // The browser beside the chat: a page, a new tab's start page, and the chat with it closed.
  "browser",
  "browser&start",
  "browser&closed",
];

/** Waits until the panel's shell has drawn its prompt. */
const terminalReady = (page) =>
  page.waitForFunction(() =>
    document
      .querySelector(".terminal-panel-viewport .xterm-accessibility-tree")
      ?.textContent?.includes(">"),
  );

const openWorktree = async (page) => {
  await page.getByTestId("new-chat-worktree").locator("summary").click();
  await page.locator(".new-chat-worktree-panel").waitFor();
};

/** Queues two messages behind the steering fixture's running turn. */
async function queueTwo(page) {
  const input = page.getByTestId("composer-input");
  for (const text of ["Then run the unit tests", "Update the changelog"]) {
    await input.fill(text);
    await input.press("Enter");
  }
  await page.getByTestId("queued-prompt").nth(1).waitFor();
}

/** Opens an agent's details on the Agents page, as a person does, and brings them into view. */
async function openAgent(page, name) {
  const row = page.locator(".agent-row", {
    has: page.locator(".agent-row-name strong", { hasText: new RegExp(`^${name}$`) }),
  });
  await row.locator(".agent-row-toggle").click();
  await row.locator(".agent-detail").waitFor();
  return row;
}

/** Centres the opened row in the page's scroller, so every probe sees the whole of it. */
const showRow = (row) => row.evaluate((node) => node.scrollIntoView({ block: "center" }));

/** Resolves after two painted frames: every scroll event of an earlier scroll has arrived. */
const frames = (page) =>
  page.evaluate(
    () => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))),
  );

/**
 * Opens a Plugin library row's menu as a person does: the row is brought into view and its scroll
 * has settled, then the click. Waits for the open state itself (the trigger expanded, the menu
 * shown, its first item holding focus) rather than for time to pass.
 */
async function openRowMenu(page, name) {
  const trigger = page.getByRole("button", { name, exact: true });
  await trigger.scrollIntoViewIfNeeded();
  await frames(page);
  await trigger.click();
  await page.getByRole("button", { name, exact: true, expanded: true }).waitFor();
  await page.getByRole("menu", { name }).waitFor();
  const first = await page
    .getByRole("menu", { name })
    .getByRole("menuitem")
    .first()
    .elementHandle();
  await page.waitForFunction((item) => document.activeElement === item, first);
}

/** How each "<mode>#<view>" opens its layer before the probes run. */
const OPENERS = {
  request: async (page) => {
    await page.getByRole("button", { name: "Ask question", exact: true }).click();
    await page.getByRole("region", { name: "Waiting for your response" }).waitFor();
  },
  "question-step": async (page) => {
    await OPENERS.request(page);
    await page.getByRole("radio", { name: "Unit tests", exact: true }).check();
    await page.getByRole("button", { name: "Next", exact: true }).click();
    await page.getByRole("checkbox", { name: "Failures", exact: true }).waitFor();
  },
  codex: async (page) => showRow(await openAgent(page, "Codex")),
  "codex-adapter": async (page) => {
    const row = await openAgent(page, "Codex");
    await choose(page.getByRole("combobox", { name: "Codex connection" }), "codex-acp");
    await row.getByText(/The ACP adapter downloads once/).waitFor();
    await showRow(row);
  },
  "claude-update": async (page) => {
    const row = await openAgent(page, "Claude Code");
    await row.getByRole("button", { name: "Update Claude Code" }).waitFor();
    await showRow(row);
  },
  "grok-missing": async (page) => showRow(await openAgent(page, "Grok Build")),
  "cursor-broken": async (page) => showRow(await openAgent(page, "Cursor")),
  executables: async (page) => {
    await page.getByRole("button", { name: "Choose executable" }).click();
    await page.getByRole("button", { name: "Use this executable" }).waitFor();
  },
  // Setup's roster draws the same details compact, with the actions inside them.
  roster: async (page) => {
    await page.waitForFunction(() => !document.querySelector("[data-cinematic]"));
    await page.getByRole("button", { name: "Get started" }).click();
    await page.getByRole("heading", { name: /^Found 3 agents\./ }).waitFor();
    await showRow(await openAgent(page, "Claude Code"));
  },
  picker: (page) => page.keyboard.press("Control+."),
  palette: (page) => page.keyboard.press("Control+k"),
  "chat-menu": (page) => page.locator('summary[aria-label="Chat details and controls"]').click(),
  changes: (page) => page.getByRole("button", { name: "Changes", exact: true }).click(),
  "task-panel": async (page) => {
    await page.getByTestId("task-pill").nth(1).click();
    await page.getByTestId("task-panel").getByText("Prompt", { exact: true }).click();
  },
  "task-menu": async (page) => {
    await page.getByTestId("task-overflow").click();
    await page.locator(".chat-task-menu").waitFor();
  },
  quit: (page) => page.getByTestId("quit-keep").waitFor(),
  "busy-folder": async (page) => {
    await choose(page.getByLabel("Move to project"), { label: "Website" });
    await choose(page.getByTestId("move-folder"), "folder-site");
    await page.getByTestId("move-busy").waitFor();
  },
  defaults: async (page) => {
    await choose(page.getByLabel("Default agent"), "codex");
    await page.getByLabel("Default model").waitFor();
  },
  "plugin-menu": (page) => openRowMenu(page, "tools actions"),
  "plugin-item": async (page) => {
    // The menu opens with its first item focused; Enter picks it without scrolling the page.
    await openRowMenu(page, "review-changes actions");
    await page.waitForFunction(
      () => document.activeElement?.textContent?.trim() === "Preview and copy…",
    );
    await page.keyboard.press("Enter");
    await page.getByLabel("Copy destination").waitFor();
  },
  queued: (page) => queueTwo(page),
  "queue-edit": async (page) => {
    await queueTwo(page);
    await page.getByTestId("queued-edit").first().click();
    await page.getByTestId("queued-edit-input").waitFor();
  },
  typed: async (page) => {
    await page.getByTestId("composer-input").click();
    await page.keyboard.type("Hi");
  },
  worktree: openWorktree,
  local: async (page) => {
    await openWorktree(page);
    await page.locator('[data-test-mode-choice="local"]').click();
    await page.keyboard.press("Escape");
  },
  "branch-error": async (page) => {
    await openWorktree(page);
    await page.getByLabel("New branch", { exact: true }).fill("release/1.4");
    await page.getByRole("button", { name: "Apply", exact: true }).click();
    await page.locator(".new-chat-worktree-error").waitFor();
  },
  setup: async (page) => {
    await page.getByRole("region", { name: "Review setup command" }).waitFor();
    await page.getByRole("button", { name: "Run reviewed setup", exact: true }).click();
    await page.locator('.terminal-panel [role="tab"][data-state="running"]').waitFor();
  },
  "clone-recovery": async (page) => {
    await page.getByRole("button", { name: "Clone a repository", exact: true }).click();
    await page.getByRole("region", { name: "Interrupted clones" }).waitFor();
  },
  recreate: async (page) => {
    await page.getByRole("button", { name: "Recreate folder", exact: true }).click();
    await page.getByRole("dialog", { name: "Recreate the deleted folder?" }).waitFor();
  },
  terminal: async (page) => {
    // The panel remembers being open, so an earlier view in this page leaves it restored here
    // without a shell, and the shortcut would close it: start from a first visit instead.
    if (await page.locator(".terminal-panel-viewport").count()) {
      await page.evaluate(() => localStorage.removeItem("auracode.terminals.preview"));
      await page.reload();
      await page.waitForLoadState("networkidle").catch(() => {});
      await page.waitForTimeout(900);
    }
    await page.keyboard.press("Control+Backquote");
    await terminalReady(page);
  },
  "shell-menu": async (page) => {
    await terminalReady(page);
    await page.locator('.terminal-panel-shells > summary[aria-label="New terminal with"]').click();
  },
  conflict: async (page) => {
    await page.getByRole("button", { name: "Review merge", exact: true }).click();
    await page.getByRole("button", { name: "Open in Cursor", exact: true }).waitFor();
  },
  include: async (page) => {
    await page.getByRole("tab", { name: "CLAUDE.md" }).click();
    await page.getByRole("button", { name: "Copy into new worktree chats", exact: true }).click();
    await page.getByRole("dialog", { name: "Copy CLAUDE.md into new worktree chats?" }).waitFor();
  },
  combine: async (page) => {
    await page.getByText("Combine them", { exact: true }).click();
    await page.getByRole("dialog", { name: "Use one file for every agent" }).waitFor();
    await page.getByTestId("diff-block").first().waitFor();
  },
  "clone-dialog": async (page) => {
    await page.getByRole("button", { name: "Clone a repository", exact: true }).click();
    await page
      .getByRole("textbox", { name: "Repository" })
      .fill("https://github.com/auracode/orbit");
    // The folder button reads "Choose a folder", or names the folder a clone used last.
    await page.locator(".clone-parent").click();
    await page.locator(".clone-command-text").waitFor();
  },
  resized: async (page) => {
    await terminalReady(page);
    await page.getByRole("separator", { name: "Resize terminal panel" }).focus();
    for (let step = 0; step < 5; step++) await page.keyboard.press("ArrowUp");
  },
};

const { values } = parseArgs({
  options: {
    server: { type: "string" },
    engines: { type: "string", default: "chromium,webkit" },
    themes: { type: "string", default: "light,dark" },
    "strict-grid": { type: "boolean", default: false },
    modes: { type: "string" },
    sizes: { type: "string", default: "1440x900,1024x700" },
    "focus-steps": { type: "string", default: "120" },
    // Halo glass: each view over an emulated window material (dev/glassPreview.ts). "worst" paints
    // the darkest light material and the lightest dark one, so contrast is checked at its limit.
    glass: { type: "string" },
  },
});
const app = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const out = resolve(app, "../../target/layout-browser");
const port = "1489";
const url = values.server ?? `http://127.0.0.1:${port}`;
const modes = values.modes ? values.modes.split(",") : MODES;
const sizes = values.sizes.split(",").map((size) => size.split("x").map(Number));
const focusSteps = Number(values["focus-steps"]);
const require = createRequire(import.meta.url);

/** Runs in the page: the four static probes. Returns plain findings. */
function probe({ strictGrid = false } = {}) {
  const GRID = 4;
  const TOLERANCE = 0.5;
  for (const animation of document.getAnimations())
    if (animation.effect?.getTiming().iterations !== Infinity) animation.finish();
  const findings = [];
  const view = { width: innerWidth, height: innerHeight };
  const path = (element) => {
    const parts = [];
    for (let node = element; node && node !== document.body && parts.length < 4; ) {
      let part = node.tagName.toLowerCase();
      const testId = node.getAttribute("data-testid");
      const label = node.getAttribute("aria-label");
      if (testId) part += `[data-testid="${testId}"]`;
      else if (node.classList.length) part += `.${[...node.classList].slice(0, 2).join(".")}`;
      if (label) part += `[aria-label="${label.slice(0, 40)}"]`;
      parts.unshift(part);
      node = node.parentElement;
    }
    return parts.join(" > ");
  };
  const opacity = (element) => {
    let value = 1;
    for (let node = element; node; node = node.parentElement)
      value *= Number(getComputedStyle(node).opacity);
    return value;
  };
  const hidden = (element) => {
    const style = getComputedStyle(element);
    if (style.visibility === "hidden" || style.display === "none") return true;
    if (element.closest("[hidden], [inert], details:not([open]) > :not(summary)")) return true;
    if (element.getClientRects().length === 0) return true;
    const box = element.getBoundingClientRect();
    if (box.width <= 1 || box.height <= 1) return true;
    return opacity(element) < 0.05;
  };
  /** The part of the box no overflow-clipping ancestor cuts away, or null when nothing shows. */
  const visibleBox = (element) => {
    const box = element.getBoundingClientRect();
    let left = Math.max(box.left, 0);
    let top = Math.max(box.top, 0);
    let right = Math.min(box.right, view.width);
    let bottom = Math.min(box.bottom, view.height);
    for (let node = element.parentElement; node; node = node.parentElement) {
      const style = getComputedStyle(node);
      if (/(hidden|auto|scroll|clip)/.test(style.overflow + style.overflowX + style.overflowY)) {
        const clip = node.getBoundingClientRect();
        left = Math.max(left, clip.left);
        top = Math.max(top, clip.top);
        right = Math.min(right, clip.right);
        bottom = Math.min(bottom, clip.bottom);
      }
      if (style.position === "fixed") break;
    }
    return right - left > 1 && bottom - top > 1 ? { left, top, right, bottom } : null;
  };

  // 0. Paint (owner, 17 November 2026: flat, as the Grok Bot app): nothing blurs what is behind it,
  // no box over 40% of the window casts a soft shadow (WebKitGTK repaints it on every change inside
  // it when it paints on the CPU), and nothing positioned with a fill lies over the whole window
  // but a dialog's own scrim. A modal dialog keeps the reference's modal shadow: what is behind it
  // is still under the scrim, and nothing streams inside it. A content pane (a landmark, such as
  // the browser covering a narrow chat, which hides the chat under it) is not a decorative layer.
  const windowArea = view.width * view.height;
  const filled = (color) => {
    const parts = color
      .match(/rgba?\(([^)]+)\)/)?.[1]
      .split(/[\s,/]+/)
      .filter(Boolean);
    return parts ? parts.length < 4 || Number(parts[3]) > 0 : color !== "transparent";
  };
  for (const element of document.body.querySelectorAll("*")) {
    if (hidden(element)) continue;
    const style = getComputedStyle(element);
    for (const property of ["backdrop-filter", "-webkit-backdrop-filter"]) {
      const value = style.getPropertyValue(property);
      if (value && value !== "none")
        findings.push({ kind: "paint", target: path(element), detail: `${property}: ${value}` });
    }
    const box = element.getBoundingClientRect();
    const area = Math.max(0, box.width) * Math.max(0, box.height);
    if (style.boxShadow !== "none" && area > windowArea * 0.4 && !element.closest("dialog")) {
      // A layer's lengths after its colour: x, y, blur, spread. Rings (blur 0) and insets are edges.
      const soft = style.boxShadow
        .split(/,(?![^(]*\))/)
        .filter((layer) => !/\binset\b/.test(layer))
        .some((layer) => Number((layer.match(/-?[\d.]+px/g) ?? [])[2]?.replace("px", "") ?? 0) > 0);
      if (soft)
        findings.push({
          kind: "paint",
          target: path(element),
          detail: `soft shadow on ${Math.round((area / windowArea) * 100)}% of the window`,
        });
    }
    if (
      (style.position === "fixed" || style.position === "absolute") &&
      area >= windowArea * 0.9 &&
      !element.closest("dialog") &&
      !element.matches("main, aside, section, nav, form, [role]") &&
      (filled(style.backgroundColor) || style.backgroundImage !== "none")
    )
      findings.push({
        kind: "paint",
        target: path(element),
        detail: `a filled ${style.position} layer over ${Math.round((area / windowArea) * 100)}% of the window`,
      });
  }

  // 1. Horizontal overflow.
  if (document.documentElement.scrollWidth > view.width + 1)
    findings.push({
      kind: "overflow",
      target: "page",
      detail: `page is ${document.documentElement.scrollWidth}px wide in a ${view.width}px window`,
    });
  for (const element of document.body.querySelectorAll("*")) {
    if (hidden(element)) continue;
    const box = element.getBoundingClientRect();
    if (box.right > view.width + 1 && visibleBox(element) && box.left < view.width) {
      const fixed = getComputedStyle(element).position === "fixed";
      // Content scrolled inside a clipping box is not overflow; only boxes the page itself shows.
      let clipped = false;
      for (let node = element.parentElement; node && !fixed; node = node.parentElement) {
        const style = getComputedStyle(node);
        if (/(hidden|auto|scroll|clip)/.test(style.overflowX)) {
          clipped = true;
          break;
        }
      }
      if (!clipped)
        findings.push({
          kind: "overflow",
          target: path(element),
          detail: `right edge at ${Math.round(box.right)}px in a ${view.width}px window`,
        });
    }
  }

  // 2. Padded block containers on the 4px grid.
  const blocks = new Set(["block", "flex", "grid", "flow-root", "list-item", "table"]);
  const padded = (element) => {
    const style = getComputedStyle(element);
    if (!blocks.has(style.display)) return false;
    return ["Top", "Right", "Bottom", "Left"].some(
      (side) => Number.parseFloat(style[`padding${side}`]) > 0,
    );
  };
  const scrollerOf = (element, stop) => {
    for (let node = element.parentElement; node && node !== stop; node = node.parentElement) {
      const style = getComputedStyle(node);
      if (/(auto|scroll)/.test(style.overflowY + style.overflowX)) return node;
    }
    return null;
  };
  const off = (value) => {
    const remainder = Math.abs(value) % GRID;
    return Math.min(remainder, GRID - remainder) > TOLERANCE;
  };
  /**
   * A box as wide as its own words (a pill, a chip, a right-aligned message) takes its width, and
   * so its place on a centred or end-aligned line, from its text; only its height must be on the
   * grid. Measured by letting the box size itself to its content.
   */
  const contentWidth = new Map();
  const sizedByContent = (element) => {
    if (contentWidth.has(element)) return contentWidth.get(element);
    const before = element.getBoundingClientRect().width;
    const saved = element.style.getPropertyValue("width");
    const priority = element.style.getPropertyPriority("width");
    element.style.setProperty("width", "max-content", "important");
    const natural = element.getBoundingClientRect().width;
    if (saved) element.style.setProperty("width", saved, priority);
    else element.style.removeProperty("width");
    const result = Math.abs(natural - before) <= TOLERANCE;
    contentWidth.set(element, result);
    return result;
  };
  for (const element of document.body.querySelectorAll("*")) {
    if (hidden(element) || !padded(element) || !visibleBox(element)) continue;
    const box = element.getBoundingClientRect();
    // Glyph, keycap and chip boxes (under 24px) have Graphite's own pixel geometry.
    if (box.width < 24 || box.height < 24) continue;
    let parent = element.parentElement;
    while (parent && !(padded(parent) && !hidden(parent))) parent = parent.parentElement;
    const scroller = scrollerOf(element, parent);
    const origin = scroller ?? parent;
    const base = origin ? origin.getBoundingClientRect() : { left: 0, top: 0 };
    const dx = box.left - base.left + (scroller ? scroller.scrollLeft : 0);
    const dy = box.top - base.top + (scroller ? scroller.scrollTop : 0);
    const ownWidth = sizedByContent(element);
    // A box placed after content-sized siblings on its line starts wherever their words end.
    let afterWords = false;
    for (let sibling = element.previousElementSibling; sibling; ) {
      const sibBox = sibling.getBoundingClientRect();
      if (
        !hidden(sibling) &&
        Math.abs(sibBox.top - box.top) < box.height &&
        sibBox.left < box.left &&
        sizedByContent(sibling)
      ) {
        afterWords = true;
        break;
      }
      sibling = sibling.previousElementSibling;
    }
    // A modal dialog centred in the window (4.6) takes its offset from the window's size, which the
    // person sets, so only that axis is exempt; its own size and everything inside keep the grid.
    // A dialog held at a set offset (the palette's) is still measured on that axis.
    const modal = !origin && element.matches("dialog:modal");
    const style = getComputedStyle(element);
    const centredX = modal && style.marginLeft === style.marginRight;
    const centredY = modal && style.marginTop === style.marginBottom;
    const problems = [];
    if (!centredX && !ownWidth && !afterWords && off(dx)) problems.push(`x ${dx.toFixed(1)}`);
    if (!centredY && off(dy)) problems.push(`y ${dy.toFixed(1)}`);
    if (!ownWidth && off(box.width)) problems.push(`width ${box.width.toFixed(1)}`);
    if (off(box.height)) problems.push(`height ${box.height.toFixed(1)}`);
    if (strictGrid && problems.length)
      findings.push({
        kind: "grid",
        target: path(element),
        detail: `${problems.join(", ")} (from ${origin ? path(origin) : "the window"})`,
      });
  }

  // 3. Overlapping controls, within one layer.
  const interactive = [
    ...document.body.querySelectorAll(
      'a[href], button, input:not([type="hidden"]), select, textarea, summary, [role="button"], [role="tab"], [role="option"], [role="menuitem"], [role="checkbox"], [role="radio"], [role="switch"], [role="link"]',
    ),
  ]
    .filter((element) => !hidden(element))
    .map((element) => ({ element, box: visibleBox(element) }))
    .filter((entry) => entry.box !== null);
  // A layer is the page, a popover, a dialog, or a box placed over the flow (a sticky prompt, a
  // row's hover menu, a floating "Latest"): controls are compared only within their own layer.
  const layerOf = (element) => {
    for (let node = element; node && node !== document.body; node = node.parentElement) {
      if (node.matches('dialog[open], .chat-popover-panel, [role="menu"], [role="listbox"]'))
        return node;
      if (/^(absolute|fixed|sticky)$/.test(getComputedStyle(node).position)) return node;
    }
    return document.body;
  };
  for (let i = 0; i < interactive.length; i++) {
    const a = interactive[i];
    for (let j = i + 1; j < interactive.length; j++) {
      const b = interactive[j];
      if (a.element.contains(b.element) || b.element.contains(a.element)) continue;
      if (layerOf(a.element) !== layerOf(b.element)) continue;
      const width = Math.min(a.box.right, b.box.right) - Math.max(a.box.left, b.box.left);
      const height = Math.min(a.box.bottom, b.box.bottom) - Math.max(a.box.top, b.box.top);
      if (width > 1 && height > 1)
        findings.push({
          kind: "overlap",
          target: path(a.element),
          detail: `overlaps ${path(b.element)} by ${width.toFixed(0)}x${height.toFixed(0)}px`,
        });
    }
  }

  // 5. Text contrast against the opaque surface behind it.
  const parse = (value) => {
    const match = value.match(/rgba?\(([^)]+)\)/);
    if (!match) return null;
    const parts = match[1]
      .split(/[\s,/]+/)
      .filter(Boolean)
      .map(Number);
    return { r: parts[0], g: parts[1], b: parts[2], a: parts[3] ?? 1 };
  };
  const over = (top, bottom) => ({
    r: top.r * top.a + bottom.r * (1 - top.a),
    g: top.g * top.a + bottom.g * (1 - top.a),
    b: top.b * top.a + bottom.b * (1 - top.a),
    a: 1,
  });
  const luminance = ({ r, g, b }) => {
    const channel = (value) => {
      const c = value / 255;
      return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
    };
    return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
  };
  const background = (element) => {
    const layers = [];
    for (let node = element; node; node = node.parentElement) {
      const style = getComputedStyle(node);
      if (style.backgroundImage !== "none" && !style.backgroundImage.startsWith("url("))
        return null;
      const color = parse(style.backgroundColor);
      if (color && color.a > 0) {
        layers.push(color);
        if (color.a >= 1) break;
      }
    }
    let result = { r: 0, g: 0, b: 0, a: 1 };
    for (const layer of layers.reverse()) result = over(layer, result);
    return result;
  };
  const seen = new Set();
  const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  for (let text = walker.nextNode(); text; text = walker.nextNode()) {
    const element = text.parentElement;
    if (!element || seen.has(element) || !text.textContent?.trim()) continue;
    seen.add(element);
    if (hidden(element)) continue;
    if (
      element.closest(
        ":disabled, [aria-disabled='true'], fieldset:disabled, option, [aria-hidden='true']",
      )
    )
      continue;
    const style = getComputedStyle(element);
    if (style.webkitTextFillColor && parse(style.webkitTextFillColor)?.a === 0) continue;
    const color = parse(style.color);
    const behind = background(element);
    if (!color || !behind) continue;
    const alpha = color.a * opacity(element);
    const shown = over({ ...color, a: alpha }, behind);
    const [light, dark] = [luminance(shown), luminance(behind)].sort((x, y) => y - x);
    const ratio = (light + 0.05) / (dark + 0.05);
    if (ratio < 4.5)
      findings.push({
        kind: "contrast",
        target: path(element),
        detail: `${ratio.toFixed(2)}:1 for "${text.textContent.trim().slice(0, 32)}"`,
      });
  }
  return findings;
}

/** Runs in the page: the styles that can show focus on the active element, and its resting ones. */
function focusedStyle() {
  const target = document.activeElement;
  if (!target || target === document.body) return null;
  // Settle the focus transition (a ring fading in over 150ms) before reading its end state.
  for (const animation of document.getAnimations())
    if (animation.effect?.getTiming().iterations !== Infinity) animation.finish();
  const style = getComputedStyle(target);
  const box = target.getBoundingClientRect();
  const name = typeof target.className === "string" ? target.className.trim() : "";
  const label =
    target.getAttribute("aria-label") ?? target.textContent?.trim().replace(/\s+/g, " ") ?? null;
  const key = target.dataset.layoutKey ?? `${Math.random()}`;
  target.dataset.layoutKey = key;
  // A field inside a larger control (the composer's message box) may light its frame instead.
  const frames = [target.parentElement, target.parentElement?.parentElement].filter(Boolean);
  const framed = frames.some((frame) => {
    const base = window.__focusBase.get(frame);
    if (!base) return false;
    const now = getComputedStyle(frame);
    return (
      now.boxShadow !== base.shadow ||
      now.borderColor !== base.border ||
      now.backgroundColor !== base.background
    );
  });
  return {
    key,
    base: window.__focusBase.get(target) ?? null,
    framed,
    focus: {
      path: `${target.tagName.toLowerCase()}${name ? `.${name.split(/\s+/).slice(0, 2).join(".")}` : ""}${label ? `[aria-label="${label.slice(0, 40)}"]` : ""}`,
      outline: style.outlineStyle !== "none" && Number.parseFloat(style.outlineWidth) >= 1,
      shadow: style.boxShadow,
      background: style.backgroundColor,
      border: style.borderColor,
      visible: box.width > 1 && box.height > 1 && style.visibility !== "hidden",
    },
  };
}

async function walkFocus(page) {
  const findings = [];
  await page.mouse.move(1, 1);
  await page.evaluate(() => {
    document.activeElement?.blur?.();
    // The blur starts transitions back to rest; read the resting styles, not their first frame.
    for (const animation of document.getAnimations())
      if (animation.effect?.getTiming().iterations !== Infinity) animation.finish();
    window.__focusBase = new Map();
    const focusable = document.querySelectorAll(
      'a[href], button, input, select, textarea, summary, [tabindex]:not([tabindex="-1"])',
    );
    for (const element of focusable)
      for (const node of [element, element.parentElement, element.parentElement?.parentElement]) {
        if (!node || window.__focusBase.has(node)) continue;
        const style = getComputedStyle(node);
        window.__focusBase.set(node, {
          shadow: style.boxShadow,
          background: style.backgroundColor,
          border: style.borderColor,
        });
      }
  });
  const visited = new Set();
  for (let step = 0; step < focusSteps; step++) {
    await page.keyboard.press("Tab");
    const result = await page.evaluate(focusedStyle);
    if (!result) continue;
    if (visited.has(result.key)) break;
    visited.add(result.key);
    const { focus, base, framed } = result;
    if (!focus.visible) continue;
    const ring = focus.outline || (base !== null && focus.shadow !== base.shadow);
    const highlight =
      base !== null && (focus.background !== base.background || focus.border !== base.border);
    if (!ring && !highlight && !framed)
      findings.push({
        kind: "focus",
        target: focus.path,
        detail: "no outline, ring shadow, fill or border change while focused from the keyboard",
      });
  }
  return { findings, stops: visited.size };
}

async function ready() {
  for (let attempt = 0; attempt < 150; attempt++) {
    if (server?.exitCode !== null && server?.exitCode !== undefined) throw new Error(serverOutput);
    try {
      if ((await fetch(url)).ok) return;
    } catch {
      /* Startup has not bound its port yet. */
    }
    await new Promise((done) => setTimeout(done, 100));
  }
  throw new Error(`Preview did not start: ${serverOutput}`);
}

let server;
let serverOutput = "";
const totals = {};
try {
  await mkdir(out, { recursive: true });
  if (!values.server) {
    const vite = resolve(dirname(require.resolve("vite/package.json")), "bin/vite.js");
    server = spawn(
      process.execPath,
      [vite, "--host", "127.0.0.1", "--port", port, "--strictPort"],
      {
        cwd: app,
        windowsHide: true,
        stdio: ["ignore", "pipe", "pipe"],
      },
    );
    for (const stream of [server.stdout, server.stderr])
      stream.on("data", (data) => {
        serverOutput = (serverOutput + data).slice(-8192);
      });
  }
  await ready();
  for (const engine of values.engines.split(",")) {
    const launcher = { chromium, webkit }[engine];
    if (!launcher) throw new Error(`Unsupported engine: ${engine}`);
    const browser = await launcher.launch({ headless: true });
    const report = [];
    try {
      for (const theme of values.themes.split(",")) {
        if (!["light", "dark"].includes(theme)) throw new Error(`Unsupported theme: ${theme}`);
        for (const [width, height] of sizes) {
          const context = await browser.newContext({
            viewport: { width, height },
            reducedMotion: "reduce",
            colorScheme: theme,
          });
          const page = await context.newPage();
          for (const mode of modes) {
            const [preview, view] = mode.split("#");
            // A fixture with its own page (an .html file) is opened by name.
            await page.goto(
              preview.split("?")[0].endsWith(".html")
                ? `${url}/${preview}`
                : `${url}/?preview=${preview}&theme=${theme}${values.glass ? `&glass=acrylic&backdrop=${values.glass}` : ""}`,
            );
            await page.waitForLoadState("networkidle").catch(() => {});
            await page.evaluate(() => document.fonts.ready);
            await page.waitForTimeout(900);
            if (view) {
              await OPENERS[view](page);
              await page.waitForTimeout(400);
            }
            const found = await page.evaluate(probe, { strictGrid: values["strict-grid"] });
            const focus = await walkFocus(page);
            const findings = [...found, ...focus.findings];
            report.push({
              mode,
              theme,
              size: `${width}x${height}`,
              focusStops: focus.stops,
              findings,
            });
            const counts = findings.reduce((all, { kind }) => {
              all[kind] = (all[kind] ?? 0) + 1;
              return all;
            }, {});
            totals[`${engine} ${theme} ${width}x${height} ${mode}`] = counts;
            console.log(
              `${engine} ${theme} ${width}x${height} ${mode}: ${findings.length === 0 ? "clean" : JSON.stringify(counts)} (${focus.stops} focus stops)`,
            );
          }
          await context.close();
        }
      }
    } finally {
      await browser.close();
    }
    await writeFile(resolve(out, `${engine}.json`), `${JSON.stringify(report, null, 2)}\n`);
  }
  const failing = Object.entries(totals).filter(([, counts]) => Object.keys(counts).length);
  if (failing.length) {
    console.error(
      `${failing.length} of ${Object.keys(totals).length} views have layout findings; see target/layout-browser/`,
    );
    process.exitCode = 1;
  } else console.log(`All ${Object.keys(totals).length} views are clean.`);
} finally {
  server?.kill();
}
