// The repository's CHANGELOG.md, parsed when the site is built (scripts/changelog.ts).

import catalog from "#release-catalog";
import days from "../changelog-data";
import { Arrow, Page } from "../ui";

export const meta = () => [{ title: "Changelog · Ghost" }];

/** A line's text with its `code` set in the mono face. */
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
 * Every change merged into Ghost, by the day it merged, straight from CHANGELOG.md. Each line
 * opens in ink and explains itself in grey, and ends with its pull request's number.
 */
export default function Changelog() {
  const release = catalog.release;
  return (
    <Page
      wide
      title="Changelog."
      tone="Every change, as it lands."
      lead={
        <>
          {release
            ? `Version ${release.version} is available, with its release notes, requirements and checksums on the download page.`
            : "No public installer is published yet. Its release notes and checksums will be on the download page."}{" "}
          <a className="text-link" href="/download">
            Download status <Arrow />
          </a>
        </>
      }
    >
      <div className="changes">
        {days.map((day) => (
          <section className="release-entry" key={day.date} aria-labelledby={`day-${day.date}`}>
            <div className="release-meta">
              <h2 id={`day-${day.date}`}>
                <time dateTime={day.date}>{day.label}</time>
              </h2>
              <span>
                {day.entries.length} {day.entries.length === 1 ? "change" : "changes"}
              </span>
            </div>
            <ul className="change-list">
              {day.entries.map((entry) => (
                <li key={entry.pull} className="change">
                  <p>
                    <strong>
                      <Inline text={entry.head} />
                    </strong>
                    {entry.rest ? (
                      <>
                        {" "}
                        <Inline text={entry.rest} />
                      </>
                    ) : null}
                  </p>
                  <span className="change-pull">
                    <span className="sr-only">Pull request </span>#{entry.pull}
                  </span>
                </li>
              ))}
            </ul>
          </section>
        ))}
      </div>
    </Page>
  );
}
