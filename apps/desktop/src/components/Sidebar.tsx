import {
  AGENT_BRAND_IDS,
  AGENT_BRANDS,
  type AgentBrandId,
  ORB_STATE_LABELS,
  type OrbState,
} from "@ghost/ui";
import { AgentMark, Wordmark } from "@ghost/ui/graphite";
import { ArrowLeft } from "lucide-react";
import { useId } from "react";
import { Select } from "./Select";

/**
 * The slim sidebar of Diagnostics: wordmark, the way back to chats, and the sample agent's live
 * mark. Its top strip is a window drag region, with room for
 * the macOS traffic lights.
 */
export function Sidebar({
  brandId,
  onBrandChange,
  orbState,
  onChats,
}: {
  readonly brandId: AgentBrandId;
  readonly onBrandChange: (brand: AgentBrandId) => void;
  readonly orbState: OrbState;
  /** Back to the chats. */
  readonly onChats: () => void;
}) {
  const brand = AGENT_BRANDS[brandId];
  const selectId = useId();
  return (
    <aside className="sidebar" aria-label="Sidebar">
      <div className="drag-strip" data-tauri-drag-region="" />
      <div className="wordmark" data-tauri-drag-region="">
        <Wordmark size={16} />
      </div>
      <nav aria-label="Main">
        <ul className="sidebar-nav">
          <li>
            <button type="button" className="nav-item" onClick={onChats}>
              <ArrowLeft size={16} aria-hidden="true" />
              Back to chats
            </button>
          </li>
        </ul>
      </nav>
      <div className="sidebar-fill" data-tauri-drag-region="" />
      <div className="agent-chip">
        <AgentMark
          agent={brand.id}
          state={orbState}
          size={32}
          label={`${brand.name} demo agent, ${ORB_STATE_LABELS[orbState]}`}
        />
        <div className="agent-chip-text">
          <label htmlFor={selectId} className="sr-only">
            Demo agent colour
          </label>
          <Select
            id={selectId}
            value={brandId}
            onChange={(event) => onBrandChange(event.currentTarget.value as AgentBrandId)}
          >
            {AGENT_BRAND_IDS.map((id) => (
              <option key={id} value={id}>
                {AGENT_BRANDS[id].name}
              </option>
            ))}
          </Select>
          <span className="agent-chip-state">Demo agent · {ORB_STATE_LABELS[orbState]}</span>
        </div>
      </div>
    </aside>
  );
}
