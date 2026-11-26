/** Every agent's mark at every size and state, for checking the published logos by eye. */
import "../styles.css";
import "../ui-tokens.css";
import { AGENT_BRAND_IDS, AGENT_BRANDS } from "@ghost/ui";
import { AgentMark } from "@ghost/ui/graphite";
import type { CSSProperties } from "react";
import { createRoot } from "react-dom/client";

const SIZES = [14, 16, 20, 24, 32, 48, 56] as const;
const STATES = ["idle", "thinking", "waiting", "done", "error"] as const;
const AGENTS = [...AGENT_BRAND_IDS.filter((id) => id !== "generic"), "my-acp-agent"];

const cell: CSSProperties = {
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
  minWidth: 64,
  height: 64,
};

function AgentMarksPreview() {
  return (
    <main
      data-testid="agent-marks"
      style={{
        padding: 32,
        background: "var(--g-bg)",
        color: "var(--g-fg-2)",
        font: "14px/20px var(--g-sans)",
        minHeight: "100vh",
      }}
    >
      <table style={{ borderCollapse: "collapse" }}>
        <thead>
          <tr>
            <th style={{ textAlign: "left", width: 140 }}>Agent</th>
            {SIZES.map((size) => (
              <th key={size}>{size}</th>
            ))}
            <th>Pill 24</th>
            {STATES.slice(1).map((state) => (
              <th key={state}>{state}</th>
            ))}
            <th>Not installed</th>
          </tr>
        </thead>
        <tbody>
          {AGENTS.map((id) => (
            <tr key={id} data-agent-row={id}>
              <td style={{ color: "var(--g-fg-1)" }}>
                {AGENT_BRANDS[id as keyof typeof AGENT_BRANDS]?.name ?? id}
              </td>
              {SIZES.map((size) => (
                <td key={size}>
                  <span style={cell}>
                    <AgentMark agent={id} size={size} />
                  </span>
                </td>
              ))}
              <td>
                <span style={cell}>
                  <span
                    style={{
                      display: "inline-flex",
                      alignItems: "center",
                      gap: 8,
                      height: 32,
                      padding: "0 12px 0 4px",
                      borderRadius: 999,
                      boxShadow: "inset 0 0 0 1px var(--g-line-strong)",
                      color: "var(--g-fg-1)",
                      whiteSpace: "nowrap",
                    }}
                  >
                    <AgentMark agent={id} size={24} className="preview-round" />
                    {AGENT_BRANDS[id as keyof typeof AGENT_BRANDS]?.name ?? id}
                  </span>
                </span>
              </td>
              {STATES.slice(1).map((state) => (
                <td key={state}>
                  <span style={cell}>
                    <AgentMark agent={id} size={24} state={state} />
                  </span>
                </td>
              ))}
              <td>
                <span style={cell}>
                  <AgentMark agent={id} size={32} ghost />
                </span>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <style>{".preview-round { border-radius: 999px !important; }"}</style>
    </main>
  );
}

export function mountAgentMarksPreview(container: HTMLElement) {
  document.documentElement.classList.add("g-root");
  createRoot(container).render(<AgentMarksPreview />);
}
