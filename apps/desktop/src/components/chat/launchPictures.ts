import b2 from "./launch-art/b-2-spirit.jpg";
import b29 from "./launch-art/b-29-superfortress.jpg";
import b747 from "./launch-art/boeing-747-400.jpg";
import b787 from "./launch-art/boeing-787-dreamliner.jpg";
import f22 from "./launch-art/f-22-raptor.jpg";
import f35 from "./launch-art/f-35-lightning-ii.jpg";
import p51 from "./launch-art/p-51-mustang.jpg";
import shuttle from "./launch-art/space-shuttle.jpg";
import sr71 from "./launch-art/sr-71-blackbird.jpg";
import wright from "./launch-art/wright-flyer.jpg";

/**
 * One picture for the start screen: an American aircraft, from the ten images the owner supplied
 * (17 November 2026: "delete all of those and just replace them with these images"; before them
 * the start screen drew fifteen military aircraft in text). Each file under `launch-art/` is one of
 * those images as an ink map: grey only, white where the picture has ink and black where it has
 * none, so the start screen can draw it in the theme's own ink on the theme's own pane.
 * third-party/ascii-art.md lists the files and how each was converted.
 */
export interface LaunchPicture {
  /** The file under `launch-art/`, and the key the last launch's choice is remembered by. */
  readonly id: string;
  /** The aircraft. */
  readonly name: string;
  /** The ink map's address. */
  readonly src: string;
  /** The ink map's size in pixels; the start screen never draws it larger. */
  readonly width: number;
  readonly height: number;
}

/** Every image the owner supplied is this size. */
const SIZE = { width: 1168, height: 784 } as const;

export const LAUNCH_ART: readonly LaunchPicture[] = [
  { id: "b-2-spirit", name: "B-2 Spirit", src: b2, ...SIZE },
  { id: "sr-71-blackbird", name: "SR-71 Blackbird", src: sr71, ...SIZE },
  { id: "f-22-raptor", name: "F-22 Raptor", src: f22, ...SIZE },
  { id: "f-35-lightning-ii", name: "F-35 Lightning II", src: f35, ...SIZE },
  { id: "p-51-mustang", name: "P-51 Mustang", src: p51, ...SIZE },
  { id: "b-29-superfortress", name: "B-29 Superfortress", src: b29, ...SIZE },
  { id: "boeing-747-400", name: "Boeing 747-400", src: b747, ...SIZE },
  { id: "boeing-787-dreamliner", name: "Boeing 787 Dreamliner", src: b787, ...SIZE },
  { id: "space-shuttle", name: "Space Shuttle", src: shuttle, ...SIZE },
  { id: "wright-flyer", name: "Wright Flyer", src: wright, ...SIZE },
];

/** The picture the last launch showed, so the next one shows another. */
const LAST_KEY = "auracode.launchArt.last";

type PictureStorage = Pick<Storage, "getItem" | "setItem">;

function deviceStorage(): PictureStorage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

/**
 * A picture other than the one `storage` says the last launch showed, remembered for the next.
 * Storage that cannot be read or written only loses the "never twice in a row" promise.
 */
export function pickLaunchPicture(
  random: () => number = Math.random,
  storage: PictureStorage | null = deviceStorage(),
): LaunchPicture {
  let last: string | null = null;
  try {
    last = storage?.getItem(LAST_KEY) ?? null;
  } catch {}
  const choices = LAUNCH_ART.filter((picture) => picture.id !== last);
  const picture = choices[
    Math.min(choices.length - 1, Math.floor(random() * choices.length))
  ] as LaunchPicture;
  try {
    storage?.setItem(LAST_KEY, picture.id);
  } catch {}
  return picture;
}

let thisLaunch: LaunchPicture | null = null;

/** This launch's picture: chosen once, so the start screen keeps it until Ghost starts again. */
export function launchPicture(): LaunchPicture {
  thisLaunch ??= pickLaunchPicture();
  return thisLaunch;
}
