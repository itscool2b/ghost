import { useEffect, useRef, useState } from "react";

export interface CopyButtonProps {
  readonly text: string;
  /** Accessible name of the button. Default "Copy". */
  readonly label?: string;
  /** Visible caption at rest. Default "Copy". */
  readonly caption?: string;
}

/**
 * Copies `text` and says so only after the platform accepts the write; a refusal says what to do
 * instead. Styled by the `g-copy*` classes in graphite.css. The `o-copy*` classes are kept so Orbit
 * screens that import this through `@ghost/ui/orbit` keep their look until they move to Graphite.
 */
export function CopyButton({ text, label = "Copy", caption = "Copy" }: CopyButtonProps) {
  const [state, setState] = useState<"ready" | "copying" | "copied" | "failed">("ready");
  const mounted = useRef(true);
  const reset = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      if (reset.current !== null) clearTimeout(reset.current);
    };
  }, []);
  const copy = async () => {
    if (reset.current !== null) clearTimeout(reset.current);
    setState("copying");
    try {
      await navigator.clipboard.writeText(text);
      if (!mounted.current) return;
      setState("copied");
      reset.current = setTimeout(() => setState("ready"), 1200);
    } catch {
      if (mounted.current) setState("failed");
    }
  };
  return (
    <span className="g-copy-control o-copy-control">
      <button
        type="button"
        className="g-copy o-copy"
        aria-label={label}
        data-state={state}
        disabled={state === "copying"}
        onClick={() => void copy()}
      >
        <svg
          width="12"
          height="12"
          viewBox="0 0 12 12"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.25"
          aria-hidden="true"
        >
          {state === "copied" ? (
            <path d="m2 6.5 2.5 2.5L10 3.5" strokeLinecap="square" />
          ) : (
            <>
              <rect x="4.5" y="4.5" width="5.5" height="5.5" rx="1" />
              <path d="M2.5 7.5V3a.5.5 0 0 1 .5-.5h4.5" />
            </>
          )}
        </svg>
        <span aria-live="polite">{state === "copied" ? "Copied" : caption}</span>
      </button>
      {state === "failed" ? (
        <span role="alert" className="g-copy-error o-copy-error">
          Could not copy. Select the text and copy it instead.
        </span>
      ) : null}
    </span>
  );
}
