/**
 * @ghost/ui: the Aura design system shared by the desktop app and onghost.dev.
 *
 * Consumed as TypeScript source (no build step). Import the stylesheets once at the app root:
 *   import "@ghost/ui/fonts.css";
 *   import "@ghost/ui/tokens.css";
 */

export {
  AGENT_BRAND_IDS,
  AGENT_BRANDS,
  type AgentBrand,
  type AgentBrandId,
  type AgentBrandMotion,
  brandCssVariables,
  brandsEqual,
  getBrand,
  monogram,
} from "./brand";
export { hexToRgb, mixHex, parseHexColor, type Rgb, rgbToHex } from "./color";
export { AuraField, type AuraFieldProps } from "./components/AuraField";
export {
  AURA_INTENSITY,
  type AuraIntensity,
  type AuraLevel,
  auraLevel,
} from "./components/auraBlobs";
export { GlassPanel, type GlassPanelProps } from "./components/GlassPanel";
export { Orb, type OrbProps } from "./components/Orb";
export {
  type CubicBezier,
  durations,
  easings,
  prefersReducedMotion,
  REDUCED_MOTION_QUERY,
  type SpringPreset,
  setMotionPreference,
  springs,
  subscribeReducedMotion,
  usePrefersReducedMotion,
} from "./motion";
export { MAX_DPR } from "./orb/pacing";
export { orbPosterBackground } from "./orb/poster";
export {
  getOrbSupport,
  type OrbHandle,
  type OrbRegistration,
  type OrbSupport,
  registerOrb,
  subscribeOrbSupport,
} from "./orb/renderer";
export {
  ORB_STATE_LABELS,
  ORB_STATES,
  type OrbState,
  type OrbStateTarget,
  type OrbTint,
  orbStateTarget,
  TINT_RAMPS,
} from "./orb/states";
export { DEFAULT_SMOOTHING, squircleClipPath, squirclePath } from "./squircle";
export { elevationRadius, type GlassElevation, radii, zIndex } from "./tokens";
