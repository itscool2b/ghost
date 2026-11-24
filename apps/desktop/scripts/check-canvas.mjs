/**
 * Canvas (`?preview=canvas`, docs/superpowers/plans/2026-11-09-canvas.md): drawing, labels, arrows
 * that follow their shapes, undo, Talk's numbered narration, and Add to message, in both
 * appearances, with axe. Runs no agent; the microphone is scripted.
 */
import assert from "node:assert/strict";
import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import AxeBuilder from "@axe-core/playwright";

async function axe(page, label) {
  const result = await new AxeBuilder({ page }).include("section[aria-label=Canvas]").analyze();
  assert.deepEqual(
    result.violations.map(
      (violation) => `${violation.id}: ${violation.nodes.map((node) => node.target).join(" ")}`,
    ),
    [],
    `${label}: axe`,
  );
}

export async function checkCanvas(browser, url, out, engine) {
  for (const theme of ["light", "dark"]) {
    const label = `${engine} ${theme}`;
    const context = await browser.newContext({
      viewport: { width: 1280, height: 820 },
      colorScheme: theme,
      reducedMotion: "reduce",
    });
    try {
      const page = await context.newPage();
      const errors = [];
      page.on("pageerror", (error) => errors.push(error.message));
      await page.goto(`${url}/?preview=canvas&theme=${theme}`);
      // Canvas is in the composer's "+" menu, beside Files and images.
      const plus = page.locator('summary[aria-label="Add to message"]');
      const opener = async () => {
        await plus.click();
        await page.getByRole("button", { name: "Canvas", exact: true }).click();
      };
      await plus.waitFor();
      await opener();
      const pane = page.locator("section[aria-label=Canvas]");
      await pane.waitFor();
      // Open, the menu's row closes it instead, and says so.
      await plus.click();
      assert.equal(
        await page.getByTestId("composer-canvas").getAttribute("aria-label"),
        "Close the canvas",
        `${label}: Canvas shows open`,
      );
      await page.keyboard.press("Escape");
      await axe(page, `${label} empty`);
      const surface = page.locator(".canvas-surface");
      let box = await surface.boundingBox();
      assert.ok(box, `${label}: the surface has a size`);
      const at = (x, y) => ({ x: box.x + x, y: box.y + y });
      const drag = async (from, to) => {
        await page.mouse.move(at(...from).x, at(...from).y);
        await page.mouse.down();
        await page.mouse.move(at(...to).x, at(...to).y, { steps: 8 });
        await page.mouse.up();
      };
      const tool = (name) => pane.getByRole("button", { name, exact: true }).click();
      const count = (selector) => surface.locator(selector).count();

      // Two labelled boxes and an arrow from one to the other.
      await tool("Rectangle");
      await drag([120, 140], [300, 220]);
      await drag([520, 140], [700, 220]);
      assert.equal(await count("rect[data-id]"), 2, `${label}: two boxes`);
      await tool("Text");
      for (const [x, text] of [
        [140, "API gateway"],
        [540, "Queue"],
      ]) {
        await page.mouse.click(at(x, 165).x, at(x, 165).y);
        const editor = pane.getByRole("textbox", { name: "Text on the canvas" });
        await editor.waitFor();
        await editor.fill(text);
        await editor.press("Enter");
      }
      assert.equal(await count("text[data-id]"), 2, `${label}: two labels`);
      await tool("Arrow");
      await drag([280, 180], [560, 180]);
      assert.equal(await count("g[data-id] line"), 1, `${label}: an arrow`);

      // Keys choose tools while the drawing has focus; undo and redo take back a stroke.
      await page.keyboard.press("p");
      assert.equal(
        await pane.getByRole("button", { name: "Pen", exact: true }).getAttribute("aria-pressed"),
        "true",
        `${label}: P chooses the pen`,
      );
      await drag([120, 360], [420, 420]);
      assert.equal(await count("path[data-id]"), 1, `${label}: a stroke`);
      await page.keyboard.press("Control+z");
      assert.equal(await count("path[data-id]"), 0, `${label}: undo takes it back`);
      await page.keyboard.press("Control+Shift+z");
      assert.equal(await count("path[data-id]"), 1, `${label}: redo returns it`);

      // Moving a box takes the arrow's end along.
      const arrowEnd = () =>
        surface.locator("g[data-id] line").evaluate((line) => Number(line.getAttribute("x2")));
      const queueAt = () =>
        surface
          .locator("text[data-id]")
          .filter({ hasText: "Queue" })
          .evaluate((text) => Number(text.getAttribute("x")));
      const before = await arrowEnd();
      const labelBefore = await queueAt();
      await page.keyboard.press("v");
      await drag([620, 205], [660, 205]);
      const after = await arrowEnd();
      assert.ok(
        Math.abs(after - before - 40) < 2,
        `${label}: the arrow follows its box (${before} to ${after})`,
      );
      assert.ok(
        Math.abs((await queueAt()) - labelBefore - 40) < 2,
        `${label}: the label moves with its box`,
      );

      // Before Talk the button names what it would add.
      const add = page.getByTestId("canvas-add");
      assert.equal(await add.getAttribute("data-state"), "fresh", `${label}: nothing added yet`);
      assert.equal(await add.textContent(), "Add drawing", `${label}: Add names the drawing`);
      const composer = page.locator(".chat-composer textarea");
      await composer.fill("Build this.");

      // Talk: a shape drawn while a phrase is said gets that phrase's number.
      await pane.getByRole("button", { name: "Talk" }).click();
      await pane.getByText("Listening").waitFor();
      await tool("Ellipse");
      await drag([140, 470], [300, 560]);
      await page.evaluate(() => {
        window.canvasSaid = ["this is the cache"];
      });
      await pane.getByText("this is the cache").waitFor();
      // Finish puts the drawing, its outline and what was said in the message at once.
      await pane.getByRole("button", { name: "Finish" }).click();
      await pane.locator(".canvas-phrases li").filter({ hasText: "this is the cache" }).waitFor();
      await page.waitForFunction(() => window.canvasPastes.length === 1);
      await pane.getByText("Added to your message below", { exact: false }).waitFor();
      await pane.locator(".canvas-in-message").waitFor();
      assert.equal(await add.getAttribute("data-state"), "added", `${label}: in the message`);
      assert.equal(await add.textContent(), "In your message", `${label}: Add says so`);
      assert.equal(
        await surface.locator(".canvas-marker text").first().textContent(),
        "1",
        `${label}: the marker`,
      );
      await axe(page, `${label} drawn`);
      await page.screenshot({ path: resolve(out, `canvas-${engine}-${theme}.png`) });

      // What Finish added: the drawing as Canvas.png, its outline and the narration in the draft.
      const pastes = await page.evaluate(() => window.canvasPastes);
      assert.equal(pastes.length, 1, `${label}: one image`);
      assert.equal(pastes[0].name, "Canvas.png", `${label}: named Canvas.png`);
      assert.equal(pastes[0].png, true, `${label}: a PNG`);
      const text = await composer.inputValue();
      for (const part of [
        "Build this.",
        "The attached canvas shows:",
        '- box "API gateway"',
        '- arrow from "API gateway" to "Queue"',
        "1. this is the cache",
      ])
        assert.ok(text.includes(part), `${label}: the draft has ${part}: ${text}`);
      await page.locator(".composer-context").getByText("Canvas.png").waitFor();
      // The draft holds the outline now; with the canvas open it scrolls instead of taking the
      // drawing's room.
      const field = await composer.boundingBox();
      assert.ok(field && field.height <= 121, `${label}: the draft keeps to 120px`);
      box = await surface.boundingBox();
      // Drawing more leaves the message behind; Update message replaces the image and the text
      // instead of doubling them.
      await tool("Pen");
      await drag([420, 300], [560, 360]);
      assert.equal(await add.getAttribute("data-state"), "update", `${label}: out of date`);
      assert.equal(await add.textContent(), "Update message", `${label}: Add offers the update`);
      await add.click();
      await page.waitForFunction(() => window.canvasPastes.length === 2);
      await page.waitForFunction(() => window.canvasDraft().attachments.length === 1);
      const again = await composer.inputValue();
      assert.equal(again.split("The attached canvas shows:").length, 2, `${label}: one outline`);
      assert.equal(await add.getAttribute("data-state"), "added", `${label}: up to date again`);

      // A long narration stays inside the strip: it scrolls on the newest words, above the
      // composer, instead of spilling over it.
      await pane.getByRole("button", { name: "Talk" }).click();
      await pane.locator("[data-testid=canvas-live]").waitFor();
      await page.evaluate(() => {
        window.canvasSaid = Array.from(
          { length: 6 },
          (_, index) =>
            `phrase ${index + 2} keeps going about the gateway, the queue, the workers and the retries`,
        );
        window.canvasForming = "and this is still forming";
      });
      await pane.getByText("and this is still forming").waitFor();
      const transcript = pane.locator(".canvas-transcript");
      const fit = await transcript.evaluate((node) => ({
        scrolls: node.scrollHeight > node.clientHeight,
        atEnd: node.scrollTop + node.clientHeight >= node.scrollHeight - 2,
        faded: node.hasAttribute("data-overflowing"),
      }));
      assert.deepEqual(fit, { scrolls: true, atEnd: true, faded: true }, `${label}: it scrolls`);
      const strip = await pane.locator("[data-testid=canvas-narration]").boundingBox();
      const capsule = await page.locator(".chat-composer").boundingBox();
      assert.ok(
        strip && capsule && strip.y + strip.height <= capsule.y,
        `${label}: the strip stays above the composer`,
      );
      await axe(page, `${label} talking`);
      await pane.getByRole("button", { name: "Cancel" }).click();
      await pane.locator("[data-testid=canvas-live]").waitFor({ state: "detached" });

      // Closing keeps the drawing; the transcript comes back.
      await pane.getByRole("button", { name: "Close the canvas" }).click();
      await pane.waitFor({ state: "detached" });
      await opener();
      await pane.waitFor();
      assert.equal(await count("rect[data-id]"), 2, `${label}: the drawing stayed`);
      assert.deepEqual(errors, [], `${label}: no page errors`);
    } finally {
      await context.close();
    }
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { chromium, webkit } = await import("playwright");
  const out = fileURLToPath(new URL("../../../target/canvas-browser/", import.meta.url));
  await mkdir(out, { recursive: true });
  for (const [engine, browserType] of Object.entries({ chromium, webkit })) {
    const browser = await browserType.launch();
    try {
      await checkCanvas(browser, process.env.CHECK_URL ?? "http://localhost:1476", out, engine);
      console.log(
        `${engine}: canvas drawing, labels, arrows, undo, Talk and Add to message passed in both appearances`,
      );
    } finally {
      await browser.close();
    }
  }
}
