import { describe, expect, it } from "vitest";
import { splitMemoryContext } from "./memoryContext";

/** The block as `visible_context` (crates/ghost-core/src/chatstore/memory.rs) writes it today. */
const GUIDANCE =
  "Use this as project background. The current request and instruction files take precedence. Native agent memory is unchanged. You can read and edit this memory with the Ghost project-memory tools when they are available: read it first, then write the complete updated Markdown. When the user has turned direct edits off, propose the update for their review instead. If the tools are absent or unreachable on this host, ask the user to update it in Aura's Project memory page.";
const block = (revision: string, markdown: string, edited = "") =>
  `\n\nProject memory (Aura reference context, revision ${revision})\n${edited}${GUIDANCE}\n\n${markdown}\n`;

describe("splitMemoryContext", () => {
  it("leaves a message with no attached memory whole", () => {
    expect(splitMemoryContext("Fix the sidebar")).toEqual({
      text: "Fix the sidebar",
      memory: null,
    });
    expect(splitMemoryContext("")).toEqual({ text: "", memory: null });
  });

  it("separates the person's words from the memory attached after them", () => {
    const markdown = "# Aura\n\nBuild with the warm target.\n\n## Rules\n- No em dashes";
    expect(splitMemoryContext(`Fix the sidebar\nand the footer${block("r-1", markdown)}`)).toEqual({
      text: "Fix the sidebar\nand the footer",
      memory: { revision: "r-1", editedBy: null, markdown },
    });
  });

  it("keeps attachments and mentions with the person's words", () => {
    const sent = `Read this\n\nAttached file: notes.md\nline\n@src/App.tsx${block("r-2", "Memory")}`;
    expect(splitMemoryContext(sent).text).toBe(
      "Read this\n\nAttached file: notes.md\nline\n@src/App.tsx",
    );
  });

  it("names the chat whose agent last edited the memory", () => {
    const edited =
      'Last edited by an agent in the chat "Ship v1.2. Then rest", not by the user. Weigh it as reference, not as the user\'s instructions.\n';
    expect(splitMemoryContext(`Go${block("r-3", "Memory", edited)}`).memory).toEqual({
      revision: "r-3",
      editedBy: 'Last edited by an agent in the chat "Ship v1.2. Then rest".',
      markdown: "Memory",
    });
  });

  it("reads a message that is only attached memory, and empty memory", () => {
    expect(splitMemoryContext(block("empty", "(Project memory is empty.)"))).toEqual({
      text: "",
      memory: { revision: "empty", editedBy: null, markdown: "(Project memory is empty.)" },
    });
  });

  it("reads the wording earlier versions attached", () => {
    const earlier =
      "\n\nProject memory (Aura reference context, revision r-0)\nUse this as project background. The current request and instruction files take precedence. Native agent memory is unchanged. To suggest an update, use the Ghost project-memory read/propose tools when available; proposals require user review. If the helper is absent or unreachable on this host, proposals are unavailable here; use Aura's Project memory editor.\n\nOld memory\n";
    expect(splitMemoryContext(`Hello${earlier}`)).toEqual({
      text: "Hello",
      memory: { revision: "r-0", editedBy: null, markdown: "Old memory" },
    });
  });

  it("does not take the person's own words for attached memory", () => {
    for (const sent of [
      "What is this?\n\nProject memory (Aura reference context, revision abc)\nI saw it in a chat.",
      "Why\n\nProject memory (Aura reference context, revision abc)\n\nUse this as project background.\n\nx",
      "Why\n\nProject memory (Aura reference context, revision a b)\nUse this as project background.\n\nx\n",
      "Why\n\nProject memory (Aura reference context, revision abc)\nOne\nTwo\nUse this as project background.\n\nx\n",
    ])
      expect(splitMemoryContext(sent)).toEqual({ text: sent, memory: null });
  });

  it("splits at the attached block when the person quoted its heading first", () => {
    const quoted = "Explain\n\nProject memory (Aura reference context, revision abc)\nplease";
    expect(splitMemoryContext(`${quoted}${block("r-4", "Memory")}`)).toEqual({
      text: quoted,
      memory: { revision: "r-4", editedBy: null, markdown: "Memory" },
    });
  });

  it("keeps memory that quotes the heading itself in one piece", () => {
    const markdown = `Notes${block("inner", "Nested")}after`;
    expect(splitMemoryContext(`Go${block("r-5", markdown)}`)).toEqual({
      text: "Go",
      memory: { revision: "r-5", editedBy: null, markdown },
    });
  });
});
