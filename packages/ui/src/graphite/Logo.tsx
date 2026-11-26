import { LOGO_PATH, LOGO_SMALL_MAX, LOGO_SMALL_PATH, LOGO_VIEWBOX } from "./logoPaths";

export interface LogoProps {
  /** Width of the logo in px; its height follows from the logo's shape. Default 20. */
  readonly size?: number;
  readonly className?: string;
  /** Accessible name; without one the logo is decorative. */
  readonly label?: string;
}

/**
 * The Ghost logo (a flying wing with two eyes) in `currentColor`. It is far wider than tall,
 * so `size` is its width and the box is as tall as the shape needs. At `LOGO_SMALL_MAX` px and
 * below it draws the small variant, whose larger eyes stay open at that size.
 */
export function Logo({ size = 20, className, label }: LogoProps) {
  const box = Number.isFinite(size) ? Math.max(8, Math.min(1024, Math.round(size))) : 20;
  const [width, height] = LOGO_VIEWBOX;
  return (
    <svg
      className={className ? `g-logo ${className}` : "g-logo"}
      width={box}
      height={Math.max(4, Math.round((box * height) / width))}
      viewBox={`0 0 ${width} ${height}`}
      fill="currentColor"
      role={label ? "img" : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
      focusable="false"
    >
      <path fillRule="evenodd" d={box <= LOGO_SMALL_MAX ? LOGO_SMALL_PATH : LOGO_PATH} />
    </svg>
  );
}

export interface WordmarkProps {
  /** Font size of "Ghost" in px. Default 16. */
  readonly size?: number;
  /** Show the logo before the word. Default true. */
  readonly mark?: boolean;
  readonly className?: string;
}

/**
 * "Ghost" in Geist 600 with tight tracking, after the logo. The word is real text, so a
 * wordmark inside a link names the link. The logo is not quite twice as wide as the type is
 * tall, which makes the wing about as tall as a lowercase letter, and is centred on the type.
 */
export function Wordmark({ size = 16, mark = true, className }: WordmarkProps) {
  const fontSize = Number.isFinite(size) ? Math.max(10, Math.min(320, size)) : 16;
  return (
    <span
      className={className ? `g-wordmark ${className}` : "g-wordmark"}
      style={{ fontSize: `${fontSize}px` }}
    >
      {mark ? <Logo size={Math.round(fontSize * 1.9)} /> : null}
      <span className="g-wordmark-text">Ghost</span>
    </span>
  );
}
