import { AGENT_BRANDS } from "@ghost/ui";
import { AGENTS, SpriteMark } from "./Sprite";

/**
 * Every agent Ghost runs, as one table of hairline cells: each agent's own mark beside its name, and
 * a last cell for any agent that speaks the Agent Client Protocol. The marks are the page's one
 * run of colour outside the product views, and each is its agent's own. The list is plain text
 * with pictures, whole without scripts, and it holds still.
 */
export function Roster() {
  return (
    <section className="section roster" id="agents" aria-labelledby="roster-title">
      <div className="wrap">
        <div className="chapter-head">
          <h2 id="roster-title" className="title">
            Nine agents. <span className="tone">Each one as it is.</span>
          </h2>
          <p className="lead">
            Every agent runs through its own harness and your own sign-in. Ghost never reads, stores
            or proxies its credentials, so what you pay for and what you are signed in to stay
            between you and it.
          </p>
        </div>
        <ul className="roster-grid" aria-label="Agents Ghost runs">
          {AGENTS.map((agent) => (
            <li key={agent} className="roster-agent">
              <SpriteMark agent={agent} size={28} />
              <span>{AGENT_BRANDS[agent].name}</span>
            </li>
          ))}
          <li className="roster-more">
            <span className="roster-plus" aria-hidden="true">
              +
            </span>
            <span>Any agent that speaks ACP</span>
          </li>
        </ul>
      </div>
    </section>
  );
}
