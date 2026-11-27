import { useEffect, useRef, useState } from "react";
import type { DictationModel } from "../../bindings";
import { type LargerApi, type LargerPhase, largerDetail } from "../../lib/dictation";
import { errorMessage } from "../../lib/errors";
import { Row, Switch } from "./SettingsRow";

/**
 * Settings > Voice > Larger model (docs/local-dictation.md, "The larger model"): Parakeet Ultra,
 * fetched once when the person asks. The built-in model is dictation's default (owner, 27 November
 * 2026: "make the smaller model the default ... they can use the bigger model if they want"), so
 * Ghost never fetches this one by itself. Download fetches it and chooses it; once it is here, the
 * switch picks between it and the built-in model, and Remove deletes it.
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
  const [choosing, setChoosing] = useState(false);
  const live = useRef(true);
  useEffect(() => {
    live.current = true;
    return () => {
      live.current = false;
    };
  }, []);
  // A download this page did not start (the page was reloaded while one ran) reports to nobody
  // here. This page looks again until it has ended.
  const elsewhere = model.larger.state === "downloading" && phase.state === "idle";
  useEffect(() => {
    if (!elsewhere) return;
    const timer = setInterval(onChanged, 1000);
    return () => clearInterval(timer);
  }, [elsewhere, onChanged]);

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
      onChanged();
    }
  };
  // One this page started ends on its own channel; any other is seen by looking.
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
  const choose = async (larger: boolean) => {
    setChoosing(true);
    try {
      await api.use(larger);
      if (live.current && phase.state === "failed") setPhase({ state: "idle" });
    } catch (reason) {
      if (live.current) setPhase({ state: "failed", message: errorMessage(reason) });
    }
    if (live.current) setChoosing(false);
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
  const inUse = ready && model.larger.chosen;
  const busy = phase.state === "downloading" || phase.state === "verifying" || elsewhere;
  return (
    <>
      <Row title="Larger model" detail={largerDetail(model, phase)}>
        <span
          className="settings-control settings-larger"
          data-testid="larger-model"
          data-state={phase.state}
        >
          {busy ? (
            <button
              type="button"
              className="g-button g-button-outline g-button-28"
              // Verifying is past the point of stopping.
              disabled={phase.state === "verifying"}
              onClick={() => void cancel()}
            >
              Cancel
            </button>
          ) : ready ? (
            <>
              <button
                type="button"
                className="g-button g-button-ghost g-button-28"
                disabled={phase.state === "removing" || choosing}
                onClick={() => void remove()}
              >
                Remove
              </button>
              <Switch
                label={`Use ${model.larger.name}`}
                checked={inUse}
                disabled={phase.state === "removing" || choosing}
                testId="larger-model-use"
                onChange={(value) => void choose(value)}
              />
            </>
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
                : inUse && phase.state === "idle"
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
