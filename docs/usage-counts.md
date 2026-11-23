# Usage counts

How many people downloaded o7, and how many copies of it run. Two counters, both anonymous, both
kept only as daily totals.

## What is counted

- **Downloads.** The download page's buttons link to `o7.codes/get/<tag>/<file>`. The site Worker
  (`apps/web-worker/src/count.rs`) adds one to that day's `download` row for the file's platform and
  version, then answers 302 to the file on GitHub. Only the six installer names the Publish
  workflow makes are accepted, with a tag that matches their version; anything else is 404 and
  nothing is counted. Robots (search engines, link previews, requests with no user agent) are sent
  on but not counted, and so is `HEAD`. If counting fails, the download still works.
- **Copies that ran.** Each released stable build, when its automatic update check runs, calls
  `o7.codes/api/count?os=…&arch=…&version=…&first=0|1` at most once per US Eastern day
  (`apps/desktop/src-tauri/src/usage_count.rs`). The site adds one to that day's `ran` row, and to
  `installed` when `first=1` (the copy had never counted itself). It answers with the seconds to
  the next Eastern midnight, which the app keeps in `counted.json` in its data folder. Turning off
  "Check for updates automatically" in Settings stops it. Development, staging and smoke launches
  never count.

There is no identifier: a `ran` total is the number of copies that ran that day, an `installed`
total the number of copies that ran for the first time. Downloads made before this counter (from
GitHub directly, or through old links) are not in it, and neither are copies older than the
version that added it until they update. Someone set on it can inflate a count; nothing else can
be done through these addresses.

## The database

D1 database `o7-counts` (`beaf6cff-cda8-4e44-aaa8-5d46b0188fbb`), bound as `COUNTS` to `o7-web`.
The preview Worker writes to `o7-counts-preview` (`a285ac49-31b4-413a-b5a9-9663d878ea63`), so
testing never touches the real numbers. Schema: `apps/web-worker/schema.sql`, one table `daily`
with `day` (US Eastern, `YYYY-MM-DD`), `event` (`download`, `ran`, `installed`), `platform`,
`version` and `n`.

## Reading the numbers

Cloudflare dashboard, Storage and databases, D1, `o7-counts`, Console; or any chat with the
Cloudflare API tools (`POST /accounts/<account>/d1/database/<id>/query`).

```sql
-- Totals so far
SELECT event, SUM(n) FROM daily GROUP BY event;
-- Copies that ran each day, last 30 days
SELECT day, SUM(n) AS copies FROM daily WHERE event = 'ran' GROUP BY day ORDER BY day DESC LIMIT 30;
-- Downloads by platform
SELECT platform, SUM(n) FROM daily WHERE event = 'download' GROUP BY platform ORDER BY 2 DESC;
-- Copies that ran in the last 7 days, by version (the same copy on several days counts each day)
SELECT version, SUM(n) FROM daily WHERE event = 'ran' AND day >= date('now', '-7 days') GROUP BY version;
```

A copy counted on several days is counted once each day, so add `ran` over days for "copy-days",
not people. "Weekly users" is best read as the largest single day in the week, or `installed`
summed for how many copies exist.
