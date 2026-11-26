/** The quit question over a preview (`&quit`): synthetic, so no native shell is asked anything. */
import { createRoot } from "react-dom/client";
import { QuitQuestionHost } from "../components/QuitQuestionDialog";
import { type QuitQuestion, QuitQuestions } from "../lib/quitQuestion";

const QUESTION: QuitQuestion = {
  token: "preview",
  message: "One chat is still active. Quitting stops its agent.",
  quit: "Stop agents and quit",
  keep: "Keep Ghost open",
};

export function mountQuitPreview(): void {
  const calls: string[] = [];
  Object.assign(window, { quitPreview: { calls: () => [...calls] } });
  const questions = new QuitQuestions({
    listen: (ask) => {
      const timer = setTimeout(() => ask(QUESTION), 0);
      return () => clearTimeout(timer);
    },
    shown: (token) => calls.push(`shown:${token}`),
    answer: (token, quit) => calls.push(`answer:${token}:${quit}`),
  });
  questions.start();
  const node = document.createElement("div");
  document.body.append(node);
  createRoot(node).render(<QuitQuestionHost questions={questions} />);
}
