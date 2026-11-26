/**
 * Reading a plan an agent wrote, for the card that presents it (components/chat/PlanCard.tsx):
 * its own title, its sections and its steps, and the words Ghost sends when the person hands it
 * back. A plan is Markdown; nothing here changes it.
 */

/** A plan's own title (its first heading, when it opens with one) and everything after it. */
export function planParts(plan: string): { readonly title: string | null; readonly body: string } {
  const lines = plan.replace(/^\s*\n/, "").split("\n");
  const first = lines[0] ?? "";
  const heading = /^#{1,3}\s+(.+?)\s*#*\s*$/.exec(first);
  if (!heading?.[1]) return { title: null, body: plan };
  return {
    title: titleWords(heading[1]),
    body: lines
      .slice(1)
      .join("\n")
      .replace(/^\s*\n/, ""),
  };
}

/**
 * A heading as a title: plain words (emphasis marks and code ticks are for the body's renderer),
 * without a leading "Plan:", which the card already says by being one.
 */
function titleWords(heading: string): string | null {
  const words = heading
    .replace(/[*_`]/g, "")
    .trim()
    .replace(/^(?:implementation\s+)?plan\s*[:\p{Pd}]\s*/iu, "");
  return words === "" ? null : words.charAt(0).toUpperCase() + words.slice(1);
}

/** The lines of a plan outside its code fences, so a fenced `# comment` is not a heading. */
function proseLines(plan: string): readonly string[] {
  const lines: string[] = [];
  let fence: string | null = null;
  for (const line of plan.split("\n")) {
    const mark = /^\s{0,3}(`{3,}|~{3,})/.exec(line)?.[1] ?? null;
    if (fence === null && mark !== null) fence = mark[0] ?? null;
    else if (fence !== null && mark !== null && mark[0] === fence) fence = null;
    else if (fence === null) lines.push(line);
  }
  return lines;
}

/** The names of a plan's sections: the headings under its title, in order. */
export function planSections(body: string): readonly string[] {
  const names: string[] = [];
  for (const line of proseLines(body)) {
    const heading = /^#{1,4}\s+(.+?)\s*#*\s*$/.exec(line);
    const name = heading?.[1]?.replace(/[*_`]/g, "").trim();
    if (name) names.push(name);
  }
  return names;
}

/** How many steps a plan lists: its numbered items and its checkboxes, at the top level. */
export function planSteps(plan: string): number {
  return proseLines(plan).filter((line) => /^(\d+[.)]|[-*+]\s+\[[ xX]\])\s+\S/.test(line)).length;
}

/** The line under a plan's title: whose it is, and how much there is to it. */
export function planMeta(agentName: string, plan: string): string {
  const steps = planSteps(plan);
  const whose = /s$/i.test(agentName) ? `${agentName}' plan` : `${agentName}'s plan`;
  return steps === 0 ? whose : `${whose} · ${steps} ${steps === 1 ? "step" : "steps"}`;
}

/** Whether the person's text is a different plan from the agent's: edits to its words. */
export function planEdited(original: string, draft: string): boolean {
  return draft.trim() !== "" && draft.trim() !== original.trim();
}

/** What Request changes sends the agent with the declined plan. */
export function planFeedback(note: string): string {
  return `Before implementing, add this to the plan and show me the updated plan:\n\n${note.trim()}`;
}

/** What sending an edited plan back sends the agent with the declined plan. */
export function planRevision(plan: string): string {
  return `I edited the plan. Use this version in place of yours, and show me the plan again before implementing:\n\n${plan.trim()}`;
}
