import { X } from "lucide-react";
import { type ReactNode, useEffect, useId, useRef } from "react";
import { createPortal } from "react-dom";

/** The dialog widths (visual-language 4.6): sm 440, md 560, lg 760. */
export type ModalSize = "sm" | "md" | "lg";

/** The classes of every dialog in the app, for the few that manage their own `<dialog>`. */
export function dialogClassName(size: ModalSize, className = ""): string {
  return ["g-root", "g-dialog", size === "sm" ? "" : `g-dialog-${size}`, className]
    .filter(Boolean)
    .join(" ");
}

/**
 * Shows a modal dialog with focus on the control it opens for, marked `data-autofocus` (a form's
 * first field), and not on the close button, where the platform puts it as the first control in
 * the document. React's own `autoFocus` runs before the dialog is shown, so it cannot do this.
 */
export function showDialog(node: HTMLDialogElement): void {
  node.showModal();
  node.querySelector<HTMLElement>("[data-autofocus]:not(:disabled)")?.focus();
}

/**
 * The one dialog shell (visual-language 4.6, 6.19): Graphite's `.g-dialog` on a native modal
 * `<dialog>`, the title and a close button on the first line. Native focus containment and Escape
 * handling; it portals outside the app root.
 */
export function Modal({
  title,
  onClose,
  children,
  className = "",
  size = "sm",
  closeLabel = "Close",
  closeDisabled = false,
}: {
  readonly title: string;
  readonly onClose: () => void;
  readonly children: ReactNode;
  readonly className?: string;
  readonly size?: ModalSize;
  /** The close button's accessible name. */
  readonly closeLabel?: string;
  readonly closeDisabled?: boolean;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const label = useId();
  useEffect(() => {
    const node = dialog.current;
    if (node) showDialog(node);
    return () => node?.close();
  }, []);
  return createPortal(
    <dialog
      ref={dialog}
      className={dialogClassName(size, className)}
      aria-labelledby={label}
      onKeyDown={(event) => {
        if (event.key === "Escape") event.stopPropagation();
      }}
      onCancel={(event) => {
        event.preventDefault();
        if (!closeDisabled) onClose();
      }}
    >
      <header className="g-dialog-header">
        <h2 id={label} className="g-dialog-title">
          {title}
        </h2>
        <DialogClose label={closeLabel} onClick={onClose} disabled={closeDisabled} />
      </header>
      {children}
    </dialog>,
    document.body,
  );
}

/** The dialog's close button: a 28px ghost icon button, the same in every dialog. */
export function DialogClose({
  label,
  onClick,
  disabled = false,
}: {
  readonly label: string;
  readonly onClick: () => void;
  readonly disabled?: boolean;
}) {
  return (
    <button
      type="button"
      className="g-button g-button-ghost g-button-icon g-button-28"
      aria-label={label}
      onClick={onClick}
      disabled={disabled}
    >
      <X size={16} aria-hidden="true" />
    </button>
  );
}
