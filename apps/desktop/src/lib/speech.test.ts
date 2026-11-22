import { describe, expect, it, vi } from "vitest";
import type { SpeechEvent, SpeechStatus, SpeechVoice } from "../bindings";
import {
  chosenVoice,
  clipSpoken,
  MISSING_MESSAGE,
  type SpeechApi,
  SpeechPlayer,
  speakable,
  speechDetail,
  UNSUPPORTED_MESSAGE,
  voiceGroups,
} from "./speech";

const voice = (id: string, locale: string): SpeechVoice => ({
  id,
  name: id,
  locale,
  gender: "female",
  description: `${id} speaking`,
});
const READY: SpeechStatus = {
  installed: true,
  supported: true,
  name: "Phonon 31M",
  voices: [voice("default", "en-us"), voice("brit", "en-gb"), voice("pirate", "characters")],
};

function fixture(status: SpeechStatus | Error = READY) {
  const events = new Map<string, (event: SpeechEvent) => void>();
  const api: SpeechApi = {
    status: vi.fn(async () => {
      if (status instanceof Error) throw status;
      return status;
    }),
    speak: vi.fn(async (id, _text, _voice, event) => {
      events.set(id, event);
    }),
    stop: vi.fn(async () => {}),
    prepare: vi.fn(async () => {}),
  };
  const player = new SpeechPlayer(api);
  const seen = vi.fn();
  player.subscribe(seen);
  const request = (index: number) => vi.mocked(api.speak).mock.calls[index]?.[0] as string;
  return {
    api,
    player,
    seen,
    request,
    emit: (index: number, event: SpeechEvent) => events.get(request(index))?.(event),
  };
}

const settled = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

describe("speakable", () => {
  it("says an answer's words without its marks", () => {
    expect(
      speakable(
        "## What changed\n\nThe **total** now rounds _once_, in `cart.ts`.\n\n- Three tests added\n- [The receipt page](https://example.test/r) updated\n\n> Nothing else moved.",
      ),
    ).toBe(
      "What changed.\nThe total now rounds once, in cart.ts.\nThree tests added.\nThe receipt page updated.\nNothing else moved.",
    );
  });

  it("names a code block in its place and never reads its lines", () => {
    const said = speakable(
      "Run this:\n\n```sh\nrm -rf build\n```\n\nThen ~~~ is not a fence.\n\n~~~\nsecret = 1\n~~~\nDone.",
    );
    expect(said).toBe("Run this:\nCode block.\nThen ~~~ is not a fence.\nCode block.\nDone.");
    expect(said).not.toContain("rm -rf");
    expect(said).not.toContain("secret");
  });

  it("does not read the rest of an answer as code when a fence is never closed", () => {
    expect(speakable("Before.\n\n```ts\nconst a = 1;")).toBe("Before.\nCode block.");
  });

  it("says link for a bare address and reads a table a row at a time", () => {
    expect(speakable("See https://example.test/a/b?c=1 and <https://example.test>.")).toBe(
      "See link and link.",
    );
    expect(speakable("| File | Lines |\n| --- | ---: |\n| cart.ts | 12 |")).toBe(
      "File, Lines.\ncart.ts, 12.",
    );
  });

  it("keeps a sentence's own ending and says nothing for marks alone", () => {
    expect(speakable("1. First, do this:\n2) Is it done?\n\n---\n\n***\n\n- [x] Checked")).toBe(
      "First, do this:\nIs it done?\nChecked.",
    );
    expect(speakable("   \n\n---\n")).toBe("");
  });

  it("leaves underscores and stars that are not emphasis", () => {
    expect(speakable("Set MAX_SPOKEN_BYTES to 2 * 3 * 4.")).toBe(
      "Set MAX_SPOKEN_BYTES to 2 * 3 * 4.",
    );
  });
});

describe("clipSpoken", () => {
  it("passes what fits and cuts the rest at a sentence's end", () => {
    expect(clipSpoken("Short.")).toEqual({ text: "Short.", clipped: false });
    const text = `${"One sentence here. ".repeat(10)}And a last one that is cut`;
    const cut = clipSpoken(text, 150);
    expect(cut.clipped).toBe(true);
    expect(cut.text.endsWith("here.")).toBe(true);
    expect(new TextEncoder().encode(cut.text).length).toBeLessThanOrEqual(150);
  });

  it("counts bytes, and never cuts inside a character", () => {
    const cut = clipSpoken("é".repeat(100), 51);
    expect(cut.clipped).toBe(true);
    expect(cut.text).toBe("é".repeat(25));
  });
});

describe("the Voice page's words", () => {
  it("says why reading aloud cannot run", () => {
    expect(speechDetail(null)).toMatch(/Checking/);
    expect(speechDetail("failed")).toMatch(/could not read/);
    expect(speechDetail({ ...READY, supported: false })).toBe(UNSUPPORTED_MESSAGE);
    expect(speechDetail({ ...READY, installed: false })).toBe(MISSING_MESSAGE);
    expect(speechDetail(READY)).toMatch(/^Phonon 31M, built in/);
  });

  it("groups the voices and falls back to the model's own for a voice this build lacks", () => {
    expect(voiceGroups(READY.voices).map((group) => [group.label, group.voices.length])).toEqual([
      ["American", 1],
      ["British", 1],
      ["Characters", 1],
    ]);
    expect(voiceGroups([voice("x", "en-au")])[0]?.label).toBe("Other");
    expect(chosenVoice(READY.voices, "brit")?.id).toBe("brit");
    expect(chosenVoice(READY.voices, "gone")?.id).toBe("default");
    expect(chosenVoice(READY.voices, null)?.id).toBe("default");
    expect(chosenVoice([], "brit")).toBeNull();
  });
});

describe("SpeechPlayer", () => {
  it("asks once whether reading aloud can run, and is unavailable when the answer fails", async () => {
    const ready = fixture();
    expect(ready.player.getSnapshot().available).toBeNull();
    ready.player.subscribe(() => {});
    await settled();
    expect(ready.api.status).toHaveBeenCalledTimes(1);
    expect(ready.player.getSnapshot().available).toBe(true);
    for (const status of [
      { ...READY, installed: false },
      { ...READY, supported: false },
      new Error("no core"),
    ]) {
      const absent = fixture(status);
      await settled();
      expect(absent.player.getSnapshot().available).toBe(false);
    }
  });

  it("reads an answer's words in the saved voice and follows the engine's events", async () => {
    const { api, player, emit } = fixture();
    await settled();
    player.setVoice("brit");
    player.toggle("turn-1", "**Done.** See `a.ts`.");
    expect(player.getSnapshot()).toMatchObject({ key: "turn-1", phase: "preparing" });
    await settled();
    expect(api.speak).toHaveBeenCalledWith(
      expect.any(String),
      "Done. See a.ts.",
      "brit",
      expect.any(Function),
    );
    emit(0, { state: "speaking" });
    expect(player.getSnapshot()).toMatchObject({ key: "turn-1", phase: "speaking" });
    emit(0, { state: "done" });
    expect(player.getSnapshot()).toMatchObject({ key: null, phase: "idle", error: null });
  });

  it("stops at a second press of the same button", async () => {
    const { api, player, request, emit } = fixture();
    await settled();
    player.toggle("turn-1", "Hello there.");
    await settled();
    emit(0, { state: "speaking" });
    player.toggle("turn-1", "Hello there.");
    expect(player.getSnapshot()).toMatchObject({ key: null, phase: "idle" });
    expect(api.stop).toHaveBeenCalledWith(request(0));
    expect(api.speak).toHaveBeenCalledTimes(1);
    // What the stopped request still reports changes nothing.
    emit(0, { state: "speaking" });
    expect(player.getSnapshot().phase).toBe("idle");
  });

  it("moves to another answer, and the first one's late events do not touch the second", async () => {
    const { api, player, request, emit } = fixture();
    await settled();
    player.toggle("turn-1", "First.");
    await settled();
    emit(0, { state: "speaking" });
    player.toggle("turn-2", "Second.");
    await settled();
    expect(api.stop).toHaveBeenCalledWith(request(0));
    expect(player.getSnapshot()).toMatchObject({ key: "turn-2", phase: "preparing" });
    emit(0, { state: "stopped" });
    expect(player.getSnapshot()).toMatchObject({ key: "turn-2", phase: "preparing" });
    emit(1, { state: "speaking" });
    expect(player.getSnapshot()).toMatchObject({ key: "turn-2", phase: "speaking" });
  });

  it("says why a request failed, beside the button that asked, until something else is read", async () => {
    const { player, emit } = fixture();
    await settled();
    player.toggle("turn-1", "Hello.");
    await settled();
    emit(0, { state: "failed", message: "No speakers were found.", reason: "output" });
    expect(player.getSnapshot()).toMatchObject({
      key: null,
      phase: "idle",
      error: "No speakers were found.",
      failedKey: "turn-1",
      failure: "output",
    });
    player.toggle("turn-2", "Again.");
    expect(player.getSnapshot()).toMatchObject({ error: null, failedKey: null });
  });

  it("reports a refused request and never asks the engine to read nothing", async () => {
    const { api, player } = fixture();
    await settled();
    vi.mocked(api.speak).mockRejectedValueOnce(new Error("This page is no longer active."));
    player.toggle("turn-1", "Hello.");
    await settled();
    expect(player.getSnapshot()).toMatchObject({
      phase: "idle",
      failedKey: "turn-1",
      error: "This page is no longer active.",
    });
    player.toggle("turn-3", "---");
    await settled();
    expect(player.getSnapshot()).toMatchObject({
      failedKey: "turn-3",
      error: "There is nothing here to read aloud.",
    });
    expect(api.speak).toHaveBeenCalledTimes(1);
  });

  it("readies the saved voice once per voice, and only where reading aloud can run", async () => {
    const { api, player } = fixture();
    player.prepare();
    expect(api.prepare).not.toHaveBeenCalled();
    await settled();
    player.prepare();
    player.prepare();
    expect(api.prepare).toHaveBeenCalledTimes(1);
    expect(api.prepare).toHaveBeenLastCalledWith(null);
    player.setVoice("brit");
    player.prepare();
    expect(api.prepare).toHaveBeenLastCalledWith("brit");
    expect(api.prepare).toHaveBeenCalledTimes(2);
  });

  it("forgets what it knew when given another engine", async () => {
    const { player } = fixture();
    await settled();
    expect(player.getSnapshot().available).toBe(true);
    const other = fixture({ ...READY, installed: false });
    player.use(other.api);
    expect(player.getSnapshot().available).toBeNull();
    await settled();
    expect(player.getSnapshot().available).toBe(false);
  });
});
