import { Wordmark } from "@ghost/ui/graphite";
import { type CSSProperties, useEffect, useRef, useState } from "react";
import { useLocation } from "react-router";
import { GitHubMark, REPOSITORY } from "./repo";

/**
 * Where the sidebar leads, as X Money's does: the home page's parts, then the pages of their own.
 * `part` is the home page's section a link stands for, `page` the path that is its own page.
 */
const LINKS: readonly { href: string; text: string; part?: string; page?: string }[] = [
  { href: "/", text: "Overview", part: "top" },
  { href: "/#agents", text: "Agents", part: "agents" },
  { href: "/#features", text: "Features", part: "features" },
  { href: "/download", text: "Download", part: "download", page: "/download" },
  { href: "/#faq", text: "FAQ", part: "faq" },
  { href: "/changelog", text: "Changelog", page: "/changelog" },
];

/** The home page's parts the sidebar follows. */
const PARTS = ["agents", "features", "download", "faq"] as const;

/**
 * The part of the home page in view: the last whose top has passed two fifths of the window, or
 * the opening above them all. Null away from the home page.
 */
function usePart(home: boolean): string | null {
  const [part, setPart] = useState<string | null>(home ? "top" : null);
  useEffect(() => {
    if (!home) {
      setPart(null);
      return;
    }
    let frame = 0;
    const read = () => {
      frame = 0;
      // The part whose top is the lowest one above the line is the one being read.
      const line = innerHeight * 0.4;
      let current = "top";
      let reached = Number.NEGATIVE_INFINITY;
      for (const id of PARTS) {
        const top = document.getElementById(id)?.getBoundingClientRect().top;
        if (top !== undefined && top <= line && top > reached) {
          current = id;
          reached = top;
        }
      }
      setPart(current);
    };
    const onScroll = () => {
      if (!frame) frame = requestAnimationFrame(read);
    };
    read();
    addEventListener("scroll", onScroll, { passive: true });
    addEventListener("resize", onScroll, { passive: true });
    return () => {
      removeEventListener("scroll", onScroll);
      removeEventListener("resize", onScroll);
      cancelAnimationFrame(frame);
    };
  }, [home]);
  return part;
}

/**
 * X Money's sidebar (from 1024px): the wordmark at the top, the page's parts and pages as a list
 * of grey words with the one in view in ink, and the two pills at its foot, Download and the
 * public repository. It stays put while the page scrolls. Under 1024px it is a bar across the top:
 * the wordmark, Download, GitHub's mark and a quiet disc for a menu built on <details>, so it opens
 * without JavaScript too, a sheet that covers the page with the same links and Download at its
 * foot.
 */
export function SiteHeader() {
  const menu = useRef<HTMLDetailsElement>(null);
  const links = useRef<HTMLElement>(null);
  const path = useLocation().pathname.replace(/\/+$/, "") || "/";
  const part = usePart(path === "/");

  // The quiet pill stands behind the link in view and slides to the next one; it arrives where it
  // belongs without sliding, and goes where no link is current.
  useEffect(() => {
    const nav = links.current;
    const pill = nav?.querySelector<HTMLElement>(".nav-pill");
    if (!nav || !pill) return;
    const at = nav.querySelector<HTMLElement>("a[aria-current]");
    if (!at) {
      nav.removeAttribute("data-pill");
      return;
    }
    const first = !nav.hasAttribute("data-pill");
    if (first) pill.style.transition = "none";
    pill.style.transform = `translateY(${at.offsetTop}px)`;
    nav.setAttribute("data-pill", "");
    if (first) {
      void pill.offsetWidth;
      pill.style.transition = "";
    }
  });

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

  /** A page of its own is the page; a part of the home page is where the reader is. */
  const current = (link: (typeof LINKS)[number]) =>
    link.page !== undefined && path === link.page
      ? ("page" as const)
      : link.part !== undefined && part === link.part
        ? ("location" as const)
        : undefined;

  return (
    <header className="site-nav">
      <div className="nav-bar">
        <a className="nav-home" href="/" aria-label="Ghost home">
          <Wordmark size={17} />
        </a>
        <nav className="nav-links" aria-label="Main" ref={links}>
          <span className="nav-pill" aria-hidden="true" />
          {LINKS.map((link, index) => (
            <a
              key={link.href}
              href={link.href}
              aria-current={current(link)}
              style={{ "--i": index } as CSSProperties}
            >
              {link.text}
            </a>
          ))}
        </nav>
        <div className="nav-end">
          <a className="btn btn-primary btn-sm nav-cta" href="/download">
            Download
          </a>
          <a
            className="btn btn-quiet btn-sm nav-github"
            href={REPOSITORY}
            aria-label="Ghost on GitHub"
          >
            <GitHubMark />
            <span className="nav-github-text">GitHub</span>
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
                {LINKS.map((link) => (
                  <a key={link.href} href={link.href} aria-current={current(link)}>
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
