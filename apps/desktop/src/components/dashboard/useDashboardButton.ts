import { useLayoutEffect, useRef } from "react";

/**
 * The ref for the button that opens the Dashboard, which takes focus back once the Dashboard
 * closes. The dialog hands focus back by itself only to a control that had it, and WebKit on macOS
 * does not focus a button it clicks, so there focus would fall to the page. A layout effect, so
 * focus lands in the same commit that closes the dialog; where something else took focus (a chat
 * the Dashboard opened, say), it stays there.
 */
export function useDashboardButton(open: boolean) {
  const button = useRef<HTMLButtonElement>(null);
  const wasOpen = useRef(open);
  useLayoutEffect(() => {
    const was = wasOpen.current;
    wasOpen.current = open;
    if (!was || open) return;
    const focused = document.activeElement;
    if (focused === null || focused === document.body) button.current?.focus();
  }, [open]);
  return button;
}
