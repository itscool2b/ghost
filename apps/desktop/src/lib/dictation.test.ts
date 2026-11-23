import { describe, expect, it, vi } from "vitest";
import type { DictationEvent, LiveText } from "../bindings";
import {
  type DictationApi,
  DictationController,
  dictationBusy,
  dictationFrame,
  joinLive,
  largerDetail,
  liveWords,
  MISSING_MESSAGE,
  offersRetry,
  placeDictation,
  recordingTime,
  WAVE_SAMPLES,
  waveLevel,
} from "./dictation";

function fixture(installed = true, supported = true) {
  let receive: (event: DictationEvent) => void = () => {};
  const api: DictationApi = {
    model: vi.fn(async () => ({
      installed,
      name: "Parakeet TDT-CTC 110M",
      bytes: 113552320,
      larger: {
        name: "Parakeet Ultra",
        bytes: 486280096,
        state: "absent" as const,
        available: true,
        received: 0,
        automatic: false,
      },
      supported,
    })),
    start: vi.fn(async (_id, event) => {
      receive = event;
    }),
    stop: vi.fn(async () => {}),
    cancel: vi.fn(async () => {}),
  };
  const insert = vi.fn();
  const controller = new DictationController(api, insert);
  return { api, insert, controller, event: (event: DictationEvent) => receive(event) };
}

const live = (settled: string, stable = "", tentative = ""): LiveText => ({
  settled,
  stable,
  tentative,
});
const listening = (text: LiveText, seconds = 1, level = 0.5, speaking = true): DictationEvent => ({
  state: "listening",
  seconds,
  level,
  speaking,
  text,
});

describe("local dictation", () => {
  it("starts listening at once when the bundled models are present", async () => {
    const f = fixture();
    await f.controller.open();
    expect(f.api.start).toHaveBeenCalledOnce();
    expect(f.controller.getSnapshot().state).toBe("starting");
    f.event({ state: "preparing" });
    f.event(listening(live("", "", "Add")));
    expect(f.controller.getSnapshot()).toMatchObject({
      state: "listening",
      seconds: 1,
      speaking: true,
      text: live("", "", "Add"),
    });
    expect(dictationBusy(f.controller.getSnapshot())).toBe(true);
  });
  it("streams live text and keeps a bounded waveform with each sample's identity", async () => {
    const f = fixture();
    await f.controller.open();
    for (let tick = 0; tick < WAVE_SAMPLES + 25; tick++)
      f.event(listening(live("Add a test."), tick, tick % 2 ? 0.6 : 0, tick % 2 === 1));
    const state = f.controller.getSnapshot();
    if (state.state !== "listening") throw new Error(state.state);
    expect(state.wave).toHaveLength(WAVE_SAMPLES);
    expect(state.tick).toBe(WAVE_SAMPLES + 24);
    expect(state.wave.at(-1)).toEqual({ level: waveLevel(0), voiced: false });
    expect(state.wave.at(-2)).toEqual({ level: waveLevel(0.6), voiced: true });
    expect(f.insert).not.toHaveBeenCalled();
  });
  it("finishes into one insertion of the final text, never a send", async () => {
    const f = fixture();
    await f.controller.open();
    f.event(listening(live("as not what")));
    await f.controller.stop();
    expect(f.api.stop).toHaveBeenCalledOnce();
    // Finishing shows at once, before the engine says so, and its last report only updates words.
    expect(f.controller.getSnapshot()).toMatchObject({ state: "finishing" });
    f.event(listening(live("ask not what")));
    expect(f.controller.getSnapshot()).toMatchObject({
      state: "finishing",
      text: live("ask not what"),
    });
    f.event({ state: "finishing", text: live("ask not what") });
    f.event({ state: "complete", text: "  Ask not what.  " });
    f.event({ state: "complete", text: "duplicate" });
    expect(f.insert).toHaveBeenCalledExactlyOnceWith("Ask not what.");
    expect(f.controller.getSnapshot().state).toBe("idle");
  });
  it("only finishes while listening", async () => {
    const f = fixture();
    await f.controller.stop();
    await f.controller.open();
    await f.controller.stop();
    expect(f.api.stop).not.toHaveBeenCalled();
  });
  it("cancels with the words shown, and discards late text", async () => {
    const f = fixture();
    await f.controller.open();
    f.event(listening(live("Keep", "this", "out")));
    await f.controller.cancel();
    expect(f.controller.getSnapshot()).toEqual({
      state: "cancelling",
      text: live("Keep", "this", "out"),
    });
    f.event(listening(live("late")));
    expect(f.controller.getSnapshot().state).toBe("cancelling");
    f.event({ state: "complete", text: "cancelled" });
    expect(f.insert).not.toHaveBeenCalled();
    expect(f.controller.getSnapshot().state).toBe("idle");
  });
  it("discards late text after unmount and a StrictMode remount", async () => {
    const f = fixture();
    await f.controller.open();
    f.controller.dispose();
    f.controller.activate();
    f.event({ state: "complete", text: "old chat" });
    expect(f.insert).not.toHaveBeenCalled();
    await f.controller.open();
    f.event({ state: "complete", text: "current" });
    expect(f.insert).toHaveBeenCalledExactlyOnceWith("current");
  });
  it("cancels again if unmounted before the start command is admitted", async () => {
    const f = fixture();
    let admit: () => void = () => {};
    f.api.start = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          admit = resolve;
        }),
    );
    const opening = f.controller.open();
    await vi.waitFor(() => expect(f.api.start).toHaveBeenCalled());
    f.controller.dispose();
    admit();
    await opening;
    expect(f.api.cancel).toHaveBeenCalledTimes(2);
    expect(f.insert).not.toHaveBeenCalled();
  });
  it.each([false, true])(
    "discards late completion when cancellation rejects (stop failure: %s)",
    async (stopFails) => {
      const f = fixture();
      f.api.cancel = vi.fn(async () => {
        throw new Error("Channel unavailable");
      });
      await f.controller.open();
      f.event(listening(live("words")));
      if (stopFails) {
        f.api.stop = vi.fn(async () => {
          throw new Error("Stop failed");
        });
        await f.controller.stop();
      } else await f.controller.cancel();
      expect(f.controller.getSnapshot()).toMatchObject({
        state: "cancelling",
        message: "Could not stop dictation. Channel unavailable",
      });
      expect(dictationBusy(f.controller.getSnapshot())).toBe(true);
      f.event(listening(live("more")));
      expect(f.controller.getSnapshot().state).toBe("cancelling");
      f.event({ state: "complete", text: "Must be discarded" });
      expect(f.insert).not.toHaveBeenCalled();
      expect(f.controller.getSnapshot().state).toBe("idle");
    },
  );
  it("keeps the words heard before the microphone failed", async () => {
    const f = fixture();
    await f.controller.open();
    f.event(listening(live("Add a test", "for the", "bar")));
    f.event({ state: "failed", message: "The microphone stopped.", reason: "microphone" });
    expect(f.insert).toHaveBeenCalledExactlyOnceWith("Add a test for the bar");
    expect(f.controller.getSnapshot()).toEqual({
      state: "failed",
      message: "The microphone stopped.",
      reason: "microphone",
      kept: true,
    });
    f.controller.dismiss();
    expect(f.controller.getSnapshot().state).toBe("idle");
  });
  it("shows a failure with nothing heard, and retries", async () => {
    const f = fixture();
    await f.controller.open();
    f.event({ state: "failed", message: "No speech was detected.", reason: "no_speech" });
    expect(f.insert).not.toHaveBeenCalled();
    expect(f.controller.getSnapshot()).toMatchObject({ state: "failed", kept: false });
    await f.controller.open();
    expect(f.api.start).toHaveBeenCalledTimes(2);
  });
  it("says a build without its models needs reinstalling, and never records", async () => {
    const f = fixture(false);
    await f.controller.open();
    expect(f.controller.getSnapshot()).toEqual({
      state: "failed",
      message: MISSING_MESSAGE,
      reason: "model",
      kept: false,
    });
    expect(f.api.start).not.toHaveBeenCalled();
  });
  it("never starts on a processor the engine cannot run on, nor offers a retry", async () => {
    const f = fixture(true, false);
    await f.controller.open();
    const state = f.controller.getSnapshot();
    expect(state).toMatchObject({ state: "failed", reason: "unsupported" });
    expect(offersRetry(state)).toBe(false);
    expect(f.api.start).not.toHaveBeenCalled();
  });
  it("offers a retry for every other failure", () => {
    for (const reason of ["model", "microphone", "no_speech", "other"] as const)
      expect(offersRetry({ state: "failed", message: "x", reason, kept: false })).toBe(true);
    expect(offersRetry({ state: "idle" })).toBe(false);
  });
  it("does not start a recording after dismissing a pending model check", async () => {
    const f = fixture();
    let ready: (value: Awaited<ReturnType<DictationApi["model"]>>) => void = () => {};
    f.api.model = () =>
      new Promise((resolve) => {
        ready = resolve;
      });
    const opening = f.controller.open();
    await f.controller.cancel();
    ready({
      installed: true,
      name: "Parakeet",
      bytes: 1,
      supported: true,
      larger: {
        name: "Larger",
        bytes: 2,
        state: "absent",
        available: true,
        received: 0,
        automatic: false,
      },
    });
    await opening;
    expect(f.api.start).not.toHaveBeenCalled();
    expect(f.controller.getSnapshot().state).toBe("idle");
  });
});

describe("live text", () => {
  it("reads settled, agreed and forming words in order", () => {
    expect(liveWords(live("Ask not", " what your ", "country"))).toEqual([
      { text: "Ask", kind: "settled" },
      { text: "not", kind: "settled" },
      { text: "what", kind: "stable" },
      { text: "your", kind: "stable" },
      { text: "country", kind: "tentative" },
    ]);
    expect(joinLive(live("Ask not", "", "what"))).toBe("Ask not what");
    expect(joinLive(live("", "", ""))).toBe("");
  });
  it("draws room noise flat and speech tall, and times recordings without a limit", () => {
    expect(waveLevel(null)).toBe(0);
    expect(waveLevel(Number.NaN)).toBe(0);
    expect(waveLevel(0.02)).toBe(0);
    expect(waveLevel(1)).toBe(1);
    expect(waveLevel(0.3)).toBeGreaterThan(0.5);
    expect(recordingTime(0)).toBe("0:00");
    expect(recordingTime(65.9)).toBe("1:05");
    expect(recordingTime(3725)).toBe("62:05");
  });
});

describe("dictation draft insertion", () => {
  it("inserts at the captured selection with readable spacing", () => {
    expect(placeDictation("Make this work", "the test", "Make this work", 5, 9)).toEqual({
      text: "Make the test work",
      start: 5,
      end: 13,
    });
    expect(placeDictation("Make it", "now", "Make it", 7, 7)).toEqual({
      text: "Make it now",
      start: 8,
      end: 11,
    });
    expect(placeDictation("", " Hello ", "", 0, 0)).toEqual({ text: "Hello", start: 0, end: 5 });
  });
  it("preserves a draft that changed since dictation began, and ignores empty results", () => {
    expect(placeDictation("New draft", "Add tests", "Old draft", 0, 9).text).toBe(
      "New draft Add tests",
    );
    expect(placeDictation("Keep me", "   ", "Keep me", 0, 7).text).toBe("Keep me");
    expect(dictationFrame("New draft", "Old draft", 0, 3)).toEqual({
      before: "New draft",
      after: "",
    });
    expect(dictationFrame("Old draft", "Old draft", 4, 4)).toEqual({
      before: "Old ",
      after: "draft",
    });
  });
});

describe("the larger model's row", () => {
  const model = (
    state: "absent" | "downloading" | "ready",
    more: { received?: number; automatic?: boolean } = {},
  ) => ({
    installed: true,
    name: state === "ready" ? "Parakeet Ultra" : "Parakeet TDT-CTC 110M",
    bytes: 113552320,
    supported: true,
    larger: {
      name: "Parakeet Ultra",
      bytes: 486280096,
      state,
      available: true,
      received: 0,
      automatic: false,
      ...more,
    },
  });

  it("says that o7 fetches it by itself, until the person has said no", () => {
    const coming = largerDetail(model("absent", { automatic: true }), { state: "idle" });
    expect(coming).toMatch(/^Parakeet Ultra makes about 40% fewer mistakes/);
    expect(coming).toContain("o7 downloads it by itself, once: 486 MB from Hugging Face.");
    // After a no the row offers it and promises nothing.
    const offer = largerDetail(model("absent"), { state: "idle" });
    expect(offer).not.toContain("by itself");
    expect(largerDetail(model("absent"), { state: "idle", kept: true })).not.toContain("by itself");
  });

  it("says what it is, what it costs and where it comes from before anything is fetched", () => {
    const offer = largerDetail(model("absent"), { state: "idle" });
    expect(offer).toMatch(/^Parakeet Ultra makes about 40% fewer mistakes/);
    expect(offer).toContain("A one-time 486 MB download from Hugging Face");
    expect(offer).not.toContain("kept");
    expect(largerDetail(model("absent"), { state: "idle", kept: true })).toMatch(
      /What arrived so far is kept\.$/,
    );
  });

  it("says how far the download has come, then that it is being checked", () => {
    expect(
      largerDetail(model("absent"), { state: "downloading", received: 0, total: 486280096 }),
    ).toBe("Downloading Parakeet Ultra: 0 of 486 MB.");
    expect(
      largerDetail(model("downloading"), {
        state: "downloading",
        received: 153_613_261,
        total: 486280096,
      }),
    ).toBe("Downloading Parakeet Ultra: 154 of 486 MB.");
    expect(largerDetail(model("downloading"), { state: "verifying" })).toBe(
      "Checking the download.",
    );
    // A download this page did not start, o7's own included: the core says how far it has come.
    expect(largerDetail(model("downloading", { received: 243_140_048 }), { state: "idle" })).toBe(
      "Downloading Parakeet Ultra: 243 of 486 MB.",
    );
  });

  it("says it is in use once it is here, and what removing it does", () => {
    const text = largerDetail(model("ready"), { state: "idle" });
    expect(text).toMatch(/^Parakeet Ultra is downloaded and in use\./);
    expect(text).toContain("frees 486 MB");
    expect(largerDetail(model("ready"), { state: "removing" })).toBe("Removing Parakeet Ultra.");
  });
});
