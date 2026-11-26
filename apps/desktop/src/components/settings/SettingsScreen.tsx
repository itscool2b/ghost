import { Keycap, Logo } from "@ghost/ui/graphite";
import {
  ArrowLeft,
  BookOpen,
  Bot,
  FileText,
  FlaskConical,
  Globe,
  Info,
  Keyboard,
  type LucideIcon,
  MessageSquare,
  Mic,
  MonitorSmartphone,
  Palette,
  Puzzle,
  Search,
  ShieldCheck,
  SlidersHorizontal,
  Smartphone,
} from "lucide-react";
import {
  type ComponentProps,
  Fragment,
  type ReactNode,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import type { AppInfo, InstructionsTarget } from "../../bindings";
import type { BrowserStore } from "../../lib/browser";
import type { AppStore } from "../../lib/chat/appStore";
import type { Transport } from "../../lib/chat/transport";
import type { DataCleanupApi } from "../../lib/dataCleanup";
import { type DictationApi, dictationApi } from "../../lib/dictation";
import { GLOBAL, type InstructionsApi, instructionsApi } from "../../lib/instructions";
import { revealDataDir } from "../../lib/ipc";
import { COPYRIGHT, OPERATOR, PRIVACY_URL, TERMS_URL } from "../../lib/legal";
import type { MobileStore } from "../../lib/mobile";
import { type ProfileApi, profileApi } from "../../lib/profiles";
import { type RemoteApi, remoteApi } from "../../lib/remote";
import { type GeneralApi, generalApi, type SetupApi, setupApi } from "../../lib/setup";
import { type SpeechApi, speechApi } from "../../lib/speech";
import { type UpdatesApi, updatesApi } from "../../lib/updates";
import { EMPTY_WORKSPACE, type WorkspaceStore } from "../../lib/workspace";
import { PluginLibraryScreen } from "../library/PluginLibraryScreen";
import { UniversalPlugins } from "../library/UniversalPlugins";
import { MobileSettings } from "../mobile/MobileSettings";
import { AgentsScreen } from "../setup/AgentsScreen";
import { DeleteDataDialog } from "./DeleteDataDialog";
import { ExperimentsPage } from "./ExperimentsPage";
import { AppearancePage, ChatsPage, GeneralPage } from "./GeneralPanel";
import { type InstructionsFolder, InstructionsPanel } from "./InstructionsPanel";
import { NoticesDialog } from "./NoticesDialog";
import { ProfilePicker } from "./ProfilePicker";
import { ProjectMemoryPanel } from "./ProjectMemoryPanel";
import { usePreferences } from "./preferences";
import { RemotePanel } from "./RemotePanel";
import { Row } from "./SettingsRow";
import {
  PAGE_GROUPS,
  PAGE_LABELS,
  type Page,
  type SettingEntry,
  searchSettings,
} from "./settingsIndex";
import { UpdatePanel } from "./UpdatePanel";
import { VoicePage } from "./VoicePage";
import "./settings.css";

export { Row } from "./SettingsRow";
export type { Page } from "./settingsIndex";

const ICONS: Readonly<Record<Page, LucideIcon>> = {
  general: SlidersHorizontal,
  appearance: Palette,
  chats: MessageSquare,
  voice: Mic,
  shortcuts: Keyboard,
  agents: Bot,
  plugins: Puzzle,
  browser: Globe,
  mobile: Smartphone,
  instructions: FileText,
  memory: BookOpen,
  experiments: FlaskConical,
  remote: MonitorSmartphone,
  privacy: ShieldCheck,
  about: Info,
};

/** Pages laid out wider than the reading column: lists and libraries with columns of their own. */
const WIDE: ReadonlySet<Page> = new Set(["agents", "plugins"]);

/** Fixture services for the pages Customize used to hold (development previews only). */
export interface CustomizeServices {
  readonly plugins?: Pick<
    ComponentProps<typeof PluginLibraryScreen>,
    "api" | "libraryApi" | "vendorApi" | "mcpApi"
  >;
  readonly instructions?: ComponentProps<typeof InstructionsPanel>["api"];
  readonly memory?: ComponentProps<typeof ProjectMemoryPanel>["api"];
  /** Universal plugins' browser (its status, history and Clear browsing data). */
  readonly browser?: BrowserStore;
  /** Mobile's devices (what is installed and running). */
  readonly mobile?: MobileStore;
}

/** One keycap per key in a chord; a repeated key shows twice. */
function Keys({ keys, mac }: { readonly keys: string; readonly mac: boolean }) {
  const names: Record<string, string> = {
    "⌘": "Command",
    "⇧": "Shift",
    "↑": "Up arrow",
    "↓": "Down arrow",
    "1–9": "1 to 9",
    "`": "Backtick",
    "\\": "Backslash",
  };
  const caps = keys === "Esc again" ? ["Esc", "Esc"] : keys.split(" ");
  return (
    <span className="settings-keys">
      {caps.map((key, index) => (
        // biome-ignore lint/suspicious/noArrayIndexKey: a chord can repeat a key, so position is its identity.
        <Fragment key={index}>
          <Keycap label={names[key]}>{mac && key === "Shift" ? "⇧" : key}</Keycap>
        </Fragment>
      ))}
    </span>
  );
}

const noSubscribe = () => () => {};
const emptyWorkspace = () => EMPTY_WORKSPACE;

/**
 * Settings: one screen for the app, the agents and what they use, remote access and the person's
 * data, with a search across every page (owner, 4 November 2026: "make the settings more
 * intuitive"). Customize and Agents open here on their pages.
 */
export function SettingsScreen({
  info,
  onBack,
  onSetup,
  onTour,
  onDiagnostics,
  setup = setupApi,
  general = generalApi,
  updates = updatesApi,
  dictation = dictationApi,
  speech = speechApi,
  remote = remoteApi,
  profiles = profileApi,
  reveal = revealDataDir,
  initialPage = "general",
  navigationId = "settings",
  backLabel = "Back to chats",
  instructions = instructionsApi,
  instructionsTarget,
  instructionFolders,
  chatLabel,
  instructionsOpenOnLoad,
  workspace,
  projectId = null,
  activeChatId,
  services,
  agents,
  dataCleanup,
  deleteDataOpenOnLoad = false,
}: {
  readonly info: AppInfo;
  readonly onBack: () => void;
  readonly onSetup: () => void;
  /** Takes the tour: back to the chats, where the wing flies it. */
  readonly onTour?: () => void;
  readonly onDiagnostics: () => void;
  readonly setup?: SetupApi;
  readonly general?: GeneralApi;
  readonly updates?: UpdatesApi;
  readonly dictation?: DictationApi;
  /** Reading aloud, for the Voice page; previews pass a synthetic one. */
  readonly speech?: SpeechApi;
  readonly remote?: RemoteApi;
  /** The data folder's profiles, for Data & privacy's list; previews pass a synthetic one. */
  readonly profiles?: ProfileApi;
  readonly reveal?: () => Promise<void>;
  readonly initialPage?: Page;
  readonly navigationId?: string;
  readonly backLabel?: string;
  readonly instructions?: InstructionsApi;
  /** The scope Instructions opens on: global, a project folder, or a chat's own folder. */
  readonly instructionsTarget?: InstructionsTarget;
  /** Instructions' folder choices; without them, the workspace's project folders. */
  readonly instructionFolders?: readonly InstructionsFolder[];
  readonly chatLabel?: string;
  /** Development previews only (see InstructionsPanel). */
  readonly instructionsOpenOnLoad?: ComponentProps<typeof InstructionsPanel>["openOnLoad"];
  /** Projects and chats for Plugins & MCP, Instructions and Project memory. */
  readonly workspace?: WorkspaceStore;
  /** The project those pages open on, when Settings opened from one. */
  readonly projectId?: string | null;
  readonly activeChatId?: string;
  readonly services?: CustomizeServices;
  /** The agents' store and transport for the Agents page; without them it is not offered. */
  readonly agents?: { readonly app: AppStore; readonly transport: Transport };
  /** "Delete all Ghost data"; previews pass a synthetic one. */
  readonly dataCleanup?: DataCleanupApi;
  /** Development previews only: Data & privacy opens with the review already showing. */
  readonly deleteDataOpenOnLoad?: boolean;
}) {
  const [page, setPage] = useState<Page>(initialPage);
  const [visited, setVisited] = useState<ReadonlySet<Page>>(() => new Set([initialPage]));
  const [query, setQuery] = useState("");
  const [found, setFound] = useState<{ readonly entry: SettingEntry; readonly id: number } | null>(
    null,
  );
  const [notices, setNotices] = useState(false);
  const [deleting, setDeleting] = useState(deleteDataOpenOnLoad);
  const prefs = usePreferences(setup);
  const heading = useRef<HTMLHeadingElement>(null);
  const main = useRef<HTMLElement>(null);
  const previousFocus = useRef<Element | null>(null);
  const restoreFocus = useRef<number | null>(null);
  const organization = useSyncExternalStore(
    workspace?.subscribe ?? noSubscribe,
    workspace?.getSnapshot ?? emptyWorkspace,
    workspace?.getSnapshot ?? emptyWorkspace,
  );
  const projects = organization.data?.projects;
  const folders =
    instructionFolders ??
    (projects ?? []).flatMap((project) =>
      project.folders.map((folder) => ({
        id: folder.id,
        label:
          project.folders.length > 1 ? `${project.name} / ${folder.display_name}` : project.name,
      })),
    );
  // Instructions open on the scope Settings was opened for: a chat, a project's first folder,
  // or every agent.
  const [target, setTarget] = useState<InstructionsTarget | null>(instructionsTarget ?? null);
  useEffect(() => {
    void navigationId;
    // A new entry (a chat's Instructions) selects its page; the pages visited before keep what
    // was typed in them.
    setPage(initialPage);
    setVisited((current) => new Set([...current, initialPage]));
    setTarget(instructionsTarget ?? null);
  }, [navigationId, initialPage, instructionsTarget]);
  useEffect(() => {
    if (target || (projectId && !organization.data)) return;
    const folder = (projects ?? [])
      .find((project) => project.id === projectId)
      ?.folders.find((entry) => entry.folder_state === "ok");
    setTarget(folder ? { type: "folder", folder_id: folder.id } : GLOBAL);
  }, [target, projects, projectId, organization.data]);
  useLayoutEffect(() => {
    if (restoreFocus.current !== null) {
      cancelAnimationFrame(restoreFocus.current);
      restoreFocus.current = null;
    } else {
      previousFocus.current = document.activeElement;
      heading.current?.focus();
    }
    return () => {
      restoreFocus.current = requestAnimationFrame(() => {
        restoreFocus.current = null;
        const previous = previousFocus.current;
        if (previous instanceof HTMLElement && previous.isConnected) previous.focus();
      });
    };
  }, []);
  useEffect(() => {
    const dismiss = (event: KeyboardEvent) => {
      if (
        event.key === "Escape" &&
        !event.defaultPrevented &&
        !document.querySelector('dialog[open], [role="dialog"]')
      ) {
        event.preventDefault();
        onBack();
      }
    };
    document.addEventListener("keydown", dismiss);
    return () => document.removeEventListener("keydown", dismiss);
  }, [onBack]);
  const open = (next: Page, entry?: SettingEntry) => {
    setPage(next);
    setVisited((current) => (current.has(next) ? current : new Set([...current, next])));
    setFound(entry ? { entry, id: Date.now() } : null);
    if (!entry)
      requestAnimationFrame(() => {
        main.current?.scrollTo({ top: 0 });
        heading.current?.focus();
      });
  };
  // A search result opens its page with its row in view, marked a moment, and its control focused.
  useEffect(() => {
    if (!found) return;
    const entry = found.entry;
    const frame = requestAnimationFrame(() => {
      const pane = main.current?.querySelector(`[data-settings-page="${entry.page}"]`);
      const row = [...(pane?.querySelectorAll<HTMLElement>("[data-setting]") ?? [])].find(
        (node) => node.dataset.setting === entry.title,
      );
      if (!row) {
        main.current?.scrollTo({ top: 0 });
        heading.current?.focus();
        return;
      }
      row.scrollIntoView({ block: "center" });
      row.dataset.found = "";
      const control = row.querySelector<HTMLElement>(
        "input:not([disabled]), select:not([disabled]), button:not([disabled])",
      );
      (control ?? heading.current)?.focus({ preventScroll: true });
    });
    const clear = window.setTimeout(() => {
      for (const node of main.current?.querySelectorAll<HTMLElement>("[data-found]") ?? [])
        delete node.dataset.found;
    }, 1600);
    return () => {
      cancelAnimationFrame(frame);
      window.clearTimeout(clear);
    };
  }, [found]);
  const results = useMemo(() => searchSettings(query), [query]);
  const offered = (value: Page) => value !== "agents" || agents !== undefined;
  const mac = info.os === "macos";
  const key = mac ? "⌘" : "Ctrl";
  const shortcuts = [
    ["New chat", `${key} N`],
    ["Open chat 1 to 9 in the sidebar", `${key} 1–9`],
    ["Search chats, projects and files", `${key} K`],
    ["Find in chat", `${key} F`],
    ["Next or previous match", "Enter / Shift Enter"],
    ["Open context panel", `${key} J`],
    ["Show or hide the terminal", `${key} \``],
    ["New terminal", `${key} Shift \``],
    ["Leave a terminal (other keys go to its program)", "Ctrl Tab"],
    ["Collapse the sidebar", `${key} \\`],
    ["Choose agent, model and effort", `${key} .`],
    ["Open settings", `${key} ,`],
    ["Send message", "Enter"],
    ["New line", "Shift Enter"],
    ["Finish dictation", "Enter"],
    ["Cancel dictation", "Esc"],
    ["Interrupt agent", "Esc"],
    ["Force stop after interrupt", "Esc again"],
    ["Move through a menu", "↑ ↓"],
    ["Close a menu or dialog", "Esc"],
  ];
  const title = PAGE_LABELS[page];
  const pane = (id: Page, content: ReactNode) =>
    visited.has(id) ? (
      <div key={id} className="settings-page" data-settings-page={id} hidden={page !== id}>
        {content}
      </div>
    ) : null;
  return (
    <div className="settings-shell g-root" data-os={info.os}>
      <aside className="settings-sidebar" aria-label="Settings navigation">
        <div className="settings-drag" data-tauri-drag-region="" />
        <Logo size={32} className="settings-logo" />
        <button type="button" className="settings-back" onClick={onBack}>
          <ArrowLeft size={16} aria-hidden="true" /> {backLabel}
        </button>
        <h2>Settings</h2>
        <div className="settings-search">
          <Search size={14} className="settings-search-icon" aria-hidden="true" />
          <input
            className="settings-search-input"
            type="search"
            aria-label="Search settings"
            placeholder="Search settings"
            value={query}
            onChange={(event) => setQuery(event.currentTarget.value)}
            onKeyDown={(event) => {
              if (event.key === "Escape" && query) {
                // Clears the search; the next Esc leaves Settings.
                event.preventDefault();
                setQuery("");
              } else if (event.key === "Enter" && results[0]) {
                event.preventDefault();
                const first = results[0];
                setQuery("");
                open(first.page, first);
              }
            }}
          />
        </div>
        {query ? (
          <nav aria-label="Matching settings" className="settings-results">
            {results.filter((entry) => offered(entry.page)).length ? (
              results
                .filter((entry) => offered(entry.page))
                .map((entry) => (
                  <button
                    key={`${entry.page}:${entry.title}`}
                    type="button"
                    onClick={() => {
                      setQuery("");
                      open(entry.page, entry);
                    }}
                  >
                    <span className="settings-nav-label">{entry.title}</span>
                    {entry.title === PAGE_LABELS[entry.page] ? null : (
                      <span className="settings-result-page">{PAGE_LABELS[entry.page]}</span>
                    )}
                  </button>
                ))
            ) : (
              <p className="settings-no-results" role="status">
                No setting matches “{query}”.
              </p>
            )}
          </nav>
        ) : (
          <nav aria-label="Settings sections">
            {PAGE_GROUPS.map((group, index) => (
              <Fragment key={group[0]}>
                {index ? <hr /> : null}
                {group.filter(offered).map((value) => {
                  const Icon = ICONS[value];
                  return (
                    <button
                      key={value}
                      type="button"
                      aria-current={page === value ? "page" : undefined}
                      onClick={() => open(value)}
                    >
                      <Icon size={16} aria-hidden="true" />
                      <span className="settings-nav-label">{PAGE_LABELS[value]}</span>
                    </button>
                  );
                })}
              </Fragment>
            ))}
          </nav>
        )}
      </aside>
      <main className="settings-main" ref={main} data-page={page}>
        <nav className="settings-breadcrumb" aria-label="Settings breadcrumbs">
          <div className="settings-breadcrumb-drag" data-tauri-drag-region="" aria-hidden="true" />
          <span>Settings</span>
          <span aria-hidden="true">/</span>
          <span aria-current="page">{title}</span>
        </nav>
        <div className="settings-content" data-wide={WIDE.has(page) ? "" : undefined}>
          {page === "about" ? (
            <header className="settings-identity">
              <Logo size={64} />
              <div>
                <h1 ref={heading} tabIndex={-1}>
                  <span className="sr-only">About </span>Ghost ADE
                </h1>
                <p>Version {info.app_version}</p>
              </div>
            </header>
          ) : (
            <h1 ref={heading} tabIndex={-1}>
              {title}
            </h1>
          )}
          {pane(
            "general",
            <GeneralPage
              info={info}
              prefs={prefs}
              lists={general}
              onSetup={onSetup}
              onTour={onTour}
            />,
          )}
          {pane("appearance", <AppearancePage prefs={prefs} />)}
          {pane("chats", <ChatsPage prefs={prefs} editors={general.editors} />)}
          {pane("voice", <VoicePage api={dictation} speech={speech} prefs={prefs} />)}
          {pane(
            "shortcuts",
            <dl className="settings-group settings-shortcuts" data-setting="Shortcuts">
              {shortcuts.map(([action, keys]) => (
                <div key={action}>
                  <dt>{action}</dt>
                  <dd>
                    <Keys keys={keys ?? ""} mac={mac} />
                  </dd>
                </div>
              ))}
            </dl>,
          )}
          {agents
            ? pane(
                "agents",
                <AgentsScreen
                  embedded
                  app={agents.app}
                  transport={agents.transport}
                  onBack={onBack}
                  onSetup={onSetup}
                />,
              )
            : null}
          {pane(
            "plugins",
            <PluginLibraryScreen
              {...services?.plugins}
              embedded
              titled={false}
              initialProjectId={projectId}
              projects={projects ?? []}
              activeChatId={activeChatId}
              onBack={onBack}
              onAgents={() => open("agents")}
            />,
          )}
          {pane(
            "browser",
            <>
              <p className="settings-lead">
                The browser beside your chats is part of Ghost, and so are the tools agents get to
                use it. It is on unless you turn it off here.
              </p>
              <UniversalPlugins prefs={prefs} os={info.os} browser={services?.browser} />
            </>,
          )}
          {pane(
            "mobile",
            <>
              <p className="settings-lead">
                The phone beside your chats is part of Ghost, and so are the tools agents get to use
                it. It is on unless you turn it off here.
              </p>
              <MobileSettings prefs={prefs} mobile={services?.mobile} />
            </>,
          )}
          {pane(
            "instructions",
            <>
              <p className="settings-lead">
                Tell agents how to work. Review the files and who reads them before applying
                changes.
              </p>
              {target ? (
                <InstructionsPanel
                  api={services?.instructions ?? instructions}
                  setup={setup}
                  initialTarget={target}
                  folders={folders}
                  chatLabel={chatLabel}
                  openOnLoad={instructionsOpenOnLoad}
                />
              ) : (
                <p role="status">Loading instruction scope…</p>
              )}
            </>,
          )}
          {pane(
            "memory",
            <ProjectMemoryPanel
              titled={false}
              api={services?.memory}
              setup={setup}
              projects={projects ?? []}
              initialProjectId={projectId ?? undefined}
            />,
          )}
          {pane("experiments", <ExperimentsPage prefs={prefs} />)}
          {pane("remote", <RemotePanel api={remote} />)}
          {pane(
            "privacy",
            <>
              <div className="settings-group">
                <Row
                  title="Data folder"
                  detail={
                    info.data_dir ? (
                      <span className="settings-mono">{info.data_dir}</span>
                    ) : (
                      "Chats stay on this device."
                    )
                  }
                >
                  <button
                    className="g-button g-button-outline g-button-28"
                    type="button"
                    disabled={!info.data_dir}
                    onClick={() => void reveal().catch(() => {})}
                  >
                    Open folder
                  </button>
                </Row>
                <Row
                  title="What stays on this device"
                  detail="Chats, drafts and attachments stay in the data folder. Dictation's audio is never saved or uploaded. Your agents talk to their own services as they always do."
                />
                <Row
                  title="Delete all Ghost data"
                  detail="As if Ghost was never installed. Your agents and repositories stay."
                >
                  <button
                    className="g-button g-button-outline g-button-28"
                    type="button"
                    data-testid="settings-delete-data"
                    onClick={() => setDeleting(true)}
                  >
                    Review
                  </button>
                </Row>
              </div>
              {/* Which profile is open matters only once the data folder holds more than one. */}
              <ProfilePicker activeId={info.profile?.id ?? null} api={profiles} />
            </>,
          )}
          {pane(
            "about",
            <>
              <div className="settings-group">
                <UpdatePanel api={updates} />
                <Row title="Diagnostics" detail="Startup details and local troubleshooting tools.">
                  <button
                    className="g-button g-button-outline g-button-28"
                    type="button"
                    onClick={onDiagnostics}
                  >
                    Open
                  </button>
                </Row>
                <Row
                  title="Third-party notices"
                  detail="Licenses for the open source software inside Ghost."
                >
                  <button
                    className="g-button g-button-outline g-button-28"
                    type="button"
                    onClick={() => setNotices(true)}
                  >
                    View
                  </button>
                </Row>
                <Row title="Terms of Service" detail="The terms for using Ghost, on onghost.dev.">
                  <a
                    className="g-button g-button-outline g-button-28"
                    href={TERMS_URL}
                    target="_blank"
                    rel="noreferrer noopener"
                  >
                    Open
                  </a>
                </Row>
                <Row
                  title="Privacy Policy"
                  detail="What Ghost keeps on your computer and what goes online."
                >
                  <a
                    className="g-button g-button-outline g-button-28"
                    href={PRIVACY_URL}
                    target="_blank"
                    rel="noreferrer noopener"
                  >
                    Open
                  </a>
                </Row>
              </div>
              <section className="settings-section" aria-labelledby="settings-this-device">
                <h2 id="settings-this-device">This device</h2>
                <div className="settings-group">
                  <Row
                    title={
                      info.os === "macos" ? "macOS" : info.os === "linux" ? "Linux" : "Windows"
                    }
                    setting="This device"
                    detail={
                      <>
                        {info.os_version} · <span className="settings-mono">{info.arch}</span>
                      </>
                    }
                  />
                </div>
              </section>
              <p className="settings-lead settings-legal" data-testid="settings-copyright">
                {COPYRIGHT} Ghost ADE is a product of {OPERATOR}. Its source code is available under
                the MIT License.
              </p>
            </>,
          )}
          {notices ? <NoticesDialog onClose={() => setNotices(false)} /> : null}
          {deleting ? (
            <DeleteDataDialog api={dataCleanup} onClose={() => setDeleting(false)} />
          ) : null}
        </div>
      </main>
    </div>
  );
}
