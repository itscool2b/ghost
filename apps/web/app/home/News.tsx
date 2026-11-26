// The repository's CHANGELOG.md, parsed when the site is built (scripts/changelog.ts).
import days from "../changelog-data";
import { Arrow } from "../ui";

const CHANGES = days.reduce((count, day) => count + day.entries.length, 0);

/** A line's text with its `code` set in the mono face, as the changelog page sets it. */
function Inline({ text }: { text: string }) {
  const parts = text.split("`");
  return (
    <>
      {parts.map((part, i) =>
        // biome-ignore lint/suspicious/noArrayIndexKey: a fixed line; the position is identity.
        i % 2 === 1 ? <code key={i}>{part}</code> : part,
      )}
    </>
  );
}

/**
 * What shipped, as a timeline: one hairline across the column with a dot for each of the three
 * newest days of the changelog, the newest lit. Under each dot its day, how many changes it
 * brought and the first of them, all one link to that day on the changelog page. The count in the
 * lead is of the changelog itself, so it is true on every build.
 */
export function News() {
  return (
    <section className="section news" aria-labelledby="news-title">
      <div className="wrap">
        <div className="chapter-head">
          <h2 id="news-title" className="title">
            Shipping daily. <span className="tone">Every change, in plain words.</span>
          </h2>
          <p className="lead">
            {CHANGES} changes so far, each written for the person using the app.{" "}
            <a className="text-link" href="/changelog">
              Read the changelog <Arrow />
            </a>
          </p>
        </div>
        {days.length ? (
          <ol className="timeline">
            {days.slice(0, 3).map((day, index) => {
              // The day's first change a person would notice; work behind the scenes waits.
              const first =
                day.entries.find((entry) => !entry.head.startsWith("Behind the scenes")) ??
                day.entries[0];
              return (
                <li key={day.date} data-latest={index === 0 ? "" : undefined}>
                  <a className="timeline-day" href={`/changelog#day-${day.date}`}>
                    <span className="timeline-dot" aria-hidden="true" />
                    <time dateTime={day.date}>{day.label}</time>
                    <span className="timeline-count">
                      {day.entries.length} {day.entries.length === 1 ? "change" : "changes"}
                    </span>
                    {first ? (
                      <span className="timeline-text">
                        <Inline text={first.head} />
                        {first.rest ? (
                          <>
                            {" "}
                            <Inline text={first.rest} />
                          </>
                        ) : null}
                      </span>
                    ) : null}
                  </a>
                </li>
              );
            })}
          </ol>
        ) : null}
      </div>
    </section>
  );
}
