# Plan and build: models by role, in each chat

Owner, 19 November 2026: "sometimes people wanna use different models for different things like fable for planning, gpt 6 astra for implementation or mixing and matching. Create the best way for that." The same day, after it shipped in Settings: "i was thinking that would be a chat by chat thing not a settings thing ... take the functionality out of the settings and add it to each chat."

Each chat names its own Plan with and Build with, in the last part of its model menu (the composer's model pill): **Plan and build**. Its line says what is chosen ("Plan: Fable 5.2 · High, Build: Codex · GPT-6 Astra") or "Same model throughout". Nothing is chosen in a new chat, and a role that is not chosen changes nothing. Settings has no section for it any more; a profile that saved Settings roles on 19 November opens as before, and those choices are not carried into chats.

| Role | What it names | When it applies | What happens |
|---|---|---|---|
| Plan with | A model and thinking level of the chat's own agent | Plan is turned on in the chat, or it is chosen while the chat already plans | The chat's model and thinking level change to the role's. The picker shows the change as any other. If the chat has since moved to another agent, the old choice is left alone. |
| Build with | Any ready agent, and optionally its model and thinking level | A plan is waiting for review | The plan card offers "Accept and build with <role>" before Accept plan. Accept plan still lets the chat's own agent implement it. |

A coordinated project's coordinator is an ordinary chat, so it has both too: the workers it starts run on its Build with unless it names another agent or model for a task. The coordinator itself starts on the agent the person chose for it, else the project's default agent.

## Accept and build with

- **The chat's own agent** (plan with one model, build with another of the same agent): the model and thinking level are set, then the plan is accepted. Nothing else changes. The offer shows only when the role would change something.
- **Another agent** (plan with Claude Code, build with Codex): the planner is stopped, which closes its request; the chat is handed to the builder with the same handoff the agent picker's switch uses; the builder's model and thinking level are set; and it is sent the plan as written, with a line saying who wrote it and that the person accepted it. The chat keeps its history, and each turn keeps the agent that ran it.

The offer needs the role's agent to be ready on this computer. If it stops being ready between the card showing and the press, the card says so and Accept plan remains.

## Where it is saved and read

- The `chat_roles` table (migration `0020_chat_roles.sql`): one row a chat and role, only for a role that was chosen, removed with the chat. It stays on this device. A fork takes its source chat's rows.
- `WorkspaceChat.roles` carries them to the window with the rest of the chat's organization; `chat_set_role(chat_id, role, choice)` sets or clears one (`crates/ghost-core/src/session/workspace.rs`). An agent is saved as the agent, not a connection of it, as the default agent is. The agent (128 bytes), model (256) and thinking level (128) must be non-empty and free of control characters; anything else is refused and nothing changes. Nothing runs when a role is saved.
- The window applies them (`apps/desktop/src/lib/roles.ts`, `ChatView.tsx`, `RolePicker.tsx`) with only what the pickers and the agent switch already do: `chat_set_option`, `respond_permission`, `chat_stop`, `continuation_preview` and `continuation_apply`, `composer_send`. Starting a worker on another agent goes through the coordinator's own checks, which refuse an agent that is not ready.
- Rust reads the coordinator chat's Build with when it starts a worker (`crates/ghost-core/src/session/coordinator.rs`).

## Limits

- A model an agent has not listed yet (it has never run here) cannot be chosen; the row says the agent lists its models once one of its chats has started.
- Thinking levels are listed per model once the agent has run that model.
- A role changes what a chat runs on. It does not change access: a builder runs with the chat's access level.
- New chat has no Plan and build part: it appears once the chat exists.

## Verification

- `cargo test -p ghost-core --lib chatstore::organization` and `session::tests::workspace` (saved per chat, replaced not duplicated, cleared, refused when not an identifier or for a missing or deleted chat, a connection kept as its agent, the chat's place in the list unchanged) and `chatstore::conversation` (a fork keeps them).
- `cargo test -p ghost-core --lib onboarding` (a profile saved with Settings roles still opens).
- `pnpm --filter @ghost/desktop exec vitest run src/lib/roles.test.ts src/components/chat/PlanCard.test.tsx`.
- `node apps/desktop/scripts/check-chat.mjs` (`planAndBuild`: choosing, applying on Plan, clearing, and Accept and build with on the chat's own agent).
