import { useRef } from "react";
import { HeroWindow, useHandoffLoop } from "./HeroWindow";

/**
 * The app itself, as dot.com shows its own under the opening, standing in one of X Money's panels:
 * the live window replays a real handoff (a message typed, the agent thinking, a tool, the reply,
 * the next message handed to another agent) and runs off the panel's foot. Without scripts, or
 * where motion is not welcome, it is the printed frame. Decorative: the page says in words what it
 * shows.
 */
export function Showcase() {
  const panel = useRef<HTMLDivElement>(null);
  const state = useHandoffLoop(panel);
  return (
    <section className="showcase" aria-label="Ghost at work" data-zone="">
      <div className="wrap">
        <div className="panel showcase-panel" ref={panel}>
          <div className="showcase-window" aria-hidden="true">
            <HeroWindow state={state} />
          </div>
        </div>
      </div>
    </section>
  );
}
