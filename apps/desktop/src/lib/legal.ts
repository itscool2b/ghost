/**
 * Ghost's Terms of Service and Privacy Policy, published by BASH.AI LLC on onghost.dev (owner, 25 November
 * 2026). Setup's first page says that continuing accepts them; a copy that finished setup before
 * they were published, or before they last changed materially, shows one notice with both links
 * (both documents promise "an in-app notice" for material changes). See docs/developing-in-ghost.md
 * ("Terms and Privacy").
 */

/**
 * The documents' version: their effective date, and a number after it when a second version takes
 * effect the same day (version 2 of 26 November 2026 followed that morning's change by hours).
 * Change it, with the date in `apps/web/app/legal/*.md`, when either document changes materially:
 * every copy then shows the notice once more, and its OK is the acceptance the documents speak of.
 */
export const LEGAL_VERSION = "2026-11-26.2";
/** The same date as people read it. */
export const LEGAL_DATE = "November 26, 2026";

export const TERMS_URL = "https://onghost.dev/terms";
export const PRIVACY_URL = "https://onghost.dev/privacy";
export const OPERATOR = "BASH.AI LLC";
export const COPYRIGHT = `© 2026 ${OPERATOR}. All rights reserved.`;

const KEY = "o7.legal.accepted";

export interface LegalStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

function storage(): LegalStorage | null {
  return typeof localStorage === "undefined" ? null : localStorage;
}

/** The version this copy last accepted, `null` when it never did. */
export function acceptedLegal(store: LegalStorage | null = storage()): string | null {
  try {
    return store?.getItem(KEY) ?? null;
  } catch {
    return null;
  }
}

/** The person accepted the current documents (setup's first page, or the notice's OK). */
export function acceptLegal(store: LegalStorage | null = storage()): void {
  try {
    store?.setItem(KEY, LEGAL_VERSION);
  } catch {
    /* A full or blocked storage shows the notice again next time, which is harmless. */
  }
}

/** What the notice says, or `null` when this copy already accepted the current documents. */
export function legalNotice(
  store: LegalStorage | null = storage(),
): { readonly title: string; readonly detail: string } | null {
  const accepted = acceptedLegal(store);
  if (accepted === LEGAL_VERSION) return null;
  return {
    title:
      accepted === null
        ? "Ghost has Terms of Service and a Privacy Policy"
        : "Ghost's Terms of Service and Privacy Policy changed",
    detail: `From ${OPERATOR}, effective ${LEGAL_DATE}. Using Ghost means you agree to them.`,
  };
}
