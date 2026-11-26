-- The site's counters (src/count.rs). One row per US Eastern day, event, platform and version.
-- event: download (an installer fetched through /get/), ran (a copy of Ghost that ran that day),
-- installed (a copy counting itself for the first time). Nothing else is stored.
CREATE TABLE IF NOT EXISTS daily (
  day TEXT NOT NULL,
  event TEXT NOT NULL,
  platform TEXT NOT NULL,
  version TEXT NOT NULL,
  n INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (day, event, platform, version)
) WITHOUT ROWID;
