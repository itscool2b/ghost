import type { OrbState } from "@ghost/ui";
import { OrbitSvg } from "@ghost/ui/orbit";
import { memo } from "react";
import { type Player, usePlayhead } from "../../kit";
import { type MarkTimeline, markFrame, signatureOf } from "./geometry";
import { useReduced } from "./ui";

/*
 * The Orbit mark (visual-language.md, section 5): the agent's own orbit, a node at its centre and a
 * square satellite, drawn the way X draws its orbital hairlines (a line, a node, a square), not as
 * a planet. Inline SVG, no glow, no blur. Below 48px the node is a filled dot and the orbit a
 * 1.25px line at 14 and 16px, so a small mark reads as a line around a node; from 48px the node is
 * hollow, filled with the surface beneath, and the far half of the orbit is the X Card's dashed
 * arc. There is no graticule below the 220px globe.
 */

/** A timeline holding one state since long ago: a still mark showing that state. */
function stillTimeline(state: OrbState): MarkTimeline {
  if (state === "idle") return { segments: [], ripples: [], streamedChars: 0 };
  return { segments: [{ from: -1e9, to: null, state }], ripples: [], streamedChars: 0 };
}

const STILL: Readonly<Record<OrbState, MarkTimeline>> = {
  idle: stillTimeline("idle"),
  thinking: stillTimeline("thinking"),
  streaming: stillTimeline("streaming"),
  tool: stillTimeline("tool"),
  waiting: stillTimeline("waiting"),
  error: stillTimeline("error"),
  done: stillTimeline("idle"),
};

export interface OrbitMarkProps {
  readonly agent: string;
  readonly size: number;
  /** The surface under the mark, which fills the body so the far side passes behind it. */
  readonly surface?: string;
  /** A still state (default idle). Ignored when `live` is given. */
  readonly state?: OrbState;
  /** A live mark: the turn's activity and the player whose playhead moves it. */
  readonly live?: { readonly timeline: MarkTimeline; readonly player: Player };
  /** A missing agent: every stroke faint, the orbit dashed, the satellite hollow. */
  readonly ghost?: boolean;
  /** The accessible name: the state as a sentence ("Claude Code, running pnpm test"). */
  readonly label?: string;
  readonly className?: string;
  /**
   * Draw the orbit in when the mark mounts (stroke-dashoffset, 400ms): the empty state's agent
   * switch. Key the mark on the agent so a new agent draws its own orbit. Skipped under reduced
   * motion.
   */
  readonly draw?: boolean;
}

/** An Orbit mark: live when given a timeline and a player, still otherwise. */
export function OrbitMark(props: OrbitMarkProps) {
  if (props.live) return <LiveMark {...props} live={props.live} />;
  return <StillMark {...props} />;
}

const StillMark = memo(function StillMark(props: OrbitMarkProps) {
  const reduced = useReduced();
  const signature = signatureOf(props.agent);
  const frame = markFrame(signature, STILL[props.state ?? "idle"], 0, props.size, reduced);
  return <OrbitSvg {...props} frame={frame} reduced={reduced} />;
});

/**
 * The playhead step a live mark reads: one frame. A step keeps the snapshot stable between React's
 * two reads in one render (a raw, moving clock would never settle), and 16ms is smooth enough.
 */
export const FRAME_MS = 16;

function LiveMark(props: OrbitMarkProps & { readonly live: NonNullable<OrbitMarkProps["live"]> }) {
  const reduced = useReduced();
  const t = usePlayhead(props.live.player, FRAME_MS);
  const frame = markFrame(signatureOf(props.agent), props.live.timeline, t, props.size, reduced);
  return <OrbitSvg {...props} frame={frame} reduced={reduced} />;
}
