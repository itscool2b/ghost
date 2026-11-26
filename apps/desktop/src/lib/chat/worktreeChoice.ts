import type { WorktreeChatPlan, WorktreeChatPreview, WorktreeChatResult } from "../../bindings";
import { errorMessage } from "../errors";
import type { WorktreeChatApi } from "../worktreeChat";

/** Where a new chat works: a new worktree of the project folder, or the folder itself. */
export type FolderMode = "worktree" | "local";

/** What the person can change about a new worktree chat. The defaults need no change at all. */
export interface WorktreeOptions {
  readonly mode: FolderMode;
  /** A local branch to start from; `null` starts from the folder's current branch. */
  readonly startBranch: string | null;
  /** The branch name the person typed; `null` keeps the generated `ghost/...` name. */
  readonly branch: string | null;
  readonly bringChanges: boolean;
  readonly runSetup: boolean;
}

export const DEFAULT_WORKTREE_OPTIONS: WorktreeOptions = {
  mode: "worktree",
  startBranch: null,
  branch: null,
  bringChanges: false,
  runSetup: true,
};

/** The control a refused change came from, so its reason shows beside it. */
export type WorktreeField = "mode" | "start" | "branch" | "bring" | "setup";

export interface WorktreeChoiceView {
  /** No folder yet, the first review on its way, a Git project folder, or a plain folder. */
  readonly kind: "none" | "checking" | "git" | "plain";
  /** The options as shown, including a change still being reviewed. */
  readonly options: WorktreeOptions;
  /** The latest reviewed plan. Create applies it (or a fresh review of the same options). */
  readonly plan: WorktreeChatPlan | null;
  /** Why a plain folder is used directly. */
  readonly plain: string | null;
  /** A change is being reviewed. */
  readonly pending: boolean;
  /** A change Rust refused, beside the control it came from. The options went back. */
  readonly error: { readonly field: WorktreeField; readonly message: string } | null;
  /** Why this Git folder cannot have a worktree right now, so the chat works in the folder. */
  readonly unavailable: string | null;
  /** The folder has a saved worktree setup, so its switch is offered. */
  readonly setupSaved: boolean;
  readonly creating: boolean;
}

/** What Create does with the reviewed choice. */
export type WorktreeResolution =
  | { readonly type: "worktree"; readonly plan: WorktreeChatPlan }
  | { readonly type: "local" }
  /** A review on its way changed what Create would do; the view says why. Nothing is created. */
  | { readonly type: "stop" };

const EMPTY: WorktreeChoiceView = {
  kind: "none",
  options: DEFAULT_WORKTREE_OPTIONS,
  plan: null,
  plain: null,
  pending: false,
  error: null,
  unavailable: null,
  setupSaved: false,
  creating: false,
};

/** Changes settle for this long before Rust reviews them; typing a branch name is applied whole. */
export const REVIEW_DELAY_MS = 200;
/** A plan this close to its expiry is reviewed again before Create applies it. */
export const EXPIRY_MARGIN_MS = 30_000;

type Queued = { options: WorktreeOptions; field: WorktreeField | null; generation: number };

/**
 * The new chat's folder choice, reviewed by Rust as the person changes it. Answers for an older
 * folder, agent or option are dropped; a refused change goes back to the last reviewed options with
 * its reason. Reviews are cached by their options until shortly before they expire, so switching
 * back and forth does not pile up open previews.
 */
export class WorktreeChoice {
  readonly #api: WorktreeChatApi;
  readonly #listeners = new Set<() => void>();
  #view: WorktreeChoiceView = EMPTY;
  #folderId: string | null = null;
  #agentId: string | null = null;
  /** The last options that were reviewed (worktree) or chosen (local) without an error. */
  #settled: WorktreeOptions = DEFAULT_WORKTREE_OPTIONS;
  /** The first generated name, kept so the branch does not change name with every review. */
  #generated: string | null = null;
  readonly #cache = new Map<string, WorktreeChatPlan>();
  #generation = 0;
  #timer: ReturnType<typeof setTimeout> | null = null;
  #queued: Queued | null = null;
  #inflight: Promise<void> | null = null;

  constructor(api: WorktreeChatApi) {
    this.#api = api;
  }

  readonly subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  };

  readonly getSnapshot = (): WorktreeChoiceView => this.#view;

  /** The project folder and agent the chat would use. A new folder starts from the defaults. */
  target(folderId: string | null, agentId: string | null): void {
    if (folderId === this.#folderId && agentId === this.#agentId) return;
    const folderChanged = folderId !== this.#folderId;
    this.#folderId = folderId;
    this.#agentId = agentId;
    this.#cancel();
    this.#generation++;
    if (folderChanged) {
      this.#generated = null;
      this.#cache.clear();
      this.#settled = DEFAULT_WORKTREE_OPTIONS;
    }
    if (!folderId || !agentId) {
      this.#publish(EMPTY);
      return;
    }
    if (folderChanged || this.#view.kind === "none") {
      this.#publish({ ...EMPTY, kind: "checking" });
      this.#queue(DEFAULT_WORKTREE_OPTIONS, null, REVIEW_DELAY_MS);
      return;
    }
    // Only the agent changed: the folder's facts stay, and a worktree plan is reviewed for it.
    if (this.#view.kind === "plain" || this.#view.options.mode === "local") {
      this.#publish({ ...this.#view, error: null });
      return;
    }
    this.#publish({ ...this.#view, pending: true, error: null });
    this.#queue(this.#view.options, null, REVIEW_DELAY_MS);
  }

  /** One change from the chip's popover. Local applies at once; worktree changes are reviewed. */
  propose(change: Partial<WorktreeOptions>, field: WorktreeField): void {
    const view = this.#view;
    if (!this.#folderId || !this.#agentId || view.creating || view.kind === "plain") return;
    if (view.kind === "none" || view.kind === "checking") return;
    let options: WorktreeOptions = { ...view.options, ...change };
    // Changes can only come along from the folder's current branch.
    if (field === "start" && options.bringChanges && !startsAtCurrent(view, options.startBranch))
      options = { ...options, bringChanges: false };
    if (options.mode === "local") {
      this.#cancel();
      this.#generation++;
      this.#settled = options;
      this.#publish({ ...view, options, pending: false, error: null });
      return;
    }
    this.#publish({ ...view, options, pending: true, error: null });
    this.#queue(options, field, field === "branch" ? 0 : REVIEW_DELAY_MS);
  }

  /** Clears a refused change's reason, as editing its field again does. */
  clearError(): void {
    if (this.#view.error) this.#publish({ ...this.#view, error: null });
  }

  /**
   * Waits for every review on its way and returns what Create should do. A plan close to expiry is
   * reviewed again first, so a stale token is never applied.
   */
  async resolve(): Promise<WorktreeResolution> {
    const before = this.#view;
    await this.#drain();
    let view = this.#view;
    // A refused change is shown beside its control; Create never applies other options instead.
    if (view.error !== null) return { type: "stop" };
    if (view.unavailable !== null && view.unavailable !== before.unavailable)
      return { type: "stop" };
    if (view.kind === "none" || view.kind === "checking") return { type: "stop" };
    if (view.kind === "plain" || view.options.mode === "local") return { type: "local" };
    if (!view.plan) return { type: "stop" };
    if (this.#fresh(view.plan)) return { type: "worktree", plan: view.plan };
    this.#cache.clear();
    this.#publish({ ...view, pending: true });
    this.#queue(view.options, null, 0);
    await this.#drain();
    view = this.#view;
    return view.kind === "git" && view.options.mode === "worktree" && view.plan
      ? { type: "worktree", plan: view.plan }
      : { type: "stop" };
  }

  /** Applies a reviewed plan. A failure reviews the same options again for the next Create. */
  async create(plan: WorktreeChatPlan): Promise<WorktreeChatResult> {
    this.#publish({ ...this.#view, creating: true, error: null });
    try {
      const result = await this.#api.apply(plan.token);
      this.#cache.clear();
      return result;
    } catch (error) {
      // The token may be spent or expired either way; only a fresh review is applied next.
      this.#cache.clear();
      this.#publish({ ...this.#view, pending: true });
      this.#queue(this.#settled, null, 0);
      throw error;
    } finally {
      this.#publish({ ...this.#view, creating: false });
    }
  }

  dispose(): void {
    this.#cancel();
    this.#generation++;
    this.#listeners.clear();
  }

  #fresh(plan: WorktreeChatPlan): boolean {
    return plan.expires_at - Date.now() > EXPIRY_MARGIN_MS;
  }

  #publish(view: WorktreeChoiceView): void {
    this.#view = view;
    for (const listener of this.#listeners) listener();
  }

  #cancel(): void {
    if (this.#timer !== null) clearTimeout(this.#timer);
    this.#timer = null;
    this.#queued = null;
  }

  #queue(options: WorktreeOptions, field: WorktreeField | null, delay: number): void {
    this.#cancel();
    this.#queued = { options, field, generation: ++this.#generation };
    this.#timer = setTimeout(() => {
      this.#timer = null;
      void this.#flush();
    }, delay);
  }

  #flush(): Promise<void> {
    const queued = this.#queued;
    this.#queued = null;
    if (!queued) return this.#inflight ?? Promise.resolve();
    const run: Promise<void> = this.#review(queued).finally(() => {
      if (this.#inflight === run) this.#inflight = null;
    });
    this.#inflight = run;
    return run;
  }

  /** Runs a queued review now and waits until no review is on its way. */
  async #drain(): Promise<void> {
    for (;;) {
      if (this.#timer !== null) {
        clearTimeout(this.#timer);
        this.#timer = null;
        void this.#flush();
      }
      const inflight = this.#inflight;
      if (!inflight) return;
      await inflight;
    }
  }

  #key(folderId: string, agentId: string, options: WorktreeOptions, branch: string | null) {
    return JSON.stringify([
      folderId,
      agentId,
      options.startBranch,
      branch,
      options.bringChanges,
      options.runSetup,
    ]);
  }

  async #review({ options, field, generation }: Queued): Promise<void> {
    const folderId = this.#folderId;
    const agentId = this.#agentId;
    if (!folderId || !agentId) return;
    const branch = options.branch ?? this.#generated;
    const key = this.#key(folderId, agentId, options, branch);
    const cached = this.#cache.get(key);
    let preview: WorktreeChatPreview;
    if (cached && this.#fresh(cached)) preview = { type: "worktree", plan: cached };
    else {
      try {
        preview = await this.#api.preview({
          folder_id: folderId,
          agent_id: agentId,
          start_branch: options.startBranch,
          branch,
          title: null,
          bring_changes: options.bringChanges,
          run_setup: options.runSetup,
        });
      } catch (error) {
        if (generation !== this.#generation) return;
        // A kept generated name that a new branch has taken since: Rust names it afresh.
        if (options.branch === null && this.#generated !== null) {
          this.#generated = null;
          return this.#review({ options, field, generation });
        }
        this.#fail(field, errorMessage(error));
        return;
      }
      if (preview.type === "worktree") this.#cache.set(key, preview.plan);
    }
    if (generation !== this.#generation) return;
    if (preview.type === "not_git_repository") {
      this.#settled = DEFAULT_WORKTREE_OPTIONS;
      this.#publish({ ...EMPTY, kind: "plain", plain: preview.message });
      return;
    }
    const plan = preview.plan;
    if (options.branch === null && plan.branch_generated) {
      this.#generated = plan.branch;
      // The same review, under the name it is asked with from now on.
      this.#cache.set(this.#key(folderId, agentId, options, plan.branch), plan);
    }
    this.#settled = options;
    this.#publish({
      kind: "git",
      options,
      plan,
      plain: null,
      pending: false,
      error: null,
      unavailable: null,
      setupSaved: this.#view.setupSaved || plan.setup !== null,
      creating: this.#view.creating,
    });
  }

  #fail(field: WorktreeField | null, message: string): void {
    const view = this.#view;
    if (field === null || view.plan === null) {
      // The folder cannot have a worktree with these options: the chat works in the folder.
      if (field === "mode") {
        this.#publish({
          ...view,
          options: this.#settled,
          pending: false,
          error: { field, message },
        });
        return;
      }
      this.#settled = { ...DEFAULT_WORKTREE_OPTIONS, mode: "local" };
      this.#publish({
        ...view,
        kind: "git",
        options: this.#settled,
        plan: null,
        pending: false,
        error: null,
        unavailable: message,
      });
      return;
    }
    this.#publish({ ...view, options: this.#settled, pending: false, error: { field, message } });
  }
}

/** True when `start` is the folder's current branch, the only start that can bring changes. */
export function startsAtCurrent(view: WorktreeChoiceView, start: string | null): boolean {
  const current = view.plan?.source.branch ?? null;
  return start === null || start === current;
}

/** The branch a worktree chat starts from, as shown: the chosen one or the folder's current one. */
export function startingBranch(view: WorktreeChoiceView): string | null {
  return view.options.startBranch ?? view.plan?.starting_branch ?? view.plan?.source.branch ?? null;
}
