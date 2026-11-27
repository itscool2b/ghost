import { AGENT_BRANDS, type AgentBrandId } from "@ghost/ui";
import type { CSSProperties } from "react";
import { SpriteMark } from "./Sprite";

/**
 * Pieces of one chat handed from agent to agent, floating at three depths around the panel's
 * figure: the near ones sharp, the far ones small and out of focus (X Money's field of cards). `x`
 * and `y` place a card's centre in the panel, in percent. A card with an agent shows its mark only
 * when near, sharp and at its own size.
 */
const CARDS: readonly {
  x: number;
  y: number;
  depth: 0 | 1 | 2;
  agent?: AgentBrandId;
  title: string;
  line: string;
}[] = [
  { x: 18, y: 20, depth: 0, agent: "claude-code", title: "Claude Code", line: "Thought for 2.2 s" },
  { x: 46, y: 13, depth: 2, title: "Ran cargo test pacing", line: "14 passed" },
  { x: 80, y: 19, depth: 1, title: "Handed to Codex", line: "The chat goes with it" },
  { x: 13, y: 78, depth: 1, title: "Edited src/pacing.ts", line: "+12 −3" },
  { x: 52, y: 86, depth: 2, title: "You", line: "Measure it on the slow machine." },
  { x: 80, y: 76, depth: 0, agent: "pi", title: "Pi", line: "p99 16.4 ms, inside one frame" },
];

/**
 * The handoff, as X Money shows its rate: the heading in the column's right half, then one wide
 * panel with a single figure in the middle and pieces of a chat floating around it at three
 * depths, drifting. Decorative pieces; the figure and the line under it say what they show.
 */
export function Handoff() {
  return (
    <section className="section stat" id="handoff" aria-labelledby="handoff-title" data-zone="">
      <div className="wrap">
        <div className="stat-head">
          <h2 id="handoff-title" className="title">
            Switch agents mid-chat. <span className="tone">The whole conversation goes along.</span>
          </h2>
        </div>
        <div className="panel stat-panel">
          <div className="stat-field" aria-hidden="true">
            {CARDS.map((card, index) => (
              <div
                key={card.title}
                className="float-card"
                data-depth={card.depth}
                style={{ "--x": `${card.x}%`, "--y": `${card.y}%`, "--n": index } as CSSProperties}
              >
                {card.agent && card.depth === 0 ? (
                  <SpriteMark agent={card.agent} size={20} />
                ) : null}
                <span className="float-copy">
                  <strong>{card.agent ? AGENT_BRANDS[card.agent].name : card.title}</strong>
                  <span>{card.line}</span>
                </span>
              </div>
            ))}
          </div>
          <p className="stat-figure">
            One chat. <span className="tone">Every agent.</span>
          </p>
          <p className="stat-note">
            Hand the chat to another agent at any turn. The handoff stays in view, so you always
            know who did what.
          </p>
        </div>
      </div>
    </section>
  );
}
