import { BotMark } from "@o7/ui/bot-mark";
import { AgentMark, BOTS, type BotId, Logo } from "@o7/ui/graphite";
import { Check, Folder, Smartphone } from "lucide-react";
import type { AgentEntry } from "../../bindings";
import "./setup-map.css";

/** The part of the picture a step is about: its lines run toward o7 while the step is open. */
export type MapFocus = "all" | "agents" | "bots" | "phone" | "project";

const WIDTH = 440;
const HEIGHT = 400;
const HUB = { x: WIDTH / 2, y: HEIGHT / 2 };
const HUB_HALF = 36;
const TILE = 44;
const BOTS_Y = 58;
const AGENTS_Y = HEIGHT - 58;
/** The most agents the picture draws; the step's list names them all. */
const AGENTS_SHOWN = 7;

/** A row of tiles centred on the hub's axis. */
function row(count: number, y: number, gap: number): { x: number; y: number }[] {
  return Array.from({ length: count }, (_, index) => ({
    x: HUB.x + (index - (count - 1) / 2) * gap,
    y,
  }));
}

/** A line from a tile to the hub that leaves both straight on and bends between them. */
function curve(from: { x: number; y: number }, above: boolean): string {
  const start = { x: from.x, y: from.y + (above ? TILE / 2 : -TILE / 2) };
  const end = { x: HUB.x + (from.x - HUB.x) * 0.16, y: HUB.y + (above ? -HUB_HALF : HUB_HALF) };
  const middle = (start.y + end.y) / 2;
  return `M ${start.x} ${start.y} C ${start.x} ${middle} ${end.x} ${middle} ${end.x} ${end.y}`;
}

/**
 * Setup's picture: the assistants that can hand o7 work above its logo, the coding agents on
 * this computer below it, the phone the agents test on to its left and the first project to its
 * right. It is the same picture on every step and fills in as setup goes: an agent that is found,
 * a bot that is linked, a phone that is ready and the project that is chosen each gain a settled
 * line and a check, and the step that is open has its lines running
 * toward o7. Whole tiles move; nothing is drawn over a mark or changes it
 * (third-party/brand-marks.md). Reduced motion shows the picture still.
 */
export function SetupMap({
  focus,
  agents,
  linked,
  phone = false,
  project,
}: {
  readonly focus: MapFocus;
  readonly agents: readonly AgentEntry[];
  /** The assistants a bot link names. */
  readonly linked: readonly BotId[];
  /** An Android phone or an iPhone can start here. */
  readonly phone?: boolean;
  /** The first project's name, once one is chosen. */
  readonly project: string | null;
}) {
  const found = agents.filter((agent) => agent.availability.state === "ready");
  const looking = agents.filter((agent) => agent.availability.state === "checking");
  // What this computer has; while nothing is found yet, the first few it could have, as ghosts.
  const drawn = (found.length + looking.length > 0 ? [...found, ...looking] : agents).slice(
    0,
    AGENTS_SHOWN,
  );
  const ghosts = found.length + looking.length === 0;
  const bots = row(BOTS.length, BOTS_Y, 84);
  const tiles = row(drawn.length, AGENTS_Y, Math.min(84, (WIDTH - TILE - 8) / drawn.length));
  const folder = { x: WIDTH - 58, y: HUB.y };
  const handset = { x: 58, y: HUB.y };
  const label = [
    found.length === 1 ? "1 agent found" : `${found.length} agents found`,
    linked.length === 1 ? "1 assistant linked" : `${linked.length} assistants linked`,
    phone ? "a phone ready" : "no phone yet",
    project ? `first project ${project}` : "no project yet",
  ].join(", ");
  const place = (at: { x: number; y: number }, order: number) =>
    ({
      "--map-x": `${(at.x / WIDTH) * 100}%`,
      "--map-y": `${(at.y / HEIGHT) * 100}%`,
      "--map-order": order,
    }) as React.CSSProperties;
  const mark = (at: { x: number; y: number }, above: boolean) => {
    // The check sits where the line is halfway, as the agents' own drawing has it.
    const start = at.y + (above ? TILE / 2 : -TILE / 2);
    const end = HUB.y + (above ? -HUB_HALF : HUB_HALF);
    return { x: (at.x + HUB.x + (at.x - HUB.x) * 0.16) / 2 - 7, y: (start + end) / 2 - 7 };
  };
  return (
    <figure className="setup-map" data-focus={focus} aria-label={label} data-testid="setup-map">
      <svg
        className="setup-map-lines"
        viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
        preserveAspectRatio="xMidYMid meet"
        aria-hidden="true"
      >
        {BOTS.map((bot, index) => {
          const at = bots[index] ?? HUB;
          const joined = linked.includes(bot.id);
          const check = mark(at, true);
          return (
            <g
              key={bot.id}
              className="setup-map-link"
              data-group="bots"
              data-state={joined ? "joined" : "open"}
              style={{ "--map-order": index } as React.CSSProperties}
            >
              <path className="setup-map-line" d={curve(at, true)} pathLength={1} />
              {joined ? null : (
                <path className="setup-map-flow" d={curve(at, true)} pathLength={1} />
              )}
              {joined ? (
                // The place is the outer group's; the inner one lands in it (a CSS transform
                // replaces an element's own).
                <g transform={`translate(${check.x} ${check.y})`}>
                  <g className="setup-map-check">
                    <circle cx={7} cy={7} r={7} />
                    <Check x={3} y={3} width={8} height={8} strokeWidth={3} />
                  </g>
                </g>
              ) : null}
            </g>
          );
        })}
        {drawn.map((agent, index) => {
          const at = tiles[index] ?? HUB;
          const state =
            agent.availability.state === "ready"
              ? "joined"
              : agent.availability.state === "checking"
                ? "joining"
                : "open";
          const check = mark(at, false);
          return (
            <g
              key={agent.id}
              className="setup-map-link"
              data-group="agents"
              data-state={state}
              style={{ "--map-order": index } as React.CSSProperties}
            >
              <path className="setup-map-line" d={curve(at, false)} pathLength={1} />
              {state === "joined" ? null : (
                <path className="setup-map-flow" d={curve(at, false)} pathLength={1} />
              )}
              {state === "joined" ? (
                // The place is the outer group's; the inner one lands in it (a CSS transform
                // replaces an element's own).
                <g transform={`translate(${check.x} ${check.y})`}>
                  <g className="setup-map-check">
                    <circle cx={7} cy={7} r={7} />
                    <Check x={3} y={3} width={8} height={8} strokeWidth={3} />
                  </g>
                </g>
              ) : null}
            </g>
          );
        })}
        <g className="setup-map-link" data-group="phone" data-state={phone ? "joined" : "open"}>
          <path
            className="setup-map-line"
            d={`M ${handset.x + TILE / 2} ${handset.y} L ${HUB.x - HUB_HALF} ${HUB.y}`}
            pathLength={1}
          />
          {phone ? null : (
            <path
              className="setup-map-flow"
              d={`M ${handset.x + TILE / 2} ${handset.y} L ${HUB.x - HUB_HALF} ${HUB.y}`}
              pathLength={1}
            />
          )}
        </g>
        <g className="setup-map-link" data-group="project" data-state={project ? "joined" : "open"}>
          <path
            className="setup-map-line"
            d={`M ${folder.x - TILE / 2} ${folder.y} L ${HUB.x + HUB_HALF} ${HUB.y}`}
            pathLength={1}
          />
          {project ? null : (
            <path
              className="setup-map-flow"
              d={`M ${folder.x - TILE / 2} ${folder.y} L ${HUB.x + HUB_HALF} ${HUB.y}`}
              pathLength={1}
            />
          )}
        </g>
      </svg>
      <div className="setup-map-hub">
        <Logo size={48} />
      </div>
      {BOTS.map((bot, index) => (
        <div
          key={bot.id}
          className="setup-map-tile"
          title={bot.name}
          style={place(bots[index] ?? HUB, index)}
        >
          <BotMark bot={bot.id} size={30} />
        </div>
      ))}
      {drawn.map((agent, index) => (
        <div
          key={agent.id}
          className="setup-map-tile"
          title={agent.name}
          data-state={agent.availability.state === "checking" ? "joining" : undefined}
          style={place(tiles[index] ?? HUB, BOTS.length + index)}
        >
          <AgentMark agent={agent.id} size={30} label={agent.name} ghost={ghosts} />
        </div>
      ))}
      <div
        className="setup-map-tile setup-map-folder"
        data-state={phone ? "joined" : "open"}
        title={phone ? "A phone for your agents" : "A phone for your agents, once set up"}
        style={place(handset, BOTS.length + drawn.length)}
      >
        <Smartphone size={20} aria-hidden="true" />
      </div>
      <div
        className="setup-map-tile setup-map-folder"
        data-state={project ? "joined" : "open"}
        title={project ?? "Your first project"}
        style={place(folder, BOTS.length + drawn.length + 1)}
      >
        <Folder size={20} aria-hidden="true" />
      </div>
      <figcaption className="sr-only">{label}</figcaption>
    </figure>
  );
}
