import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { AgentConnect, CONNECT_SHOWN, connectState, placeTiles } from "./AgentConnect";

describe("the agents joined to Ghost", () => {
  it("draws what each row shows, and leaves out an agent that is not here", () => {
    expect(connectState("ready", false)).toBe("connected");
    expect(connectState("signin", false)).toBe("signin");
    expect(connectState("checking", false)).toBe("connecting");
    expect(connectState("broken", false)).toBe("failed");
    expect(connectState("missing", false)).toBeNull();
    // An install, update or sign-in under way is a connection being made.
    expect(connectState("ready", true)).toBe("connecting");
  });

  it("places tiles on both sides of the hub without two in one spot", () => {
    for (let count = 1; count <= CONNECT_SHOWN; count++) {
      const places = placeTiles(count);
      expect(places).toHaveLength(count);
      const keys = new Set(places.map(({ x, y }) => `${x},${y}`));
      expect(keys.size).toBe(count);
      for (const { x } of places) expect(Math.abs(x - 300)).toBeGreaterThan(60);
    }
  });

  it("names how many are connected, and shows no more than it has room for", () => {
    const agents = Array.from({ length: 10 }, (_, index) => ({
      id: `agent-${index}`,
      name: `Agent ${index}`,
      state: index === 0 ? ("signin" as const) : ("connected" as const),
    }));
    const html = renderToStaticMarkup(<AgentConnect agents={agents} />);
    expect(html).toContain('aria-label="9 of 10 agents connected to Ghost."');
    expect(html.match(/class="agent-connect-tile"/g)).toHaveLength(CONNECT_SHOWN);
  });

  it("puts each tick on its own line, on a group the landing animation does not move", () => {
    const agents = ["one", "two", "three"].map((id) => ({
      id,
      name: id,
      state: "connected" as const,
    }));
    const html = renderToStaticMarkup(<AgentConnect agents={agents} />);
    // The animated group carries no place of its own: a CSS animation of `transform` would
    // replace it, which once drew every tick in the picture's corner.
    expect(html).not.toMatch(/class="agent-connect-check"[^>]*transform=/);
    const places = [
      ...html.matchAll(/<g transform="translate\(([^)]+)\)"><g class="agent-connect-check">/g),
    ].map((match) => match[1]);
    expect(places).toHaveLength(3);
    expect(new Set(places).size).toBe(3);
  });
});
