import { memo, type PointerEvent, useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { LineEditBucket, LineEdits } from "../../bindings";
import { errorMessage } from "../../lib/errors";
import {
  agentLabel,
  agentsOf,
  calendarOf,
  calendarStart,
  type Day,
  dayKey,
  dayLabel,
  formatDay,
  formatLines,
  formatStreak,
  type Level,
  type LineEditsApi,
  lineEditsApi,
} from "../../lib/lineEdits";

/** How often the open card reads the figures again, counting the turns that ended meanwhile. */
export const LINE_EDITS_REFRESH_MS = 30_000;

/** How soon it reads again while earlier turns are still being counted. */
const COUNTING_RETRY_MS = 1_000;

const LEVELS: readonly Level[] = [0, 1, 2, 3, 4];

const NO_BUCKETS: readonly LineEditBucket[] = [];

/** The square under the pointer (its day's place in the calendar), placed over the calendar. */
interface Tip {
  readonly index: number;
  readonly left: number;
  readonly top: number;
}

/**
 * AI line edits: a year of days as a calendar of weeks, each square lit by how many lines agents'
 * edits changed that day, with the year's total, its busiest month and day, and its streaks. All,
 * or one agent at a time. The figures come from the core, which counts each turn once it ends.
 */
export const LineEditsCard = memo(function LineEditsCard({
  api = lineEditsApi,
  agentNames,
  today,
}: {
  readonly api?: LineEditsApi;
  /** Agents' names by id, as their chats show them. */
  readonly agentNames: ReadonlyMap<string, string>;
  /** Local midnight of today: the calendar ends on its week. */
  readonly today: number;
}) {
  const [edits, setEdits] = useState<LineEdits | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [agent, setAgent] = useState<string | null>(null);
  const [tip, setTip] = useState<Tip | null>(null);
  const calendarRef = useRef<HTMLDivElement>(null);
  const load = useCallback(async () => {
    try {
      const next = await api.read(calendarStart(Date.now()).getTime());
      setEdits(next);
      setError(null);
      return next.complete;
    } catch (failure) {
      setError(errorMessage(failure));
      return true;
    }
  }, [api]);

  useEffect(() => {
    let alive = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const tick = async () => {
      if (!alive) return;
      const complete = document.hidden ? true : await load();
      if (alive) timer = setTimeout(tick, complete ? LINE_EDITS_REFRESH_MS : COUNTING_RETRY_MS);
    };
    void tick();
    return () => {
      alive = false;
      clearTimeout(timer);
    };
  }, [load]);

  const buckets = edits?.buckets ?? NO_BUCKETS;
  const agents = useMemo(() => agentsOf(buckets), [buckets]);
  // An agent whose edits are all gone (its chats deleted) falls back to All.
  const shown = agent !== null && agents.includes(agent) ? agent : null;
  const calendar = useMemo(() => calendarOf(buckets, shown, today), [buckets, shown, today]);
  const busy = edits === null && error === null;
  // Read from the calendar shown, so choosing another agent updates an open tip.
  const tipDay: Day | undefined = tip === null ? undefined : calendar.days[tip.index];

  const point = (event: PointerEvent<HTMLDivElement>) => {
    const square = (event.target as HTMLElement).closest<HTMLElement>("[data-day]");
    const box = calendarRef.current?.getBoundingClientRect();
    const index = Number(square?.dataset.day);
    if (!square || !box || !Number.isInteger(index)) {
      setTip(null);
      return;
    }
    const rect = square.getBoundingClientRect();
    setTip({ index, left: rect.left + rect.width / 2 - box.left, top: rect.top - box.top });
  };

  return (
    <section
      className="dashboard-activity"
      aria-labelledby="dashboard-activity-title"
      data-testid="dashboard-activity"
    >
      <div className="dashboard-activity-head">
        <div className="dashboard-activity-heading">
          <h3 id="dashboard-activity-title" className="dashboard-activity-title">
            AI line edits
          </h3>
          <p className="dashboard-activity-total" data-testid="dashboard-activity-total">
            {busy ? "–" : formatLines(calendar.total)}
          </p>
        </div>
        {agents.length > 1 ? (
          <fieldset className="g-segmented dashboard-activity-agents" aria-label="Agent">
            {[null, ...agents].map((choice) => (
              <button
                key={choice ?? ""}
                type="button"
                className="g-segment"
                aria-pressed={shown === choice}
                data-testid="dashboard-activity-agent"
                onClick={() => setAgent(choice)}
              >
                {choice === null ? "All" : agentLabel(choice, agentNames)}
              </button>
            ))}
          </fieldset>
        ) : null}
      </div>
      {error !== null ? (
        <p role="alert" className="ui-error dashboard-activity-error">
          Couldn't read line edits. {error}
        </p>
      ) : null}
      <div
        ref={calendarRef}
        className="dashboard-activity-calendar"
        role="img"
        aria-busy={busy}
        aria-label={
          busy
            ? "Reading line edits"
            : `${formatLines(calendar.total)} line edits since ${formatDay(calendar.days[0]?.date ?? new Date(today))}`
        }
      >
        <div className="dashboard-activity-months" aria-hidden="true">
          {calendar.months.map((month) => (
            <span key={month.week} style={{ gridColumnStart: month.week + 1 }}>
              {month.name}
            </span>
          ))}
        </div>
        <div
          className="dashboard-activity-grid"
          aria-hidden="true"
          onPointerOver={point}
          onPointerLeave={() => setTip(null)}
        >
          {calendar.days.map((day, index) => (
            <span
              key={dayKey(day.date)}
              className="dashboard-activity-day"
              data-level={day.level}
              data-day={index}
            />
          ))}
        </div>
        {tip !== null && tipDay !== undefined ? (
          <span
            className="g-tooltip dashboard-activity-tip"
            aria-hidden="true"
            style={{ left: tip.left, top: tip.top }}
          >
            {dayLabel(tipDay)}
          </span>
        ) : null}
      </div>
      <dl className="dashboard-activity-stats" data-testid="dashboard-activity-stats">
        <div>
          <dt>Most active month</dt>
          <dd>{calendar.mostActiveMonth ?? "None yet"}</dd>
        </div>
        <div>
          <dt>Most active day</dt>
          <dd>{calendar.mostActiveDay ? formatDay(calendar.mostActiveDay) : "None yet"}</dd>
        </div>
        <div>
          <dt>Longest streak</dt>
          <dd>{formatStreak(calendar.longestStreak)}</dd>
        </div>
        <div>
          <dt>Current streak</dt>
          <dd>{formatStreak(calendar.currentStreak)}</dd>
        </div>
      </dl>
      <div className="dashboard-activity-foot">
        <div className="dashboard-activity-legend" aria-hidden="true">
          <span>Less</span>
          {LEVELS.map((level) => (
            <span key={level} className="dashboard-activity-day" data-level={level} />
          ))}
          <span>More</span>
        </div>
        {edits !== null && !edits.complete ? (
          <p className="dashboard-activity-note" role="status">
            Counting earlier chats…
          </p>
        ) : null}
      </div>
    </section>
  );
});
