import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { type QuitChannel, type QuitQuestion, QuitQuestions } from "../lib/quitQuestion";
import { QuitQuestionContents, QuitQuestionHost } from "./QuitQuestionDialog";

const QUESTION: QuitQuestion = {
  token: "t1",
  message:
    "One project terminal is open. Quitting stops its shell and owned child processes.\n\nOne chat is still active. Quitting stops its agent.",
  quit: "Stop work and quit",
  keep: "Keep Ghost open",
};

/** A channel that records what the window tells Rust. */
function fakeChannel() {
  const sent: string[] = [];
  let ask: (question: QuitQuestion) => void = () => {};
  const channel: QuitChannel = {
    listen: (handler) => {
      ask = handler;
      return () => {};
    },
    shown: (token) => sent.push(`shown:${token}`),
    answer: (token, quit) => sent.push(`answer:${token}:${quit}`),
  };
  return { channel, sent, ask: (question: QuitQuestion) => ask(question) };
}

describe("the quit question", () => {
  it("lists the facts, then Keep open first and the red explicit choice", () => {
    const html = renderToStaticMarkup(
      <QuitQuestionContents question={QUESTION} onAnswer={() => {}} />,
    );
    expect(html).toContain("<p>One project terminal is open.");
    expect(html).toContain("<p>One chat is still active. Quitting stops its agent.</p>");
    const keep = html.indexOf('data-testid="quit-keep"');
    const confirm = html.indexOf('data-testid="quit-confirm"');
    expect(keep).toBeGreaterThan(-1);
    expect(confirm).toBeGreaterThan(keep);
    expect(html).toMatch(
      /class="g-button g-button-danger" data-testid="quit-confirm">Stop work and quit</,
    );
    expect(html).toMatch(/class="g-button" data-testid="quit-keep">Keep Ghost open</);
  });

  it("tells Rust it shows the question, then the one choice, once", () => {
    const fake = fakeChannel();
    const questions = new QuitQuestions(fake.channel);
    questions.start();
    expect(questions.getSnapshot()).toBeNull();
    questions.answer("t1", true);
    expect(fake.sent).toEqual([]);
    fake.ask(QUESTION);
    expect(questions.getSnapshot()).toEqual(QUESTION);
    questions.shown("other");
    questions.shown("t1");
    questions.answer("other", true);
    questions.answer("t1", false);
    questions.answer("t1", true);
    expect(fake.sent).toEqual(["shown:t1", "answer:t1:false"]);
    expect(questions.getSnapshot()).toBeNull();
  });

  it("lets a newer question replace an older one", () => {
    const fake = fakeChannel();
    const questions = new QuitQuestions(fake.channel);
    questions.start();
    fake.ask(QUESTION);
    fake.ask({ ...QUESTION, token: "t2" });
    questions.answer("t1", true);
    questions.answer("t2", true);
    expect(fake.sent).toEqual(["answer:t2:true"]);
  });

  it("shows nothing while no question is open", () => {
    const questions = new QuitQuestions(fakeChannel().channel);
    expect(renderToStaticMarkup(<QuitQuestionHost questions={questions} />)).toBe("");
  });
});
