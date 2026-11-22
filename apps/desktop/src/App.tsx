import type { AgentBrandId, OrbState } from "@o7/ui";
import { MotionConfig } from "motion/react";
import { useEffect, useRef, useState } from "react";
import type { AppInfo, InstructionsTarget, StressConfig } from "./bindings";
import { ChatsScreen } from "./components/chat/ChatsScreen";
import { DashboardScreen } from "./components/dashboard/DashboardScreen";
import { ErrorScreen } from "./components/ErrorScreen";
import { Home } from "./components/Home";
import { Sidebar } from "./components/Sidebar";
import { StressView } from "./components/StressView";
import { type Page as SettingsPage, SettingsScreen } from "./components/settings/SettingsScreen";
import { Onboarding } from "./components/setup/Onboarding";
import type { AppStore } from "./lib/chat/appStore";
import type { Transport } from "./lib/chat/transport";
import { errorMessage } from "./lib/errors";
import { installFocusReturn, restoreFocusSoon } from "./lib/focusReturn";
import { useStream } from "./lib/hooks";
import { getAppInfo, inTauri, sendSmokeReport, windowReady } from "./lib/ipc";
import { subscribeNativeNavigation } from "./lib/nativeNavigation";
import { LATE_NOTICE_CHECKS_MS, withLateNotices } from "./lib/notices";
import { paintsOnCpu } from "./lib/paint";
import { installProfileEvents, takeSettingsReturn } from "./lib/profiles";
import { afterPaint, whenPainted } from "./lib/ready";
import type { StreamStore } from "./lib/session";
import { setupApi } from "./lib/setup";
import { runSmoke, type SmokeHooks } from "./lib/smoke";
import { runChatSmoke } from "./lib/smokeChat";
import { setNoticeSoundsAvailable, subscribeNoticeSounds } from "./lib/sounds";
import { activityOf } from "./lib/stream";
import { DEFAULT_STRESS } from "./lib/stress";
import { updatesApi } from "./lib/updates";
import type { WorkspaceStore } from "./lib/workspace";

type Boot =
  | { readonly status: "loading" }
  | { readonly status: "ready"; readonly info: AppInfo }
  | { readonly status: "outside" }
  | { readonly status: "failed"; readonly message: string };

/** The chats (the default), the M0 home (the info card and the demo stream), or the stress view. */
type View = "chats" | "diagnostics" | "stress" | "setup" | "dashboard" | "settings";

/**
 * Switching views ends the current stream: the shell stops it (so it does not keep streaming into
 * a channel nobody reads) and the store forgets it (so the new view starts empty).
 */
function leaveStream(store: StreamStore): void {
  store.abandon().catch((error: unknown) => console.error("o7: could not stop the stream", error));
}

export function App({
  store,
  chats,
  transport,
  workspace,
  setUp = false,
}: {
  readonly store: StreamStore;
  readonly chats: AppStore;
  readonly transport: Transport;
  readonly workspace?: WorkspaceStore;
  /**
   * This profile finished setup on an earlier launch. It opens on its chats: setup's page would
   * only show the mark for a beat and hand over to them.
   */
  readonly setUp?: boolean;
}) {
  const [boot, setBoot] = useState<Boot>({ status: "loading" });
  const [view, setView] = useState<View>(setUp ? "chats" : "setup");
  // Keyboard focus never falls to nothing: back from Settings or the Dashboard, the terminal or
  // control that held it takes it again (lib/focusReturn.ts).
  useEffect(() => {
    installFocusReturn();
  }, []);
  useEffect(() => {
    if (view === "chats") restoreFocusSoon();
  }, [view]);
  const [restartSetup, setRestartSetup] = useState(false);
  const [settingsReturn, setSettingsReturn] = useState<"chats" | "setup">("chats");
  /** Where Settings opens: its page, and the project, chat or revision a page opens on. */
  const [settingsEntry, setSettingsEntry] = useState<{
    id: string;
    page: SettingsPage;
    target?: InstructionsTarget;
    chatLabel?: string;
    projectId?: string | null;
    activeChatId?: string;
  }>({
    id: "initial",
    page: "general",
  });
  /** Opens Settings from the chats on `page`, with what that page should open on. */
  const openSettings = (
    page: SettingsPage,
    entry: Omit<typeof settingsEntry, "id" | "page"> = {},
  ) => {
    setSettingsReturn("chats");
    setSettingsEntry({ id: String(Date.now()), page, ...entry });
    setView("settings");
  };
  useEffect(() => {
    const openSettings = (event: KeyboardEvent) => {
      if (
        (view === "chats" || view === "settings") &&
        (event.metaKey || event.ctrlKey) &&
        event.key === "," &&
        !event.altKey &&
        !event.isComposing &&
        !event.defaultPrevented &&
        !document.querySelector('dialog[open], [role="dialog"]')
      ) {
        event.preventDefault();
        if (view === "settings") setView(settingsReturn);
        else {
          setSettingsReturn("chats");
          setSettingsEntry({ id: "shortcut", page: "general" });
          setView("settings");
        }
      }
    };
    document.addEventListener("keydown", openSettings);
    return () => document.removeEventListener("keydown", openSettings);
  }, [view, settingsReturn]);
  const [openChatRequest, setOpenChatRequest] = useState<{ id: string }>();
  const [newChatRequest, setNewChatRequest] = useState<{ id: string; agent: string }>();
  useEffect(
    () =>
      subscribeNativeNavigation((id) => {
        setView("chats");
        setOpenChatRequest({ id });
      }),
    [],
  );
  // Settings' Sounds: the native shell sends a notice only while they are on.
  useEffect(() => subscribeNoticeSounds(), []);
  const [stressConfig, setStressConfig] = useState<StressConfig>(DEFAULT_STRESS);
  const [stressAutoStart, setStressAutoStart] = useState(false);
  const [brandId, setBrandId] = useState<AgentBrandId>("claude-code");
  const snapshot = useStream(store);

  useEffect(() => {
    if (!inTauri()) {
      setBoot({ status: "outside" });
      return;
    }
    let live = true;
    installProfileEvents()
      .then(() => getAppInfo())
      .then(
        (info) => {
          if (!live) return;
          if (info.launch.type === "stress") {
            setStressConfig(info.launch.config);
            setStressAutoStart(true);
            setView("stress");
          }
          if (info.launch.type === "smoke") setView("chats");
          // A profile opened from Settings shows Settings again, on the page that opened it.
          else if (takeSettingsReturn(sessionStorage) === "privacy") {
            // Back leads to setup while this profile has not finished it, as a launch would.
            setSettingsReturn("setup");
            setSettingsEntry({ id: "profile-switch", page: "privacy" });
            setView("settings");
            void setupApi.snapshot().then(
              (setup) => {
                if (live && setup.preferences.completed)
                  setSettingsReturn((previous) => (previous === "setup" ? "chats" : previous));
              },
              () => undefined,
            );
          }
          setBoot({ status: "ready", info });
        },
        (error: unknown) => {
          if (live) setBoot({ status: "failed", message: errorMessage(error) });
        },
      );
    const timers = LATE_NOTICE_CHECKS_MS.map((delay) =>
      setTimeout(() => {
        getAppInfo().then(
          (later) => {
            if (!live) return;
            setBoot((current) =>
              current.status === "ready"
                ? { status: "ready", info: withLateNotices(current.info, later) }
                : current,
            );
          },
          () => undefined,
        );
      }, delay),
    );
    return () => {
      live = false;
      for (const timer of timers) clearTimeout(timer);
    };
  }, []);

  // The material behind the window; with none, CSS paints the opaque base (see styles.css). Left
  // unset while loading, so the page never fades from one to the other.
  const glass =
    boot.status === "ready" ? boot.info.glass : boot.status === "loading" ? null : "none";
  useEffect(() => {
    if (glass !== null) document.documentElement.dataset.glass = glass;
  }, [glass]);

  // Sounds stay silent until the app info says this window may make them (Linux without
  // GStreamer's plugins may not).
  const media = boot.status === "ready" && boot.info.media;
  useEffect(() => setNoticeSoundsAvailable(media), [media]);

  // The platform, for what only one draws differently (Windows' scrollbars, styles.css).
  const os = boot.status === "ready" ? boot.info.os : null;
  useEffect(() => {
    if (os === null) return;
    document.documentElement.dataset.os = os;
    // Where the page is painted on the CPU, the panes draw more cheaply (ui-tokens.css).
    if (paintsOnCpu(os)) document.documentElement.dataset.paint = "cpu";
  }, [os]);

  // Once real content has painted, show the window; in smoke mode, then run the check. A ref, not
  // the effect's cleanup, guards this: StrictMode re-runs effects in development.
  const started = useRef(false);
  useEffect(() => {
    if (boot.status === "loading" || started.current || !inTauri()) return;
    started.current = true;
    void (async () => {
      try {
        await whenPainted(windowReady);
      } catch (error) {
        console.error("o7: window_ready failed", error);
      }
      if (boot.status === "ready" && boot.info.launch.type === "smoke") {
        const { plan } = boot.info.launch;
        const hooks: SmokeHooks = {
          store,
          showHome: async () => {
            setView("diagnostics");
            await afterPaint(500);
          },
          showStress: async (config) => {
            setStressAutoStart(false);
            setStressConfig(config);
            setView("stress");
            await afterPaint(500);
          },
          renderedAnswer: () => {
            const answers = document.querySelectorAll("[data-answer]");
            return answers[answers.length - 1]?.textContent ?? "";
          },
          settle: () => afterPaint(500),
        };
        const report =
          plan.scenario === "stream"
            ? await runSmoke(boot.info, hooks)
            : await runChatSmoke(boot.info, plan, {
                showChats: async () => {
                  setView("chats");
                  await afterPaint(500);
                },
              });
        try {
          await sendSmokeReport(report);
        } catch (error) {
          console.error("o7: smoke_report failed", error);
        }
      }
    })();
  }, [boot, store]);

  if (boot.status === "loading") return null;
  if (boot.status === "outside") {
    return (
      <ErrorScreen
        title="This is o7's interface"
        message="It runs inside the desktop app, which provides its data and agents. Start it with `pnpm dev` from the repository root."
        dataDir={null}
        os=""
      />
    );
  }
  if (boot.status === "failed") {
    return <ErrorScreen title="o7 could not start" message={boot.message} dataDir={null} os="" />;
  }

  const { info } = boot;
  if (info.fatal !== null) {
    return (
      <ErrorScreen
        title="o7 couldn't open your data"
        message={info.fatal}
        dataDir={info.data_dir}
        os={info.os}
        notices={info.notices}
      />
    );
  }

  // The shell has acknowledged the repair, so `app_info` no longer reports it; mirror that here.
  // Not in the notice's own state: the home screen, notice included, unmounts while the stress
  // view is open and would come back showing the repair again.
  const onRecoveryDismissed = (): void => {
    setBoot((current) =>
      current.status === "ready"
        ? { status: "ready", info: { ...current.info, recovery_notice: null } }
        : current,
    );
  };

  const activity = snapshot.phase === "idle" ? "idle" : activityOf(snapshot.stream);
  const orbState: OrbState = activity;

  if (view === "setup")
    return (
      <Onboarding
        app={chats}
        transport={transport}
        workspace={workspace}
        restart={restartSetup}
        onDone={() => {
          setRestartSetup(false);
          setView("chats");
        }}
        onDemo={() => setNewChatRequest({ id: String(Date.now()), agent: "mock" })}
      />
    );
  // Settings and the Dashboard keep the chats mounted behind them: the open chat and its draft survive
  // the visit.
  if (view === "chats" || view === "settings" || view === "dashboard") {
    return (
      <>
        <div hidden={view !== "chats"} className="app-view">
          <ChatsScreen
            active={view === "chats"}
            updates={updatesApi}
            workspace={workspace}
            openChatRequest={openChatRequest}
            newChatRequest={newChatRequest}
            app={chats}
            transport={transport}
            info={info}
            onRecoveryDismissed={onRecoveryDismissed}
            onDiagnostics={() => setView("diagnostics")}
            onAgents={() => openSettings("agents")}
            onDashboard={() => setView("dashboard")}
            onCustomize={(projectId, activeChatId) =>
              openSettings("plugins", { projectId, activeChatId })
            }
            onOpenMemory={(projectId) => openSettings("memory", { projectId })}
            onSettings={() => openSettings("general")}
            onInstructions={(target, chatLabel) => {
              const organization = workspace?.getSnapshot().data;
              const projectId =
                target.type === "chat"
                  ? organization?.chats.find((entry) => entry.chat.id === target.chat_id)
                      ?.project_id
                  : target.type === "folder"
                    ? organization?.projects.find((project) =>
                        project.folders.some((folder) => folder.id === target.folder_id),
                      )?.id
                    : null;
              openSettings("instructions", {
                projectId: projectId ?? null,
                activeChatId: target.type === "chat" ? target.chat_id : undefined,
                target,
                chatLabel,
              });
            }}
          />
        </div>
        {view === "settings" ? (
          <SettingsScreen
            navigationId={settingsEntry.id}
            initialPage={settingsEntry.page}
            instructionsTarget={settingsEntry.target}
            chatLabel={settingsEntry.chatLabel}
            workspace={workspace}
            projectId={settingsEntry.projectId ?? null}
            activeChatId={settingsEntry.activeChatId}
            agents={{ app: chats, transport }}
            backLabel={settingsReturn === "setup" ? "Back to setup" : "Back to chats"}
            info={info}
            onBack={() => setView(settingsReturn)}
            onDiagnostics={() => setView("diagnostics")}
            onSetup={() => {
              setRestartSetup(true);
              setView("setup");
            }}
          />
        ) : null}
        {view === "dashboard" ? (
          <div className="pane-shell g-root">
            <DashboardScreen
              app={chats}
              workspace={workspace}
              onBack={() => setView("chats")}
              onOpenChat={(id) => {
                setView("chats");
                setOpenChatRequest({ id });
              }}
            />
          </div>
        ) : null}
      </>
    );
  }

  return (
    <MotionConfig reducedMotion="user">
      <div className="app diagnostics-shell">
        <Sidebar
          brandId={brandId}
          onBrandChange={setBrandId}
          orbState={orbState}
          onChats={() => {
            leaveStream(store);
            setStressAutoStart(false);
            setView("chats");
          }}
        />
        <main className="main">
          <div className="drag-strip" data-tauri-drag-region="" />
          <div className="main-scroll">
            {view === "diagnostics" ? (
              <Home
                info={info}
                store={store}
                onOpenStress={() => {
                  leaveStream(store);
                  setStressAutoStart(false);
                  setView("stress");
                }}
                onRecoveryDismissed={onRecoveryDismissed}
              />
            ) : (
              <StressView
                store={store}
                config={stressConfig}
                onConfigChange={setStressConfig}
                autoStart={stressAutoStart}
                onExit={() => {
                  leaveStream(store);
                  setStressAutoStart(false);
                  setView("diagnostics");
                }}
              />
            )}
          </div>
        </main>
      </div>
    </MotionConfig>
  );
}
