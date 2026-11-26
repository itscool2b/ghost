import { type Ref, useEffect, useRef, useSyncExternalStore } from "react";
import { createRoot } from "react-dom/client";
import {
  type QuitQuestion,
  QuitQuestions,
  quitQuestionsAvailable,
  tauriQuitChannel,
} from "../lib/quitQuestion";
import { Modal } from "./chat/Modal";

/**
 * The quit question in the window's own Graphite dialog (6.19), dark like the rest of the app:
 * "Quit Ghost?", what quitting stops, then Keep Ghost open (focused, so Enter keeps it) and
 * the red explicit choice. Esc, the scrim and the close button keep Ghost open too.
 */
export function QuitQuestionDialog({
  question,
  onAnswer,
  onShown,
}: {
  readonly question: QuitQuestion;
  readonly onAnswer: (quit: boolean) => void;
  readonly onShown?: () => void;
}) {
  const keep = useRef<HTMLButtonElement>(null);
  const shown = useRef(onShown);
  shown.current = onShown;
  useEffect(() => {
    // Once, after Modal's own effect opened the dialog: Keep open takes the focus, then Rust learns
    // the question is on screen.
    keep.current?.focus();
    shown.current?.();
  }, []);
  return (
    <Modal
      title="Quit Ghost?"
      onClose={() => onAnswer(false)}
      closeLabel={question.keep}
      className="quit-question"
    >
      <QuitQuestionContents question={question} onAnswer={onAnswer} keepRef={keep} />
    </Modal>
  );
}

/** The dialog's facts, one paragraph each, then Keep open (first) and the red explicit choice. */
export function QuitQuestionContents({
  question,
  onAnswer,
  keepRef,
}: {
  readonly question: QuitQuestion;
  readonly onAnswer: (quit: boolean) => void;
  readonly keepRef?: Ref<HTMLButtonElement>;
}) {
  const paragraphs = question.message.split(/\n\s*\n/).filter((part) => part.trim() !== "");
  return (
    <>
      <div className="quit-question-body" data-testid="quit-question">
        {paragraphs.map((paragraph) => (
          <p key={paragraph}>{paragraph}</p>
        ))}
      </div>
      <div className="g-dialog-actions">
        <button
          ref={keepRef}
          type="button"
          className="g-button"
          data-testid="quit-keep"
          onClick={() => onAnswer(false)}
        >
          {question.keep}
        </button>
        <button
          type="button"
          className="g-button g-button-danger"
          data-testid="quit-confirm"
          onClick={() => onAnswer(true)}
        >
          {question.quit}
        </button>
      </div>
    </>
  );
}

/** Shows Rust's quit question while one is open. */
export function QuitQuestionHost({ questions }: { readonly questions: QuitQuestions }) {
  const question = useSyncExternalStore(
    questions.subscribe,
    questions.getSnapshot,
    questions.getSnapshot,
  );
  if (question === null) return null;
  return (
    <QuitQuestionDialog
      key={question.token}
      question={question}
      onShown={() => questions.shown(question.token)}
      onAnswer={(quit) => questions.answer(question.token, quit)}
    />
  );
}

/**
 * Listens for the quit question in its own React root, outside the app's tree, so a screen that
 * failed to render still lets the window answer.
 */
export function installQuitQuestion(): void {
  if (!quitQuestionsAvailable()) return;
  const questions = new QuitQuestions(tauriQuitChannel);
  questions.start();
  const node = document.createElement("div");
  node.className = "quit-question-root";
  document.body.append(node);
  createRoot(node).render(<QuitQuestionHost questions={questions} />);
}
