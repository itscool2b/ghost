/**
 * Everything a direction needs, in one import: `import { usePlayhead, parseMarkdown } from "../../kit"`.
 * This is the lab's contract with the directions; the files behind it may move, this list stays.
 * Brands, orbs, the aura field, glass, tokens and motion presets come from `@ghost/ui`.
 */

export type { Direction, DirectionCard, DirectionProps } from "./directions/types";
export type {
  AgentEffort,
  AgentInfo,
  AgentModel,
  Autonomy,
  ChangesScope,
  ChatStatus,
  ChatSummary,
  Composer,
  Cost,
  DiscoveredAgent,
  DiscoveryEvent,
  DiscoveryModel,
  DiscoveryTimeline,
  LimitWindow,
  PermissionOption,
  PermissionOptionKind,
  PlanEntry,
  PlanStatus,
  Project,
  RightPanel,
  RightPanelTab,
  StopReason,
  TokenUsage,
  ToolContent,
  ToolKind,
  ToolLocation,
  ToolStatus,
  UsageInfo,
  Workspace,
} from "./fixture/types";
export {
  AUTONOMY_LABELS,
  allChats,
  composerLabel,
  effortLabel,
  findAgent,
  findChat,
  findModel,
  findProject,
  modelLabel,
} from "./fixture/workspace";
export { stageHref } from "./lab/route";
export { CORE_STATES, LAB_STATES, type LabState, STATE_INFO } from "./lab/states";
export { LAB_THEMES, type LabTheme } from "./lab/theme";
export {
  ANSI_COLOR_NAMES,
  type AnsiColor,
  type AnsiColorName,
  type AnsiLine,
  type AnsiSpan,
  stripAnsi,
} from "./model/ansi";
export type { DiffHunk, DiffLine, FileDiff } from "./model/diff";
export { discoveryCounts } from "./model/discovery";
export {
  basename,
  dirname,
  formatCost,
  formatCount,
  formatDuration,
  formatElapsed,
  plural,
} from "./model/format";
export {
  type Block as MarkdownBlockNode,
  type Inline,
  type ListItem,
  type MarkdownBlock,
  parseInline,
  parseMarkdown,
  plainText,
} from "./model/markdown";
export type { ToolFamily } from "./model/tools";
export type * from "./model/types";
export { type Player, type PlayerConfig, usePlayer, usePlayhead } from "./model/usePlayer";
export {
  TITLEBAR_HEIGHT,
  TRAFFIC_LIGHTS,
  WINDOW_HEIGHT,
  WINDOW_RADIUS,
  WINDOW_WIDTH,
} from "./shared/window";
