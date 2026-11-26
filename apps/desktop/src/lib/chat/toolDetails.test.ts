import { describe, expect, it } from "vitest";
import type { ToolContent, Value } from "../../bindings";
import { diffDelta, prettyJson, searchLinks, toolInput, withoutSearchLinks } from "./toolDetails";
import type { ToolItem } from "./transcript";

const tool = (
  toolName: string,
  rawInput: Value | null,
  content: readonly ToolContent[] = [],
  title = toolName,
): ToolItem => ({
  kind: "tool",
  key: "t",
  id: "t",
  toolKind: "other",
  title,
  status: "completed",
  locations: [],
  rawInput,
  toolName,
  content,
  truncated: null,
});

describe("a tool call's input", () => {
  it("shows what the call was given, set out by size", () => {
    expect(
      toolInput(
        tool("mcp__github__search_issues", {
          query: "is:open label:bug",
          limit: 20,
          filters: { state: "open", labels: ["bug"] },
          empty: "",
          missing: null,
        }),
      ),
    ).toEqual([
      { key: "query", value: "is:open label:bug", block: false },
      { key: "limit", value: "20", block: false },
      {
        key: "filters",
        value: '{\n  "state": "open",\n  "labels": [\n    "bug"\n  ]\n}',
        block: true,
      },
    ]);
    const command = "cargo test -p ghost-core --lib \\\n  onboarding";
    expect(toolInput(tool("Bash", { command }, [], "cargo test -p ghost-core …"))).toEqual([
      { key: "command", value: command, block: true },
    ]);
  });
  it("leaves out what the call shows elsewhere, and a title that says it all", () => {
    const edit = tool("Edit", { file_path: "a.rs", old_string: "a", new_string: "b" }, [
      { type: "diff", path: "a.rs", old_text: "a\n", new_text: "b\n", truncated: null },
    ]);
    expect(toolInput(edit)).toEqual([{ key: "file_path", value: "a.rs", block: false }]);
    expect(toolInput({ ...edit, locations: [{ path: "a.rs", line: 3 }] })).toEqual([]);
    expect(toolInput(tool("TodoWrite", { todos: [] }))).toEqual([]);
    expect(toolInput(tool("Read", { file_path: "src/a.rs" }, [], "src/a.rs"))).toEqual([]);
    expect(
      toolInput(
        tool("WebSearch", { query: "tauri channels" }, [], "Search the web for tauri channels"),
      ),
    ).toEqual([]);
    expect(toolInput(tool("Other", ["not", "an", "object"]))).toEqual([]);
    const long = "x".repeat(3000);
    expect(toolInput(tool("Write", { note: long }))[0]?.value).toHaveLength(2001);
  });
});

describe("a tool's text output", () => {
  it("sets out JSON over lines, and leaves everything else alone", () => {
    expect(prettyJson('{"ok":true,"items":[1,2]}')).toBe(
      '{\n  "ok": true,\n  "items": [\n    1,\n    2\n  ]\n}',
    );
    expect(prettyJson("[]")).toBeNull();
    expect(prettyJson('{\n  "ok": true\n}')).toBeNull();
    expect(prettyJson("{not json}")).toBeNull();
    expect(prettyJson("plain text")).toBeNull();
  });
});

describe("a web search's links", () => {
  it("reads the pages Claude Code's WebSearch found, once each, http and https only", () => {
    const text = [
      'Web search results for query: "tauri channels"',
      "",
      'Links: [{"title":"Calling the frontend [Tauri]","url":"https://v2.tauri.app/develop/calling-frontend/"},{"title":"","url":"https://www.example.com/a"},{"title":"bad","url":"javascript:alert(1)"},{"title":"again","url":"https://v2.tauri.app/develop/calling-frontend/"}]',
      "",
      "Channels stream ordered data to the frontend.",
    ].join("\n");
    expect(searchLinks(tool("WebSearch", null, [{ type: "text", text, truncated: null }]))).toEqual(
      [
        {
          title: "Calling the frontend [Tauri]",
          url: "https://v2.tauri.app/develop/calling-frontend/",
          host: "v2.tauri.app",
        },
        {
          title: "https://www.example.com/a",
          url: "https://www.example.com/a",
          host: "example.com",
        },
      ],
    );
    expect(searchLinks(tool("Bash", null, [{ type: "text", text, truncated: null }]))).toEqual([]);
    expect(withoutSearchLinks(text)).toBe(
      'Web search results for query: "tauri channels"\n\nChannels stream ordered data to the frontend.',
    );
    expect(
      searchLinks(tool("WebSearch", null, [{ type: "text", text: "Links: [{", truncated: null }])),
    ).toEqual([]);
  });
});

describe("an edit's line counts", () => {
  it("adds up its diffs, and counts nothing it cannot see", () => {
    expect(
      diffDelta(
        tool("MultiEdit", null, [
          { type: "diff", path: "a", old_text: "a\nb\n", new_text: "a\nc\nd\n", truncated: null },
          { type: "diff", path: "b", old_text: null, new_text: "new\n", truncated: null },
        ]),
      ),
    ).toEqual({ added: 3, removed: 1 });
    expect(
      diffDelta(tool("Bash", null, [{ type: "text", text: "x", truncated: null }])),
    ).toBeNull();
    const unloaded = {
      type: "diff",
      path: "big",
      old_text: null,
      new_text: "",
      truncated: { blob: "b", total_bytes: 9_000_000 },
    } as unknown as ToolContent;
    expect(diffDelta(tool("Write", null, [unloaded]))).toBeNull();
  });
});
