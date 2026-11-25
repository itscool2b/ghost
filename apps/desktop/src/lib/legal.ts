/**
 * o7's Terms of Service and Privacy Policy, published by BASH.AI LLC on o7.codes (owner, 25 November
 * 2026). Setup's first page says that continuing accepts them; a copy that finished setup before
 * they were published, or before they last changed materially, shows one notice with both links
 * (both documents promise "an in-app notice" for material changes). See docs/developing-in-o7.md
 * ("Terms and Privacy").
 */

/**
 * The documents' effective date. Raise it, with the date in `apps/web/app/legal/*.md`, when either
 * changes materially: every copy then shows the notice once more.
 */
export const LEGAL_VERSION = "2026-11-26";
/** The same date as people read it. */
export const LEGAL_DATE = "November 26, 2026";

export const TERMS_URL = "https://o7.codes/terms";
export const PRIVACY_URL = "https://o7.codes/privacy";
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
        ? "o7 has Terms of Service and a Privacy Policy"
        : "o7's Terms of Service and Privacy Policy changed",
    detail: `From ${OPERATOR}, effective ${LEGAL_DATE}. Using o7 means you agree to them.`,
  };
}
