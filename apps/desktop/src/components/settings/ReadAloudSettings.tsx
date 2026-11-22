import { Square, Volume2 } from "lucide-react";
import { useEffect, useState, useSyncExternalStore } from "react";
import type { SpeechStatus } from "../../bindings";
import {
  chosenVoice,
  type SpeechApi,
  type SpeechPlayer,
  speechDetail,
  speechPlayer,
  VOICE_SAMPLE,
  voiceGroups,
} from "../../lib/speech";
import { Select } from "../Select";
import type { Preferences } from "./preferences";
import { Row } from "./SettingsRow";

/** The key the Voice page's sample plays under, so no answer's button shows as speaking. */
const SAMPLE = "settings:voice-sample";

/**
 * Settings > Voice > Read aloud (docs/read-aloud.md): the built-in voice model, the voice answers
 * are read in, and a sample of it. A choice is saved at once, like every other setting.
 */
export function ReadAloudSettings({
  api,
  prefs,
  player = speechPlayer,
}: {
  readonly api: SpeechApi;
  readonly prefs: Preferences;
  readonly player?: SpeechPlayer;
}) {
  const [status, setStatus] = useState<SpeechStatus | null | "failed">(null);
  const playing = useSyncExternalStore(player.subscribe, player.getSnapshot, player.getSnapshot);
  useEffect(() => {
    let live = true;
    api.status().then(
      (value) => {
        if (live) setStatus(value);
      },
      () => {
        if (live) setStatus("failed");
      },
    );
    return () => {
      live = false;
    };
  }, [api]);
  // Leaving the page ends its sample.
  useEffect(
    () => () => {
      if (player.getSnapshot().key === SAMPLE) player.stop();
    },
    [player],
  );
  const ready = status !== null && status !== "failed" && status.installed && status.supported;
  const voices = ready ? status.voices : [];
  const saved = prefs.shown?.general.speech_voice ?? null;
  const voice = chosenVoice(voices, saved);
  const sampling = playing.key === SAMPLE && playing.phase !== "idle";
  return (
    <div className="settings-group" data-testid="read-aloud-settings">
      <Row title="Read aloud" detail={speechDetail(status)} />
      {ready && voice ? (
        <Row title="Voice" detail={voice.description}>
          <span className="settings-control settings-voice">
            <Select
              aria-label="Voice"
              className="g-select settings-select"
              value={voice.id}
              onChange={(event) => {
                const next = event.currentTarget.value;
                player.setVoice(next);
                prefs.change({ type: "speech_voice", voice: next });
                // A choice is heard: the sample plays in the voice just picked.
                void player.speak(SAMPLE, VOICE_SAMPLE, next);
              }}
            >
              {voiceGroups(voices).map((group) => (
                <optgroup key={group.label} label={group.label}>
                  {group.voices.map((entry) => (
                    <option key={entry.id} value={entry.id}>
                      {entry.name}
                    </option>
                  ))}
                </optgroup>
              ))}
            </Select>
            <button
              type="button"
              className="g-button g-button-outline g-button-28"
              data-testid="voice-sample"
              aria-pressed={sampling}
              onClick={() =>
                sampling ? player.stop() : void player.speak(SAMPLE, VOICE_SAMPLE, voice.id)
              }
            >
              {sampling ? (
                <Square size={12} aria-hidden="true" />
              ) : (
                <Volume2 size={14} aria-hidden="true" />
              )}
              {sampling ? "Stop" : "Hear it"}
            </button>
          </span>
        </Row>
      ) : null}
      {playing.failedKey === SAMPLE && playing.error ? (
        <p className="settings-error" role="alert">
          {playing.error}
        </p>
      ) : null}
    </div>
  );
}
