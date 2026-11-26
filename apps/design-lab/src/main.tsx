// First: wraps matchMedia so `?motion=reduce` reaches everything that reads the preference.
import "./lab/reducedMotion";
import "./styles.css";
import { getOrbSupport } from "@ghost/ui";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { DIRECTIONS } from "./directions";
import { Lab } from "./lab/Lab";
import { LAB_STATES } from "./lab/states";
import type { Player } from "./model/usePlayer";

declare global {
  interface Window {
    /** What scripts/shoot.mjs reads: the directions and states, and the open stage's player. */
    __designLab?: {
      readonly directions: readonly {
        readonly id: string;
        readonly name: string;
        readonly tagline: string;
        /** The states it renders: the shoot script skips the others. */
        readonly states: readonly string[];
        /** The themes it renders: the shoot script skips the others. */
        readonly themes: readonly string[];
        readonly wallpaper: string;
      }[];
      readonly states: readonly string[];
      player?: Player;
    };
  }
}

window.__designLab = {
  directions: DIRECTIONS.map(({ id, name, tagline, states, themes, wallpaper }) => ({
    id,
    name,
    tagline,
    states,
    themes,
    wallpaper,
  })),
  states: LAB_STATES,
};

// Probe WebGL2 and compile the orb shader before the first orb renders.
getOrbSupport();

const container = document.getElementById("root");
if (container) {
  createRoot(container).render(
    <StrictMode>
      <Lab />
    </StrictMode>,
  );
}
