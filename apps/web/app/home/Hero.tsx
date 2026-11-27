import { useEffect, useState } from "react";
import catalog from "#release-catalog";
import { GitHubMark, REPOSITORY } from "../repo";
import { HeroWing } from "./HeroWing";
import { Words } from "./Words";

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

/**
 * The page's opening, as X Money opens its own: one statement in the middle whose words flip up
 * into place one after another, the object under it (the wing as a solid slab, HeroWing.tsx), then
 * one grey line with its point in ink and the two pills, Download naming the visitor's system as
 * dot.com's does.
 */
export function Hero() {
  const platform = usePlatform();
  return (
    <section className="hero" id="top" aria-labelledby="hero-title" data-zone="">
      <div className="wrap hero-inner">
        <h1 id="hero-title" className="hero-title">
          <span className="hero-line">
            <Words text="Every coding agent," />
          </span>{" "}
          <span className="hero-line">
            <Words text="in one window." from={3} />
          </span>
        </h1>
        <HeroWing />
        <p className="hero-lead">
          Ghost ADE is free for <span>Windows, macOS and Linux</span>. No account, MIT licensed.
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
      </div>
    </section>
  );
}
