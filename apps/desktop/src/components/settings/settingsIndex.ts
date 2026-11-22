/**
 * Settings' pages, in the order and groups the navigation shows them (owner's plan, 4 November
 * 2026): the app, then agents and what they use, then remote access and the person's data. The
 * search reads the index below; check-settings.mjs keeps it in step with the rows the pages draw.
 */
export type Page =
  | "general"
  | "appearance"
  | "chats"
  | "voice"
  | "shortcuts"
  | "agents"
  | "plugins"
  | "browser"
  | "mobile"
  | "instructions"
  | "memory"
  | "experiments"
  | "remote"
  | "privacy"
  | "about";

export const PAGE_GROUPS: readonly (readonly Page[])[] = [
  ["general", "appearance", "chats", "voice", "shortcuts"],
  ["agents", "plugins", "browser", "mobile", "instructions", "memory", "experiments"],
  ["remote", "privacy", "about"],
];

export const PAGE_LABELS: Readonly<Record<Page, string>> = {
  general: "General",
  appearance: "Appearance",
  chats: "Chats",
  voice: "Voice",
  shortcuts: "Shortcuts",
  agents: "Agents",
  plugins: "Plugins & MCP",
  browser: "Browser",
  mobile: "Mobile",
  instructions: "Instructions",
  memory: "Project memory",
  experiments: "Experiments",
  remote: "Remote access",
  privacy: "Data & privacy",
  about: "About",
};

/**
 * A setting the search can find: its page, the row's own title, and other words for it.
 */
export interface SettingEntry {
  readonly page: Page;
  readonly title: string;
  readonly words?: string;
}

export const SETTINGS_INDEX: readonly SettingEntry[] = [
  {
    page: "experiments",
    title: "Coordinated projects",
    words: "coordinator workers subagents board tasks context routines experimental labs beta",
  },
  { page: "general", title: "Agent", words: "default new chat harness" },
  { page: "general", title: "Model", words: "default new chat" },
  { page: "general", title: "Thinking", words: "effort reasoning level default" },
  { page: "general", title: "Autonomy", words: "permissions ask auto-edit full auto approve" },
  { page: "general", title: "Notifications", words: "alerts background finished" },
  { page: "general", title: "Sounds", words: "audio chime tone finished needs you" },
  { page: "general", title: "Keep awake", words: "sleep power" },
  {
    page: "general",
    title: "Check for updates automatically",
    words: "update upgrade version release new",
  },
  {
    page: "general",
    title: "Keep running in the background",
    words: "tray menu bar close quit window",
  },
  {
    page: "general",
    title: "Welcome and setup",
    words: "onboarding run setup again first project",
  },
  { page: "general", title: "Tour", words: "walkthrough guide show me around wing help" },
  { page: "voice", title: "Read aloud", words: "speak speech tts text to speech listen audio" },
  { page: "voice", title: "Voice", words: "speaker narrator accent read aloud sound" },
  { page: "appearance", title: "Appearance", words: "dark light system mode" },
  { page: "appearance", title: "Theme", words: "palette colours colors claude codex grok cursor" },
  { page: "appearance", title: "Motion", words: "animation reduced off" },
  { page: "appearance", title: "Text size", words: "font density compact comfortable" },
  {
    page: "appearance",
    title: "Terminal font",
    words: "monospace typeface kitty alacritty ghostty foot nerd font system terminal",
  },
  { page: "appearance", title: "Terminal size", words: "font size points pixels terminal text" },
  {
    page: "chats",
    title: "Open new chats in",
    words: "terminal tui gui interface claude code codex own cli new chat default",
  },
  {
    page: "chats",
    title: "Keep steps open",
    words: "worked for fold folding thinking tools interim messages",
  },
  { page: "chats", title: "Automatic checkpoints", words: "undo files changes snapshot" },
  {
    page: "chats",
    title: "Open files in",
    words: "editor cursor vs code visual studio zed jetbrains intellij line links",
  },
  { page: "voice", title: "Dictation", words: "speech voice talk parakeet transcribe" },
  { page: "voice", title: "Microphone", words: "input audio device mic" },
  { page: "voice", title: "Test your microphone", words: "mic level check speech" },
  { page: "shortcuts", title: "Shortcuts", words: "keyboard keys hotkeys" },
  { page: "agents", title: "Agents", words: "install update claude code codex pi cursor harness" },
  {
    page: "plugins",
    title: "Plugins & MCP",
    words: "mcp servers marketplaces skills extensions tools",
  },
  {
    page: "browser",
    title: "Browser use",
    words: "browser universal plugin built-in web pages annotate agents tools turn off",
  },
  {
    page: "browser",
    title: "Tools for agents",
    words: "browser tools exposed agents navigate click screenshot",
  },
  { page: "browser", title: "Save passwords", words: "browser sign-ins sign in autofill" },
  { page: "browser", title: "Browsing history", words: "browser pages visited" },
  {
    page: "browser",
    title: "Clear browsing data",
    words: "browser cookies cache history passwords sign out",
  },
  {
    page: "mobile",
    title: "Mobile",
    words:
      "mobile universal plugin built-in phone app android emulator device agents tools turn off",
  },
  {
    page: "mobile",
    title: "Device tools for agents",
    words: "mobile phone tools agents tap swipe screenshot install launch logs",
  },
  { page: "mobile", title: "Android", words: "mobile sdk emulator adb set up download google" },
  { page: "instructions", title: "Instructions", words: "agents.md claude.md rules prompt" },
  { page: "instructions", title: "Applies to", words: "scope folder project global chat" },
  {
    page: "instructions",
    title: "Copy into new worktree chats",
    words: "worktree gitignored local instruction files",
  },
  { page: "memory", title: "Project memory", words: "context notes shared" },
  {
    page: "memory",
    title: "Let agents edit memory",
    words: "agent edits write update read only",
  },
  {
    page: "remote",
    title: "Remote access",
    words: "phone mobile browser relay connect away sessions",
  },
  { page: "remote", title: "Pair a device", words: "phone qr code link scan remote" },
  { page: "remote", title: "Devices", words: "paired phone browser remove unpair remote" },
  { page: "remote", title: "Link a bot", words: "grok bot mcp connector assistant remote" },
  { page: "remote", title: "Bots", words: "linked bot remove level full control remote" },
  { page: "privacy", title: "Data folder", words: "files location storage" },
  { page: "privacy", title: "What stays on this device", words: "privacy local audio" },
  {
    page: "privacy",
    title: "Delete all o7 data",
    words: "reset remove uninstall erase",
  },
  { page: "about", title: "Updates", words: "version upgrade release" },
  { page: "about", title: "Diagnostics", words: "logs troubleshooting startup" },
  { page: "about", title: "Third-party notices", words: "licenses open source" },
  { page: "about", title: "This device", words: "operating system version windows macos" },
];

/**
 * The settings matching `query`, best first: titles that start with it, then titles containing
 * it, then other words or the page's name. Every word of the query must match somewhere.
 */
export function searchSettings(query: string): SettingEntry[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (!words.length) return [];
  const scored: { entry: SettingEntry; score: number; index: number }[] = [];
  SETTINGS_INDEX.forEach((entry, index) => {
    const title = entry.title.toLowerCase();
    const haystack = `${title} ${entry.words ?? ""} ${PAGE_LABELS[entry.page].toLowerCase()}`;
    if (!words.every((word) => haystack.includes(word))) return;
    const phrase = words.join(" ");
    const score = title.startsWith(phrase) ? 0 : title.includes(phrase) ? 1 : 2;
    scored.push({ entry, score, index });
  });
  return scored.sort((a, b) => a.score - b.score || a.index - b.index).map((match) => match.entry);
}
