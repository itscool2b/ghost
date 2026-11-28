import { useCallback, useEffect, useState } from "react";
import type { EngineProbe, GameProject } from "../../bindings";
import { errorMessage } from "../../lib/errors";
import type { GameApi } from "../../lib/game";

type Connected = "unity" | "unreal" | "roblox";

/** One step of a maker's own setup: words, with what to type or press set apart. */
type Step = readonly (string | { readonly code: string })[];

interface Guide {
  readonly name: string;
  /** Where the game runs, and what the tools are for. */
  readonly lead: (agent: string) => string;
  readonly steps: readonly Step[];
  /** What the maker says about who can reach the tools. */
  readonly caution: string | null;
  /** Ghost can add the maker's MCP server to the chat's project. */
  readonly addable: boolean;
  readonly links: readonly (readonly [string, string])[];
}

const code = (text: string) => ({ code: text });

/**
 * Each maker's own steps, as its guide gave them when this was written (27 November 2026). They
 * are the makers' to change: each card links the guide it was read from.
 */
const GUIDES: Readonly<Record<Connected, Guide>> = {
  unity: {
    name: "Unity",
    lead: (agent) =>
      `Ghost does not run Unity itself. Unity's own agent tools let ${agent} work in the Unity Editor, where the game runs and shows. They need Unity 6 or later.`,
    steps: [
      [
        "Install the Unity CLI: ",
        code("winget install Unity.CLI"),
        " on Windows, or ",
        code("curl -fsSL https://unity.com/install.sh | bash"),
        " on macOS and Linux. Open a new terminal afterwards.",
      ],
      ["Open this project in the Unity Editor, and sign in with ", code("unity auth login"), "."],
      [
        "In the project's folder run ",
        code("unity pipeline install"),
        ", wait for the Editor to recompile, and check with ",
        code("unity pipeline editors"),
        ".",
      ],
      [
        "For Claude Code, run ",
        code("/plugin marketplace add Unity-Technologies/unity-agent-plugin"),
        " and then ",
        code("/plugin install unity@unity-agent-plugin"),
        " in its terminal. For another agent, run ",
        code("unity mcp configure --list"),
        " and then ",
        code("unity mcp configure"),
        " with its name.",
      ],
    ],
    caution: null,
    addable: false,
    links: [
      [
        "Unity's guide for agents",
        "https://docs.unity.com/en-us/ai/unity-plugin/about-unity-plugin",
      ],
      ["The Unity CLI", "https://docs.unity.com/en-us/unity-cli/use-unity-cli"],
    ],
  },
  unreal: {
    name: "Unreal Engine",
    lead: (agent) =>
      `Ghost does not run Unreal itself. The Unreal editor has an MCP server of its own that lets ${agent} work in it, where the game runs and shows. Epic marks it experimental.`,
    steps: [
      [
        "In the Unreal editor open Edit > Plugins. Check that Unreal MCP is on, turn on All Toolsets, and restart the editor when it asks.",
      ],
      [
        "The server starts with the editor, at ",
        code("http://127.0.0.1:8000/mcp"),
        ". Its settings are under Edit > Editor Preferences > Agentic Workflows > Model Context Protocol.",
      ],
      [
        "Press Add to this project below, then start a new chat here, or stop and start this chat's agent. Agents that take MCP servers over HTTP get Unreal's tools.",
      ],
    ],
    caution:
      "Unreal's server has no sign-in: while the editor runs, any program on this computer can call its tools.",
    addable: true,
    links: [
      [
        "Epic's guide",
        "https://dev.epicgames.com/documentation/en-us/unreal-engine/unreal-mcp-in-unreal-editor",
      ],
    ],
  },
  roblox: {
    name: "Roblox Studio",
    lead: (agent) =>
      `Ghost does not run Roblox Studio itself. Studio has an MCP server built in that lets ${agent} work in your open place: scripts, the data model, and playtests.`,
    steps: [
      [
        "In Roblox Studio open Assistant, press the three dots, choose Manage MCP Servers, and turn on Enable Studio as MCP server.",
      ],
      [
        "Press Add to this project below, which adds Studio's own launcher as an MCP server. Or turn your agent on under Quick connect in Studio.",
      ],
      ["Restart Studio, then start a new chat here, or stop and start this chat's agent."],
    ],
    caution:
      "A connected agent can read and change what is in your open places. Studio does not publish for it.",
    addable: true,
    links: [["Roblox's guide", "https://create.roblox.com/docs/studio/mcp"]],
  },
};

/** Whether `engine` is one Ghost connects to instead of running. */
export function connected(engine: GameProject["engine"]): engine is Connected {
  return engine === "unity" || engine === "unreal" || engine === "roblox";
}

/**
 * A project made with Unity, Unreal or Roblox (docs/game.md, "Other engines"): what its maker's
 * own agent tools are, the maker's steps to turn them on, whether they are within reach on this
 * computer now, and, where the maker gives an MCP server, one press that adds it to the chat's
 * project. Adding it is the person's: no agent's tool does.
 */
export function EngineConnect({
  chatId,
  api,
  project,
  agentName,
}: {
  readonly chatId: string;
  readonly api: GameApi;
  readonly project: GameProject;
  readonly agentName: string;
}) {
  const guide = connected(project.engine) ? GUIDES[project.engine] : null;
  const [probe, setProbe] = useState<EngineProbe | null>(null);
  const [looking, setLooking] = useState(true);
  const [added, setAdded] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const look = useCallback(() => {
    setLooking(true);
    api.probe(chatId).then(
      (found) => {
        setProbe(found);
        setLooking(false);
      },
      () => setLooking(false),
    );
  }, [api, chatId]);
  useEffect(look, [look]);
  if (!guide) return null;
  const add = () => {
    setProblem(null);
    api.connect(chatId).then(
      (result) =>
        setAdded(
          result === "added"
            ? `Added. New chats in this project get ${guide.name}'s tools; stop and start this chat's agent to use them here.`
            : `${guide.name}'s server is already one of this project's MCP servers.`,
        ),
      (error) => setProblem(errorMessage(error)),
    );
  };
  return (
    <div
      className="game-message game-connect"
      data-testid="game-connect"
      data-engine={project.engine}
    >
      <h2>
        Connect {guide.name}
        {project.name ? ` for ${project.name}` : ""}
      </h2>
      <p>{guide.lead(agentName)}</p>
      <ol className="game-steps">
        {guide.steps.map((step, index) => (
          // biome-ignore lint/suspicious/noArrayIndexKey: the steps are a fixed list in a fixed order
          <li key={index}>
            {step.map((part, at) =>
              typeof part === "string" ? (
                part
              ) : (
                // biome-ignore lint/suspicious/noArrayIndexKey: a step's parts never change order
                <code key={at} className="game-code">
                  {part.code}
                </code>
              ),
            )}
          </li>
        ))}
      </ol>
      {guide.caution ? <p className="game-note">{guide.caution}</p> : null}
      <p className="game-probe" data-testid="game-probe" data-ready={probe?.ready ? "" : undefined}>
        <span className="game-probe-dot" aria-hidden="true" />
        <span className="game-probe-text" role="status">
          {probe === null
            ? looking
              ? "Looking on this computer."
              : "Could not look."
            : probe.detail}
        </span>
        <button
          type="button"
          className="g-button g-button-ghost g-button-28"
          data-testid="game-probe-again"
          disabled={looking}
          onClick={look}
        >
          Check again
        </button>
      </p>
      {added ? (
        <output className="game-note" data-testid="game-connected">
          {added}
        </output>
      ) : null}
      {problem ? (
        <p className="ui-error" role="alert">
          {problem}
        </p>
      ) : null}
      <div className="game-actions">
        {guide.addable ? (
          <button
            type="button"
            className="g-button g-button-primary g-button-28"
            data-testid="game-connect-add"
            onClick={add}
          >
            Add to this project
          </button>
        ) : null}
        {guide.links.map(([label, href]) => (
          <a
            key={href}
            className="g-button g-button-ghost g-button-28"
            href={href}
            target="_blank"
            rel="noreferrer noopener"
          >
            {label}
          </a>
        ))}
      </div>
    </div>
  );
}
