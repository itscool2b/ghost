import { useEffect, useRef } from "react";
import { AGENT_NAMES } from "../agents";
import { useReducedMotion } from "../motion";
import { Arrow } from "../ui";
import { AGENTS, SpriteMark } from "./Sprite";

const agentList = `${AGENT_NAMES.slice(0, -1).join(", ")} and ${AGENT_NAMES.at(-1) ?? ""}`;

const QUESTIONS: readonly { question: string; answer: string; agents?: boolean }[] = [
  {
    question: "Is Ghost free?",
    answer:
      "Yes. The desktop app is free and there is no paid tier. There is no Ghost account and nothing to sign up for.",
  },
  {
    question: "Which agents does it run?",
    answer: `${agentList}, plus any agent that speaks the Agent Client Protocol. Each one runs through its own harness.`,
    agents: true,
  },
  {
    question: "Can my assistant use Ghost?",
    answer:
      "Yes. Link Grok Bot, Muse, Hermes, OpenClaw or any assistant that takes an MCP server in Settings, Remote access. It hands Ghost a goal; Ghost runs it on its own branch with your agents, has another agent review it, and tells your assistant when it is done. It can leave work for the night, and Ghost's skill teaches it the rest: onghost.dev/skill.md.",
  },
  {
    question: "Does Ghost see my agent logins?",
    answer:
      "No. Ghost never reads, stores or proxies agent credentials. Each agent signs in through its own flow and keeps its own keys.",
  },
  {
    question: "Does it work offline?",
    answer:
      "Yes. Projects, chats and sessions are stored on your computer and the app needs no connection. Your agents may still need one for their own services.",
  },
  {
    question: "Does Ghost sync or upload my work?",
    answer:
      "No. Chats, projects and settings stay on your computer. There is no Ghost account, so nothing is uploaded by one. Your agents still send what they need to their own services.",
  },
  {
    question: "Which systems are supported?",
    answer:
      "Windows 10 or newer on x64 and ARM64, macOS 13.3 or newer on Apple silicon and Intel, and Linux on x64: Ubuntu 22.04, Debian 12, Fedora 36 or newer.",
  },
];

/** How long an answer takes to close; it opens over the same 300ms. */
const CLOSE = 300;

/**
 * Questions as rows between hairlines, as dot.com sets its FAQs: the heading and a quiet link at
 * left, the questions at right with the first answer open, one open at a time; the plus turns into
 * a cross as its answer slides open. Built on <details>, so every answer opens without JavaScript;
 * the script adds the eased open and close and keeps one row open.
 */
export function Faq() {
  const list = useRef<HTMLDivElement>(null);
  const reduced = useReducedMotion();

  useEffect(() => {
    const root = list.current;
    if (!root) return;
    const items = Array.from(root.querySelectorAll<HTMLDetailsElement>("details.faq-item"));
    const timers = new Set<number>();

    const open = (item: HTMLDetailsElement) => {
      item.dataset.state = "opening";
      item.open = true;
      void item.offsetHeight;
      requestAnimationFrame(() => {
        if (item.dataset.state === "opening") item.dataset.state = "open";
      });
    };
    const close = (item: HTMLDetailsElement) => {
      item.dataset.state = "closing";
      const finish = () => {
        if (item.dataset.state !== "closing") return;
        item.open = false;
        delete item.dataset.state;
      };
      if (reduced) return finish();
      const id = window.setTimeout(() => {
        timers.delete(id);
        finish();
      }, CLOSE);
      timers.add(id);
    };

    const handlers = items.map((item) => {
      const summary = item.querySelector("summary");
      const onClick = (event: Event) => {
        event.preventDefault();
        const showing = item.open && item.dataset.state !== "closing";
        for (const other of items) if (other !== item && other.open) close(other);
        if (showing) close(item);
        else open(item);
      };
      // Find in page can open a row natively; keep one open.
      const onToggle = () => {
        if (!item.open || item.dataset.state) return;
        for (const other of items) if (other !== item && other.open) close(other);
        item.dataset.state = "open";
      };
      summary?.addEventListener("click", onClick);
      item.addEventListener("toggle", onToggle);
      return () => {
        summary?.removeEventListener("click", onClick);
        item.removeEventListener("toggle", onToggle);
      };
    });

    return () => {
      for (const off of handlers) off();
      for (const id of timers) window.clearTimeout(id);
    };
  }, [reduced]);

  return (
    <section className="section faq" id="faq" aria-labelledby="faq-title">
      <div className="wrap two-col">
        <div className="two-col-head faq-head">
          <h2 id="faq-title" className="title">
            Questions. <span className="tone">Answered plainly.</span>
          </h2>
          <a className="text-link faq-link" href="/privacy">
            Privacy notice <Arrow />
          </a>
        </div>
        <div className="faq-list" ref={list}>
          {QUESTIONS.map((item, index) => (
            // The first answer stands open.
            <details className="faq-item" key={item.question} open={index === 0}>
              <summary>
                <span className="faq-question">{item.question}</span>
                <span className="faq-icon" aria-hidden="true">
                  <span />
                  <span />
                </span>
              </summary>
              <div className="faq-body">
                <div className="faq-answer">
                  <p>{item.answer}</p>
                  {/* The answer names every agent; the marks beside it are decorative. */}
                  {item.agents ? (
                    <span className="faq-agents" aria-hidden="true">
                      {AGENTS.map((agent) => (
                        <SpriteMark key={agent} agent={agent} size={22} />
                      ))}
                    </span>
                  ) : null}
                </div>
              </div>
            </details>
          ))}
        </div>
      </div>
    </section>
  );
}
