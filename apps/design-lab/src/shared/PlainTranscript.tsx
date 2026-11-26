import "./plain.css";
import { AuraField, brandCssVariables, getBrand, Orb } from "@ghost/ui";
import { type CSSProperties, useLayoutEffect, useRef } from "react";
import type { DirectionProps } from "../directions/types";
import type { ChatStatus, PlanStatus } from "../fixture/types";
import {
  AUTONOMY_LABELS,
  composerLabel,
  findAgent,
  findChat,
  findModel,
  findProject,
  modelLabel,
} from "../fixture/workspace";
import { stripAnsi } from "../model/ansi";
import { formatCost, formatCount, formatDuration, formatElapsed, plural } from "../model/format";
import type { ThoughtItem, ToolRunItem, Turn, TurnItem } from "../model/types";
import { type Player, usePlayhead } from "../model/usePlayer";

/**
 * Every field of the model and workspace as plain text: the placeholder each direction starts
 * from, and a reference for what a finished direction must show. Deliberately undesigned.
 */
export function PlainTranscript({
  name,
  accent,
  state,
  model,
  workspace,
  player,
}: DirectionProps & { readonly name: string; readonly accent: string }) {
  const chat = findChat(workspace, workspace.activeChatId);
  const agentId = chat?.agent ?? workspace.composer.agent;
  const brand = getBrand(agentId);
  const project = findProject(workspace, workspace.activeProjectId);
  const scrollRef = useRef<HTMLDivElement>(null);

  // Keep the newest output in view, as a chat does while it streams.
  useLayoutEffect(() => {
    const element = scrollRef.current;
    if (element && model.eventCount >= 0) element.scrollTop = element.scrollHeight;
  }, [model]);

  return (
    <div
      className="lab-plain"
      data-state={state}
      style={{ ...brandCssVariables(brand), "--plain-accent": accent } as CSSProperties}
    >
      <aside className="lab-plain-sidebar">
        <p className="lab-plain-badge">{name} placeholder</p>
        <p className="lab-plain-label">Projects</p>
        {workspace.projects.map((item) => (
          <div key={item.id} className="lab-plain-project">
            <p className="lab-plain-project-name">
              <span className="lab-plain-dot" style={{ background: item.color }} />
              {item.name}
            </p>
            {item.expanded
              ? item.chats.map((row) => (
                  <p
                    key={row.id}
                    className="lab-plain-chat"
                    data-active={row.id === workspace.activeChatId ? "" : undefined}
                  >
                    <span className="lab-plain-chat-title">{row.title}</span>
                    <span className="lab-plain-meta">
                      {getBrand(row.agent).name} · {STATUS_TEXT[row.status]} · {row.updated}
                    </span>
                  </p>
                ))
              : null}
          </div>
        ))}
        <p className="lab-plain-label">Agents</p>
        {workspace.agents.map((agent) => (
          <p
            key={agent.id}
            className="lab-plain-agent"
            data-missing={agent.installed ? undefined : ""}
          >
            {agent.name}
            <span className="lab-plain-meta">
              {agent.installed
                ? `${agent.version}${agent.update ? `, update ${agent.update}` : ""}`
                : "not installed"}
            </span>
          </p>
        ))}
        <p className="lab-plain-account">{workspace.account.label} profile</p>
      </aside>

      <main className="lab-plain-main">
        <AuraField brand={brand} intensity={model.aura} />
        <header className="lab-plain-header">
          <Orb brand={brand} state={model.activity} size={22} />
          <span>{chat?.title ?? `New chat in ${project?.name ?? "a project"}`}</span>
          <span className="lab-plain-meta">
            {model.activity} ·{" "}
            {model.session.model
              ? modelLabel(workspace, agentId, model.session.model)
              : "no session yet"}
          </span>
        </header>

        <div ref={scrollRef} className="lab-plain-scroll">
          <div className="lab-plain-column">
            {model.turns.length === 0 ? (
              <div className="lab-plain-hero">
                <Orb brand={brand} state="idle" size={96} />
                <p className="lab-plain-hero-title">What should we build in {project?.name}?</p>
                <p className="lab-plain-meta">{composerLabel(workspace)}</p>
              </div>
            ) : (
              model.turns.map((turn) => <TurnView key={turn.id} turn={turn} player={player} />)
            )}
          </div>
        </div>

        <footer className="lab-plain-bottom">
          {model.plan ? (
            <div className="lab-plain-plan">
              <p className="lab-plain-meta">
                Plan {model.plan.completed}/{model.plan.total}
              </p>
              {model.plan.entries.map((entry) => (
                <p key={entry.content} data-status={entry.status}>
                  {PLAN_GLYPH[entry.status]} {entry.content}
                </p>
              ))}
            </div>
          ) : null}
          {workspace.composer.pickerOpen ? <PickerView {...{ workspace }} /> : null}
          <div className="lab-plain-composer">
            <span className="lab-plain-placeholder">
              {workspace.composer.draft ||
                `Message ${findAgent(workspace, workspace.composer.agent)?.name ?? "the agent"}`}
            </span>
            <span className="lab-plain-pill">{composerLabel(workspace)}</span>
            <span className="lab-plain-meta">{AUTONOMY_LABELS[workspace.composer.autonomy]}</span>
          </div>
        </footer>
      </main>
    </div>
  );
}

const STATUS_TEXT: Readonly<Record<ChatStatus, string>> = {
  running: "running",
  needs_approval: "needs approval",
  done_unseen: "done, unseen",
  error: "error",
  idle: "idle",
};

const PLAN_GLYPH: Readonly<Record<PlanStatus, string>> = {
  completed: "[x]",
  in_progress: "[~]",
  pending: "[ ]",
};

function TurnView({ turn, player }: { readonly turn: Turn; readonly player: Player }) {
  return (
    <section className="lab-plain-turn">
      {turn.user ? <p className="lab-plain-user">{turn.user.text}</p> : null}
      {turn.items.map((item) => (
        <ItemView key={`${item.kind}:${item.id}`} item={item} player={player} />
      ))}
      {turn.footer ? (
        <p className="lab-plain-meta lab-plain-footer">
          {[
            formatDuration(turn.footer.durationMs),
            turn.footer.usage ? `${formatCount(turn.footer.usage.input_tokens)} in` : null,
            turn.footer.usage ? `${formatCount(turn.footer.usage.output_tokens)} out` : null,
            turn.footer.usage?.cached_input_tokens
              ? `${formatCount(turn.footer.usage.cached_input_tokens)} cached`
              : null,
            turn.footer.cost
              ? formatCost(turn.footer.cost.amount, turn.footer.cost.currency)
              : null,
            `${plural(turn.footer.files.length, "file")} changed`,
          ]
            .filter(Boolean)
            .join(" · ")}
        </p>
      ) : null}
    </section>
  );
}

function ThoughtView({ item, player }: { readonly item: ThoughtItem; readonly player: Player }) {
  // A live counter re-renders once a second, not every frame.
  const now = usePlayhead(player, 1000);
  const label =
    item.durationMs === null
      ? `Thinking ${formatElapsed(now - item.startedAt)}`
      : `Thought for ${formatDuration(item.durationMs)}`;
  return (
    <div className="lab-plain-thought">
      <p className="lab-plain-meta">{label}</p>
      <p className="lab-plain-dim">{item.text}</p>
    </div>
  );
}

function RunView({ item }: { readonly item: ToolRunItem }) {
  return (
    <div className="lab-plain-run" data-status={item.status}>
      <p>
        {item.label}
        {item.added + item.removed > 0 ? (
          <span className="lab-plain-meta">
            {" "}
            +{item.added} -{item.removed}
          </span>
        ) : null}
        <span className="lab-plain-meta"> · {item.status}</span>
      </p>
      {item.calls.map((call) => (
        <div key={call.id} className="lab-plain-call">
          <p className="lab-plain-meta">
            {call.label} ({call.status})
          </p>
          {call.search
            ? call.search.matches.map((match) => (
                <p key={`${match.path}:${match.line}`} className="lab-plain-code">
                  {match.path}:{match.line} {match.text.trim()}
                </p>
              ))
            : null}
          {call.diffs.map((diff) => (
            <p key={diff.path} className="lab-plain-code">
              {diff.path} +{diff.added} -{diff.removed} ({plural(diff.hunks.length, "hunk")})
            </p>
          ))}
          {call.terminal ? (
            <pre className="lab-plain-code">
              {stripAnsi(call.terminal.output).trim().split("\n").slice(-6).join("\n")}
              {call.terminal.exitCode === null ? "" : `\nexit ${call.terminal.exitCode}`}
            </pre>
          ) : null}
        </div>
      ))}
    </div>
  );
}

function ItemView({ item, player }: { readonly item: TurnItem; readonly player: Player }) {
  switch (item.kind) {
    case "thought":
      return <ThoughtView item={item} player={player} />;
    case "message":
      return (
        <p className="lab-plain-message" data-streaming={item.streaming ? "" : undefined}>
          {item.text}
        </p>
      );
    case "tools":
      return <RunView item={item} />;
    case "permission":
      return (
        <div className="lab-plain-permission" data-status={item.permission.status}>
          <p>{item.permission.title}</p>
          <p>
            {item.permission.options.map((option) => (
              <button key={option.option_id} type="button" disabled className="lab-plain-option">
                {option.label}
              </button>
            ))}
          </p>
        </div>
      );
  }
}

function PickerView({ workspace }: { readonly workspace: DirectionProps["workspace"] }) {
  const { composer } = workspace;
  const agent = findAgent(workspace, composer.agent);
  const model = findModel(agent, composer.model);
  return (
    <div className="lab-plain-picker">
      <div>
        <p className="lab-plain-meta">Agent</p>
        {workspace.agents.map((item) => (
          <p key={item.id} data-selected={item.id === composer.agent ? "" : undefined}>
            {item.name}
            {item.installed ? "" : " (install)"}
          </p>
        ))}
      </div>
      <div>
        <p className="lab-plain-meta">Model</p>
        {agent?.models.map((item) => (
          <p key={item.id} data-selected={item.id === composer.model ? "" : undefined}>
            {item.label} <span className="lab-plain-meta">{item.description}</span>
          </p>
        ))}
      </div>
      <div>
        <p className="lab-plain-meta">Effort</p>
        {model?.efforts.map((item) => (
          <p key={item.id} data-selected={item.id === composer.effort ? "" : undefined}>
            {item.label}
          </p>
        ))}
      </div>
    </div>
  );
}
