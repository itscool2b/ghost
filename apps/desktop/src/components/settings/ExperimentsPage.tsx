import { Workflow } from "lucide-react";
import { DEFAULT_GENERAL } from "../../lib/setup";
import type { Preferences } from "./preferences";
import { Row, Switch } from "./SettingsRow";

/**
 * Settings > Experiments (owner, 19 November 2026: "add it as an experimental feature so in
 * settings u can see experiment features if u enable the whole UI and functionality should
 * work"). Each experiment is one switch that applies at once: on, its whole interface and
 * behaviour are there; off, they are gone and what was made with it is kept.
 */
export function ExperimentsPage({ prefs }: { readonly prefs: Preferences }) {
  const general = prefs.shown?.general ?? DEFAULT_GENERAL;
  return (
    <>
      <p className="settings-lead">
        Features that work and are still finding their shape. Each is off until you turn it on, and
        turning one off keeps what you made with it.
      </p>
      <div className="settings-group">
        <Row
          title="Coordinated projects"
          mark={<Workflow size={16} />}
          detail="A project gets a coordinator: a chat that plans the work, hands it to worker chats with fresh contexts, and keeps a board, a decision log, shared Context files and routines. Open it from the board button beside a project's name."
        >
          <Switch
            label="Coordinated projects"
            testId="settings-coordinated-projects"
            checked={general.coordinated_projects}
            disabled={!prefs.saved}
            onChange={(enabled) => prefs.change({ type: "coordinated_projects", enabled })}
          />
        </Row>
      </div>
      <details className="settings-more">
        <summary>What a coordinator can and cannot do</summary>
        <p className="settings-note">
          The coordinator and its workers are ordinary chats on this computer, run by the agents you
          already use, so their turns are billed as any chat's. A worker starts in one of the
          project's own folders, in its own Git worktree where the folder is a repository, with the
          access level your new chats get; the coordinator cannot raise it, and a worker cannot
          start workers. Only you pause, resume or change the brief, and only you and the
          coordinator record decisions. At most 4 workers work at once, and Ghost pauses a project
          whose coordinator was sent 60 reports and routines in an hour. Routines run only while
          Ghost is open. Devices and bots you paired see these chats as the chats they are, and get
          no project tools.
        </p>
      </details>
    </>
  );
}
