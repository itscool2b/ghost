import { Check, ChevronDown } from "lucide-react";
import {
  Children,
  type FocusEvent,
  Fragment,
  isValidElement,
  type KeyboardEvent,
  type ReactNode,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { createPortal } from "react-dom";
import { OVER_BROWSER } from "../lib/browserOcclusion";
import "./select.css";

/**
 * The app's own dropdown, in place of a native <select> (owner, 17 November 2026: settings
 * "look dirty"; nothing GTK-drawn should show). A native select opens the toolkit's menu on Linux
 * (a GTK popover in the desktop's own theme) and the system's on macOS and Windows, none of which
 * a stylesheet reaches. This draws the field as the app's other fields and its list as the app's
 * menus: the raised layer at radius 16, 32px rows at radius 10, 6px in (the Grok Bot app's menus).
 *
 * It takes the same <option> and <optgroup> children and hands onChange the part of a change event
 * callers read (`event.currentTarget.value`), so a native select becomes this by its tag alone. The
 * list opens in the top layer (the Popover API), above dialogs and anything clipping the field, and
 * marks itself as covering a browser page. Keys follow the select-only combobox pattern: focus
 * stays on the field and the list's active row is its aria-activedescendant.
 */

export interface SelectChoice {
  readonly value: string;
  readonly label: string;
  readonly disabled: boolean;
  /** The <optgroup> label it sits under, if any. */
  readonly group?: string;
  /** The option's own title, shown under the pointer as a native option's is. */
  readonly title?: string;
}

/** What onChange receives: the part of a native select's change event that callers read. */
export interface SelectChange {
  readonly currentTarget: { readonly value: string };
  readonly target: { readonly value: string };
}

export interface SelectProps {
  readonly value: string;
  readonly onChange?: (change: SelectChange) => void;
  /**
   * Report a pick of the choice already shown too. A native select cannot (it fires no change), but
   * where the shown choice is only a default, picking it says the person means it.
   */
  readonly repick?: boolean;
  readonly children?: ReactNode;
  readonly id?: string;
  readonly className?: string;
  readonly disabled?: boolean;
  readonly title?: string;
  readonly onFocus?: (event: FocusEvent<HTMLButtonElement>) => void;
  readonly onBlur?: (event: FocusEvent<HTMLButtonElement>) => void;
  readonly "aria-label"?: string;
  readonly "aria-labelledby"?: string;
  readonly "aria-describedby"?: string;
  readonly "aria-invalid"?: boolean | "true" | "false";
  readonly "data-testid"?: string;
  /** Any other data-* hooks the checks read are passed through to the field. */
  readonly [data: `data-${string}`]: string | number | boolean | undefined;
}

function text(node: ReactNode): string {
  if (node === null || node === undefined || typeof node === "boolean") return "";
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(text).join("");
  if (isValidElement<{ children?: ReactNode }>(node)) return text(node.props.children);
  return "";
}

/** The choices a select's <option> and <optgroup> children describe, in order. */
export function choicesOf(children: ReactNode, group?: string): SelectChoice[] {
  const choices: SelectChoice[] = [];
  Children.forEach(children, (child) => {
    if (!isValidElement<Record<string, unknown>>(child)) return;
    const props = child.props;
    if (child.type === "option") {
      const label = text(props.children as ReactNode);
      choices.push({
        value: props.value === undefined ? label : String(props.value),
        label,
        disabled: Boolean(props.disabled),
        ...(group === undefined ? {} : { group }),
        ...(typeof props.title === "string" && props.title ? { title: props.title } : {}),
      });
    } else if (child.type === "optgroup") {
      choices.push(...choicesOf(props.children as ReactNode, String(props.label ?? "")));
    } else if (child.type === Fragment) {
      choices.push(...choicesOf(props.children as ReactNode, group));
    }
  });
  return choices;
}

/** The next choice that can be picked from `from`, `step` at a time; `from` when there is none. */
export function stepChoice(choices: readonly SelectChoice[], from: number, step: number): number {
  let index = from;
  for (let moved = 0; moved < choices.length; moved++) {
    const next = index + step;
    if (next < 0 || next >= choices.length) break;
    index = next;
    if (!choices[index]?.disabled) return index;
  }
  return choices[from]?.disabled === false ? from : firstEnabled(choices, step > 0 ? -1 : 1);
}

function firstEnabled(choices: readonly SelectChoice[], direction: 1 | -1): number {
  const order = direction === 1 ? choices.keys() : [...choices.keys()].reverse();
  for (const index of order) if (!choices[index]?.disabled) return index;
  return -1;
}

/** The first choice after `from` whose label starts with `typed`, wrapping, as a native select finds one. */
export function typedChoice(choices: readonly SelectChoice[], from: number, typed: string): number {
  const wanted = typed.toLocaleLowerCase();
  if (!wanted) return from;
  // Repeating one letter cycles through the choices that start with it.
  const repeated = [...wanted].every((letter) => letter === wanted[0]);
  const search = repeated ? (wanted[0] ?? "") : wanted;
  const start = repeated || wanted.length === 1 ? from + 1 : from;
  for (let offset = 0; offset < choices.length; offset++) {
    const index = (start + offset) % choices.length;
    const choice = choices[index];
    if (choice && !choice.disabled && choice.label.toLocaleLowerCase().startsWith(search))
      return index;
  }
  return from;
}

const supportsPopover = () =>
  typeof HTMLElement !== "undefined" && typeof HTMLElement.prototype.showPopover === "function";

/** Pixels between the field and its list, and between the list and the window's edge. */
const GAP = 4;
const EDGE = 8;
const MAX_HEIGHT = 320;

export function Select({
  value,
  onChange,
  repick = false,
  children,
  id,
  className,
  disabled = false,
  title,
  onFocus,
  onBlur,
  ...rest
}: SelectProps) {
  const choices = choicesOf(children);
  const selected = choices.findIndex((choice) => choice.value === value);
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(-1);
  const [labelledBy, setLabelledBy] = useState<string | undefined>(undefined);
  const field = useRef<HTMLButtonElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const typed = useRef({ text: "", at: 0 });
  const base = useId();
  const listId = `${base}-list`;
  const optionId = (index: number) => `${base}-option-${index}`;

  const ariaLabel = rest["aria-label"];
  const ariaLabelledBy = rest["aria-labelledby"];
  // A field named by a <label> (wrapping it or pointing at its id) takes that label's words as its
  // name: a combobox draws no name from its contents, which show the chosen value.
  useLayoutEffect(() => {
    if (ariaLabel || ariaLabelledBy) return;
    const labels = [...(field.current?.labels ?? [])];
    if (labels.length === 0) return;
    const ids = labels.map((label, index) => {
      if (!label.id) label.id = `${base}-label-${index}`;
      return label.id;
    });
    setLabelledBy(ids.join(" "));
  }, [ariaLabel, ariaLabelledBy, base]);

  const choose = (index: number) => {
    const choice = choices[index];
    setOpen(false);
    field.current?.focus({ preventScroll: true });
    if (!choice || choice.disabled || (choice.value === value && !repick)) return;
    onChange?.({ currentTarget: { value: choice.value }, target: { value: choice.value } });
  };

  const show = (index: number) => {
    if (disabled || choices.length === 0) return;
    setActive(index >= 0 && !choices[index]?.disabled ? index : stepChoice(choices, -1, 1));
    setOpen(true);
  };

  // Place the list under the field (over it when there is more room above), inside the window.
  const place = () => {
    const anchor = field.current;
    const panel = list.current;
    if (!anchor || !panel) return;
    const box = anchor.getBoundingClientRect();
    const below = window.innerHeight - box.bottom - EDGE - GAP;
    const above = box.top - EDGE - GAP;
    const natural = Math.min(MAX_HEIGHT, panel.scrollHeight);
    const upward = natural > below && above > below;
    panel.style.minWidth = `${box.width}px`;
    panel.style.maxHeight = `${Math.max(96, Math.min(MAX_HEIGHT, upward ? above : below))}px`;
    const width = panel.offsetWidth;
    const left = Math.max(EDGE, Math.min(box.left, window.innerWidth - width - EDGE));
    panel.style.left = `${left}px`;
    panel.style.top = upward ? "auto" : `${box.bottom + GAP}px`;
    panel.style.bottom = upward ? `${window.innerHeight - box.top + GAP}px` : "auto";
    panel.dataset.side = upward ? "top" : "bottom";
  };

  // Opening: into the top layer, then into place, before the first paint. The list opens with the
  // chosen row active, so keeping the active row in view also shows the choice in a long list.
  const placed = useRef(false);
  useLayoutEffect(() => {
    const panel = list.current;
    if (!open || !panel) {
      placed.current = false;
      return;
    }
    if (!placed.current) {
      placed.current = true;
      if (supportsPopover() && !panel.matches(":popover-open")) panel.showPopover();
      place();
    }
    if (active >= 0)
      panel
        .querySelector<HTMLElement>(`#${CSS.escape(`${base}-option-${active}`)}`)
        ?.scrollIntoView({ block: "nearest" });
  });

  // Close on a press anywhere else, when the window loses focus, and follow the field as it moves.
  useEffect(() => {
    if (!open) return;
    const outside = (event: PointerEvent) => {
      const target = event.target as Node | null;
      if (target && (field.current?.contains(target) || list.current?.contains(target))) return;
      setOpen(false);
    };
    const away = () => setOpen(false);
    const follow = (event: Event) => {
      if (event.target instanceof Node && list.current?.contains(event.target)) return;
      place();
    };
    document.addEventListener("pointerdown", outside, true);
    window.addEventListener("blur", away);
    window.addEventListener("resize", follow);
    window.addEventListener("scroll", follow, true);
    return () => {
      document.removeEventListener("pointerdown", outside, true);
      window.removeEventListener("blur", away);
      window.removeEventListener("resize", follow);
      window.removeEventListener("scroll", follow, true);
    };
  });

  // A field that turns disabled while open closes its list.
  useEffect(() => {
    if (disabled) setOpen(false);
  }, [disabled]);

  const onKeyDown = (event: KeyboardEvent<HTMLButtonElement>) => {
    const key = event.key;
    if (!open) {
      if (key === "ArrowDown" || key === "ArrowUp" || key === "Enter" || key === " ") {
        event.preventDefault();
        show(selected);
        return;
      }
      if (key.length === 1 && !event.ctrlKey && !event.metaKey && !event.altKey) {
        // Typing on a closed field picks the next matching choice, as a native select does.
        const now = event.timeStamp;
        typed.current = {
          text: now - typed.current.at < 700 ? typed.current.text + key : key,
          at: now,
        };
        const match = typedChoice(choices, selected, typed.current.text);
        if (match !== selected && match >= 0) {
          event.preventDefault();
          const choice = choices[match];
          if (choice)
            onChange?.({ currentTarget: { value: choice.value }, target: { value: choice.value } });
        }
      }
      return;
    }
    switch (key) {
      case "ArrowDown":
        event.preventDefault();
        setActive((index) => stepChoice(choices, index, 1));
        return;
      case "ArrowUp":
        event.preventDefault();
        setActive((index) => stepChoice(choices, index, -1));
        return;
      case "PageDown":
        event.preventDefault();
        setActive((index) => stepChoice(choices, Math.min(choices.length, index + 9), 1));
        return;
      case "PageUp":
        event.preventDefault();
        setActive((index) => stepChoice(choices, Math.max(-1, index - 9), -1));
        return;
      case "Home":
        event.preventDefault();
        setActive(stepChoice(choices, -1, 1));
        return;
      case "End":
        event.preventDefault();
        setActive(stepChoice(choices, choices.length, -1));
        return;
      case "Enter":
      case " ":
        event.preventDefault();
        choose(active);
        return;
      case "Escape":
        // The list closes and nothing behind it hears the key: not a dialog, not Settings.
        event.preventDefault();
        event.stopPropagation();
        setOpen(false);
        return;
      case "Tab":
        setOpen(false);
        return;
      default:
        if (key.length === 1 && !event.ctrlKey && !event.metaKey && !event.altKey) {
          event.preventDefault();
          const now = event.timeStamp;
          typed.current = {
            text: now - typed.current.at < 700 ? typed.current.text + key : key,
            at: now,
          };
          setActive((index) => typedChoice(choices, index, typed.current.text));
        }
    }
  };

  const chosen = choices[selected];
  // The list belongs to what holds the field: inside a modal dialog everything outside it is
  // inert, even what shows above it in the top layer, and a popover (a <details>) closes on a
  // press outside itself. It still shows in the top layer, whatever clips its holder.
  const portalTarget =
    typeof document === "undefined"
      ? null
      : (field.current?.closest("dialog, details") ?? document.body);

  const rows: ReactNode[] = [];
  let group: string | undefined;
  choices.forEach((choice, index) => {
    if (choice.group !== undefined && choice.group !== group) {
      rows.push(
        <div key={`group-${choice.group}`} className="ui-select-group" role="presentation">
          {choice.group}
        </div>,
      );
    }
    group = choice.group;
    rows.push(
      // biome-ignore lint/a11y/useKeyWithClickEvents: The field keeps focus and handles every key (aria-activedescendant).
      // biome-ignore lint/a11y/useFocusableInteractive: Options are pointed at through aria-activedescendant, never focused.
      <div
        key={`option-${choice.group ?? ""}-${choice.value}`}
        id={optionId(index)}
        role="option"
        className="ui-select-option"
        aria-selected={index === selected}
        aria-disabled={choice.disabled || undefined}
        data-value={choice.value}
        data-active={index === active ? "" : undefined}
        title={choice.title}
        onPointerMove={() => {
          if (!choice.disabled && index !== active) setActive(index);
        }}
        onClick={() => {
          if (!choice.disabled) choose(index);
        }}
      >
        <span className="ui-select-option-label">{choice.label}</span>
        {index === selected ? <Check size={14} aria-hidden="true" /> : null}
      </div>,
    );
  });

  return (
    <>
      <button
        {...rest}
        ref={field}
        id={id}
        type="button"
        role="combobox"
        className={className ? `ui-select ${className}` : "ui-select g-select"}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={open ? listId : undefined}
        aria-activedescendant={open && active >= 0 ? optionId(active) : undefined}
        aria-labelledby={ariaLabelledBy ?? (ariaLabel ? undefined : labelledBy)}
        data-value={value}
        disabled={disabled}
        title={title}
        onFocus={onFocus}
        onBlur={(event) => {
          // Moving focus elsewhere closes the list; a press on the list keeps focus here.
          if (open && !list.current?.contains(event.relatedTarget as Node | null)) setOpen(false);
          onBlur?.(event);
        }}
        onClick={(event) => {
          // WebKit does not focus a button it is clicked on: without this the list a click opened
          // gets no keys, and Escape reaches whatever is behind it.
          event.currentTarget.focus();
          if (open) setOpen(false);
          else show(selected);
        }}
        onKeyDown={onKeyDown}
      >
        {/*
         * Every label, unseen, so the field is as wide as its widest choice, as a native select is.
         * Each carries its value and whether it can be picked, so what is offered reads from the
         * closed field.
         */}
        <span className="ui-select-sizer" aria-hidden="true">
          {choices.map((choice) => (
            <span
              key={`sizer-${choice.group ?? ""}-${choice.value}`}
              data-value={choice.value}
              data-disabled={choice.disabled ? "" : undefined}
            >
              {choice.label}
            </span>
          ))}
        </span>
        <span className="ui-select-value">{chosen?.label ?? ""}</span>
        <ChevronDown className="ui-select-chevron" size={14} aria-hidden="true" />
      </button>
      {open && portalTarget
        ? createPortal(
            <div
              ref={list}
              id={listId}
              role="listbox"
              className="ui-select-list"
              aria-label={ariaLabel}
              aria-labelledby={ariaLabel ? undefined : (ariaLabelledBy ?? labelledBy)}
              {...{ popover: supportsPopover() ? "manual" : undefined }}
              {...{ [OVER_BROWSER]: "" }}
              // A press on the list must not take focus from the field.
              onPointerDown={(event) => event.preventDefault()}
              onMouseDown={(event) => event.preventDefault()}
            >
              {rows}
            </div>,
            portalTarget,
          )
        : null}
    </>
  );
}
