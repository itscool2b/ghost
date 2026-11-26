import { useEffect, useRef, useState } from "react";
import catalog from "#release-catalog";
import { GitHubMark, REPOSITORY } from "../repo";
import { HeroWindow, useHandoffLoop } from "./HeroWindow";
import { Stage } from "./Stage";
import { Wing } from "./Wing";

/**
 * Whether installers are published. Until they are, the action does not name the visitor's system:
 * "Download for Windows" would promise a file the download page cannot give yet.
 */
const PUBLISHED = catalog.release !== null;

/** The visitor's system, once the page runs, so the action can name it. Null until then. */
function usePlatform(): string | null {
  const [platform, setPlatform] = useState<string | null>(null);
  useEffect(() => {
    const agent = navigator.userAgent;
    if (/Android|iPhone|iPad|iPod/.test(agent)) return;
    if (/Windows/.test(agent)) setPlatform("Windows");
    else if (/Macintosh|Mac OS X/.test(agent)) setPlatform("macOS");
    else if (/Linux|X11/.test(agent)) setPlatform("Linux");
  }, []);
  return platform;
}

const FACTS = ["Free", "MIT licensed", "No account", "Windows, macOS and Linux"] as const;

/**
 * The page's opening, at the left edge: the name in ink and what it is in grey as one heading, a
 * paragraph, the two pills and one line of facts. Under them the stage: a field of contour lines
 * with the wing flying over it and the app itself standing on it, running off the stage's foot.
 * The window is live: one session replayed, handed from agent to agent. The lines arrive one after
 * another by position alone, so the words are readable from the first frame.
 */
export function Hero() {
  const stage = useRef<HTMLDivElement>(null);
  const state = useHandoffLoop(stage);
  const platform = usePlatform();

  return (
    <section className="hero" aria-labelledby="hero-title" data-zone="">
      <div className="wrap hero-copy">
        <h1 id="hero-title" className="hero-title">
          <span className="hero-line">Meet o7.</span>{" "}
          <span className="hero-line tone">One window for every coding agent.</span>
        </h1>
        <p className="hero-lead">
          Run Claude Code, Codex, Cursor and six more side by side on your own machine. Hand a chat
          from one to the next, see every change they make, and steer it all from your phone.
        </p>
        <div className="hero-actions">
          <a className="btn btn-primary btn-lg" href="/download">
            {PUBLISHED && platform ? `Download for ${platform}` : "Download"}
          </a>
          <a className="btn btn-quiet btn-lg" href={REPOSITORY}>
            <GitHubMark />
            Star on GitHub
          </a>
        </div>
        <ul className="hero-facts">
          {FACTS.map((fact) => (
            <li key={fact}>{fact}</li>
          ))}
        </ul>
      </div>
      <div className="wrap hero-stage" ref={stage}>
        <Stage className="hero-field">
          <span className="hero-wing" aria-hidden="true">
            <span className="hero-wing-float">
              <Wing look />
            </span>
          </span>
          <div className="hero-detail" aria-hidden="true">
            <HeroWindow state={state} />
          </div>
        </Stage>
      </div>
    </section>
  );
}
