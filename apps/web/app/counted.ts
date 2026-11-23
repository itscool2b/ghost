/**
 * A download button's address: `/get/<tag>/<file>` on this site, which counts the download and
 * sends the browser on to the same file on GitHub (`apps/web-worker/src/count.rs`). An address
 * that is not one of this repository's release assets is left as it is.
 */
export function counted(url: string): string {
  const match =
    /^https:\/\/github\.com\/itscool2b\/(?:o7|auracode)\/releases\/download\/([^/]+)\/([^/]+)$/.exec(
      url,
    );
  return match ? `/get/${match[1]}/${match[2]}` : url;
}
