/**
 * A pairing link carries its key after the `#`. The page takes it out of the address bar and the
 * history the moment it sees one, so it is never shared, bookmarked or sent anywhere.
 *
 * A link can arrive at any time, not only when the page loads: a phone that already has the
 * remote page open and scans another computer's code, or a link pasted into this tab, changes only
 * the part after the `#`. The browser then keeps the page and says `hashchange`, so the page
 * takes the link on every such change, and never caches the first one it saw.
 */

/** The parts of `window` the link is read from and removed with, so tests can stand in for them. */
export interface Address {
  readonly location: Pick<Location, "hash" | "pathname" | "search">;
  readonly history: Pick<History, "replaceState" | "state">;
}

/**
 * The pairing link in the address, removed from it, or "" when there is none. Whatever the history
 * entry held is kept, so the site's router still knows where it is.
 */
export function takePairingLink(address: Address): string {
  const { hash, pathname, search } = address.location;
  if (hash.replace(/^#/, "") === "") return "";
  address.history.replaceState(address.history.state, "", pathname + search);
  return hash;
}
