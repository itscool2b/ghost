import { Fragment, type ReactNode } from "react";

/**
 * The Terms of Service and the Privacy Policy, kept as Markdown beside this file so the text the
 * operator approved is the text the site shows (owner, 25 November 2026). Only what those texts
 * use is read: `##` and `###` headings, paragraphs, `-` lists, `**bold**`, `` `code` ``,
 * `[text](url)` links and a line break where a line ends in two spaces. Everything renders on the
 * server, so the pages read whole without JavaScript.
 */
export function Legal({ source }: { readonly source: string }) {
  const blocks = source
    .replace(/\r\n/g, "\n")
    .trim()
    .split(/\n{2,}/);
  return <>{blocks.map((block, index) => renderBlock(block, index))}</>;
}

function renderBlock(block: string, key: number): ReactNode {
  if (block.startsWith("### ")) return <h3 key={key}>{inline(block.slice(4))}</h3>;
  if (block.startsWith("## ")) return <h2 key={key}>{inline(block.slice(3))}</h2>;
  const lines = block.split("\n");
  if (lines.every((line) => line.startsWith("- "))) {
    return (
      <ul key={key}>
        {lines.map((line, index) => (
          // biome-ignore lint/suspicious/noArrayIndexKey: a fixed text, read in order
          <li key={index}>{inline(line.slice(2))}</li>
        ))}
      </ul>
    );
  }
  return (
    <p key={key}>
      {lines.map((line, index) => (
        // biome-ignore lint/suspicious/noArrayIndexKey: a fixed text, read in order
        <Fragment key={index}>
          {inline(line.replace(/ {2}$/, ""))}
          {line.endsWith("  ") && index < lines.length - 1 ? <br /> : null}
          {!line.endsWith("  ") && index < lines.length - 1 ? " " : null}
        </Fragment>
      ))}
    </p>
  );
}

const TOKEN = /\*\*(.+?)\*\*|`([^`]+)`|\[([^\]]+)\]\(([^)\s]+)\)/g;

/** Bold, code and links inside one line. */
function inline(text: string): ReactNode[] {
  const out: ReactNode[] = [];
  let last = 0;
  for (const match of text.matchAll(TOKEN)) {
    const at = match.index ?? 0;
    if (at > last) out.push(text.slice(last, at));
    const [, bold, code, label, href] = match;
    if (bold !== undefined) out.push(<strong key={at}>{inline(bold)}</strong>);
    else if (code !== undefined) out.push(<code key={at}>{code}</code>);
    else if (label !== undefined && href !== undefined) {
      const external = /^https?:/.test(href) && !href.startsWith("https://onghost.dev");
      out.push(
        <a key={at} href={href} {...(external ? { rel: "noreferrer" } : {})}>
          {label}
        </a>,
      );
    }
    last = at + match[0].length;
  }
  if (last < text.length) out.push(text.slice(last));
  return out;
}
