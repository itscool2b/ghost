/**
 * What an export made (docs/game.md, "Export"): the zip, where it is, and how to put it on
 * itch.io. Uploading is the person's own: Ghost packs the game and stops there.
 */
import { FolderOpen, X } from "lucide-react";
import { useEffect, useRef } from "react";
import type { GameExport } from "../../bindings";
import { sizeText } from "../../lib/gameAssets";

/** itch.io's page for a new game, and the page of its own that says how a web game is uploaded. */
export const ITCH_NEW = "https://itch.io/game/new";
export const ITCH_GUIDE = "https://itch.io/docs/creators/html5";
/** The most files itch.io unpacks from one zip, as its guide said on 28 November 2026. */
export const ITCH_FILES = 1000;

export function ExportSheet({
  exported,
  onShow,
  onClose,
}: {
  readonly exported: GameExport;
  /** Shows the zip in its folder. */
  readonly onShow: () => void;
  readonly onClose: () => void;
}) {
  const close = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    close.current?.focus();
  }, []);
  return (
    <section className="game-log game-export" aria-label="Exported" data-testid="game-exported">
      <div className="game-log-head">
        <h2>Packed for the web</h2>
        <button
          ref={close}
          type="button"
          className="g-button g-button-ghost g-button-icon g-button-28"
          aria-label="Close"
          onClick={onClose}
        >
          <X size={14} />
        </button>
      </div>
      <div className="game-log-body">
        <p data-testid="game-exported-what">
          {exported.what}: {exported.files} {exported.files === 1 ? "file" : "files"},{" "}
          {sizeText(exported.bytes)} zipped.
        </p>
        <p className="game-code" data-testid="game-exported-zip">
          {exported.zip}
        </p>
        <p className="game-note">To put it on itch.io, where people play it in their browser:</p>
        <ol className="game-steps">
          <li>Create a new project there and pick "HTML Game" from the "Kind Of Game" list.</li>
          <li>Upload this zip. Its index.html is at its top, as itch.io asks.</li>
          <li>Save, and open the page it gives you.</li>
        </ol>
        {exported.files > ITCH_FILES ? (
          <p className="ui-error" role="alert" data-testid="game-exported-too-many">
            This zip holds {exported.files} files. itch.io takes at most {ITCH_FILES} in one.
          </p>
        ) : (
          <p className="game-note">
            itch.io takes up to {ITCH_FILES} files and 500 MB unpacked, with no file over 200 MB.
          </p>
        )}
      </div>
      <div className="game-tune-foot">
        <span className="game-note">Nothing was uploaded. That part is yours.</span>
        <button
          type="button"
          className="g-button g-button-ghost g-button-28"
          data-testid="game-exported-show"
          onClick={onShow}
        >
          <FolderOpen size={14} />
          Show the zip
        </button>
        <a
          className="g-button g-button-ghost g-button-28"
          href={ITCH_GUIDE}
          target="_blank"
          rel="noreferrer noopener"
        >
          itch.io's guide
        </a>
        <a
          className="g-button g-button-primary g-button-28"
          href={ITCH_NEW}
          target="_blank"
          rel="noreferrer noopener"
          data-testid="game-exported-itch"
        >
          Open itch.io
        </a>
      </div>
    </section>
  );
}
