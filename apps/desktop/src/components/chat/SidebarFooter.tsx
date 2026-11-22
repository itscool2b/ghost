import { Logo } from "@o7/ui/graphite";
import { Settings } from "lucide-react";
import type { ReactNode } from "react";
import { commandKey } from "./Keys";

/**
 * The foot of the sidebar, as calm as Grok's: the logo, the notifications bell and one Settings
 * button, and Update before them while a newer o7 is on offer. Profiles, agents and
 * diagnostics live in Settings and the command palette.
 */
export function SidebarFooter({
  onSettings,
  notifications,
  update,
  wingAway = false,
}: {
  readonly onSettings?: () => void;
  /** The Update pill (`AppUpdate`), which draws nothing while the app is current. */
  readonly update?: ReactNode;
  /** The notifications bell (`Notifications`), beside Settings. */
  readonly notifications?: ReactNode;
  /**
   * The tour's wing is out: it lifted off from this logo and draws itself until it lands here
   * again, so the logo keeps its place and is not drawn twice.
   */
  readonly wingAway?: boolean;
}) {
  return (
    <div className="chat-sidebar-footer" data-wing-away={wingAway ? "" : undefined}>
      <Logo size={30} className="chat-sidebar-footer-logo" />
      <div className="chat-sidebar-footer-actions">
        {update}
        {notifications}
        {onSettings ? (
          <button
            type="button"
            className="g-button g-button-ghost g-button-icon g-button-28 chat-sidebar-settings"
            aria-label="Open settings"
            aria-keyshortcuts="Control+, Meta+,"
            data-tip={`Settings (${commandKey()} ,)`}
            data-testid="sidebar-settings"
            data-tour="settings"
            onClick={onSettings}
          >
            <Settings size={16} aria-hidden="true" />
          </button>
        ) : null}
      </div>
    </div>
  );
}
