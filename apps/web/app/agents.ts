import { AGENT_BRAND_IDS, AGENT_BRANDS } from "@ghost/ui";

/** Every agent with a designed brand, by display name, in brand.ts order (never the generic one). */
export const AGENT_NAMES: readonly string[] = AGENT_BRAND_IDS.filter((id) => id !== "generic").map(
  (id) => AGENT_BRANDS[id].name,
);
