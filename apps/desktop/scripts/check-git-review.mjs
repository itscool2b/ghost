import assert from "node:assert/strict";
import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import AxeBuilder from "@axe-core/playwright";

// Sheets fade in and controls settle their colours; audit only once every finite animation ends.
const settled = (page) =>
  page.waitForFunction(() =>
    document
      .getAnimations()
      .every((a) => a.playState !== "running" || a.effect?.getTiming().iterations === Infinity),
  );

export async function checkGitReview(page, url, out, engine) {
  await page.goto(`${url}/?preview=git&non-git&live`);
  const initialize = page.getByRole("button", { name: "Review Git initialization", exact: true });
  await initialize.waitFor();
  assert.equal(await initialize.isDisabled(), true);
  assert.deepEqual(await page.evaluate(() => window.gitFixture.calls()), []);
  await page.getByRole("button", { name: "Stop session", exact: true }).click();
  await initialize.click();
  await page.getByRole("heading", { name: "Review Git initialization", exact: true }).waitFor();
  assert.deepEqual(await page.evaluate(() => window.gitFixture.calls()), [
    "stop",
    "review-initialize",
  ]);
  await page.getByRole("button", { name: "Cancel", exact: true }).click();
  await initialize.click();
  await settled(page);
  assert.deepEqual((await new AxeBuilder({ page }).analyze()).violations, []);
  await page.screenshot({ path: resolve(out, `${engine}-git-initialize-review.png`) });
  await page.getByRole("button", { name: "Initialize reviewed folder", exact: true }).click();
  await page
    .getByText(
      "Git is initialized. Existing files remain unstaged. Create the first commit before isolation.",
      { exact: true },
    )
    .waitFor();
  assert.equal(
    await page.getByRole("button", { name: "Review isolation", exact: true }).isDisabled(),
    true,
  );
  assert.deepEqual(await page.evaluate(() => window.gitFixture.calls()), [
    "stop",
    "review-initialize",
    "review-initialize",
    "apply-initialize:initialize-once",
    "refresh-chat",
  ]);
  await page.goto(`${url}/?preview=git&non-git&stale-init`);
  await initialize.click();
  await page.getByRole("button", { name: "Initialize reviewed folder", exact: true }).click();
  await page
    .getByText("The folder changed since review. Review Git initialization again.", { exact: true })
    .waitFor();
  assert.equal(
    await page.getByRole("button", { name: "Initialize reviewed folder", exact: true }).count(),
    0,
  );
  await page.goto(`${url}/?preview=git&live`);
  const isolate = page.getByRole("button", { name: "Review isolation", exact: true });
  await isolate.waitFor();
  assert.equal(await isolate.isDisabled(), true);
  assert.deepEqual(await page.evaluate(() => window.gitFixture.calls()), []);
  await page.getByRole("button", { name: "Stop session", exact: true }).click();
  await page.getByLabel("Bring uncommitted changes", { exact: false }).check();
  await isolate.click();
  await page.getByRole("heading", { name: "Review isolation" }).waitFor();
  await page.getByText("Copy reviewed changes as unstaged files", { exact: true }).waitFor();
  assert.equal(
    await page.evaluate(() => window.gitFixture.calls().includes("apply-isolation")),
    false,
  );
  await settled(page);
  assert.deepEqual((await new AxeBuilder({ page }).analyze()).violations, []);
  await page.screenshot({ path: resolve(out, `${engine}-isolation-review.png`) });
  await page.getByRole("button", { name: "Create isolated workspace" }).click();
  await page.getByRole("heading", { name: "Owned worktrees" }).waitFor();
  await page.getByRole("button", { name: "Review moving this chat", exact: true }).click();
  await page.getByRole("heading", { name: "Context for your next prompt" }).waitFor();
  assert.equal(
    await page.evaluate(() => window.gitFixture.calls().includes("apply-relocation")),
    false,
  );
  await settled(page);
  assert.deepEqual((await new AxeBuilder({ page }).analyze()).violations, []);
  await page.screenshot({ path: resolve(out, `${engine}-worktree-move.png`) });
  await page.getByRole("button", { name: "Move chat to reviewed folder" }).click();
  await page.getByRole("button", { name: "Review return to project folder", exact: true }).click();
  await page.getByRole("button", { name: "Move chat to reviewed folder" }).click();
  await page.getByRole("button", { name: "Review moving this chat", exact: true }).waitFor();
  await page.evaluate(() => sessionStorage.setItem("git-conflict", "yes"));
  await page.getByRole("button", { name: "Review merge", exact: true }).click();
  await page.getByRole("heading", { name: "Conflicts to resolve first" }).waitFor();
  assert.equal(
    await page.getByRole("button", { name: "Merge reviewed changes" }).isDisabled(),
    true,
  );
  // This chat is back in the project folder, so nothing is sent or opened from here.
  await page.getByText("Move this chat into it", { exact: false }).waitFor();
  assert.equal(await page.getByTestId("git-ask-agent").count(), 0);
  assert.equal(
    await page.evaluate(() => window.gitFixture.calls().includes("apply-action")),
    false,
  );
  await page.getByRole("button", { name: "Cancel", exact: true }).click();
  await page.evaluate(() => {
    sessionStorage.removeItem("git-conflict");
    sessionStorage.setItem("git-stale", "yes");
  });
  await page.getByRole("button", { name: "Review merge", exact: true }).click();
  await page.getByRole("button", { name: "Merge reviewed changes" }).click();
  await page.getByRole("alert").filter({ hasText: "repository changed" }).waitFor();
  assert.equal(await page.getByRole("button", { name: "Merge reviewed changes" }).count(), 0);
  await page.evaluate(() => sessionStorage.removeItem("git-stale"));
  await page.getByRole("button", { name: "Review retirement", exact: true }).click();
  await page.getByRole("button", { name: "Retire and keep files" }).click();
  await page.getByRole("button", { name: "Review restoration", exact: true }).click();
  await page.getByRole("heading", { name: "Review restoration" }).waitFor();
  await settled(page);
  assert.deepEqual((await new AxeBuilder({ page }).analyze()).violations, []);
  await page.screenshot({ path: resolve(out, `${engine}-worktree-restore.png`) });
  await page.getByRole("button", { name: "Restore retained worktree" }).click();
  await page.getByRole("button", { name: "Review merge", exact: true }).waitFor();
  await page.getByRole("button", { name: "Prepare pull request", exact: true }).click();
  await page.getByLabel("Pull request title", { exact: true }).fill("Finish release checks");
  await page
    .getByLabel("Description", { exact: true })
    .fill("Review the completed changes.\n\nVerified with the product checks.");
  await page.getByRole("button", { name: "Review pull request", exact: true }).click();
  await page.getByRole("heading", { name: "Review pull request", exact: true }).waitFor();
  await page.getByText("fixture/orbit", { exact: true }).waitFor();
  assert.equal(await page.evaluate(() => window.gitFixture.calls().includes("apply-pr")), false);
  await settled(page);
  assert.deepEqual((await new AxeBuilder({ page }).analyze()).violations, []);
  await page.screenshot({ path: resolve(out, `${engine}-pull-request-review.png`) });
  await page.getByRole("button", { name: "Publish reviewed pull request" }).click();
  await page.getByRole("heading", { name: "Pull request created" }).waitFor();
  assert.equal(
    await page.getByRole("link", { name: "Open on GitHub" }).getAttribute("href"),
    "https://github.com/fixture/orbit/pull/1",
  );
  assert.deepEqual(
    await page.evaluate(() => window.gitFixture.calls().filter((call) => call.startsWith("apply"))),
    [
      "apply-isolation",
      "apply-relocation",
      "apply-relocation",
      "apply-action",
      "apply-action",
      "apply-recovery",
      "apply-pr",
    ],
  );
  await page.keyboard.press("Escape");
  await page.getByRole("button", { name: "Open Git workspace" }).waitFor();
  await checkWorktreeRecreate(page, url, out, engine);
  await checkMergeConflicts(page, url, out, engine);
  await checkMissingWorktreeStart(page, url, out, engine);
}

/**
 * A chat whose worktree folder was deleted by hand fails to start: its error card leads with
 * Recreate folder (Retry steps back to a soft pill), which opens the same reviewed confirmation as
 * the Git workspace dialog. Cancel changes nothing, a stale review is refused and reviewed again,
 * and once the folder is back the card says so and Retry is the way on. Nothing starts by itself.
 */
export async function checkMissingWorktreeStart(page, url, out, engine) {
  const calls = () => page.evaluate(() => window.missingWorktreeFixture.calls());
  await page.goto(`${url}/?preview=missing-worktree&stale`);
  const recreate = page.getByTestId("error-recreate-folder");
  await recreate.waitFor();
  assert.equal(await recreate.textContent(), "Recreate folder");
  assert.match(await recreate.getAttribute("class"), /g-button-primary/);
  assert.doesNotMatch(
    await page.getByTestId("error-retry").getAttribute("class"),
    /g-button-primary/,
  );
  assert.deepEqual(await calls(), ["list"]);
  await settled(page);
  assert.deepEqual(
    (await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa", "wcag21aa"]).analyze())
      .violations,
    [],
  );
  await page.screenshot({ path: resolve(out, `${engine}-missing-worktree-start.png`) });
  await recreate.click();
  const dialog = page.getByRole("dialog", { name: "Recreate the deleted folder?" });
  await dialog.getByText("ghost/sidebar-scan", { exact: true }).waitFor();
  await dialog.getByText("C:\\auracode\\worktrees\\sidebar-scan", { exact: true }).waitFor();
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  await dialog.waitFor({ state: "detached" });
  assert.deepEqual(await calls(), ["list", "review-recreate"]);
  await recreate.click();
  await dialog.getByRole("button", { name: "Recreate folder", exact: true }).click();
  await dialog.getByRole("alert").filter({ hasText: "expired or was already used" }).waitFor();
  await dialog.getByRole("button", { name: "Recreate folder", exact: true }).click();
  await dialog.waitFor({ state: "detached" });
  await page
    .getByTestId("recreate-note")
    .getByText(
      "The folder is back on ghost/sidebar-scan at its last commit. Uncommitted files from the deleted folder could not be restored. Retry to start Claude Code.",
      { exact: true },
    )
    .waitFor();
  assert.equal(await recreate.count(), 0);
  assert.match(await page.getByTestId("error-retry").getAttribute("class"), /g-button-primary/);
  assert.deepEqual(await calls(), [
    "list",
    "review-recreate",
    "review-recreate",
    "apply-recreate:recreate-2",
    "review-recreate",
    "apply-recreate:recreate-3",
  ]);
  await page.screenshot({ path: resolve(out, `${engine}-missing-worktree-recreated.png`) });
}

/**
 * A worktree chat whose merge conflicts: Merge stays disabled; the exact message the agent would
 * receive is on screen and is what Ask sends, as the chat's own message; Open in hands the folder
 * to a detected editor and reviews again for the next; a refused send says why and keeps the
 * dialog; no detected editor says so. Nothing merges.
 */
export async function checkMergeConflicts(page, url, out, engine) {
  await page.goto(`${url}/?preview=git-conflict`);
  await page.getByRole("button", { name: "Review merge", exact: true }).click();
  await page.getByRole("heading", { name: "Conflicts to resolve first" }).waitFor();
  const merge = page.getByRole("button", { name: "Merge reviewed changes" });
  assert.equal(await merge.isDisabled(), true);
  const prompt = page.getByTestId("git-conflict-prompt");
  const text = await prompt.textContent();
  assert.match(text, /^Merging ghost\/release-readiness into main has conflicts in 2 files:\n/);
  assert.match(text, /\n- src\/orbit\.rs\n- apps\/desktop\/src\/components\/chat\/Sidebar\.tsx\n/);
  assert.match(text, /in C:\/auracode\/worktrees\/owned-one: merge main into/);
  await page.getByRole("button", { name: "Open in Cursor", exact: true }).waitFor();
  await page.getByRole("button", { name: "Open in Zed", exact: true }).waitFor();
  await page.getByRole("button", { name: "Open in RustRover", exact: true }).waitFor();
  await settled(page);
  assert.deepEqual((await new AxeBuilder({ page }).analyze()).violations, []);
  await page.screenshot({ path: resolve(out, `${engine}-merge-conflicts.png`) });
  await page.getByRole("button", { name: "Open in Cursor", exact: true }).click();
  await page.getByRole("status").filter({ hasText: "accepted the request for Cursor" }).waitFor();
  await page.getByRole("button", { name: "Open in Cursor", exact: true }).waitFor();
  assert.equal(await merge.isDisabled(), true);
  // One open, with the newest review's token (development StrictMode reviews twice on mount), then
  // a fresh review for the next.
  const tools = (await page.evaluate(() => window.gitFixture.calls())).filter(
    (call) => call.startsWith("editor") || call === "review-tools",
  );
  const reviewsBefore = tools.indexOf(tools.find((call) => call.startsWith("editor")));
  assert.deepEqual(tools.slice(reviewsBefore), [
    `editor:cursor:tools-${reviewsBefore}`,
    "review-tools",
  ]);
  assert.ok(tools.slice(0, reviewsBefore).every((call) => call === "review-tools"));
  await page.getByRole("button", { name: "Ask Claude Code to resolve", exact: true }).click();
  await page.getByRole("button", { name: "Open Git workspace" }).waitFor();
  assert.equal(await page.evaluate(() => window.gitFixture.sent()), text);
  const calls = await page.evaluate(() => window.gitFixture.calls());
  assert.equal(calls.filter((call) => call === "ask-agent").length, 1);
  assert.equal(calls.includes("apply-action"), false);

  await page.goto(`${url}/?preview=git-conflict&refuse-send&no-editors`);
  await page.getByRole("button", { name: "Review merge", exact: true }).click();
  await page.getByText("No supported editor was found.", { exact: false }).waitFor();
  assert.equal(await page.getByRole("button", { name: /^Open in / }).count(), 0);
  await page.getByRole("button", { name: "Ask Claude Code to resolve", exact: true }).click();
  await page.getByRole("alert").filter({ hasText: "this message was not sent" }).waitFor();
  assert.equal(await merge.isDisabled(), true);
  assert.equal(await page.evaluate(() => window.gitFixture.sent()), "");
  await settled(page);
  assert.deepEqual((await new AxeBuilder({ page }).analyze()).violations, []);
  await page.screenshot({ path: resolve(out, `${engine}-merge-conflicts-refused.png`) });
}

/**
 * A worktree folder deleted by hand: "Recreate folder" opens a confirmation that leads with what
 * is lost and what is kept, Cancel changes nothing, a stale review is refused and reviewed again,
 * and the next confirmation brings the folder back on its branch.
 */
export async function checkWorktreeRecreate(page, url, out, engine) {
  await page.goto(`${url}/?preview=git-missing`);
  const recreate = page.getByRole("button", { name: "Recreate folder", exact: true });
  await recreate.waitFor();
  await page.getByText("missing", { exact: true }).waitFor();
  await recreate.click();
  const dialog = page.getByRole("dialog", { name: "Recreate the deleted folder?" });
  await dialog
    .getByText(
      "Uncommitted files that were in the deleted folder are gone. Everything committed on the branch is kept.",
      { exact: true },
    )
    .waitFor();
  await dialog.getByText("ghost/release-readiness", { exact: true }).waitFor();
  await dialog.getByText("9c41e07d2b8a", { exact: true }).waitFor();
  await dialog.getByText("C:/auracode/worktrees/owned-one", { exact: true }).waitFor();
  await settled(page);
  assert.deepEqual((await new AxeBuilder({ page }).analyze()).violations, []);
  await page.screenshot({ path: resolve(out, `${engine}-worktree-recreate.png`) });
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  await dialog.waitFor({ state: "detached" });
  assert.deepEqual(await page.evaluate(() => window.gitFixture.calls()), ["review-recreate"]);
  await recreate.click();
  await page.evaluate(() => sessionStorage.setItem("git-stale", "yes"));
  await dialog.getByRole("button", { name: "Recreate folder", exact: true }).click();
  await dialog.getByRole("alert").filter({ hasText: "expired or was already used" }).waitFor();
  await page.waitForFunction(
    () => window.gitFixture.calls().filter((call) => call === "review-recreate").length === 3,
  );
  await dialog.getByRole("button", { name: "Recreate folder", exact: true }).click();
  await dialog.waitFor({ state: "detached" });
  await page.getByRole("heading", { name: "Workspace updated", exact: true }).waitFor();
  await page
    .getByText(
      "The folder is back on ghost/release-readiness at its last commit. Uncommitted files from the deleted folder could not be restored.",
      { exact: true },
    )
    .waitFor();
  await page.getByText("ready", { exact: true }).waitFor();
  assert.deepEqual(await page.evaluate(() => window.gitFixture.calls()), [
    "review-recreate",
    "review-recreate",
    "apply-recreate:recreate-2",
    "review-recreate",
    "apply-recreate:recreate-3",
    "refresh-chat",
  ]);
  await page.keyboard.press("Escape");
  await page.getByRole("button", { name: "Open Git workspace" }).waitFor();
  await commitAndPush(page, url, out, engine);
}

/**
 * Commit and push from the dialog (missing features C): only the chosen files go into the
 * reviewed commit, a stale review is refused, and a push shows its remote, branch and commits.
 */
async function commitAndPush(page, url, out, engine) {
  await page.goto(`${url}/?preview=git&changes&live`);
  const form = page.getByRole("form", { name: "Commit changes" });
  await form.waitFor();
  const review = form.getByRole("button", { name: "Review commit", exact: true });
  assert.equal(await review.isDisabled(), true, "a live session allowed a commit");
  await page.getByRole("button", { name: "Stop session", exact: true }).click();
  // A file left out of the commit stays out; a renamed file shows where it came from.
  await form.getByText("src/start.rs → src/launch.rs", { exact: true }).waitFor();
  await form.getByRole("checkbox", { name: /docs\/checklist\.md/ }).uncheck();
  await form.getByLabel("Commit message").fill("Add the release checklist");
  await review.click();
  await page.getByRole("heading", { name: "Review commit", exact: true }).waitFor();
  await page.getByText("Fixture <fixture@localhost>", { exact: true }).waitFor();
  await page.getByText("Git hooks do not run", { exact: false }).waitFor();
  await settled(page);
  assert.deepEqual((await new AxeBuilder({ page }).analyze()).violations, []);
  await page.screenshot({ path: resolve(out, `${engine}-git-commit-review.png`) });
  // A review the folder outgrew is refused, and nothing is committed.
  await page.evaluate(() => sessionStorage.setItem("git-stale", "yes"));
  await page.getByRole("button", { name: "Commit reviewed files", exact: true }).click();
  await page
    .getByText("The files changed since the review. Review the commit again.", { exact: true })
    .waitFor();
  await page.evaluate(() => sessionStorage.removeItem("git-stale"));
  await review.click();
  await page.getByRole("button", { name: "Commit reviewed files", exact: true }).click();
  await page.getByRole("heading", { name: "Committed on main", exact: true }).waitFor();
  // Only the file left out is still there to commit.
  assert.equal(await form.getByRole("checkbox").count(), 1);
  assert.deepEqual(await page.evaluate(() => window.gitFixture.calls()), [
    "stop",
    "review-commit:src/orbit.rs,src/launch.rs",
    "apply-commit:commit-1",
    "refresh-chat",
    "review-commit:src/orbit.rs,src/launch.rs",
    "apply-commit:commit-2",
    "refresh-chat",
  ]);
  await page.getByRole("button", { name: "Review push", exact: true }).click();
  await page.getByRole("heading", { name: "Review push", exact: true }).waitFor();
  await page.getByText("origin, https://github.com/fixture/orbit.git", { exact: true }).waitFor();
  await page.getByText("7e1f0c9 Add the release checklist", { exact: false }).waitFor();
  await settled(page);
  assert.deepEqual((await new AxeBuilder({ page }).analyze()).violations, []);
  await page.screenshot({ path: resolve(out, `${engine}-git-push-review.png`) });
  await page.getByRole("button", { name: "Push reviewed commits", exact: true }).click();
  await page.getByRole("heading", { name: "Pushed origin/main", exact: true }).waitFor();
  await blockedCommit(page, url, out, engine);
}

/**
 * Another chat working in the folder: the commit is refused naming it, the review stays, and
 * "Stop this chat" stops it and commits the same review.
 */
async function blockedCommit(page, url, out, engine) {
  await page.goto(`${url}/?preview=git&changes&blocked`);
  const form = page.getByRole("form", { name: "Commit changes" });
  await form.getByLabel("Commit message").fill("Add the release checklist");
  await form.getByRole("button", { name: "Review commit", exact: true }).click();
  await page.getByRole("button", { name: "Commit reviewed files", exact: true }).click();
  const busy = page.getByTestId("folder-busy");
  await busy.getByText("A chat is working in this folder", { exact: true }).waitFor();
  await busy.getByText("Fix the tests in orbit: working", { exact: true }).waitFor();
  assert.equal(
    await page.getByRole("button", { name: "Commit reviewed files", exact: true }).count(),
    1,
    "the review stays for the retry",
  );
  await settled(page);
  assert.deepEqual((await new AxeBuilder({ page }).analyze()).violations, []);
  await page.screenshot({ path: resolve(out, `${engine}-git-folder-busy.png`) });
  await busy.getByRole("button", { name: "Stop this chat", exact: true }).click();
  await page.getByRole("heading", { name: "Committed on main", exact: true }).waitFor();
  assert.deepEqual(
    (await page.evaluate(() => window.gitFixture.calls())).filter(
      (call) => call.startsWith("apply-commit") || call.startsWith("stop-chats"),
    ),
    ["apply-commit:commit-1", "stop-chats:other", "apply-commit:commit-1"],
  );
  assert.equal(await page.getByTestId("folder-busy").count(), 0);
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { chromium, webkit } = await import("playwright");
  const out = fileURLToPath(new URL("../../../target/settings-browser/", import.meta.url));
  await mkdir(out, { recursive: true });
  for (const [engine, browserType] of Object.entries({ chromium, webkit })) {
    const browser = await browserType.launch();
    try {
      const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
      const page = await context.newPage();
      await checkGitReview(page, process.env.CHECK_URL ?? "http://localhost:1426", out, engine);
      console.log(
        `${engine}: reviewed Git workspace, pull request, worktree recreation (from the dialog and a start error) and merge conflict flows passed`,
      );
    } finally {
      await browser.close();
    }
  }
}
