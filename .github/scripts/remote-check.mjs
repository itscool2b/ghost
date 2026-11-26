// Runs a branch's checks on GitHub's machines (.github/workflows/check.yml) instead of this PC,
// waits for them, prints each step's result and the failed steps' logs, and reports the month's
// GitHub bill so far, which the owner asked to hear every time a build runs (24 November 2026).
//
//   node .github/scripts/remote-check.mjs [options]
//     --os linux|windows|macos   the system to check on (linux)
//     --tests "<args>"|none      arguments for cargo test (--workspace)
//     --no-clippy                skip rustfmt and Clippy
//     --no-frontend              skip pnpm lint, typecheck and the TypeScript tests
//     --smoke stream|chat|all    smoke runs of the real app (none)
//     --browser "<checks>"       browser checks separated by ";" (none)
//     --ref <branch>             check that branch as GitHub has it (main, say) instead of this one
//     --watch <run id>           wait for a run already started instead of starting one
//     --no-wait                  start the run and return
//
// Without --ref, this checkout's branch must be pushed: the run checks out what GitHub has. A
// worktree cannot check out main, so `--ref main` refreshes main's build cache. Needs `gh`; the bill needs its
// `user` scope (`gh auth refresh -h github.com -s user`), and without it the bill is reported as
// unreadable, never guessed.
import { execFileSync } from "node:child_process";
import { pathToFileURL } from "node:url";
import { parseArgs } from "node:util";

const WORKFLOW = "check.yml";
const SYSTEMS = ["linux", "windows", "macos"];
const POLL_MS = 20_000;

const VALUED = new Set(["--os", "--tests", "--smoke", "--browser", "--ref", "--watch"]);
// A branch name git accepts and a shell or a URL cannot misread.
const BRANCH = /^(?!-)(?!.*\.\.)[A-Za-z0-9._/-]+$/u;

/** The run's settings from the command line. Throws on anything it does not know. */
export function parseOptions(argv) {
  // An option's value may itself start with "-" (--tests "-p ghost-core"): join it to its option,
  // which parseArgs would otherwise read as a second option.
  const joined = [];
  for (let index = 0; index < argv.length; index += 1) {
    if (VALUED.has(argv[index]) && index + 1 < argv.length) {
      joined.push(`${argv[index]}=${argv[index + 1]}`);
      index += 1;
    } else {
      joined.push(argv[index]);
    }
  }
  const { values, positionals } = parseArgs({
    args: joined,
    allowPositionals: true,
    options: {
      os: { type: "string", default: "linux" },
      tests: { type: "string", default: "--workspace" },
      "no-clippy": { type: "boolean", default: false },
      "no-frontend": { type: "boolean", default: false },
      smoke: { type: "string", default: "none" },
      browser: { type: "string", default: "none" },
      ref: { type: "string" },
      watch: { type: "string" },
      "no-wait": { type: "boolean", default: false },
    },
  });
  if (values.ref !== undefined && !BRANCH.test(values.ref)) {
    throw new Error("--ref takes a branch name");
  }
  if (positionals.length > 0) throw new Error(`Unexpected argument: ${positionals[0]}`);
  if (!SYSTEMS.includes(values.os)) throw new Error(`--os must be one of ${SYSTEMS.join(", ")}`);
  const smoke = values.smoke.trim() || "none";
  for (const scenario of smoke.split(/\s+/u)) {
    if (!["none", "stream", "chat", "all"].includes(scenario)) {
      throw new Error(`--smoke takes stream, chat, all or none, not ${scenario}`);
    }
  }
  if (values.watch !== undefined && !/^\d+$/u.test(values.watch)) {
    throw new Error("--watch takes a run id");
  }
  return {
    os: values.os,
    tests: values.tests.trim() || "none",
    clippy: !values["no-clippy"],
    frontend: !values["no-frontend"],
    smoke,
    browser: values.browser.trim() || "none",
    ref: values.ref,
    watch: values.watch,
    wait: !values["no-wait"],
  };
}

/** The `gh workflow run` fields for the options. */
export function dispatchFields(options) {
  return [
    ["os", options.os],
    ["tests", options.tests],
    ["clippy", String(options.clippy)],
    ["frontend", String(options.frontend)],
    ["smoke", options.smoke],
    ["browser", options.browser],
  ].flatMap(([name, value]) => ["-f", `${name}=${value}`]);
}

const LABELS = {
  actions_macos: "Mac",
  actions_windows: "Windows",
  actions_linux: "Linux",
  actions_storage: "storage",
};

/** The month's net amount and its parts from a billing usage summary, largest first. */
export function summarizeBill(summary) {
  const parts = new Map();
  for (const item of summary?.usageItems ?? []) {
    const amount = Number(item.netAmount) || 0;
    const label = LABELS[item.sku] ?? `${item.product} ${item.sku}`.trim();
    parts.set(label, (parts.get(label) ?? 0) + amount);
  }
  const total = [...parts.values()].reduce((sum, amount) => sum + amount, 0);
  return {
    total,
    parts: [...parts]
      .filter(([, amount]) => amount >= 0.005)
      .sort((a, b) => b[1] - a[1])
      .map(([label, amount]) => ({ label, amount })),
  };
}

/** One line a person reads: "$94.61 this month so far (Mac $75.58, Windows $16.96)". */
export function formatBill(bill) {
  const money = (amount) => `$${amount.toFixed(2)}`;
  const parts = bill.parts.map(({ label, amount }) => `${label} ${money(amount)}`).join(", ");
  return `${money(bill.total)} this month so far${parts ? ` (${parts})` : ""}`;
}

function gh(args, options = {}) {
  return execFileSync("gh", args, {
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
    stdio: ["ignore", "pipe", options.quiet ? "ignore" : "pipe"],
  });
}

function git(args) {
  return execFileSync("git", args, { encoding: "utf8" }).trim();
}

/** The bill line, or why it cannot be read. */
function billLine() {
  try {
    const repo = JSON.parse(gh(["repo", "view", "--json", "owner"]));
    const owner = repo.owner.login;
    const type = gh(["api", `users/${owner}`, "--jq", ".type"]).trim();
    const path =
      type === "Organization"
        ? `organizations/${owner}/settings/billing/usage/summary`
        : `users/${owner}/settings/billing/usage/summary`;
    return `GitHub bill: ${formatBill(summarizeBill(JSON.parse(gh(["api", path], { quiet: true }))))}.`;
  } catch {
    return "GitHub bill: unreadable here. It needs gh's user scope (gh auth refresh -h github.com -s user); github.com/settings/billing shows it.";
  }
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** Starts the run for the pushed branch (or --ref's) and returns its id. */
async function start(options) {
  const branch = options.ref ?? git(["rev-parse", "--abbrev-ref", "HEAD"]);
  if (branch === "HEAD") {
    throw new Error(
      "Check out a branch first, or name one with --ref: the run checks out a branch.",
    );
  }
  const pushed = git(["ls-remote", "origin", `refs/heads/${branch}`]).split(/\s+/u)[0];
  if (options.ref !== undefined && !pushed) throw new Error(`GitHub has no branch ${branch}.`);
  const head = options.ref === undefined ? git(["rev-parse", "HEAD"]) : pushed;
  if (pushed !== head) {
    throw new Error(
      `Push ${branch} first: GitHub has ${pushed ? pushed.slice(0, 8) : "no such branch"}, this checkout has ${head.slice(0, 8)}.`,
    );
  }
  const since = Date.now() - 5_000;
  gh(["workflow", "run", WORKFLOW, "--ref", branch, ...dispatchFields(options)]);
  for (let attempt = 0; attempt < 30; attempt += 1) {
    await sleep(2_000);
    const runs = JSON.parse(
      gh([
        "run",
        "list",
        "--workflow",
        WORKFLOW,
        "--branch",
        branch,
        "--event",
        "workflow_dispatch",
        "--limit",
        "10",
        "--json",
        "databaseId,headSha,createdAt",
      ]),
    );
    const run = runs.find((item) => item.headSha === head && Date.parse(item.createdAt) >= since);
    if (run) return String(run.databaseId);
  }
  throw new Error("The run did not appear within a minute; see the Actions tab.");
}

/** Waits for the run, printing each finished step once. Returns its conclusion. */
async function watch(id) {
  const printed = new Set();
  for (;;) {
    const run = JSON.parse(gh(["run", "view", id, "--json", "status,conclusion,jobs"]));
    for (const job of run.jobs ?? []) {
      for (const step of job.steps ?? []) {
        const key = `${job.name}/${step.number}`;
        if (step.status !== "completed" || step.conclusion === "skipped" || printed.has(key))
          continue;
        printed.add(key);
        console.log(
          `  ${step.conclusion === "success" ? "ok  " : "FAIL"} ${job.name}: ${step.name}`,
        );
      }
    }
    if (run.status === "completed") return run.conclusion;
    await sleep(POLL_MS);
  }
}

async function main() {
  const options = parseOptions(process.argv.slice(2));
  const id = options.watch ?? (await start(options));
  const url = gh(["run", "view", id, "--json", "url", "--jq", ".url"]).trim();
  console.log(`Check run ${id}: ${url}`);
  console.log(billLine());
  if (!options.wait) return;
  const conclusion = await watch(id);
  console.log(`Check run ${id}: ${conclusion}.`);
  if (conclusion !== "success") {
    const log = gh(["run", "view", id, "--log-failed"]).split("\n");
    console.log(log.slice(-150).join("\n"));
  }
  console.log(billLine());
  process.exitCode = conclusion === "success" ? 0 : 1;
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  main().catch((error) => {
    console.error(error.message);
    process.exitCode = 2;
  });
}
