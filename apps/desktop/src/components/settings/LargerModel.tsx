import { useEffect, useRef, useState } from "react";
import type { DictationModel } from "../../bindings";
import { type LargerApi, type LargerPhase, largerDetail } from "../../lib/dictation";
import { errorMessage } from "../../lib/errors";
import { Row } from "./SettingsRow";

/**
 * Settings > Voice > Larger model (docs/local-dictation.md, "The larger model"): Parakeet Ultra,
 * fetched once. It is too large to come inside the installer, so the built-in model is what
 * dictation starts with, and Ghost fetches the better one by itself shortly after it starts. This row
 * says so and is where a person says no: Cancel while it is on its way (before it starts, too),
 * Remove once it is in use. After a no it is theirs to ask for again, with Download.
 *
 * `onChanged` is told when the model in use may have changed, so the page reads its status again.
 */
export function LargerModel({
  api,
  model,
  onChanged,
}: {
  readonly api: LargerApi;
  readonly model: DictationModel;
  readonly onChanged: () => void;
}) {
  const [phase, setPhase] = useState<LargerPhase>({ state: "idle" });
  const live = useRef(true);
  useEffect(() => {
    live.current = true;
    return () => {
      live.current = false;
    };
  }, []);
  // A download this page did not start (Ghost's own, or the page was reloaded while one ran) reports
  // to nobody here. This page looks again until it has ended, and while Ghost is about to start one.
  const elsewhere = model.larger.state === "downloading" && phase.state === "idle";
  const coming =
    model.larger.state === "absent" && model.larger.automatic && phase.state === "idle";
  const watching = elsewhere || coming;
  useEffect(() => {
    if (!watching) return;
    const timer = setInterval(onChanged, 1000);
    return () => clearInterval(timer);
  }, [watching, onChanged]);

  const download = async () => {
    setPhase({ state: "downloading", received: 0, total: model.larger.bytes });
    try {
      await api.download((event) => {
        if (!live.current) return;
        switch (event.state) {
          case "downloading":
            setPhase({ state: "downloading", received: event.received, total: event.total });
            break;
          case "verifying":
            setPhase({ state: "verifying" });
            break;
          case "ready":
            setPhase({ state: "idle" });
            onChanged();
            break;
          case "cancelled":
            setPhase({ state: "idle", kept: true });
            onChanged();
            break;
          case "failed":
            setPhase({ state: "failed", message: event.message });
            onChanged();
            break;
        }
      });
    } catch (reason) {
      if (live.current) setPhase({ state: "failed", message: errorMessage(reason) });
      // It may have failed because Ghost had just started the download itself.
      onChanged();
    }
  };
  // Cancel is also a no to Ghost starting it by itself, so it is remembered whether or not a
  // download runs. One this page started ends on its own channel; any other is seen by looking.
  const cancel = async () => {
    const mine = phase.state === "downloading";
    try {
      await api.cancel();
      if (live.current && !mine) setPhase({ state: "idle", kept: elsewhere });
    } catch (reason) {
      if (live.current && !mine) setPhase({ state: "failed", message: errorMessage(reason) });
    }
    onChanged();
  };
  const remove = async () => {
    setPhase({ state: "removing" });
    try {
      await api.remove();
      if (live.current) setPhase({ state: "idle" });
    } catch (reason) {
      if (live.current) setPhase({ state: "failed", message: errorMessage(reason) });
    }
    onChanged();
  };

  const ready = model.larger.state === "ready";
  const busy = phase.state === "downloading" || phase.state === "verifying" || watching;
  return (
    <>
      <Row title="Larger model" detail={largerDetail(model, phase)}>
        <span className="settings-control" data-testid="larger-model" data-state={phase.state}>
          {busy ? (
            <button
              type="button"
              className="g-button g-button-outline g-button-28"
              // Verifying is past the point of stopping. A download this page did not start is
              // cancelled as any other, and so is one Ghost has yet to start.
              disabled={phase.state === "verifying"}
              onClick={() => void cancel()}
            >
              Cancel
            </button>
          ) : ready ? (
            <button
              type="button"
              className="g-button g-button-outline g-button-28"
              disabled={phase.state === "removing"}
              onClick={() => void remove()}
            >
              Remove
            </button>
          ) : (
            <button
              type="button"
              className="g-button g-button-outline g-button-28"
              onClick={() => void download()}
            >
              {phase.state === "idle" && phase.kept ? "Continue" : "Download"}
            </button>
          )}
          <span className="sr-only" aria-live="polite">
            {phase.state === "downloading" && phase.received === 0
              ? `Downloading ${model.larger.name}`
              : phase.state === "verifying"
                ? "Checking the download"
                : ready && phase.state === "idle"
                  ? `${model.larger.name} is in use`
                  : ""}
          </span>
        </span>
      </Row>
      {phase.state === "failed" ? (
        <p className="settings-error" role="alert" data-testid="larger-model-error">
          {phase.message}
        </p>
      ) : null}
    </>
  );
}
