import { usePrefersReducedMotion } from "@o7/ui";
import { BOTS, type BotId, BotMark, botNamed, Keycap, Wing, Wordmark } from "@o7/ui/graphite";
import { listen } from "@tauri-apps/api/event";
import { ArrowRight, Check, FolderOpen, GitBranch, RefreshCw } from "lucide-react";
import {
  type CSSProperties,
  type RefObject,
  useEffect,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import type {
  AgentEntry,
  ProjectSummary,
  SetupAction,
  SetupSnapshot,
  SetupStep,
} from "../../bindings";
import { realAgents } from "../../lib/chat/agentLineup";
import type { AppStore } from "../../lib/chat/appStore";
import type { Transport } from "../../lib/chat/transport";
import { errorMessage } from "../../lib/errors";
import { inTauri } from "../../lib/ipc";
import { type RemoteApi, remoteApi } from "../../lib/remote";
import { type SetupApi, setMotionPreference, setupApi } from "../../lib/setup";
import { type CloneApi, cloneApi } from "../../lib/setup-actions/api";
import type { CloneDraft } from "../../lib/setup-actions/clone";
import { FLIGHT_MS } from "../../lib/tour/flight";
import type { WorkspaceStore } from "../../lib/workspace";
import { projectName } from "../chat/ChatList";
import { type BotChoice, BotDialog } from "../settings/RemotePanel";
import { AgentRoster } from "./AgentsScreen";
import { CloneBody, cloneHeading, useClone } from "./CloneProject";
import { type IntroMode, SetupIntro } from "./SetupIntro";
import { type MapFocus, SetupMap } from "./SetupMap";
import "../tour/wing.css";
import "./setup.css";
import { Select } from "../Select";

/**
 * A page of setup. All but one are saved steps; linking an assistant is a page of the agents
 * step (owner, 21 November 2026), so a setup left there resumes at the agents and nothing new is
 * saved for a copy of the app from before it to misread.
 */
type Page = SetupStep | "bots";

/**
 * Four pages, each skippable: Continue never requires anything. Motion is not one of them: Full
 * already follows the system's reduced motion, and Settings changes it.
 */
const STEPS: readonly { id: Page; label: string }[] = [
  { id: "welcome", label: "Welcome" },
  { id: "agents", label: "Your agents" },
  { id: "bots", label: "Your assistant" },
  { id: "project", label: "First project" },
];

/** The saved step a page belongs to. */
const saves = (page: Page): SetupStep => (page === "bots" ? "agents" : page);

/** How long the intro holds before setup shows: the swim on a first launch, a beat later. */
const INTRO_MS = { live: 1800, still: 240, reducedFirst: 300 } as const;
/** The intro's mark fades over the water (800ms), or quickly once someone skips it. */
const FADE_MS = { slow: 800, fast: 240 } as const;

/** Setups saved before motion left the steps resume at the step that followed it. */
function current(step: SetupStep | undefined): SetupStep {
  if (step === undefined) return "welcome";
  return step === "motion" ? "project" : step;
}

export function Onboarding({
  app,
  transport,
  workspace,
  restart = false,
  onDone,
  onDemo,
  api = setupApi,
  remote = remoteApi,
  clone,
}: {
  readonly app: AppStore;
  readonly transport: Transport;
  readonly workspace?: WorkspaceStore;
  readonly restart?: boolean;
  readonly onDone: () => void;
  readonly onDemo: () => void;
  readonly api?: SetupApi;
  /** Remote access, for the page that links an assistant. */
  readonly remote?: RemoteApi;
  /** The clone's commands, and a form to open the project step on (previews). */
  readonly clone?: { readonly api?: CloneApi; readonly draft?: CloneDraft };
}) {
  const reduced = usePrefersReducedMotion();
  const agentsState = useSyncExternalStore(app.subscribe, app.getSnapshot, app.getSnapshot);
  const [saved, setSaved] = useState<SetupSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [cinematic, setCinematic] = useState(!restart);
  const [skipped, setSkipped] = useState(false);
  const [introGone, setIntroGone] = useState(restart);
  const [project, setProject] = useState<ProjectSummary | null>(null);
  const [folder, setFolder] = useState("");
  const [name, setName] = useState("");
  const [existing, setExisting] = useState<readonly ProjectSummary[]>([]);
  // The project step's clone, in place of the folder choice until it is added or left.
  const [cloning, setCloning] = useState(clone?.draft !== undefined);
  const heading = useRef<HTMLHeadingElement>(null);
  const started = useRef<Promise<SetupSnapshot> | null>(null);
  const lock = useRef(false);
  // Linking an assistant: the page is open, the dialog is open for one, and the links there are.
  const [onBots, setOnBots] = useState(false);
  const [linking, setLinking] = useState<BotChoice | null>(null);
  const [links, setLinks] = useState<readonly string[]>([]);
  const agents = agentsState.agents ?? [];
  const saving = current(saved?.preferences.step);
  const step: Page = saving === "agents" && onBots ? "bots" : saving;
  const index = STEPS.findIndex((item) => item.id === step);
  // The wing on the progress line: which way the last move went, so the page glides in from
  // that side, and the wing flies back nose first before it turns to face what is next.
  const [travel, setTravel] = useState({ from: index, to: index });
  const [facing, setFacing] = useState<"forward" | "back">("forward");
  if (travel.to !== index) {
    setTravel({ from: travel.to, to: index });
    setFacing(index < travel.to ? "back" : "forward");
  }
  const back = travel.to < travel.from;
  useEffect(() => {
    if (facing !== "back") return;
    const timer = setTimeout(() => setFacing("forward"), reduced ? 0 : FLIGHT_MS);
    return () => clearTimeout(timer);
  }, [facing, reduced]);
  useEffect(() => {
    if (step !== "bots") return;
    let live = true;
    // A setup that is run again shows the links it already has. Nothing is turned on by looking.
    void remote.status().then(
      (status) => {
        if (live) setLinks(status.bots.map((bot) => bot.name));
      },
      () => {
        // Remote access cannot be read (a preview without it): the page still offers the four.
      },
    );
    return () => {
      live = false;
    };
  }, [step, remote]);
  useEffect(() => {
    app.start();
    return () => app.stop();
  }, [app]);
  useEffect(() => {
    let live = true;
    // A StrictMode remount joins the first request, including an explicit Restart mutation.
    started.current ??= api
      .snapshot()
      .then((value) =>
        restart ? api.apply(value.preferences.revision, { type: "restart" }) : value,
      );
    void started.current.then(
      (value) => {
        if (!live) return;
        setMotionPreference(value.preferences.motion);
        setSaved(value);
      },
      (reason) => {
        if (live) {
          setError(errorMessage(reason));
          setCinematic(false);
        }
      },
    );
    return () => {
      live = false;
    };
  }, [api, restart]);
  useEffect(() => {
    if (!saved || !cinematic) return;
    // A first launch plays the dissolve; later launches and reduced motion hold the formed mark.
    const hold = saved.cold_start
      ? reduced
        ? INTRO_MS.reducedFirst
        : INTRO_MS.live
      : reduced
        ? 0
        : INTRO_MS.still;
    if (hold === 0) {
      setCinematic(false);
      return;
    }
    const timer = setTimeout(() => setCinematic(false), hold);
    return () => clearTimeout(timer);
  }, [saved, cinematic, reduced]);
  useEffect(() => {
    if (cinematic || introGone) return;
    if (reduced) {
      setIntroGone(true);
      return;
    }
    const timer = setTimeout(() => setIntroGone(true), skipped ? FADE_MS.fast : FADE_MS.slow);
    return () => clearTimeout(timer);
  }, [cinematic, introGone, reduced, skipped]);
  useEffect(() => {
    if (saved?.preferences.completed && !cinematic) onDone();
  }, [saved, cinematic, onDone]);
  // biome-ignore lint/correctness/useExhaustiveDependencies: Moving to a different step must announce its heading.
  useEffect(() => {
    // The clone form opens on its address field instead.
    if (!cinematic && !cloning) heading.current?.focus();
  }, [step, cinematic, cloning]);
  useEffect(() => {
    if (!cinematic) return;
    const skip = (event: KeyboardEvent) => {
      if (event.key === "Escape" || event.key === "Enter") {
        setSkipped(true);
        setCinematic(false);
      }
    };
    window.addEventListener("keydown", skip);
    return () => window.removeEventListener("keydown", skip);
  }, [cinematic]);
  useEffect(() => {
    if (step !== "project" || !workspace) return;
    let live = true;
    void workspace.api.snapshot().then(
      (value) => {
        if (!live) return;
        setExisting(value.projects);
        setProject(
          value.projects.find((item) => item.id === saved?.preferences.first_project_id) ?? null,
        );
      },
      (reason) => {
        if (live) setError(errorMessage(reason));
      },
    );
    return () => {
      live = false;
    };
  }, [step, workspace, saved?.preferences.first_project_id]);
  useEffect(() => {
    if (step !== "project" || cloning || !inTauri()) return;
    let live = true;
    let unlisten: (() => void) | undefined;
    void listen<{ paths: string[] }>("tauri://drag-drop", (event) => {
      if (!live || lock.current) return;
      if (event.payload.paths.length !== 1) {
        setError("Drop one project folder at a time.");
        return;
      }
      const path = event.payload.paths[0];
      if (path) {
        setFolder(path);
        setName(projectName(path));
        setProject(null);
      }
    }).then(
      (stop) => {
        if (live) unlisten = stop;
        else stop();
      },
      (reason) => {
        if (live) setError(errorMessage(reason));
      },
    );
    return () => {
      live = false;
      unlisten?.();
    };
  }, [step, cloning]);

  const run = async (operation: () => Promise<void>) => {
    if (lock.current) return;
    lock.current = true;
    setBusy(true);
    setError(null);
    try {
      await operation();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      lock.current = false;
      setBusy(false);
    }
  };
  const apply = async (action: SetupAction) => {
    if (!saved) return;
    const value = await api.apply(saved.preferences.revision, action);
    setSaved(value);
    setMotionPreference(value.preferences.motion);
  };
  /** Opens a page: its saved step, and whether the agents step shows its second page. */
  const open = (page: Page) =>
    void run(async () => {
      if (saves(page) !== saving) await apply({ type: "visit", step: saves(page) });
      setOnBots(page === "bots");
    });
  const advance = () => {
    const next = STEPS[index + 1]?.id;
    if (next === undefined) void run(() => apply({ type: "finish" }));
    else open(next);
  };
  const pick = () =>
    void run(async () => {
      const path = await transport.pickFolder();
      if (path) {
        setFolder(path);
        setName(projectName(path));
        setProject(null);
      }
    });
  const addProject = (path: string, title: string, created: ProjectSummary | null) =>
    void run(async () => {
      if (!workspace || !saved) return;
      // Keep the created project in memory if saving setup subsequently fails. Retrying cannot
      // create a second project, and the existing-project list makes it recoverable after restart.
      const selected =
        created ??
        (await workspace.api.createProject({
          folder: path,
          project: { name: title, color: null, default_agent: null, default_model: null },
        }));
      setProject(selected);
      await apply({ type: "project", project_id: selected.id });
      await workspace.refresh();
    });
  const saveProject = () => addProject(folder, name, project);
  // A finished clone becomes the first project at once: the folder is new, so there is nothing
  // to choose. A failure leaves the folder and name chosen, with Add project to retry.
  const cloned = (path: string, title: string) => {
    setFolder(path);
    setName(title);
    setProject(null);
    setCloning(false);
    addProject(path, title, null);
  };
  const roster = primaryAgents(agents);
  const installed = roster.filter((agent) => agent.availability.state === "ready").length;
  const looking = roster.filter((agent) => agent.availability.state === "checking").length;
  const absent = roster.length - installed - looking;
  useEffect(() => {
    // The clone has its own primary (Clone) and must not finish setup under a running Git.
    if (cinematic || busy || !saved || cloning) return;
    const next = (event: KeyboardEvent) => {
      if (
        event.key !== "Enter" ||
        event.isComposing ||
        event.metaKey ||
        event.ctrlKey ||
        event.altKey
      )
        return;
      if (
        event.target instanceof Element &&
        event.target.closest(
          "button, input, select, textarea, a, dialog, [role=menu], [contenteditable=true]",
        )
      )
        return;
      event.preventDefault();
      advance();
    };
    window.addEventListener("keydown", next);
    return () => window.removeEventListener("keydown", next);
  });
  // Agents follows the scan, so the heading always says what this computer actually has.
  const agentsHeading: [string, string] =
    agentsState.agents === null
      ? ["Looking for agents.", "Checking this computer."]
      : looking > 0
        ? ["Looking for agents.", `Still looking for ${looking}.`]
        : installed === 0
          ? ["No agents found.", "Install one, or try the demo."]
          : [
              `Found ${installed} ${installed === 1 ? "agent" : "agents"}.`,
              absent > 0 ? `${absent} more can be installed.` : "Nothing left to install.",
            ];
  const headings: Record<Page, [string, string]> = {
    welcome: ["Welcome to o7.", "Every agent, one window."],
    agents: agentsHeading,
    bots: ["Bring your assistant.", "It can run the work for you."],
    motion: ["Add a first project.", "Any folder works."],
    project: ["Add a first project.", "Any folder works."],
  };
  const focus: MapFocus =
    step === "agents"
      ? "agents"
      : step === "bots"
        ? "bots"
        : step === "welcome"
          ? "all"
          : "project";
  const introMode: IntroMode =
    saved === null ? "pending" : saved.cold_start && !reduced ? "live" : "still";
  return (
    <main className="setup-shell g-root" data-cinematic={cinematic || undefined}>
      <div className="setup-drag" data-tauri-drag-region="" />
      <header className="setup-top">
        <Wordmark size={16} />
        <button
          type="button"
          className="g-button g-button-ghost g-button-28 setup-skip"
          disabled={busy}
          onClick={() => (saved ? void run(() => apply({ type: "finish" })) : onDone())}
        >
          Skip setup
        </button>
      </header>
      <section className="setup-column">
        <div className="setup-panel">
          <nav
            aria-label="Setup steps"
            className="setup-progress"
            data-facing={facing}
            style={{ "--at": Math.max(0, index), "--legs": STEPS.length - 1 } as CSSProperties}
          >
            <ol>
              {STEPS.map((item, at) => (
                <li key={item.id}>
                  <button
                    type="button"
                    className="setup-progress-step"
                    data-state={at < index ? "done" : at === index ? "current" : "next"}
                    aria-current={at === index ? "step" : undefined}
                    aria-label={`${item.label}${at < index ? ", done" : ""}`}
                    title={item.label}
                    disabled={busy || !saved || at === index}
                    onClick={() => open(item.id)}
                  >
                    <span aria-hidden="true" />
                  </button>
                </li>
              ))}
            </ol>
            <span className="setup-progress-flown" aria-hidden="true" />
            <span className="setup-progress-wing" aria-hidden="true">
              {/* A new wing at each step: its first blink is its nod on landing. */}
              <Wing small key={index} />
            </span>
          </nav>
          {step === "project" && cloning ? (
            <div className="setup-content" key="clone" data-travel="forward">
              <ProjectClone
                heading={heading}
                api={clone?.api ?? cloneApi}
                draft={clone?.draft}
                pickParent={() => transport.pickFolder()}
                onBack={() => setCloning(false)}
                onCloned={cloned}
              />
            </div>
          ) : (
            <div className="setup-content" key={step} data-travel={back ? "back" : "forward"}>
              <h1 ref={heading} tabIndex={-1} className="setup-heading">
                {headings[step][0]}
                <span>{headings[step][1]}</span>
              </h1>
              {step === "welcome" ? (
                <p className="setup-lead">
                  Run the coding agents on this computer side by side, and steer them from your
                  phone or your own assistant when you are away. Each agent keeps its own sign-in,
                  and o7 itself needs no account.
                </p>
              ) : null}
              {step === "bots" ? (
                <>
                  <p className="setup-lead">
                    Link Grok Bot, Muse, Hermes or OpenClaw and hand it a goal from anywhere. It
                    starts your agents here, answers what they ask and brings back the result.
                    Settings links one later, too.
                  </p>
                  <ul className="remote-bot-picks setup-bots">
                    {BOTS.map((bot) => {
                      const linked = links.some((name) => botNamed(name) === bot.id);
                      return (
                        <li key={bot.id}>
                          <button
                            className="remote-bot-pick"
                            type="button"
                            data-testid={`setup-link-${bot.id}`}
                            disabled={busy}
                            onClick={() => setLinking(bot.id)}
                          >
                            <BotMark bot={bot.id} size={36} />
                            <span>
                              <strong>{bot.name}</strong>
                              <span>{linked ? "Linked" : bot.maker}</span>
                            </span>
                            {linked ? (
                              <Check className="setup-bot-linked" size={16} aria-hidden="true" />
                            ) : null}
                          </button>
                        </li>
                      );
                    })}
                  </ul>
                  <div className="setup-inline">
                    <button
                      type="button"
                      className="g-button g-button-ghost g-button-28 setup-refresh"
                      disabled={busy}
                      onClick={() => setLinking("other")}
                    >
                      Link another assistant
                    </button>
                  </div>
                </>
              ) : null}
              {step === "agents" ? (
                <>
                  <AgentRoster
                    agents={roster}
                    transport={transport}
                    onRefresh={() => void app.refreshAgents()}
                  />
                  <div className="setup-inline">
                    {installed === 0 && agentsState.agents !== null && looking === 0 ? (
                      <button
                        type="button"
                        className="g-button g-button-outline"
                        disabled={busy}
                        onClick={() => {
                          // Setup ends and New chat opens with the scripted Demo agent chosen.
                          onDemo();
                          if (saved) void run(() => apply({ type: "finish" }));
                          else onDone();
                        }}
                      >
                        Explore the local demo <ArrowRight size={14} aria-hidden="true" />
                      </button>
                    ) : null}
                    <button
                      type="button"
                      className="g-button g-button-ghost g-button-28 setup-refresh"
                      disabled={agentsState.agentsRefreshing}
                      onClick={() => void app.refreshAgents()}
                    >
                      <RefreshCw size={14} aria-hidden="true" />
                      {agentsState.agentsRefreshing ? "Refreshing…" : "Refresh"}
                    </button>
                  </div>
                </>
              ) : null}
              {step === "project" ? (
                <>
                  <p className="setup-lead">
                    Choose a folder, drop one here or clone a repository. Files stay where they are.
                  </p>
                  <button
                    type="button"
                    className="setup-folder"
                    data-chosen={folder ? true : undefined}
                    disabled={busy || !workspace}
                    onClick={pick}
                  >
                    <FolderOpen size={20} aria-hidden="true" />
                    <span>
                      <strong>{folder ? projectName(folder) : "Choose a folder"}</strong>
                      <small className={folder ? "setup-path" : undefined}>
                        {folder || "or drop it here"}
                      </small>
                    </span>
                  </button>
                  {folder ? (
                    <form
                      className="setup-project-form"
                      onSubmit={(event) => {
                        event.preventDefault();
                        saveProject();
                      }}
                    >
                      <label className="setup-field">
                        Project name
                        <input
                          className="g-input"
                          required
                          maxLength={128}
                          value={name}
                          disabled={busy}
                          onChange={(event) => setName(event.currentTarget.value)}
                        />
                      </label>
                      <button
                        type="submit"
                        className="g-button g-button-outline g-button-36"
                        disabled={busy || !name.trim()}
                      >
                        {project ? "Save selection" : "Add project"}
                      </button>
                    </form>
                  ) : null}
                  {saved?.preferences.first_project_id && project ? (
                    <p role="status" className="setup-project-saved">
                      <Check size={14} aria-hidden="true" /> {project.name} is ready.
                    </p>
                  ) : null}
                  <div className="setup-project-more">
                    <button
                      type="button"
                      className="g-button g-button-ghost g-button-28 setup-clone"
                      disabled={busy || !workspace}
                      onClick={() => {
                        setError(null);
                        setCloning(true);
                      }}
                    >
                      <GitBranch size={14} aria-hidden="true" />
                      Clone a repository
                    </button>
                    {existing.length > 0 ? (
                      <Select
                        className="g-select setup-existing"
                        aria-label="Or choose a project"
                        disabled={busy}
                        value={project?.id ?? ""}
                        onChange={(event) => {
                          const selected = existing.find(
                            (item) => item.id === event.currentTarget.value,
                          );
                          if (selected) {
                            setProject(selected);
                            setFolder("");
                            void run(() => apply({ type: "project", project_id: selected.id }));
                          }
                        }}
                      >
                        <option value="">Or choose a project</option>
                        {existing.map((item) => (
                          <option key={item.id} value={item.id}>
                            {item.name}
                          </option>
                        ))}
                      </Select>
                    ) : null}
                  </div>
                </>
              ) : null}
              {saved?.notice ? (
                <p className="setup-footnote" role="status">
                  {saved.notice}
                </p>
              ) : null}
              {error ? (
                <div className="setup-error" role="alert">
                  <p>{error}</p>
                  <div>
                    <button
                      type="button"
                      className="g-button g-button-ghost g-button-28"
                      onClick={() =>
                        void run(async () => {
                          const value = await api.snapshot();
                          setSaved(value);
                          setMotionPreference(value.preferences.motion);
                        })
                      }
                    >
                      Reload saved setup
                    </button>
                    <button
                      type="button"
                      className="g-button g-button-ghost g-button-28"
                      onClick={onDone}
                    >
                      Continue without saving setup
                    </button>
                  </div>
                </div>
              ) : null}
              <div className="setup-actions">
                <button
                  type="button"
                  className="g-button g-button-primary g-button-36"
                  disabled={busy || !saved}
                  onClick={advance}
                >
                  {busy
                    ? "Saving…"
                    : step === "welcome"
                      ? "Get started"
                      : step === "project"
                        ? "Open o7"
                        : "Continue"}
                  <span className="setup-enter" aria-hidden="true">
                    <Keycap>↵</Keycap>
                  </span>
                </button>
              </div>
            </div>
          )}
        </div>
        <aside className="setup-side">
          <SetupMap
            focus={focus}
            agents={roster}
            linked={links.map((name) => botNamed(name)).filter((bot): bot is BotId => bot !== null)}
            project={saved?.preferences.first_project_id && project ? project.name : null}
          />
        </aside>
      </section>
      {linking ? (
        <BotDialog
          initial={linking}
          create={async (name, rights) => {
            // Remote access turns on when the link is made, as in Settings.
            if (!(await remote.status()).enabled) await remote.setEnabled(true);
            const link = await remote.createBot(name, rights);
            setLinks((names) => [...names, link.name]);
            return link;
          }}
          onClose={() => setLinking(null)}
        />
      ) : null}
      {introGone ? null : (
        <SetupIntro mode={introMode} leaving={cinematic ? false : skipped ? "fast" : "slow"} />
      )}
      {cinematic ? (
        <button
          type="button"
          className="g-button g-button-ghost g-button-28 setup-skip-intro"
          onClick={() => {
            setSkipped(true);
            setCinematic(false);
          }}
        >
          Skip intro <Keycap>Esc</Keycap>
        </button>
      ) : null}
    </main>
  );
}

/** The project step's clone: its two-tone heading follows Git, then the form or the progress. */
function ProjectClone({
  heading,
  api,
  draft,
  pickParent,
  onBack,
  onCloned,
}: {
  readonly heading: RefObject<HTMLHeadingElement | null>;
  readonly api: CloneApi;
  readonly draft?: CloneDraft;
  readonly pickParent: () => Promise<string | null>;
  readonly onBack: () => void;
  readonly onCloned: (path: string, name: string) => void;
}) {
  const { controller, view } = useClone(api, draft);
  const [title, rest] = cloneHeading(view);
  return (
    <>
      <h1 ref={heading} tabIndex={-1} className="setup-heading">
        {title}
        <span>{rest}</span>
      </h1>
      <CloneBody
        controller={controller}
        view={view}
        pickParent={pickParent}
        layout="inline"
        onBack={onBack}
        onCloned={onCloned}
      />
    </>
  );
}

/**
 * Setup's roster: one row per agent, as Rust reports them (an adapter or a text fallback is a
 * connection of its agent, not a row), without the scripted demo.
 */
export function primaryAgents(agents: readonly AgentEntry[]): readonly AgentEntry[] {
  return realAgents(agents);
}
