/**
 * Added agent accounts on synthetic data: an agent's Accounts in its details, and a chat's
 * usage-limit card. `?accounts=none` starts with no added account, `?accounts=unavailable` on a
 * computer without secure storage. No native call is made and no real token exists here.
 */
import "../styles.css";
import "../ui-tokens.css";
import "../chat.css";
import "../shell.css";
import "../components/setup/agents.css";
import "../components/setup/setup.css";
import "../components/settings/settings.css";
import { createRoot } from "react-dom/client";
import type { UsageLimit } from "../bindings";
import { AccountPicker } from "../components/chat/AccountPicker";
import { UsageLimitCard } from "../components/chat/UsageLimitCard";
import { AgentAccounts } from "../components/setup/AgentAccounts";
import { createAccountsFixture, FIXTURE_TOKEN } from "./accountsFixture";

const variant = new URLSearchParams(location.search).get("accounts");
const fixture = createAccountsFixture(
  variant === "none" || variant === "unavailable" ? [] : ["Work"],
  variant === "unavailable"
    ? { unavailable: "This computer has no secure storage Ghost can keep account tokens in." }
    : variant === "nocapture"
      ? { tokenOutput: "missing" }
      : variant === "code"
        ? { tokenOutput: "code" }
        : {},
);
const managed: string[] = [];
const limit: UsageLimit = {
  message: "You've hit your 5-hour limit · resets 5:30pm (America/New_York)",
  resets_at: Date.UTC(2026, 10, 1, 22, 30),
};
Object.assign(window, {
  accountsFixture: {
    calls: () => [...fixture.calls],
    switched: () => fixture.switched(),
    managed: () => [...managed],
    token: FIXTURE_TOKEN,
  },
});

function Preview() {
  return (
    <main className="g-root accounts-preview">
      <h1>Agent accounts</h1>
      <section aria-labelledby="accounts-agents" className="accounts-preview-section">
        <h2 id="accounts-agents">Claude Code on the Agents page</h2>
        <div className="agent-list">
          <div className="agent-detail">
            <AgentAccounts agentId="claude" agentName="Claude Code" api={fixture} />
          </div>
        </div>
      </section>
      <section aria-labelledby="accounts-composer" className="accounts-preview-section">
        <h2 id="accounts-composer">The composer's account picker</h2>
        <div className="accounts-preview-composer" data-testid="composer-idle">
          <div className="chat-pickers">
            <AccountPicker
              chatId="chat-idle"
              agentId="claude"
              agentName="Claude Code"
              status="ready"
              runningAccount={null}
              held={false}
              api={fixture}
              onManage={() => managed.push("agents")}
            />
          </div>
        </div>
        <div className="accounts-preview-composer" data-testid="composer-running">
          <div className="chat-pickers">
            <AccountPicker
              chatId="chat-running"
              agentId="claude"
              agentName="Claude Code"
              status="running"
              runningAccount={null}
              held={false}
              api={fixture}
            />
          </div>
        </div>
      </section>
      <section aria-labelledby="accounts-chat" className="accounts-preview-section">
        <h2 id="accounts-chat">A chat at its usage limit</h2>
        <UsageLimitCard
          limit={limit}
          chatId="chat-limited"
          agentId="claude"
          agentName="Claude Code"
          accountId={null}
          api={fixture}
        />
      </section>
    </main>
  );
}

export function mountAccountsPreview(container: HTMLElement): void {
  const style = document.createElement("style");
  style.textContent = `
    .accounts-preview { min-height: 100vh; padding: 32px; background: var(--g-surface-1); color: var(--g-fg-1); }
    .accounts-preview h1 { margin: 0 0 24px; font-size: 20px; }
    .accounts-preview h2 { margin: 0 0 12px; font-size: 14px; color: var(--g-fg-2); font-weight: 500; }
    .accounts-preview-section { max-width: 760px; margin-bottom: 32px; }
    .accounts-preview .agent-detail { padding-top: 16px; border-radius: 14px; }
    .accounts-preview-composer { position: relative; display: flex; margin-top: 360px; padding: 8px; border-radius: 22px; background: var(--g-glass-pop); box-shadow: inset 0 0 0 1px var(--g-glass-rim); }
    .accounts-preview-composer + .accounts-preview-composer { margin-top: 16px; }
  `;
  document.head.append(style);
  createRoot(container).render(<Preview />);
}
