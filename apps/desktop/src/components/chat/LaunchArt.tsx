import { LOGO_VIEWBOX, Wing } from "@ghost/ui/graphite";
import { useEffect, useLayoutEffect, useRef } from "react";
import "../tour/wing.css";
import "./launch-art.css";

/** The outline's width on screen while it draws, in pixels. */
const HAIRLINE = 1.25;

/**
 * The start screen (owner, 26 November 2026: "it should just be our logo in a really sexy way",
 * in place of the aircraft pictures of 17 November): the wing alone in the pane. Once per launch
 * its outline draws itself in a hairline of ink, the solid wing sweeps in over the line and opens
 * its eyes; then it hovers, blinks now and then and watches the pointer. New chat, a project's New
 * chat and Ctrl+N open the composer as before, and the wing flies up into New chat's own.
 *
 * It is the shared wing (`Wing` in @ghost/ui) in the theme's ink, its eyes cut through to the pane,
 * so it is right in every theme and both appearances. Under reduced motion it is simply there.
 */
export function LaunchArt() {
  const root = useRef<HTMLDivElement>(null);

  // The outline is one screen hairline whatever size the pane gives the wing: its stroke is set
  // in the drawing's own units from the width it is drawn at.
  useLayoutEffect(() => {
    const node = root.current;
    const line = node?.querySelector<SVGSVGElement>(".launch-art-line");
    if (!node || !line) return;
    const measure = () => {
      const width = line.getBoundingClientRect().width;
      if (width > 0)
        node.style.setProperty(
          "--launch-art-stroke",
          ((LOGO_VIEWBOX[0] / width) * HAIRLINE).toFixed(3),
        );
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(line);
    return () => observer.disconnect();
  }, []);

  // A hidden window holds the hover and the blink where they are.
  useEffect(() => {
    const node = root.current;
    if (!node) return;
    const sync = () => node.toggleAttribute("data-hidden", document.hidden);
    sync();
    document.addEventListener("visibilitychange", sync);
    return () => document.removeEventListener("visibilitychange", sync);
  }, []);

  return (
    <div ref={root} className="launch-art" data-testid="launch-art" role="img" aria-label="Ghost">
      <div className="launch-art-wing">
        <div className="launch-art-float">
          <Wing className="launch-art-line" line />
          <Wing className="launch-art-solid" look />
        </div>
      </div>
    </div>
  );
}
