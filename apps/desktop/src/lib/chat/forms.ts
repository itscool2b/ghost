/**
 * An MCP server's form (MCP elicitation, form mode) while it is filled in, and the answer it
 * sends. The checks mirror `ghost_protocol::forms`, which the session runs again before anything
 * reaches the agent, so a mistake shows at its field instead of as a refused answer.
 */
import type { UiAnswer, UiField, UiFieldValue, UiTextFormat } from "../../bindings";

/** Text and number fields hold what was typed; a flag holds `null` until touched. */
export type FieldDraft =
  | { readonly kind: "text"; readonly text: string }
  | { readonly kind: "flag"; readonly value: boolean | null }
  | { readonly kind: "choices"; readonly selected: readonly number[] };

export type FormDrafts = ReadonlyMap<string, FieldDraft>;

/** `2026-11-05T08:30:00.000Z` as a `datetime-local` input shows it, in local time. */
export function localDateTime(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "";
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

/** Each field starts at the server's default, if it gave one. */
export function initialDrafts(fields: readonly UiField[]): Map<string, FieldDraft> {
  const drafts = new Map<string, FieldDraft>();
  for (const field of fields) {
    const kind = field.kind;
    switch (kind.type) {
      case "text":
        drafts.set(field.id, {
          kind: "text",
          text:
            kind.format === "date_time" && kind.default
              ? localDateTime(kind.default)
              : (kind.default ?? ""),
        });
        break;
      case "number":
        drafts.set(field.id, {
          kind: "text",
          text: kind.default === null ? "" : String(kind.default),
        });
        break;
      case "boolean":
        drafts.set(field.id, { kind: "flag", value: kind.default });
        break;
      case "choice":
        drafts.set(field.id, { kind: "choices", selected: kind.default });
        break;
    }
  }
  return drafts;
}

/** A `datetime-local` value as the UTC moment it names; text that is no date stays as typed. */
function isoMoment(local: string): string {
  const date = new Date(local);
  return Number.isNaN(date.getTime()) ? local : date.toISOString();
}

const EMAIL = /^[^\s@]+@[^\s@.]+\.[^\s@]+$/;
const URI = /^[A-Za-z][A-Za-z0-9+.-]*:\S+$/;
const DATE = /^\d{4}-(0[1-9]|1[0-2])-(0[1-9]|[12]\d|3[01])$/;

export function acceptsFormat(format: UiTextFormat, text: string): boolean {
  switch (format) {
    case "email":
      return EMAIL.test(text);
    case "uri":
      return URI.test(text);
    case "date":
      return DATE.test(text);
    case "date_time":
      return !Number.isNaN(new Date(text).getTime()) && /T\d{2}:\d{2}:\d{2}/.test(text);
  }
}

const FORMAT_WORDS: Record<UiTextFormat, string> = {
  email: "an email address",
  uri: "a web address or other URI",
  date: "a date",
  date_time: "a date and time",
};

/** A field's answer, or why it cannot be sent. An empty optional field sends nothing. */
export function fieldAnswer(
  field: UiField,
  draft: FieldDraft | undefined,
): { readonly value: UiFieldValue | null; readonly error: string | null } {
  const name = field.label;
  const none = (error: string | null = null) => ({ value: null, error });
  const some = (value: UiFieldValue["value"]) => ({ value: { id: field.id, value }, error: null });
  const kind = field.kind;
  switch (kind.type) {
    case "text": {
      const raw = draft?.kind === "text" ? draft.text : "";
      if (raw.trim() === "") return none(field.required ? `${name} is required.` : null);
      // A datetime-local input has no zone or seconds: it is sent as the moment it names.
      const text = kind.format === "date_time" ? isoMoment(raw) : raw;
      const length = [...text].length;
      if (kind.min_length !== null && length < kind.min_length)
        return none(`${name} needs at least ${kind.min_length} characters.`);
      if (kind.max_length !== null && length > kind.max_length)
        return none(`${name} takes at most ${kind.max_length} characters.`);
      if (kind.format !== null && !acceptsFormat(kind.format, text))
        return none(`${name} must be ${FORMAT_WORDS[kind.format]}.`);
      return some({ type: "text", text });
    }
    case "number": {
      const raw = draft?.kind === "text" ? draft.text.trim() : "";
      if (raw === "") return none(field.required ? `${name} is required.` : null);
      const number = Number(raw);
      if (!Number.isFinite(number)) return none(`${name} must be a number.`);
      if (kind.integer && !Number.isInteger(number)) return none(`${name} must be a whole number.`);
      if (kind.minimum !== null && number < kind.minimum)
        return none(`${name} must be at least ${kind.minimum}.`);
      if (kind.maximum !== null && number > kind.maximum)
        return none(`${name} must be at most ${kind.maximum}.`);
      return some({ type: "number", number });
    }
    case "boolean": {
      const value = draft?.kind === "flag" ? draft.value : null;
      // A required checkbox left alone is a no; an optional one left alone says nothing.
      if (value === null) return field.required ? some({ type: "boolean", value: false }) : none();
      return some({ type: "boolean", value });
    }
    case "choice": {
      const selected = draft?.kind === "choices" ? [...draft.selected] : [];
      if (!kind.multiple) {
        if (selected.length === 0) return none(field.required ? `Choose one for ${name}.` : null);
        return some({ type: "choices", selected: selected.slice(0, 1) });
      }
      if (selected.length === 0 && !field.required && kind.min_items === null) return none();
      if (kind.min_items !== null && selected.length < kind.min_items)
        return none(`Choose at least ${kind.min_items} for ${name}.`);
      if (kind.max_items !== null && selected.length > kind.max_items)
        return none(`Choose at most ${kind.max_items} for ${name}.`);
      return some({ type: "choices", selected });
    }
  }
}

/** The submitted form, or each field's problem by id when it cannot be sent yet. */
export function formAnswer(
  fields: readonly UiField[],
  drafts: FormDrafts,
): { readonly answer: UiAnswer | null; readonly errors: ReadonlyMap<string, string> } {
  const values: UiFieldValue[] = [];
  const errors = new Map<string, string>();
  for (const field of fields) {
    const { value, error } = fieldAnswer(field, drafts.get(field.id));
    if (error !== null) errors.set(field.id, error);
    else if (value !== null) values.push(value);
  }
  return {
    answer: errors.size === 0 ? { type: "form", accept: true, values } : null,
    errors,
  };
}

/** A choice field after the person picks `index`: one choice replaces another, many toggle. */
export function pickChoice(
  field: UiField,
  draft: FieldDraft | undefined,
  index: number,
): FieldDraft {
  const selected = draft?.kind === "choices" ? draft.selected : [];
  if (field.kind.type !== "choice" || !field.kind.multiple)
    return { kind: "choices", selected: [index] };
  return {
    kind: "choices",
    selected: selected.includes(index)
      ? selected.filter((item) => item !== index)
      : [...selected, index],
  };
}
