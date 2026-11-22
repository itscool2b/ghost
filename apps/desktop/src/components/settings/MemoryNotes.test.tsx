import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { MemoryNoteSummary } from "../../bindings";
import { MAX_NOTES, type ProjectMemoryApi } from "../../lib/projectMemory";
import { MemoryNotes, noteAge } from "./MemoryNotes";

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
const DAY = 86_400_000;
const note = (name: string, by: string, daysAgo: number): MemoryNoteSummary => ({
  name,
  description: `Read for ${name}`,
  revision: `r-${name}`,
  updated_at: Date.now() - daysAgo * DAY - 1000,
  by,
  bytes: 10,
});
const render = (notes: readonly MemoryNoteSummary[]) =>
  renderToStaticMarkup(
    <MemoryNotes projectId="one" notes={notes} api={api} onChanged={() => {}} />,
  );

describe("the notes of a project's memory", () => {
  it("lists each note with what it is for, who saved it last and how long ago", () => {
    const html = render([
      note("windows-build", "agent", 3),
      note("release", "user", 0),
      note("phone", "bot", 1),
    ]);
    expect(html).toContain("windows-build");
    expect(html).toContain("Read for windows-build");
    expect(html).toContain("Saved by an agent, 3 days ago.");
    expect(html).toContain("Saved by you, today.");
    expect(html).toContain("Saved by a linked assistant, yesterday.");
    expect(html).toContain('aria-label="Open windows-build"');
    // No note is open until one is opened, and there is no history to look through.
    expect(html).not.toContain("memory-note-editor");
    for (const gone of ["Revision", "History", "Restore", "Proposals"])
      expect(html).not.toContain(gone);
  });
  it("says what notes are for when there are none, and still offers a new one", () => {
    const html = render([]);
    expect(html).toContain("No notes yet.");
    expect(html).toMatch(/<button[^>]*>New note<\/button>/);
    expect(html).not.toMatch(/<button[^>]*disabled=""[^>]*>New note<\/button>/);
  });
  it("offers no new note once the project has as many as it may", () => {
    const html = render(Array.from({ length: MAX_NOTES }, (_, at) => note(`n${at}`, "user", 0)));
    expect(html).toMatch(/<button[^>]*disabled=""[^>]*>New note<\/button>/);
    expect(html).toContain(`A project has at most ${MAX_NOTES} notes.`);
  });
  it("tells an old note from a fresh one in whole days", () => {
    const now = 100 * DAY;
    expect(noteAge(now, now)).toBe("today");
    expect(noteAge(now - DAY + 1, now)).toBe("today");
    expect(noteAge(now - DAY, now)).toBe("yesterday");
    expect(noteAge(now - 40 * DAY, now)).toBe("40 days ago");
    // A clock that went backwards never reads as the future.
    expect(noteAge(now + DAY, now)).toBe("today");
  });
});
