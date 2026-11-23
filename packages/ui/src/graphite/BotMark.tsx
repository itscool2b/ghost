import { logoSide } from "./AgentMark";
import grokBot from "./bot-logos/grok-bot.png";
import hermes from "./bot-logos/hermes.svg?url";
import muse from "./bot-logos/muse.png";
import openclaw from "./bot-logos/openclaw.svg?raw";
import { BOTS, type BotId } from "./bots";

export interface BotMarkProps {
  readonly bot: BotId;
  /** Width and height of the tile in px. Default 36. */
  readonly size?: number;
  readonly className?: string;
}

/**
 * Each assistant's own app icon, as its app shows it (third-party/brand-marks.md): Grok Bot's from
 * its desktop app, Muse's from its App Store listing, Hermes' from its repository.
 */
const APP_ICONS: Partial<Record<BotId, string>> = { grok: grokBot, muse, hermes };

/** OpenClaw's mark is drawn with a gradient, so it is shown as the image it is published as. */
const OPENCLAW = `data:image/svg+xml,${encodeURIComponent(openclaw)}`;

/**
 * The logo of an assistant that can be linked as a bot. Decorative: the assistant's name sits
 * beside it. An app icon is the whole tile, its corners masked by the tile's as a launcher masks
 * them; OpenClaw's lobster sits on the same tile as the agent marks, with their clear space.
 */
export function BotMark({ bot, size = 36, className }: BotMarkProps) {
  const box = Math.max(12, Math.min(64, Math.round(size)));
  const name = BOTS.find((kind) => kind.id === bot)?.name ?? "";
  const icon = APP_ICONS[bot];
  const side = icon ? box : logoSide(box);
  return (
    <span
      className={["g-agent-mark", "g-model-mark", className].filter(Boolean).join(" ")}
      data-logo=""
      data-bot={bot}
      data-app-icon={icon ? "" : undefined}
      title={name}
      aria-hidden="true"
      style={{
        width: `${box}px`,
        height: `${box}px`,
        borderRadius: `${Math.max(3, Math.round(box * 0.22))}px`,
      }}
    >
      <img
        className="g-agent-mark-logo"
        src={icon ?? OPENCLAW}
        alt=""
        width={side}
        height={side}
        draggable={false}
      />
    </span>
  );
}
