import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { LaunchArt } from "./LaunchArt";

const stylesheet = readFileSync(
  fileURLToPath(new URL("./launch-art.css", import.meta.url)),
  "utf8",
);

describe("the start screen", () => {
  const markup = renderToStaticMarkup(<LaunchArt />);

  it("is the Ghost wing and nothing else, named for assistive technology", () => {
    expect(markup).toContain('data-testid="launch-art"');
    expect(markup).toContain('role="img"');
    expect(markup).toContain('aria-label="Ghost"');
    expect(markup).not.toMatch(/<image|\.jpg|<figcaption/);
  });

  it("draws the wing twice in one cell: the outline that draws itself and the solid wing over it", () => {
    expect(markup.match(/class="wing[ "]/g)).toHaveLength(2);
    expect(markup).toContain("wing wing-line launch-art-line");
    expect(markup).toContain("wing launch-art-solid");
  });

  it("stands still where motion is not welcome, with no outline left under the wing", () => {
    expect(stylesheet).toMatch(/@media \(prefers-reduced-motion: reduce\)[\s\S]*animation: none/);
    expect(stylesheet).toMatch(/\[data-motion="reduced"\], \[data-motion="off"\]/);
    expect(stylesheet).toMatch(/\.launch-art-line \{\s*display: none;/);
  });
});
