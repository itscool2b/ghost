import { Component, type ReactNode } from "react";
import { inTauri, windowReady } from "../lib/ipc";
import { whenPainted } from "../lib/ready";
import { ErrorScreen } from "./ErrorScreen";

interface State {
  readonly error: Error | null;
}

/**
 * Last line of defence for the window's contents: a render error shows a calm error screen
 * instead of an empty transparent window (the shell still shows the window after its fallback).
 */
export class ErrorBoundary extends Component<{ readonly children: ReactNode }, State> {
  override state: State = { error: null };

  static getDerivedStateFromError(error: unknown): State {
    return { error: error instanceof Error ? error : new Error(String(error)) };
  }

  override componentDidCatch(error: unknown): void {
    console.error("Ghost: the interface failed to render", error);
    // The app may have crashed before reporting its first paint: show the window now rather than
    // after the shell's fallback delay. Idempotent in the shell.
    if (inTauri()) {
      whenPainted(windowReady).catch((reason: unknown) =>
        console.error("Ghost: window_ready failed", reason),
      );
    }
  }

  override render(): ReactNode {
    if (this.state.error) {
      return (
        <ErrorScreen
          title="Ghost's window ran into a problem"
          message={this.state.error.message || "The interface failed to render."}
          dataDir={null}
          os=""
        />
      );
    }
    return this.props.children;
  }
}
