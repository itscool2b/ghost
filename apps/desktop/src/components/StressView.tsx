import { AGENT_BRAND_IDS, Orb } from "@ghost/ui";
import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import type { StressConfig } from "../bindings";
import { errorMessage } from "../lib/errors";
import { FpsMeter, FrameLog, trackFrames } from "../lib/fps";
import { useStickToBottom, useStream } from "../lib/hooks";
import { isRunning, type StreamStore } from "../lib/session";
import { lastMessage } from "../lib/stream";
import { formatStressConfig, parseStressConfig, STRESS_LIMITS, StressRun } from "../lib/stress";
import { ArrowLeftIcon } from "./icons";
import { Answer } from "./Markdown";

/**
 * The worst case the UI must stay smooth under: up to 24 live orbs and a fast markdown stream,
 * with the frame rate on screen. `seconds` > 0 streams for
 * that long, in one turn or, past what a turn holds, several back to back; 0 streams turn after
 * turn until stopped (what the power spike measures). `StressRun` sizes the turns.
 */
export function StressView({
  store,
  config,
  onConfigChange,
  autoStart,
  onExit,
}: {
  readonly store: StreamStore;
  readonly config: StressConfig;
  readonly onConfigChange: (config: StressConfig) => void;
  /** Start streaming on mount (GHOST_STRESS); the smoke run starts streams itself. */
  readonly autoStart: boolean;
  readonly onExit: () => void;
}) {
  const snapshot = useStream(store);
  const running = isRunning(snapshot);
  const [active, setActive] = useState(autoStart);
  const [problem, setProblem] = useState<string | null>(null);
  // The latest settings, for turns started from effects (declared first, so it runs first).
  const configRef = useRef(config);
  useLayoutEffect(() => {
    configRef.current = config;
  }, [config]);

  // The run in progress, which knows how long its turns have streamed so far.
  const run = useRef<StressRun | null>(null);

  const startTurn = (): void => {
    const current = configRef.current;
    run.current ??= new StressRun();
    const tokens = run.current.next(current);
    if (tokens === null) {
      // A timed run has streamed for its `seconds`.
      setActive(false);
      return;
    }
    setProblem(null);
    store.start({ type: "stress", total_tokens: tokens }, current.rate).catch((error: unknown) => {
      setActive(false);
      setProblem(errorMessage(error));
    });
  };

  const startRun = (): void => {
    run.current = new StressRun();
    setActive(true);
    startTurn();
  };

  // Auto start once, on mount.
  const autoStarted = useRef(false);
  // biome-ignore lint/correctness/useExhaustiveDependencies: runs once per mount by design.
  useEffect(() => {
    if (autoStart && !autoStarted.current) {
      autoStarted.current = true;
      startRun();
    }
  }, []);

  // A turn that ends starts the run's next one, until the run is stopped or a timed run is over.
  const phase = snapshot.phase;
  // biome-ignore lint/correctness/useExhaustiveDependencies: reacts to the turn ending only.
  useEffect(() => {
    if (phase === "ended" && active) startTurn();
  }, [phase]);

  const answer = lastMessage(snapshot.stream);
  const streamRef = useStickToBottom<HTMLDivElement>(answer?.text);

  return (
    <div className="stress-view">
      <header className="stress-header">
        <h1 className="diagnostics-title">
          Stress view
          <span>The worst case, live.</span>
        </h1>
        <FpsReadout />
      </header>

      <section className="diagnostics-panel stress-controls" aria-label="Stress controls">
        <div className="stress-controls-row">
          <button type="button" className="g-button g-button-ghost stress-back" onClick={onExit}>
            <ArrowLeftIcon /> Back
          </button>
          <RangeField
            label="Orbs"
            value={config.orbs}
            min={STRESS_LIMITS.orbs.min}
            max={STRESS_LIMITS.orbs.max}
            onChange={(orbs) => onConfigChange({ ...config, orbs })}
          />
          <RangeField
            label="Tokens/s"
            value={config.rate}
            min={10}
            max={2000}
            step={10}
            onChange={(rate) => onConfigChange({ ...config, rate })}
          />
          <div className="stress-run">
            <button
              type="button"
              className="g-button g-button-primary"
              disabled={running}
              onClick={startRun}
            >
              Start
            </button>
            <button
              type="button"
              className="g-button"
              disabled={!running}
              onClick={() => {
                setActive(false);
                store.stop().catch((error: unknown) => setProblem(errorMessage(error)));
              }}
            >
              Stop
            </button>
          </div>
        </div>
        <ConfigLine config={config} onChange={onConfigChange} />
        {problem ? (
          <p role="alert" className="diagnostics-alert">
            {problem}
          </p>
        ) : null}
      </section>

      <section className="orb-grid" aria-label={`${config.orbs} live orbs`}>
        {Array.from({ length: config.orbs }, (_, index) => {
          const brand = AGENT_BRAND_IDS[index % AGENT_BRAND_IDS.length] ?? "generic";
          return (
            <Orb
              // biome-ignore lint/suspicious/noArrayIndexKey: orb slots are positional.
              key={index}
              brand={brand}
              state={running ? (index % 3 === 0 ? "thinking" : "streaming") : "idle"}
              size={64}
              seed={(index + 1) / (config.orbs + 1)}
            />
          );
        })}
      </section>

      <div
        ref={streamRef}
        className="focus-gutter stress-stream"
        role="log"
        aria-label="Stress stream"
        aria-busy={running}
      >
        {answer ? (
          <Answer key={`${snapshot.sessionId}:${answer.id}`} text={answer.text} />
        ) : (
          <p className="muted stress-empty">The stream appears here.</p>
        )}
      </div>
    </div>
  );
}

function RangeField({
  label,
  value,
  min,
  max,
  step = 1,
  onChange,
}: {
  readonly label: string;
  readonly value: number;
  readonly min: number;
  readonly max: number;
  readonly step?: number;
  readonly onChange: (value: number) => void;
}) {
  const id = useId();
  return (
    <div className="field">
      <label htmlFor={id}>{label}</label>
      <input
        id={id}
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        onChange={(event) => onChange(Number(event.currentTarget.value))}
      />
      <output htmlFor={id}>{value}</output>
    </div>
  );
}

/** The settings as a `GHOST_STRESS` value: copy it to reproduce a run, or edit it here. */
function ConfigLine({
  config,
  onChange,
}: {
  readonly config: StressConfig;
  readonly onChange: (config: StressConfig) => void;
}) {
  const id = useId();
  const formatted = formatStressConfig(config);
  const [draft, setDraft] = useState(formatted);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    setDraft(formatted);
    setError(null);
  }, [formatted]);

  const commit = (): void => {
    const parsed = parseStressConfig(draft);
    if (parsed.ok) {
      setError(null);
      onChange(parsed.config);
    } else {
      setError(parsed.error);
    }
  };

  return (
    <div className="field">
      <label htmlFor={id} className="stress-config-label">
        GHOST_STRESS=
      </label>
      <input
        id={id}
        className="g-input stress-config-input"
        value={draft}
        spellCheck={false}
        aria-invalid={error !== null}
        onChange={(event) => setDraft(event.currentTarget.value)}
        onBlur={commit}
        onKeyDown={(event) => {
          if (event.key === "Enter") commit();
        }}
      />
      {error ? <span className="diagnostics-alert">{error}</span> : null}
    </div>
  );
}

/**
 * localStorage key of the stress view's per-second frame counts (a `FrameLog`, as JSON). The render
 * and power spike's harness reads it from WebKit's storage file to get the frame rate over exactly
 * its measurement window (docs/spikes/render-power.md).
 */
const FRAME_LOG_KEY = "auracode.stress.frames";

/** Live frame rate: current and lowest over half-second windows, updated four times a second. */
function FpsReadout() {
  const currentRef = useRef<HTMLElement>(null);
  const minRef = useRef<HTMLElement>(null);
  useEffect(() => {
    const meter = new FpsMeter(500);
    let shownAt = 0;
    // Written straight to the DOM: a React render per frame would cost the frames it measures.
    const stopFrames = trackFrames(meter, () => {
      const now = performance.now();
      if (now - shownAt < 250) return;
      shownAt = now;
      const summary = meter.summary();
      if (currentRef.current) currentRef.current.textContent = summary.current.toFixed(0);
      if (minRef.current) minRef.current.textContent = summary.min.toFixed(0);
    });
    // A timer, not animation frames: it keeps sampling while the window is hidden, which is when
    // the log should show frames stopping.
    const log = new FrameLog();
    const timer = window.setInterval(() => {
      log.record(Date.now(), meter.summary().frames, document.hidden);
      try {
        localStorage.setItem(FRAME_LOG_KEY, JSON.stringify(log.samples));
      } catch {
        // Storage disabled or full: only outside measurement loses its data, the readout stays.
      }
    }, 1000);
    return () => {
      stopFrames();
      window.clearInterval(timer);
    };
  }, []);
  return (
    <p className="fps">
      <strong ref={currentRef}>0</strong> fps
      <br />
      <span className="muted">
        min <span ref={minRef}>0</span>
      </span>
    </p>
  );
}
