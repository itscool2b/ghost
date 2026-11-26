/**
 * A synthetic installer for previews: Rust's review of a pinned npm install, and a terminal that
 * prints npm's first lines and then keeps running. No process starts and nothing is installed.
 */
import type { TerminalChunk } from "../bindings";
import type { InstallApi } from "../lib/setup-actions/api";

const PACKAGES: Readonly<Record<string, [string, string]>> = {
  grok: ["@xai-official/grok", "1.0.70"],
  claude: ["@anthropic-ai/claude-code", "2.1.310"],
  codex: ["@openai/codex", "0.162.1"],
  pi: ["@earendil-works/pi-coding-agent", "1.0.2"],
  opencode: ["opencode-ai", "1.19.4"],
};

const OUTPUT = [
  "\x1b[2mnpm\x1b[0m install --global @xai-official/grok@1.0.70\r\n",
  "\r\n",
  "\x1b[2m⠙\x1b[0m reify:@xai-official/grok: timing reifyNode:node_modules/@xai-official/grok\r\n",
  "\x1b[2m⠹\x1b[0m reify: http fetch GET 200 https://registry.npmjs.org/@xai-official/grok 412ms\r\n",
];

export function installFixture(): InstallApi {
  return {
    review: async (agentId) => {
      const [name, version] = PACKAGES[agentId] ?? ["@xai-official/grok", "1.0.70"];
      return {
        agent_id: agentId,
        package: name,
        version,
        program: "C:\\Program Files\\nodejs\\node.exe",
        arguments: [
          "C:\\Program Files\\nodejs\\node_modules\\npm\\bin\\npm-cli.js",
          "install",
          "--global",
          `${name}@${version}`,
        ],
        working_directory: "C:\\Users\\fixture",
        revision: "synthetic",
        timeout_seconds: 600,
        notices: [
          "Installs the integration's last verified version into npm's global prefix. npm uses its existing settings; Ghost does not read them.",
          "Closing the terminal or reaching ten minutes stops this owned installation. A partial package may require another installation attempt.",
          "Sign-in is a separate action. A successful command still needs a fresh agent version check.",
        ],
      };
    },
    terminal: () => {
      let sent = 0;
      let closed = false;
      return {
        reserve: async () => "fixture-install",
        start: async () => "fixture-install",
        read: () =>
          new Promise<TerminalChunk>((resolve) => {
            const line = OUTPUT[sent];
            if (line !== undefined && !closed) {
              sent++;
              setTimeout(
                () =>
                  resolve({
                    bytes: [...new TextEncoder().encode(line)],
                    done: false,
                    exit_code: null,
                    truncated: false,
                  }),
                120,
              );
            }
            // After its first lines npm keeps working: the read waits until the sheet closes.
          }),
        write: async () => {},
        resize: async () => {},
        close: async () => {
          closed = true;
        },
      };
    },
  };
}
