import { AGENT_BRANDS } from "@o7/ui";
import type { CSSProperties } from "react";
import { Flock } from "./Flock";
import { AGENTS, SpriteMark } from "./Sprite";
import { Wing } from "./Wing";

/**
 * What o7 is, as dot.com says what its product is: a large title at left and two paragraphs
 * in ink at right, then a flock of o7's wing in colour across the column, where dot.com sets its
 * row of coloured bots (Flock.tsx), and under it every agent in one row. The agents' marks stay at
 * the website's 24px (third-party/brand-marks.md), each beside its name. The row repeats the chips
 * further down, so it is hidden from assistive technology. As dot.com's bots do, the wings and
 * then the agents arrive one after another when their rows scroll in, and a faint wing crosses
 * behind the section as the page moves past it.
 */
export function Statement() {
  return (
    <section className="statement" aria-labelledby="statement-title">
      <Wing className="flyby" />
      <div className="wrap statement-grid">
        <h2 id="statement-title" className="title-xl">
          A home for the agents you already use
        </h2>
        <div className="statement-text">
          <p>
            Claude Code, Codex, Grok Build, Cursor and the rest run side by side in one desktop app,
            on your machine, through their own sign-ins.
          </p>
          <p>
            Steer them from your phone, or link Grok Bot, Muse, Hermes or OpenClaw and hand your own
            assistant whole tasks: each runs on its own branch, gets a second agent's review, and
            your assistant hears when it is done. Chats and history stay on your computer, with
            nothing to sign up for and nothing to pay.
          </p>
        </div>
      </div>
      <Flock />
      <div className="wrap statement-roster" aria-hidden="true" data-reveal="">
        {AGENTS.map((agent, i) => (
          <span key={agent} className="statement-agent" style={{ "--i": i } as CSSProperties}>
            <SpriteMark agent={agent} size={24} />
            {AGENT_BRANDS[agent].name}
          </span>
        ))}
      </div>
    </section>
  );
}
