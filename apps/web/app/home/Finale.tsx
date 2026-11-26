import { Wing } from "./Wing";

/**
 * The page's last call: one two-tone line at the left edge, the two pills, and the wing drawn very
 * large in one hairline under them, running off into the footer. The line draws itself when the
 * section arrives, and the eyes blink and look around.
 */
export function Finale() {
  return (
    <section className="finale" aria-labelledby="finale-title">
      <div className="wrap finale-head">
        <h2 id="finale-title" className="finale-title">
          Your agents. <span className="tone">Your machine.</span>
        </h2>
        <div className="finale-actions">
          <a className="btn btn-primary btn-lg" href="/download">
            Download for free
          </a>
          <a className="btn btn-quiet btn-lg" href="/changelog">
            Read the changelog
          </a>
        </div>
      </div>
      <div className="wrap finale-art" aria-hidden="true" data-reveal="" data-zone="">
        <Wing line look className="finale-wing" />
      </div>
    </section>
  );
}
