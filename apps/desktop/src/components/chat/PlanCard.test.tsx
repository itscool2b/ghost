import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { PermissionRequest } from "../../bindings";
import {
  planEdited,
  planFeedback,
  planMeta,
  planParts,
  planRevision,
  planSections,
  planSteps,
} from "../../lib/chat/plan";
import type { PermissionItem } from "../../lib/chat/transcript";
import { PermissionCard } from "./PermissionCard";
import { planApproval } from "./PlanCard";

const request = (overrides: Partial<PermissionRequest> = {}): PermissionRequest => ({
  request_id: "exit-plan",
  tool_call_id: null,
  title: "ExitPlanMode",
  kind: "switch_mode",
  raw_input: { plan: "# Ship it\n\n1. Build the picker" },
  content: [],
  locations: [],
  options: [
    { option_id: "allow", kind: "allow_once", label: "Allow once" },
    { option_id: "deny", kind: "reject_once", label: "Deny" },
  ],
  truncated: null,
  ...overrides,
});

const item = (ask: PermissionRequest, resolved: string | null = null) =>
  ({
    kind: "permission",
    key: ask.request_id,
    request: ask,
    resolution:
      resolved === null
        ? null
        : { by: "user", decision: { type: "selected", option_id: resolved } },
  }) as unknown as PermissionItem;

describe("a plan waiting for review", () => {
  it("is a mode switch that carries a plan and both an allow and a reject", () => {
    expect(planApproval(request())?.plan).toContain("# Ship it");
    expect(planApproval(request({ kind: "execute" }))).toBeNull();
    expect(planApproval(request({ raw_input: { plan: "  " } }))).toBeNull();
    expect(planApproval(request({ raw_input: null }))).toBeNull();
    expect(
      planApproval(request({ options: [{ option_id: "a", kind: "allow_once", label: "A" }] })),
    ).toBeNull();
  });

  it("presents the plan as a document: its title, whose it is, its prose and its answers", () => {
    const html = renderToStaticMarkup(
      <PermissionCard
        item={item(request())}
        request={undefined}
        onRespond={() => {}}
        agentName="Claude Code"
        onAddToPlan={async () => {}}
        blobs={new Map()}
        onLoadBlob={() => {}}
      />,
    );
    expect(html).toContain('data-kind="plan-approval"');
    // Its own title leads, in the card's heading and not repeated in the prose under it.
    expect(html).toMatch(/<h3[^>]*class="chat-plan-title"[^>]*>Ship it<\/h3>/);
    expect(html.match(/Ship it/g)).toHaveLength(1);
    expect(html).toContain("Claude Code&#x27;s plan · 1 step");
    expect(html).toContain("Build the picker");
    // Nothing about it reads as a tool's request.
    for (const gone of ["Plan ready for review", "ExitPlanMode", "chat-block", "Allow once"])
      expect(html).not.toContain(gone);
    expect(html).toContain("Accept plan");
    expect(html).toContain("Request changes");
    expect(html).toMatch(/data-testid="plan-edit"[^>]*>.*Edit<\/button>/);
    expect(html).toMatch(/data-test-option-kind="reject_once"[^>]*>Decline<\/button>/);
    expect(html).toContain("Accepting lets Claude Code start implementing.");
  });

  it("offers the chat's Build with first when it can take the plan, and keeps Accept plan", () => {
    const html = renderToStaticMarkup(
      <PermissionCard
        item={item(request())}
        request={undefined}
        onRespond={() => {}}
        agentName="Claude Code"
        onAddToPlan={async () => {}}
        planBuild={{ label: "Codex · gpt-6-astra", run: async () => {} }}
        blobs={new Map()}
        onLoadBlob={() => {}}
      />,
    );
    expect(html).toContain("Accept and build with Codex · gpt-6-astra");
    // The handing button is the filled one; Accept plan stays, as a plain button.
    expect(html).toMatch(/class="g-button g-button-primary"[^>]*data-testid="plan-build"/);
    expect(html).toMatch(/class="g-button"[^>]*data-testid="permission-option"[^>]*>Accept plan/);
    expect(html).toContain(
      "Accept and build hands the plan to Codex · gpt-6-astra, this chat&#x27;s Build with. Accept plan lets Claude Code implement it.",
    );
    // An answered plan offers nothing more.
    const answered = renderToStaticMarkup(
      <PermissionCard
        item={item(request(), "allow")}
        request={undefined}
        onRespond={() => {}}
        planBuild={{ label: "Codex", run: async () => {} }}
        blobs={new Map()}
        onLoadBlob={() => {}}
      />,
    );
    expect(answered).not.toContain("Accept and build");
  });

  it("says what was decided once it is answered", () => {
    const accepted = renderToStaticMarkup(
      <PermissionCard
        item={item(request(), "allow")}
        request={undefined}
        onRespond={() => {}}
        blobs={new Map()}
        onLoadBlob={() => {}}
      />,
    );
    expect(accepted).toContain("Plan accepted.");
    expect(accepted).not.toContain("Accept plan");
    const declined = renderToStaticMarkup(
      <PermissionCard
        item={item(request(), "deny")}
        request={undefined}
        onRespond={() => {}}
        blobs={new Map()}
        onLoadBlob={() => {}}
      />,
    );
    expect(declined).toContain("Sent back to keep planning.");
  });

  it("shows an accepted plan as the person edited it, and says so", () => {
    const edited = {
      kind: "permission",
      key: "exit-plan",
      request: request(),
      resolution: {
        by: "user",
        decision: {
          type: "selected",
          option_id: "allow",
          plan: "# Ship it today\n\n1. Build\n2. Test",
        },
      },
    } as unknown as PermissionItem;
    const html = renderToStaticMarkup(
      <PermissionCard
        item={edited}
        request={undefined}
        onRespond={() => {}}
        agentName="Claude Code"
        blobs={new Map()}
        onLoadBlob={() => {}}
      />,
    );
    expect(html).toContain("Plan accepted with your edits.");
    expect(html).toMatch(/class="chat-plan-title"[^>]*>Ship it today</);
    expect(html).toContain("Claude Code&#x27;s plan · 2 steps");
    expect(html).not.toContain("Build the picker");
    // A declined plan is the agent's own, whatever a record holds.
    const declined = renderToStaticMarkup(
      <PermissionCard
        item={
          {
            ...edited,
            resolution: {
              by: "user",
              decision: { type: "selected", option_id: "deny", plan: "# Not this" },
            },
          } as unknown as PermissionItem
        }
        request={undefined}
        onRespond={() => {}}
        blobs={new Map()}
        onLoadBlob={() => {}}
      />,
    );
    expect(declined).toContain("Build the picker");
    expect(declined).not.toContain("Not this");
  });

  it("offers no edits where the plan can neither be sent back nor accepted edited", () => {
    const html = renderToStaticMarkup(
      <PermissionCard
        item={item(request())}
        request={undefined}
        onRespond={() => {}}
        blobs={new Map()}
        onLoadBlob={() => {}}
      />,
    );
    expect(html).not.toContain("plan-edit");
    expect(html).not.toContain("Request changes");
    expect(html).toContain("Accept plan");
    expect(html).toContain("Decline");
  });

  it("reads a plan's title, sections and steps without being fooled by its code", () => {
    const plan = [
      "",
      "## **Ship** `it`",
      "",
      "Why.",
      "",
      "### Changes",
      "1. One",
      "2) Two",
      "   1. A nested step is not counted",
      "- [ ] Three",
      "- a plain bullet is not a step",
      "```sh",
      "# not a section",
      "1. not a step",
      "```",
      "### Verification",
    ].join("\n");
    const parts = planParts(plan);
    expect(parts.title).toBe("Ship it");
    expect(parts.body.startsWith("Why.")).toBe(true);
    expect(planSections(parts.body)).toEqual(["Changes", "Verification"]);
    expect(planSteps(plan)).toBe(3);
    expect(planMeta("Codex", plan)).toBe("Codex's plan · 3 steps");
    expect(planMeta("Hermes", "Just do it.")).toBe("Hermes' plan");
    // A plan that opens with words has no title of its own.
    expect(planParts("Do this.\n\n# Later")).toEqual({ title: null, body: "Do this.\n\n# Later" });
  });

  it("tells an edit from the same plan, and words each way back", () => {
    expect(planEdited("# A\n1. B", "  # A\n1. B\n")).toBe(false);
    expect(planEdited("# A\n1. B", "# A\n1. B\n2. C")).toBe(true);
    expect(planEdited("# A", "   ")).toBe(false);
    expect(planRevision("  # A\n1. B  ")).toBe(
      "I edited the plan. Use this version in place of yours, and show me the plan again before implementing:\n\n# A\n1. B",
    );
  });

  it("asks the agent to fold the note into the plan", () => {
    expect(planFeedback("  also cover Codex  ")).toBe(
      "Before implementing, add this to the plan and show me the updated plan:\n\nalso cover Codex",
    );
  });
});
