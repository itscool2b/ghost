import { squirclePath } from "@ghost/ui";
import {
  type CSSProperties,
  type HTMLAttributes,
  type Ref,
  useCallback,
  useId,
  useLayoutEffect,
  useRef,
} from "react";
import { cx } from "./ui";

/**
 * Aurora's glass levels. 0 is a well carved into the surface it sits on (tool runs, code, diffs);
 * 1 is the frosted sidebar; 2 the floating transcript sheet and the plan; 3 the composer and every
 * popover. Higher levels are denser, crisper at the rim, and cast further.
 */
export type GlassLevel = 0 | 1 | 2 | 3;

type GlassTag = "div" | "aside" | "section" | "header" | "nav";

export interface GlassProps extends HTMLAttributes<HTMLElement> {
  readonly level: GlassLevel;
  /** Corner radius in CSS px; the corner is a squircle with this radius. */
  readonly radius: number;
  readonly smoothing?: number;
  /**
   * Frosts what is behind the surface (backdrop-filter, clipped to the squircle). Only for small
   * overlays that float over text: the large panels sit on the already blurred aura, where a blur
   * would cost GPU time every frame and change nothing visible.
   */
  readonly frost?: boolean;
  readonly as?: GlassTag;
  readonly ref?: Ref<HTMLElement>;
}

/**
 * A glass surface with continuous corners: a squircle-clipped body (fill, specular light, optional
 * frost), a 1px gradient rim stroked on the same path inside the clip (so the clip never shaves it),
 * and a shadow on an unclipped layer behind. Children are not clipped, so menus can escape.
 *
 * The path follows the element's size through a ResizeObserver and is written straight to the DOM,
 * so expanding a card costs no React render.
 */
export function Glass({
  level,
  radius,
  smoothing = 0.6,
  frost = false,
  as: Tag = "div",
  className,
  style,
  children,
  ref,
  ...rest
}: GlassProps) {
  const rootRef = useRef<HTMLElement | null>(null);
  const bodyRef = useRef<HTMLSpanElement>(null);
  const svgRef = useRef<SVGSVGElement>(null);
  const rimRef = useRef<SVGPathElement>(null);
  const gradientId = `au-rim-${useId().replace(/[^\w-]/g, "")}`;

  const setRoot = useCallback(
    (node: HTMLElement | null) => {
      rootRef.current = node;
      if (typeof ref === "function") ref(node);
      else if (ref) (ref as { current: HTMLElement | null }).current = node;
    },
    [ref],
  );

  useLayoutEffect(() => {
    const root = rootRef.current;
    const body = bodyRef.current;
    if (!root || !body) return;
    let width = -1;
    let height = -1;
    const apply = (nextWidth: number, nextHeight: number): void => {
      if (Math.abs(nextWidth - width) < 0.25 && Math.abs(nextHeight - height) < 0.25) return;
      width = nextWidth;
      height = nextHeight;
      const path = squirclePath(width, height, radius, smoothing);
      const clip = `path("${path}")`;
      body.style.setProperty("-webkit-clip-path", clip);
      body.style.setProperty("clip-path", clip);
      svgRef.current?.setAttribute("viewBox", `0 0 ${width} ${height}`);
      rimRef.current?.setAttribute("d", path);
    };
    apply(root.offsetWidth, root.offsetHeight);
    if (typeof ResizeObserver !== "function") return;
    const observer = new ResizeObserver((entries) => {
      const box = entries[entries.length - 1]?.borderBoxSize?.[0];
      if (box) apply(box.inlineSize, box.blockSize);
      else apply(root.offsetWidth, root.offsetHeight);
    });
    observer.observe(root, { box: "border-box" });
    return () => observer.disconnect();
  }, [radius, smoothing]);

  return (
    <Tag
      {...rest}
      ref={setRoot}
      className={cx("au-glass", className)}
      data-glass={level}
      data-frost={frost ? "" : undefined}
      style={{ "--au-r": `${radius}px`, ...style } as CSSProperties}
    >
      {level > 0 ? <span className="au-glass-shadow" aria-hidden="true" /> : null}
      <span ref={bodyRef} className="au-glass-body" aria-hidden="true">
        <svg
          ref={svgRef}
          className="au-glass-rim"
          preserveAspectRatio="none"
          aria-hidden="true"
          focusable="false"
        >
          <defs>
            <linearGradient id={gradientId} x1="0" y1="0" x2="0" y2="1">
              <stop offset="0" className="au-rim-0" />
              <stop offset="0.14" className="au-rim-1" />
              <stop offset="0.62" className="au-rim-2" />
              <stop offset="1" className="au-rim-3" />
            </linearGradient>
          </defs>
          <path ref={rimRef} fill="none" stroke={`url(#${gradientId})`} strokeWidth={2} />
        </svg>
      </span>
      {children}
    </Tag>
  );
}
