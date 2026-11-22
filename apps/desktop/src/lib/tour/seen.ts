import type { SetupApi } from "../setup";

/**
 * Adds `stops` to the tour stops this profile has seen and returns the saved list. Setup's
 * preferences are saved against a revision, so a save that lost a race with another change (a
 * setting flipped in the same moment) reads again and tries once more.
 */
export async function markTourSeen(
  api: Pick<SetupApi, "snapshot" | "apply">,
  stops: readonly string[],
): Promise<readonly string[]> {
  let failure: unknown;
  for (let attempt = 0; attempt < 2; attempt += 1) {
    const saved = await api.snapshot();
    const seen = saved.preferences.tour_seen;
    const add = stops.filter((stop) => !seen.includes(stop));
    if (add.length === 0) return seen;
    try {
      const next = await api.apply(saved.preferences.revision, { type: "tour_seen", stops: add });
      return next.preferences.tour_seen;
    } catch (error) {
      failure = error;
    }
  }
  throw failure;
}
