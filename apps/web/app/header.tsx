import { Wordmark } from "@ghost/ui/graphite";
import { useEffect, useRef } from "react";
import { GitHubMark, REPOSITORY } from "./repo";

const SECTIONS = [
  { href: "/#features", text: "Features" },
  { href: "/#agents", text: "Agents" },
  { href: "/#download", text: "Download" },
  { href: "/#faq", text: "FAQ" },
  { href: "/changelog", text: "Changelog" },
] as const;

/**
 * The bar dot.com has: the wordmark, the page's sections beside it, and the primary Download at
 * right, with the public repository as a quiet GitHub pill before it (its mark alone under 1024px). Under 1024px the links fold into a menu built on <details>, so it opens without
 * JavaScript too: a quiet disc beside Download, opening a sheet that covers the page with the
 * sections and Download at its foot.
 */
export function SiteHeader() {
  const menu = useRef<HTMLDetailsElement>(null);

  // Below 1024px the bar's Download waits while the hero's own is in view, as dot.com's does, so
  // the screen never shows two of it; it fades in once the hero's has scrolled away.
  useEffect(() => {
    const header = document.querySelector(".site-nav");
    const action = document.querySelector(".hero-actions .btn-primary");
    if (!header || !action || typeof IntersectionObserver === "undefined") return;
    const observer = new IntersectionObserver(
      (entries) => {
        const entry = entries[entries.length - 1];
        if (entry) header.toggleAttribute("data-hero-action", entry.isIntersecting);
      },
      { rootMargin: "-84px 0px 0px 0px" },
    );
    observer.observe(action);
    return () => {
      observer.disconnect();
      header.removeAttribute("data-hero-action");
    };
  }, []);

  useEffect(() => {
    const details = menu.current;
    if (!details) return;
    const close = (restoreFocus: boolean) => {
      if (!details.open) return;
      details.open = false;
      if (restoreFocus) details.querySelector("summary")?.focus();
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") close(true);
    };
    const onPointer = (event: PointerEvent) => {
      if (event.target instanceof Node && !details.contains(event.target)) close(false);
    };
    // A link to a section of this page leaves the menu open otherwise.
    const onClick = (event: MouseEvent) => {
      if (event.target instanceof Element && event.target.closest("a")) close(false);
    };
    document.addEventListener("keydown", onKey);
    document.addEventListener("pointerdown", onPointer);
    details.addEventListener("click", onClick);
    return () => {
      document.removeEventListener("keydown", onKey);
      document.removeEventListener("pointerdown", onPointer);
      details.removeEventListener("click", onClick);
    };
  }, []);

  return (
    <header className="site-nav">
      <div className="wrap nav-bar">
        <a className="nav-home" href="/" aria-label="Ghost home">
          <Wordmark size={17} />
        </a>
        <nav className="nav-links" aria-label="Main">
          {SECTIONS.map((link) => (
            <a key={link.href} href={link.href}>
              {link.text}
            </a>
          ))}
        </nav>
        <div className="nav-end">
          <a
            className="btn btn-quiet btn-sm nav-github"
            href={REPOSITORY}
            aria-label="Ghost on GitHub"
          >
            <GitHubMark />
            <span className="nav-github-text">GitHub</span>
          </a>
          <a className="btn btn-primary btn-sm nav-cta" href="/download">
            Download
          </a>
          <details className="nav-menu" ref={menu}>
            <summary className="nav-menu-button">
              <span className="sr-only">Menu</span>
              <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden="true" focusable="false">
                <path className="nav-menu-bar" d="M2 5.5h12" />
                <path className="nav-menu-bar" d="M2 10.5h12" />
              </svg>
            </summary>
            <div className="nav-sheet">
              <nav aria-label="Menu">
                {SECTIONS.map((link) => (
                  <a key={link.href} href={link.href}>
                    {link.text}
                  </a>
                ))}
                <a href={REPOSITORY}>GitHub</a>
              </nav>
              <a className="btn btn-primary btn-lg" href="/download">
                Download Ghost
              </a>
            </div>
          </details>
        </div>
      </div>
    </header>
  );
}
