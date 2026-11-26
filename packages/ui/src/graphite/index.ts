/**
 * Aura's shared foundation retains the Graphite import path for compatibility. Production
 * styling follows docs/design/visual-language.md. Import the stylesheet once at the app root:
 *   import "@ghost/ui/graphite.css"; // or graphite-tokens.css for the variables alone
 */

export { createFrameClock, type FrameClock, type FrameHost, frameClock } from "../frameClock";
export { AgentLogo, type AgentLogoProps, LogoSymbol, logoViewBox } from "./AgentLogo";
export { AgentMark, type AgentMarkProps, agentCode, logoSide } from "./AgentMark";
export { AGENT_LOGOS, type AgentLogo as AgentLogoData, agentLogo } from "./agentLogos";
// BotMark has an entry of its own, @ghost/ui/bot-mark: its app icons are files, and the website,
// which imports this one, must not carry them.
export { BOTS, type BotId, type BotKind, botNamed } from "./bots";
export { CopyButton, type CopyButtonProps } from "./CopyButton";
export { Keycap, type KeycapProps } from "./Keycap";
export { Logo, type LogoProps, Wordmark, type WordmarkProps } from "./Logo";
export {
  LOGO_EYE_PATHS,
  LOGO_OUTLINE_PATH,
  LOGO_PATH,
  LOGO_SMALL_EYE_PATHS,
  LOGO_SMALL_MAX,
  LOGO_SMALL_PATH,
  LOGO_VIEWBOX,
  type LogoSvgOptions,
  logoSvg,
} from "./logoPaths";
export { ModelMark, type ModelMarkProps } from "./ModelMark";
export { MODEL_MAKERS, type ModelMaker, type ModelMakerId, modelMaker } from "./modelMakers";
export {
  type LoaderTempo,
  type LoaderTone,
  PixelLoader,
  type PixelLoaderProps,
} from "./PixelLoader";
export {
  gridPath,
  gridRects,
  LOADER_ORDER,
  LOADER_STEP_MS,
  loaderFrame,
  type PixelRect,
} from "./pixels";
export { StatusDot, type StatusDotProps, type StatusTone } from "./StatusDot";
export {
  THEME_IDS,
  THEMES,
  type ThemeDefinition,
  type ThemeId,
  type ThemeSeed,
  themesCss,
  themeTokens,
} from "./themes";
export {
  contrastRatio,
  GRAPHITE_COLORS,
  GRAPHITE_DURATIONS,
  GRAPHITE_EASINGS,
  GRAPHITE_LIGHT_COLORS,
  GRAPHITE_STAGGERS,
  GRAPHITE_THEMES,
  type GraphiteColor,
  relativeLuminance,
} from "./tokens";
export { Wing, type WingProps } from "./Wing";
