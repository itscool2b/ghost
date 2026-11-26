import { AgentMark, Logo } from "@ghost/ui/graphite";
import { Check } from "lucide-react";
import type { AgentPhase } from "./AgentRow";
import "./agent-connect.css";

/** What the picture shows of one agent: connected, waiting on a sign-in, connecting or failing. */
export type ConnectState = "connected" | "signin" | "connecting" | "failed";

export interface ConnectAgent {
  readonly id: string;
  readonly name: string;
  readonly state: ConnectState;
}

/** The most agents the picture draws; the list under it names them all. */
export const CONNECT_SHOWN = 8;

/** A row's phase as the picture shows it; an agent that is not here is not drawn. */
export function connectState(phase: AgentPhase, busy: boolean): ConnectState | null {
  if (busy) return "connecting";
  switch (phase) {
    case "ready":
      return "connected";
    case "signin":
      return "signin";
    case "checking":
      return "connecting";
    case "broken":
      return "failed";
    case "missing":
      return null;
  }
}

const WIDTH = 600;
const HEIGHT = 152;
const HUB = { x: WIDTH / 2, y: HEIGHT / 2 };
const HUB_RADIUS = 33;
const TILE = 44;

/** Where each agent's tile sits: alternately left and right of the hub, on a shallow arc. */
export function placeTiles(count: number): { x: number; y: number }[] {
  const places: { x: number; y: number }[] = [];
  const left = Math.ceil(count / 2);
  const right = count - left;
  const side = (n: number, index: number, direction: -1 | 1) => {
    const x = HUB.x + direction * (112 + index * 60);
    // Rows above and below the hub's line alternate, so lines never cross a neighbour's tile.
    const y = n === 1 ? HUB.y : HUB.y + (index % 2 === 0 ? -26 : 26);
    return { x, y };
  };
  for (let i = 0; i < left; i++) places.push(side(left, i, -1));
  for (let i = 0; i < right; i++) places.push(side(right, i, 1));
  return places;
}

/**
 * The agents' marks on tiles, each joined to Ghost's logo (owner, 17 November 2026: "when
 * you're connecting your agent, add some cool animations of the different logos of the agents").
 * Tiles rise in one after another and their lines draw toward the logo; a connecting agent's line
 * runs toward it and its tile breathes, a connected one's line settles with a check at its middle,
 * one that needs a sign-in waits on a dashed line with a dot, and one that failed shows a red
 * dot. Whole tiles move and nothing is ever drawn over a mark (third-party/brand-marks.md).
 * Reduced motion shows the finished picture.
 */
export function AgentConnect({ agents }: { readonly agents: readonly ConnectAgent[] }) {
  const shown = agents.slice(0, CONNECT_SHOWN);
  const places = placeTiles(shown.length);
  const connected = agents.filter((agent) => agent.state === "connected").length;
  const label =
    agents.length === 0
      ? "No agents are connected yet."
      : `${connected} of ${agents.length} agents connected to Ghost.`;
  return (
    <figure className="agent-connect" aria-label={label} data-testid="agent-connect">
      <svg
        className="agent-connect-lines"
        viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
        preserveAspectRatio="xMidYMid meet"
        aria-hidden="true"
      >
        {shown.map((agent, index) => {
          const place = places[index] ?? HUB;
          const from = {
            x: place.x + (place.x < HUB.x ? TILE / 2 : -TILE / 2),
            y: place.y,
          };
          const to = { x: HUB.x + (place.x < HUB.x ? -HUB_RADIUS : HUB_RADIUS), y: HUB.y };
          const bend = { x: (from.x + to.x) / 2, y: (from.y + to.y) / 2 + (place.y - HUB.y) / 2 };
          const path = `M ${from.x} ${from.y} Q ${bend.x} ${bend.y} ${to.x} ${to.y}`;
          const middle = {
            x: 0.25 * from.x + 0.5 * bend.x + 0.25 * to.x,
            y: 0.25 * from.y + 0.5 * bend.y + 0.25 * to.y,
          };
          return (
            <g
              key={agent.id}
              className="agent-connect-link"
              data-state={agent.state}
              style={{ "--connect-order": index } as React.CSSProperties}
            >
              <path className="agent-connect-line" d={path} pathLength={1} />
              {agent.state === "connecting" ? (
                <path className="agent-connect-flow" d={path} pathLength={1} />
              ) : null}
              {agent.state === "signin" || agent.state === "failed" ? (
                <circle
                  className="agent-connect-dot"
                  cx={middle.x}
                  cy={middle.y}
                  r={3.5}
                  data-state={agent.state}
                />
              ) : null}
              {agent.state === "connected" ? (
                <g
                  className="agent-connect-check"
                  transform={`translate(${middle.x - 7} ${middle.y - 7})`}
                >
                  <circle cx={7} cy={7} r={7} />
                  <Check x={3} y={3} width={8} height={8} strokeWidth={3} />
                </g>
              ) : null}
            </g>
          );
        })}
      </svg>
      <div className="agent-connect-hub">
        <Logo size={44} />
      </div>
      {shown.map((agent, index) => {
        const place = places[index] ?? HUB;
        return (
          <div
            key={agent.id}
            className="agent-connect-tile"
            data-state={agent.state}
            title={agent.name}
            style={
              {
                "--connect-x": `${(place.x / WIDTH) * 100}%`,
                "--connect-y": `${(place.y / HEIGHT) * 100}%`,
                "--connect-order": index,
              } as React.CSSProperties
            }
          >
            <AgentMark agent={agent.id} size={30} label={agent.name} />
          </div>
        );
      })}
      <figcaption className="sr-only">{label}</figcaption>
    </figure>
  );
}
