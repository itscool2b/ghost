import { type CSSProperties, Fragment } from "react";

/**
 * A line whose words flip up into place one after another, as X Money opens its page: each word
 * rises from under its own line, tipped back 40 degrees, and settles (home.css). A rising word is
 * clipped by its line, never faded, so it is either readable or not there yet; everyone else reads
 * one line. `from` continues the count of a line that runs on from another.
 */
export function Words({ text, from = 0 }: { text: string; from?: number }) {
  return text.split(" ").map((word, index) => (
    // biome-ignore lint/suspicious/noArrayIndexKey: a word's place in a fixed line is its identity
    <Fragment key={index}>
      {index > 0 ? " " : null}
      <span className="word">
        <span className="word-in" style={{ "--w": from + index } as CSSProperties}>
          {word}
        </span>
      </span>
    </Fragment>
  ));
}
