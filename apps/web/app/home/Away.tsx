import { AGENT_BRANDS, type AgentBrandId } from "@ghost/ui";
import { SpriteMark } from "./Sprite";
import { Stage } from "./Stage";

const WAYS: readonly { title: string; text: string }[] = [
  {
    title: "Pair your phone with a code",
    text: "Scan it with the camera, or open the link in any browser. The link opens once, within ten minutes.",
  },
  {
    title: "Nothing listens on your computer",
    text: "Ghost connects out to its relay. A paired phone's session is encrypted end to end: the relay passes it along and cannot read it.",
  },
  {
    title: "Hand your assistant whole tasks",
    text: "Link Grok Bot, Muse, Hermes or OpenClaw. Each task runs on its own branch, gets a second agent's review, and your assistant hears when it is done.",
  },
];

const CHATS: readonly {
  agent: AgentBrandId;
  title: string;
  state: "needs" | "working" | "done";
}[] = [
  { agent: "claude-code", title: "Ship the frame pacing fix", state: "needs" },
  { agent: "codex", title: "Tighten the hero spacing", state: "working" },
  { agent: "pi", title: "Update the changelog", state: "done" },
  { agent: "grok", title: "Find what blocks a frame", state: "done" },
];

const STATE_WORDS = { needs: "Needs you", working: "Working", done: "Finished" } as const;

/**
 * Ghost from somewhere else: three plain statements at the left, and at the right a phone standing
 * on a stage, showing this computer's chats as the remote page lists them. The phone is drawn in
 * CSS, an illustration of a supported state, hidden from assistive technology.
 */
export function Away() {
  return (
    <section className="section away" aria-labelledby="away-title">
      <div className="wrap">
        <div className="chapter-head">
          <h2 id="away-title" className="title">
            From anywhere. <span className="tone">Your phone, or your own assistant.</span>
          </h2>
          <p className="lead">
            The work happens on your computer. You do not have to be sitting at it to see how it is
            going, answer a question or start the next thing.
          </p>
        </div>
        <div className="away-grid">
          <ul className="away-list">
            {WAYS.map((way) => (
              <li key={way.title}>
                <h3>{way.title}</h3>
                <p>{way.text}</p>
              </li>
            ))}
          </ul>
          <Stage quiet className="away-stage">
            <div className="phone" aria-hidden="true">
              <div className="phone-screen">
                <p className="phone-bar">
                  <span className="phone-title">This computer</span>
                  <span className="phone-live">
                    <i />
                    Connected
                  </span>
                </p>
                <ul className="phone-chats">
                  {CHATS.map((chat) => (
                    <li key={chat.title} data-state={chat.state}>
                      <SpriteMark agent={chat.agent} size={22} />
                      <span className="phone-chat">
                        <strong>{chat.title}</strong>
                        <span>
                          {AGENT_BRANDS[chat.agent].name} · {STATE_WORDS[chat.state]}
                        </span>
                      </span>
                      <i className="phone-dot" />
                    </li>
                  ))}
                </ul>
                <p className="phone-box">
                  <span>Message Claude Code</span>
                  <i className="phone-send" />
                </p>
              </div>
            </div>
          </Stage>
        </div>
      </div>
    </section>
  );
}
