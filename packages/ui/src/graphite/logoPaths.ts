/**
 * The Ghost logo: a flying wing seen from above, with two eyes, drawn from the owner's artwork
 * (docs/design/brand/logo-source.png) as straight edges with rounded corners. The outline and the
 * eyes are separate paths so the eyes can move on their own; together they are one even-odd shape
 * whose holes are the eyes.
 */

/**
 * Width and height of the logo's coordinate space. The logo is far wider than tall: it fills the
 * width and sits centred in the height.
 */
export const LOGO_VIEWBOX = [1044, 440] as const;

/** The wing's outline, without its eyes. */
export const LOGO_OUTLINE_PATH =
  "M519.1 41.4C520.9 40.4 523.1 40.4 524.9 41.4L1038.7 327.1" +
  "C1041.6 328.7 1043.3 331.7 1043.3 334.9L1043.3 350.7" +
  "C1043.3 353.8 1041.8 356.7 1039.2 358.3L987.5 391" +
  "C985.1 392.5 982.1 392.8 979.5 391.6L790.7 307.3" +
  "C789.9 307 788.9 307.1 788.2 307.6L681.3 384.9C678.3 387 674.4 387 671.4 384.9" +
  "L608 339.9C607.1 339.3 606 339.3 605.1 339.9L527 396.5" +
  "C524 398.7 520 398.7 517 396.5L438.9 339.9C438 339.3 436.9 339.3 436 339.9" +
  "L372.6 384.9C369.6 387 365.7 387 362.7 384.9L255.8 307.6" +
  "C255.1 307.1 254.1 307 253.3 307.3L64.5 391.6C61.9 392.8 58.9 392.5 56.5 391" +
  "L4.8 358.3C2.2 356.7 0.7 353.8 0.7 350.7L0.7 334.9C0.7 331.7 2.4 328.7 5.3 327.1" +
  "Z";

/** The two eyes, left then right. */
export const LOGO_EYE_PATHS = [
  "M508.5 220.4C508.5 222.3 506.9 223.9 505 223.9L430.4 223.9" +
    "C428 223.9 425.9 222.4 425.2 220.2L411.8 181.5C409.3 174.2 416 167.2 423.4 169.2" +
    "L505.9 192.3C507.4 192.8 508.5 194.1 508.5 195.7Z",
  "M535.5 195.7C535.5 194.1 536.6 192.8 538.1 192.3L620.6 169.2" +
    "C628 167.2 634.7 174.2 632.2 181.5L618.8 220.2C618.1 222.4 616 223.9 613.6 223.9" +
    "L539 223.9C537.1 223.9 535.5 222.3 535.5 220.4Z",
] as const;

/** The full logo, above `LOGO_SMALL_MAX` px wide. */
export const LOGO_PATH = LOGO_OUTLINE_PATH + LOGO_EYE_PATHS.join("");

/** The eyes of the small variant: half again as large, grown away from the centre line. */
export const LOGO_SMALL_EYE_PATHS = [
  "M508.5 233.5C508.5 236.4 506.1 238.8 503.2 238.8L391.3 238.8" +
    "C387.8 238.8 384.7 236.6 383.5 233.2L363.5 175.2" +
    "C359.7 164.3 369.7 153.7 380.8 156.8L504.6 191.4" +
    "C506.9 192.1 508.5 194.1 508.5 196.5Z",
  "M535.5 196.5C535.5 194.1 537.1 192.1 539.4 191.4L663.2 156.8" +
    "C674.3 153.7 684.3 164.3 680.5 175.2L660.5 233.2" +
    "C659.3 236.6 656.2 238.8 652.7 238.8L540.8 238.8" +
    "C537.9 238.8 535.5 236.4 535.5 233.5Z",
] as const;

/** The small variant: the same outline with larger eyes, which stay open at icon sizes. */
export const LOGO_SMALL_PATH = LOGO_OUTLINE_PATH + LOGO_SMALL_EYE_PATHS.join("");

/** The widest box, in px, that draws the small variant. */
export const LOGO_SMALL_MAX = 40;

export interface LogoSvgOptions {
  /** Width and height of the icon in px. Default 1024. */
  readonly size?: number;
  /** Space around the logo as a fraction of the size. Default 0.18. */
  readonly padding?: number;
  /** Logo colour. Default Graphite's fg-1. */
  readonly color?: string;
  /** Tile colour, or null for none. Default black. */
  readonly background?: string | null;
  /** Tile corner radius as a fraction of the size. Default 0.225, the macOS icon shape. */
  readonly radius?: number;
}

/** A standalone SVG document of the logo, for app icons, favicons and social images. */
export function logoSvg(options: LogoSvgOptions = {}): string {
  const size = Math.max(8, Math.round(options.size ?? 1024));
  const padding = Math.min(0.45, Math.max(0, options.padding ?? 0.18));
  const background = options.background === undefined ? "#000000" : options.background;
  const radius = Math.round(size * Math.min(0.5, Math.max(0, options.radius ?? 0.225)));
  const [vw, vh] = LOGO_VIEWBOX;
  const inner = size * (1 - 2 * padding);
  const scale = inner / Math.max(vw, vh);
  const x = (size - vw * scale) / 2;
  const y = (size - vh * scale) / 2;
  const d = size <= LOGO_SMALL_MAX ? LOGO_SMALL_PATH : LOGO_PATH;
  const tile =
    background === null
      ? ""
      : `<rect width="${size}" height="${size}" rx="${radius}" fill="${background}"/>`;
  return (
    `<svg xmlns="http://www.w3.org/2000/svg" width="${size}" height="${size}" ` +
    `viewBox="0 0 ${size} ${size}">${tile}` +
    `<path transform="translate(${round(x)} ${round(y)}) scale(${round(scale, 5)})" ` +
    `fill="${options.color ?? "#ededed"}" fill-rule="evenodd" d="${d}"/></svg>`
  );
}

function round(value: number, places = 3): number {
  const f = 10 ** places;
  return Math.round(value * f) / f;
}
