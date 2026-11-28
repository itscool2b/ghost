/**
 * What the Game pane hands the agent on the person's behalf (docs/game.md, "Telling the agent"):
 * the words for something pointed at, for the last seconds of play, and for values turned while
 * the game ran. Words only; the pane adds the picture.
 */
import type { GamePicked, GameRecent, GameTunable, GameTunables } from "../bindings";

/** A number from the app, where one that was not finite arrives as nothing. */
export const num = (value: number | null | undefined): number => value ?? 0;
const round = (value: number | null) => Math.round(num(value));

/** Where a picked node is in the picture, in the game's own pixels: x, y, width, height. */
export function areaOf(picked: GamePicked): readonly [number, number, number, number] {
  const [x, y, width, height] = picked.area;
  return [num(x), num(y), num(width), num(height)];
}

/** `Player (Polygon2D)`: a node in a line. */
export function pickedLabel(picked: GamePicked): string {
  // A node left with its kind for a name is named once.
  return picked.name === picked.class ? picked.name : `${picked.name} (${picked.class})`;
}

/** The message for a node the person pointed at: their note, then where the agent finds it. */
export function pointText(
  note: string,
  picked: GamePicked,
  game: { readonly name: string | null; readonly width: number; readonly height: number },
  withImage: boolean,
): string {
  const lines: string[] = [];
  const said = note.trim();
  if (said) lines.push(said, "");
  lines.push(`Pointed at in the game${game.name ? ` (${game.name})` : ""}, frozen as it was:`);
  lines.push(`- ${pickedLabel(picked)} at ${picked.path}`);
  const indent = "   ";
  if (picked.text) lines.push(`${indent}Shows: "${picked.text}"`);
  if (picked.scene) {
    const where = picked.scene_line ? `${picked.scene}:${picked.scene_line}` : picked.scene;
    lines.push(
      `${indent}Defined in ${where}${picked.in_scene && picked.in_scene !== "." ? ` as ${picked.in_scene}` : ""}`,
    );
  } else {
    lines.push(`${indent}Made while the game ran: it is in no scene file.`);
  }
  if (picked.script) {
    lines.push(
      `${indent}Script: ${picked.script}${picked.script_on && picked.script_on !== picked.path ? ` (on ${picked.script_on})` : ""}`,
    );
  }
  const [x, y, width, height] = picked.area;
  lines.push(
    `${indent}${round(width)}×${round(height)} at (${round(x)}, ${round(y)}) in a ${game.width}×${game.height} picture`,
  );
  if (withImage) lines.push("The attached image shows it outlined in the game.");
  return lines.join("\n");
}

const seconds = (value: number | null) => {
  const rounded = Math.round(num(value) * 10) / 10;
  return `${rounded} ${rounded === 1 ? "second" : "seconds"}`;
};

/** The message for the last seconds of play: the note, the pictures, and how to see it again. */
export function recentText(
  note: string,
  recent: GameRecent,
  name: string | null,
  withImage: boolean,
): string {
  const lines: string[] = [];
  const said = note.trim();
  if (said) lines.push(said, "");
  const whose = name ? ` in ${name}` : "";
  lines.push(
    withImage
      ? `The last ${seconds(recent.seconds)} of my play${whose} are in the attached filmstrip: ${recent.pictures} ${recent.pictures === 1 ? "picture" : "pictures"}, left to right and top to bottom, the newest last. The run is ${seconds(recent.run_seconds)} long.`
      : `This is about the last ${seconds(recent.seconds)} of my play${whose}. The run is ${seconds(recent.run_seconds)} long.`,
  );
  if (recent.replay) {
    lines.push(
      `Play that run again with game_replay and replay "${recent.replay}": the same seed and the same ${recent.inputs} inputs at the same steps, frozen where mine ended. until_s stops it earlier.`,
    );
  } else if (recent.unrepeatable) {
    lines.push(recent.unrepeatable);
  }
  return lines.join("\n");
}

/** A tunable's value in words: a switch as on or off, a number without a float's noise. */
export function shownValue(kind: string, given: number | null): string {
  const value = num(given);
  if (kind === "bool") return value !== 0 ? "true" : "false";
  if (kind === "int") return String(Math.round(value));
  return String(Math.round(value * 10000) / 10000);
}

/** The values that are not what they were when the sheet opened. */
export function turned(
  now: readonly GameTunable[],
  first: readonly GameTunable[],
): readonly {
  readonly name: string;
  readonly kind: string;
  readonly value: number;
  readonly was: number;
}[] {
  return now.flatMap((tunable) => {
    const before = first.find((entry) => entry.name === tunable.name);
    if (!before || num(before.value) === num(tunable.value)) return [];
    return [
      {
        name: tunable.name,
        kind: tunable.kind,
        value: num(tunable.value),
        was: num(before.value),
      },
    ];
  });
}

/** The message that asks the agent to write tuned values into the project. */
export function keepText(
  tunables: GameTunables,
  changes: ReturnType<typeof turned>,
  name: string | null,
): string {
  const lines = [
    `Keep these values I tuned while ${name ?? "the game"} ran. They are set in the running game only: write each where it lives, the script's own start value or the scene that overrides it.`,
    `On ${tunables.name} (${tunables.class}) at ${tunables.path}${tunables.script ? `, script ${tunables.script}` : ""}:`,
  ];
  for (const change of changes) {
    lines.push(
      `- ${change.name} = ${shownValue(change.kind, change.value)} (was ${shownValue(change.kind, change.was)})`,
    );
  }
  return lines.join("\n");
}

function niceStep(span: number): number {
  const raw = span / 200;
  if (!(raw > 0)) return 0.01;
  const power = 10 ** Math.floor(Math.log10(raw));
  const lead = raw / power;
  return power * (lead >= 5 ? 5 : lead >= 2 ? 2 : 1);
}

/**
 * A slider's range for a value: its author's, when the script gives one (`@export_range`), or
 * one made from the value itself, from nothing to twice what it is, on both sides of zero when
 * it is negative.
 */
export function tunableRange(tunable: GameTunable): {
  readonly min: number;
  readonly max: number;
  readonly step: number;
} {
  if (tunable.kind === "bool") return { min: 0, max: 1, step: 1 };
  const whole = tunable.kind === "int";
  const { min, max } = tunable;
  if (typeof min === "number" && typeof max === "number" && max > min) {
    return { min, max, step: tunable.step ?? (whole ? 1 : niceStep(max - min)) };
  }
  const value = num(tunable.value);
  const first = tunable.initial ?? value;
  const size = Math.max(Math.abs(value), Math.abs(first));
  const reach =
    size === 0 ? (whole ? 10 : 1) : whole ? Math.max(10, Math.ceil(size * 2)) : size * 2;
  const low = value < 0 || first < 0 ? -reach : 0;
  return { min: low, max: reach, step: whole ? 1 : niceStep(reach - low) };
}
