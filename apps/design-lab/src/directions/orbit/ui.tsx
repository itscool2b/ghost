import { usePrefersReducedMotion } from "@ghost/ui";
import type { LucideIcon } from "lucide-react";
import { type ButtonHTMLAttributes, type ReactNode, type Ref, useEffect } from "react";
import { type Autonomy, formatElapsed, type Player, usePlayhead } from "../../kit";
import { diffSquares, splitCents } from "./text";

/*
 * Orbit's shared components (visual-language.md 3.9 and 5.6): keycaps, the 2x3 dot loader, pips,
 * diffstats, the autonomy level glyph, status pills, icon buttons, and numerals with built
 * superscript decimals. All flat: hairlines and value steps, no glow.
 */

/** Reduced motion from the OS or `?motion=reduce` (the lab routes both through matchMedia). */
export function useReduced(): boolean {
  return usePrefersReducedMotion();
}

/** A keycap: 18px, Geist Mono 10.5, ink 6% with a 10% ring (or on-ink tones on an ink button). */
export function Keycap({
  children,
  onInk = false,
  label,
}: {
  readonly children: ReactNode;
  readonly onInk?: boolean;
  /** What the key is called, for assistive tech, when the glyph alone is a symbol. */
  readonly label?: string;
}) {
  return (
    <kbd className="o-key" data-on-ink={onInk ? "" : undefined} aria-label={label}>
      {children}
    </kbd>
  );
}

/**
 * Grok Build's 2x3 dot loader: a light walks the grid in 0.66s. Under reduced motion, dots 1, 2
 * and 4 stay lit.
 */
export function DotLoader({ label = "Working" }: { readonly label?: string }) {
  return (
    <span className="o-loader" role="img" aria-label={label}>
      {[0, 1, 2, 3, 4, 5].map((index) => (
        <span key={index} className="o-loader-dot" data-dot={index} />
      ))}
    </span>
  );
}

export type PipKind = "running" | "needs" | "done" | "error" | "idle";

/** The list-sized state in an 8px slot (section 5.6). */
export function Pip({ kind }: { readonly kind: PipKind }) {
  if (kind === "idle") return null;
  if (kind === "running") return <DotLoader />;
  if (kind === "error") {
    return (
      <svg className="o-pip o-pip-error" viewBox="0 0 8 8" aria-label="Failed" role="img">
        <path d="M1.5 1.5l5 5M6.5 1.5l-5 5" />
      </svg>
    );
  }
  return (
    <span
      className="o-pip"
      data-kind={kind}
      role="img"
      aria-label={kind === "needs" ? "Needs you" : "Done, not seen yet"}
    />
  );
}

/** Five (or ten) 6px diffstat squares: green share, red share, the rest ink 12%. */
export function DiffSquares({
  added,
  removed,
  count = 5,
}: {
  readonly added: number;
  readonly removed: number;
  readonly count?: number;
}) {
  return (
    <span className="o-squares" aria-hidden="true">
      {diffSquares(added, removed, count).map((tone, index) => (
        // biome-ignore lint/suspicious/noArrayIndexKey: the squares are positional.
        <span key={index} className="o-square" data-tone={tone} />
      ))}
    </span>
  );
}

/** "+59 −23" in mono, green and red; the minus is a real minus sign. */
export function DiffCounts({
  added,
  removed,
  size = 12,
}: {
  readonly added: number;
  readonly removed: number;
  readonly size?: 11 | 12;
}) {
  return (
    <span className="o-counts" data-size={size}>
      <span className="o-add">+{added}</span>
      <span className="o-del">
        {"−"}
        {removed}
      </span>
    </span>
  );
}

/**
 * The autonomy level glyph (Family's three-level stepper): three 2px ticks, 4, 6 and 8px tall. Ask
 * fills one, Auto-edit two, Full auto three; empty ticks are text-4. An instrument reading.
 */
export function LevelGlyph({ autonomy }: { readonly autonomy: Autonomy }) {
  const level = autonomy === "ask" ? 1 : autonomy === "auto_edit" ? 2 : 3;
  return (
    <svg className="o-level" viewBox="0 0 10 8" width="10" height="8" aria-hidden="true">
      {[0, 1, 2].map((index) => (
        <rect
          key={index}
          x={index * 4}
          y={8 - (4 + index * 2)}
          width="2"
          height={4 + index * 2}
          data-on={index < level ? "" : undefined}
        />
      ))}
    </svg>
  );
}

export type Tone = "ink" | "signal" | "green" | "red";

/** A status pill: tone text on the tone at 12% (16% dark), with a 5px square of the tone. */
export function StatusPill({
  tone,
  children,
  running = false,
}: {
  readonly tone: Tone;
  readonly children: ReactNode;
  readonly running?: boolean;
}) {
  return (
    <span className="o-status" data-tone={tone}>
      {running ? <DotLoader /> : <span className="o-status-mark" aria-hidden="true" />}
      {children}
    </span>
  );
}

type IconButtonProps = Omit<ButtonHTMLAttributes<HTMLButtonElement>, "children"> & {
  readonly icon: LucideIcon;
  readonly label: string;
  readonly size?: 24 | 28;
  readonly iconSize?: number;
  readonly active?: boolean;
  readonly ref?: Ref<HTMLButtonElement>;
};

/** A 28px (or 24px) square icon button: a 16px Lucide icon at stroke 1.5 in text-3. */
export function IconButton({
  icon: Icon,
  label,
  size = 28,
  iconSize,
  active = false,
  className,
  ...rest
}: IconButtonProps) {
  return (
    <button
      type="button"
      className={`o-icon-btn${className ? ` ${className}` : ""}`}
      data-size={size}
      data-active={active ? "" : undefined}
      aria-label={label}
      title={label}
      {...rest}
    >
      <Icon size={iconSize ?? (size === 24 ? 14 : 16)} strokeWidth={1.5} aria-hidden="true" />
    </button>
  );
}

/** A cost as a numeral with superscript cents: "$0" and "21". */
export function MoneyNumeral({
  amount,
  currency,
}: {
  readonly amount: number;
  readonly currency: string;
}) {
  const parts = splitCents(amount, currency);
  if (!parts) return null;
  return (
    <span className="o-numeral o-t-numeral">
      {parts.whole}
      <span className="o-sup">{parts.cents}</span>
    </span>
  );
}

/** A live counter in whole seconds, read from the playhead once a second. */
export function Elapsed({ player, since }: { readonly player: Player; readonly since: number }) {
  const now = usePlayhead(player, 1000);
  return <span className="o-num">{formatElapsed(Math.max(0, now - since))}</span>;
}

/** The grey-on-grey shimmer for a live label (X Money's 30% to 60% ink sweep). */
export function Shimmer({ children }: { readonly children: ReactNode }) {
  return <span className="o-shimmer">{children}</span>;
}

/** The dashed 8/8 tear line (X Money's receipt). */
export function TearLine({ className }: { readonly className?: string }) {
  return <div className={`o-tear${className ? ` ${className}` : ""}`} aria-hidden="true" />;
}

/** A popover's open: opacity, scale 0.98 to 1 and a 4px rise over --o-base. */
export function popover(reduced: boolean) {
  return {
    initial: reduced ? { opacity: 0 } : { opacity: 0, scale: 0.98, y: 4 },
    animate: { opacity: 1, scale: 1, y: 0 },
    exit: reduced ? { opacity: 0 } : { opacity: 0, scale: 0.98, y: 4 },
    transition: { duration: reduced ? 0.12 : 0.2, ease: [0.23, 1, 0.32, 1] as const },
  };
}

/** Closes a popover on a click outside it. */
export function useOutsideClose(
  ref: { readonly current: HTMLElement | null },
  onClose: () => void,
  ignore?: { readonly current: HTMLElement | null },
) {
  useEffect(() => {
    const onDown = (event: PointerEvent): void => {
      const target = event.target as Node | null;
      if (!target || ref.current?.contains(target) || ignore?.current?.contains(target)) return;
      onClose();
    };
    window.addEventListener("pointerdown", onDown);
    return () => window.removeEventListener("pointerdown", onDown);
  }, [ref, onClose, ignore]);
}
