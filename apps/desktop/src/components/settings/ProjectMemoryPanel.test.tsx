import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { ProjectMemoryApi } from "../../lib/projectMemory";
import type { SetupApi } from "../../lib/setup";
import { ProjectMemoryPanel } from "./ProjectMemoryPanel";

const never = () => new Promise<never>(() => {});
const api: ProjectMemoryApi = {
  read: never,
  save: never,
  readNote: never,
  saveNote: never,
  deleteNote: never,
  undo: never,
  subscribe: () => () => {},
};
const setup: SetupApi = { snapshot: never, apply: never };

describe("the project memory panel", () => {
  it("offers the switch for agents' edits, on until the profile says otherwise", () => {
    const html = renderToStaticMarkup(
      <ProjectMemoryPanel projects={[{ id: "one", name: "auracode" }]} api={api} setup={setup} />,
    );
    expect(html).toContain('role="switch"');
    expect(html).toContain('aria-label="Let agents edit memory"');
    expect(html).toMatch(/role="switch"[^>]*checked=""[^>]*disabled=""|disabled=""[^>]*checked=""/);
    expect(html).toContain("A notice with Undo follows each edit.");
  });
  it("is the notes and nothing else: no history, proposals, reasons or imports", () => {
    const html = renderToStaticMarkup(
      <ProjectMemoryPanel projects={[{ id: "one", name: "auracode" }]} api={api} setup={setup} />,
    );
    for (const gone of [
      "Proposals",
      "Revision",
      "Restore",
      "Reason",
      "Import",
      "Export",
      "Refresh",
    ])
      expect(html).not.toContain(gone);
  });
  it("offers a project to choose only when there is more than one", () => {
    const one = renderToStaticMarkup(
      <ProjectMemoryPanel projects={[{ id: "one", name: "auracode" }]} api={api} setup={setup} />,
    );
    expect(one).not.toContain("<select");
    const two = renderToStaticMarkup(
      <ProjectMemoryPanel
        projects={[
          { id: "one", name: "auracode" },
          { id: "two", name: "website" },
        ]}
        initialProjectId="two"
        api={api}
        setup={setup}
      />,
    );
    expect(two).toContain('aria-label="Project"');
    expect(two).toContain('data-value="two"');
    expect(two).toContain('<span class="ui-select-value">website</span>');
  });
  it("renders without projects or a subscription, and starts no write", () => {
    const save = vi.fn(never);
    const html = renderToStaticMarkup(
      <ProjectMemoryPanel projects={[]} api={{ ...api, save }} setup={setup} />,
    );
    expect(html).toContain("Create a project to keep shared project memory.");
    expect(save).not.toHaveBeenCalled();
  });
});
