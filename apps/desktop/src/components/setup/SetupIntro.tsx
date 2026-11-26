import { Wing, Wordmark } from "@ghost/ui/graphite";
import "../tour/wing.css";

export type IntroMode = "pending" | "live" | "still";

/**
 * The first launch's opening: the wing flies in from below, nose first, and levels out over the
 * name. Later launches and reduced motion hold the formed mark. It keeps setup's timing and skip.
 */
export function SetupIntro({
  mode,
  leaving,
}: {
  readonly mode: IntroMode;
  readonly leaving: false | "slow" | "fast";
}) {
  return (
    <div
      className="setup-intro"
      data-mode={mode}
      data-cinematic={leaving ? undefined : ""}
      data-leaving={leaving || undefined}
      aria-hidden="true"
    >
      {mode === "pending" ? null : (
        <div className="setup-intro-stage">
          <span className="setup-intro-mark">
            <Wing />
          </span>
          <span className="setup-intro-word">
            <Wordmark size={32} mark={false} />
          </span>
        </div>
      )}
    </div>
  );
}
