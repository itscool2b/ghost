import { Logo } from "@ghost/ui/graphite";
import type { ReactNode } from "react";

/**
 * An inner page, in the home page's language: a two-tone title at the statement's size (the first
 * clause in ink, the rest in grey on its own line) and a grey line, then flat cards. Wide pages
 * take the page's column; documents keep a narrower one. No eyebrow above the title; an optional
 * picture stands beside it.
 */
export function Page({
  title,
  tone,
  lead,
  art,
  wide = false,
  children,
}: {
  title: string;
  tone?: string;
  lead?: ReactNode;
  /** Decorative, beside the title on wide screens. */
  art?: ReactNode;
  wide?: boolean;
  children: ReactNode;
}) {
  return (
    <main id="main" className="page">
      <div className={wide ? "wrap page-column page-column-wide" : "wrap page-column"}>
        <header className={art ? "page-head page-head-art" : "page-head"}>
          <div className="page-head-text">
            <h1 className="page-title">
              {title}
              {tone ? (
                <>
                  {" "}
                  <span>{tone}</span>
                </>
              ) : null}
            </h1>
            {lead ? <p className="page-lead">{lead}</p> : null}
          </div>
          {art ? (
            <div className="page-art" aria-hidden="true">
              {art}
            </div>
          ) : null}
        </header>
        {children}
      </div>
    </main>
  );
}

/**
 * The app as an object: the Ghost logo on a disc of the page's ink. Decorative; the page around
 * it names the app.
 */
export function AppTile({ small = false }: { small?: boolean }) {
  return (
    <span className={small ? "app-tile app-tile-small" : "app-tile"}>
      <Logo size={small ? 44 : 68} />
    </span>
  );
}

/** A small arrow drawn on whole pixels, for links that move you somewhere. */
export function Arrow({ direction = "right" }: { direction?: "right" | "up-right" }) {
  const path = direction === "up-right" ? "M4 3h5v5M9 3 3 9" : "M2 6h8M6.5 2.5 10 6l-3.5 3.5";
  return (
    <svg
      className="arrow"
      width="12"
      height="12"
      viewBox="0 0 12 12"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      aria-hidden="true"
      focusable="false"
    >
      <path d={path} />
    </svg>
  );
}
