/**
 * A browser page is a native view drawn above the window's HTML, so nothing Ghost shows can
 * sit on top of it. While something opens over the page's area (a menu, a dialog, the command
 * palette, a toast, the address bar's suggestions), the panel swaps the live page for its picture
 * until it closes. This finds what opens over it.
 */

/** Marks an element that floats over other content and must read above a browser page. */
export const OVER_BROWSER = "data-over-browser";

const FLOATING = `dialog[open], details[open] > :not(summary), [${OVER_BROWSER}]`;

export interface Box {
  readonly left: number;
  readonly top: number;
  readonly right: number;
  readonly bottom: number;
}

export function overlaps(a: Box, b: Box): boolean {
  return a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom;
}

/**
 * Whether anything open in `root` covers `page`: a modal dialog always does (its backdrop dims the
 * whole window); anything else when its box meets the page's. Elements inside `ignore` (the page
 * area's own stand-ins) never count.
 */
export function covered(page: Box, root: ParentNode = document, ignore?: Element | null): boolean {
  for (const element of root.querySelectorAll<HTMLElement>(FLOATING)) {
    if (ignore?.contains(element)) continue;
    if (
      typeof HTMLDialogElement !== "undefined" &&
      element instanceof HTMLDialogElement &&
      isModal(element)
    )
      return true;
    const box = element.getBoundingClientRect();
    if (box.width <= 0 || box.height <= 0) continue;
    if (overlaps(box, page)) return true;
  }
  return false;
}

function isModal(dialog: HTMLDialogElement): boolean {
  try {
    return dialog.matches(":modal");
  } catch {
    // An engine without :modal: a dialog shown by showModal() is in the top layer either way.
    return dialog.open;
  }
}
