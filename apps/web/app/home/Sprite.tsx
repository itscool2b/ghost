import { AGENT_BRAND_IDS, type AgentBrandId } from "@ghost/ui";
import { agentCode, agentLogo, LogoSymbol, logoSide } from "@ghost/ui/graphite";

/** Every agent with a designed brand, in brand order (never the generic one). */
export const AGENTS: readonly AgentBrandId[] = AGENT_BRAND_IDS.filter((id) => id !== "generic");

const symbolId = (agent: string) => `agent-logo-${agent}`;

/**
 * Each agent's own mark, defined once for the whole page. The home page shows the marks dozens of
 * times; drawing every copy inline made the HTML four times heavier (one mark is 50 KB of paths).
 */
export function AgentSprite() {
  return (
    <svg className="sprite" width="0" height="0" aria-hidden="true" focusable="false">
      <defs>
        {AGENTS.map((agent) => {
          const logo = agentLogo(agent);
          return logo ? <LogoSymbol key={agent} logo={logo} id={symbolId(agent)} /> : null;
        })}
      </defs>
    </svg>
  );
}

/**
 * An agent's tile, as the desktop's AgentMark draws it at rest, pointing at the sprite. Decorative:
 * the agent's name always sits beside it.
 */
export function SpriteMark({ agent, size = 24 }: { agent: AgentBrandId; size?: number }) {
  const logo = agentLogo(agent);
  const side = logoSide(size);
  return (
    <span
      className="g-agent-mark"
      data-agent={agent}
      data-state="idle"
      data-logo={logo ? "" : undefined}
      aria-hidden="true"
      style={{
        width: `${size}px`,
        height: `${size}px`,
        borderRadius: `${Math.max(3, Math.round(size * 0.22))}px`,
      }}
    >
      {logo ? (
        <svg
          className="g-agent-mark-logo"
          width={side}
          height={side}
          aria-hidden="true"
          focusable="false"
        >
          <use href={`#${symbolId(agent)}`} />
        </svg>
      ) : (
        <span className="g-agent-mark-glyph">{agentCode(agent)}</span>
      )}
    </span>
  );
}
