import { spawn } from "node:child_process";
import { once } from "node:events";
import { mkdir, rename, rm, stat } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { chromium } from "@playwright/test";

const root = fileURLToPath(new URL("../..", import.meta.url));
const artifacts = fileURLToPath(
  new URL("../../target/demo-gif", import.meta.url),
);
const output = fileURLToPath(
  new URL("../../docs/editor-demo.gif", import.meta.url),
);
const pendingOutput = fileURLToPath(
  new URL("../../target/demo-gif/editor-demo.gif", import.meta.url),
);
const port = 18084;
const url = `http://127.0.0.1:${port}`;
const pause = (ms = 400) => new Promise((resolve) => setTimeout(resolve, ms));

function run(command, args, options = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      cwd: root,
      stdio: "inherit",
      ...options,
    });
    child.once("error", reject);
    child.once("exit", (code) =>
      code === 0
        ? resolve()
        : reject(new Error(`${command} exited with ${code}`)),
    );
  });
}

async function waitForServer(server) {
  const deadline = Date.now() + 300_000;
  while (Date.now() < deadline) {
    if (server.exitCode !== null) {
      throw new Error(`fixture server exited with ${server.exitCode}`);
    }
    try {
      const response = await fetch(url);
      const body = await response.text();
      if (response.ok && !/building your app|starting the build/i.test(body)) {
        await pause(500);
        return;
      }
    } catch {}
    await pause(500);
  }
  throw new Error("timed out waiting for the fixture server");
}

await rm(artifacts, { recursive: true, force: true });
await mkdir(artifacts, { recursive: true });
await mkdir(fileURLToPath(new URL("../../docs", import.meta.url)), {
  recursive: true,
});

const server = spawn(
  "mise",
  [
    "exec",
    "--",
    "dx",
    "serve",
    "--package",
    "dioxus-editor-fixture",
    "--web",
    "--addr",
    "127.0.0.1",
    "--port",
    String(port),
    "--open",
    "false",
    "--interactive",
    "false",
    "--watch",
    "false",
  ],
  { cwd: root, stdio: "inherit" },
);

let browser;
try {
  await waitForServer(server);
  browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    viewport: { width: 1120, height: 820 },
    colorScheme: "light",
    recordVideo: { dir: artifacts, size: { width: 1120, height: 820 } },
  });
  const recordingStartedAt = Date.now();
  const page = await context.newPage();
  const video = page.video();
  await page.goto(url, { waitUntil: "domcontentloaded" });
  await page
    .locator("#fixture-root > h1")
    .waitFor({ state: "visible", timeout: 120_000 });

  await page.addStyleTag({
    content: `
      /* A paper writing app floating on an oxide-rust canvas. Flat fills
       * only — gradients dither into noise once ffmpeg quantizes the gif. */
      body {
        margin: 0; padding: 0; min-height: 100vh;
        display: flex; align-items: center; justify-content: center;
        background: #9e3a10;
        -webkit-font-smoothing: antialiased;
      }
      #fixture-root > label, #fixture-root > output, #state-dump, #selection-dump { display: none; }
      #fixture-root {
        width: 880px; max-width: none; margin: 0;
        display: flex; flex-direction: column;
        background: #fbf6ec; border-radius: 14px; overflow: hidden;
        box-shadow: 0 32px 80px rgba(28, 10, 2, .5);
      }
      /* Title bar */
      #fixture-root > h1 {
        order: 1; margin: 0; height: 44px; flex: none;
        display: flex; align-items: center; gap: 8px; padding: 0 16px;
        background: #26190e; text-transform: none;
        font: 500 13px/1 Menlo, Consolas, monospace; color: #e8d5b5; letter-spacing: .02em;
      }
      .demo-light { width: 12px; height: 12px; border-radius: 50%; flex: none; }
      .demo-title { margin: 0 auto; }
      .demo-title-spacer { width: 52px; flex: none; }
      /* Toolbar */
      .fixture-actions {
        order: 2; flex: none; display: flex; flex-wrap: wrap; gap: 6px;
        margin: 0; padding: 10px 20px; border-bottom: 1px solid #eadfc9;
      }
      .fixture-actions button {
        font: 12px/1.2 Menlo, Consolas, monospace; color: #7a6142;
        background: transparent; border: 1px solid #e0d3b8; border-radius: 6px;
        padding: 5px 10px;
      }
      /* Document */
      .fixture-frame {
        order: 3; height: 560px; overflow: auto;
        border: none; border-radius: 0; background: #fbf6ec; padding: 26px 46px 30px;
      }
      .editor {
        min-height: 100%; color: #2a2014; caret-color: #b7410e;
        font: 17.5px/1.7 "Iowan Old Style", Charter, Georgia, serif;
      }
      .editor::selection, .editor ::selection { background: #f3d3b3; }
      .editor[data-placeholder]:empty::before, .editor--empty[data-placeholder]::before { color: #b9a47f; }
      .editor__p, .editor__h, .editor__quote, .editor__pre, .editor__ul, .editor__ol { margin: 0 0 10px; }
      .editor__h1 {
        font-size: 33px; line-height: 1.2; font-weight: 700; text-transform: none;
        color: #1f1509; letter-spacing: -.01em; margin: 0 0 12px;
      }
      .editor__quote {
        border-left: 3px solid #b7410e; background: none;
        padding: 2px 0 2px 16px; margin: 14px 0; color: #6e5636; font-style: italic;
      }
      .editor__ul { padding-left: 26px; }
      .editor__ul li::marker { color: #b7410e; }
      .editor__pre {
        background: #26190e; color: #f5c88a; border-radius: 8px;
        padding: 12px 18px; margin: 12px 0;
        font: 14px/1.6 Menlo, Consolas, monospace;
      }
      .e-c {
        background: #f1e6d0; color: #8a3a0f; border-radius: 4px; padding: 1px 5px;
        font: 15px Menlo, Consolas, monospace;
      }
      .fixture-link {
        color: #b7410e; text-decoration: underline;
        text-underline-offset: 3px; text-decoration-thickness: 1.5px;
      }
      .fixture-mention { background: #f7e2cf; color: #9a4a14; border-radius: 4px; padding: 0 4px; }
      .fixture-mention-popup {
        min-width: 250px; background: #fffdf8; border: 1px solid #e0d3b8;
        border-radius: 10px; padding: 5px; box-shadow: 0 14px 40px rgba(60, 30, 5, .18);
        font: 14.5px/1.4 -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
      }
      .fixture-mention-item { padding: 7px 9px; }
      .fixture-mention-item:hover, .fixture-mention-item:first-child { background: #f6ead7; }
      .fixture-mention-avatar { background: #b7410e; width: 22px; height: 22px; }
      .fixture-mention-name { color: #2a2014; }
      .fixture-mention-full { color: #a08b6a; font-size: 12.5px; }
      .fixture-embed {
        margin: 12px 0; padding: 10px 16px; border: 1.5px dashed #c86a31;
        border-radius: 8px; color: #9a4a14; background: #f9edda;
        font: 13px/1.5 Menlo, Consolas, monospace;
      }
      .editor__table { width: 100%; border-collapse: collapse; margin: 8px 0; }
      .editor__table th, .editor__table td {
        min-width: 120px; height: 34px; border: 1px solid #e0d3b8;
        padding: 5px 12px; text-align: left; font-size: 16px;
      }
      .editor__table th { background: #f3ead7; font-weight: 600; }
      .editor__decorator-remove, .editor__cell-menu, .editor__table-add { display: none; }
      /* Status bar — the caption lives here, like a real editor's status line */
      #demo-status {
        order: 4; flex: none; display: flex; align-items: center; gap: 10px;
        height: 38px; padding: 0 18px; background: #26190e;
        font: 12.5px/1 Menlo, Consolas, monospace; color: #f5b963;
      }
      #demo-status::before { content: ""; width: 8px; height: 8px; border-radius: 50%; background: #e8632b; flex: none; }
      #demo-status .demo-meta { margin-left: auto; color: #8a6f4c; }
    `,
  });
  await page.evaluate(() => {
    const titleBar = document.querySelector("#fixture-root > h1");
    titleBar.textContent = "";
    for (const color of ["#ff5f57", "#febc2e", "#28c840"]) {
      const light = document.createElement("span");
      light.className = "demo-light";
      light.style.background = color;
      titleBar.append(light);
    }
    const title = document.createElement("span");
    title.className = "demo-title";
    title.textContent = "dioxus-editor";
    const spacer = document.createElement("span");
    spacer.className = "demo-title-spacer";
    titleBar.append(title, spacer);

    const status = document.createElement("div");
    status.id = "demo-status";
    const caption = document.createElement("span");
    caption.id = "demo-caption";
    caption.textContent = "Rich text, entirely in Rust";
    const meta = document.createElement("span");
    meta.className = "demo-meta";
    meta.textContent = "rust · dioxus · wasm";
    status.append(caption, meta);
    document.querySelector("#fixture-root").append(status);
  });

  const editor = page.getByRole("textbox", { name: "Rich text editor" });
  const caption = async (text) => {
    await page.locator("#demo-caption").evaluate((node, value) => {
      node.textContent = value;
    }, text);
    await pause(350);
  };
  const type = async (text, delay = 22) => {
    await page.keyboard.type(text, { delay });
    await pause(180);
  };
  const enter = async () => {
    await page.keyboard.press("Enter");
    await pause(180);
  };

  await editor.click({ timeout: 60_000 });
  const trimStart = (Date.now() - recordingStartedAt) / 1_000;
  await caption("Markdown block shortcuts");
  await type("# ");
  await type("Build rich text with Dioxus", 28);
  await enter();

  await caption("Bold · italic · strike · inline code");
  await type(
    "A **fast**, _composable_ editor with ~~fragile~~ predictable state and `typed commands`.",
  );
  await enter();

  await caption("Schema-backed links");
  await type("Explore [Dioxus](https://dioxuslabs.com) with ");
  await caption("Slack-style mention picker");
  await type("@", 60);
  await pause(500);
  await type("f", 60);
  await pause(500);
  await type("e", 60);
  await pause(650);
  await page
    .locator(".fixture-mention-item")
    .filter({ hasText: "@ferris" })
    .click();
  await pause(500);
  await enter();

  await caption("Blockquotes and lists");
  await type("> Transactions keep every change explicit.");
  await enter();
  await page.getByRole("button", { name: "blockquote", exact: true }).click();
  await type("- Keyboard-first editing");
  await enter();
  await type("Undo and redo history");
  await enter();
  await type("Extensible decorators");
  await enter();
  await page.keyboard.press("Backspace");
  await pause(250);

  await caption("Undo and redo");
  await type("History is built in.");
  const historyText = page.getByText("History is built in.", { exact: true });
  await historyText.waitFor({ state: "visible" });
  const mod = process.platform === "darwin" ? "Meta" : "Control";
  await page.keyboard.press(`${mod}+z`);
  await historyText.waitFor({ state: "hidden" });
  await pause(300);
  await page.keyboard.press(`${mod}+Shift+z`);
  await historyText.waitFor({ state: "visible" });
  await pause(300);
  await enter();

  await caption("Code blocks");
  await type("let state = editor.read_state();");
  await page.getByRole("button", { name: "code block", exact: true }).click();
  await pause(550);

  await caption("Atomic decorators");
  await page
    .getByRole("button", { name: "insert block embed", exact: true })
    .click();
  await pause(550);

  await caption("Editable tables");
  await page.getByRole("button", { name: "insert table", exact: true }).click();
  await pause(400);
  // The caret lands in the first header cell; Tab hops to the next cell.
  await type("Command");
  await page.keyboard.press("Tab");
  await type("Keys");
  await page.keyboard.press("Tab");
  await type("toggle_bold");
  await page.keyboard.press("Tab");
  await type("Mod-B");
  await page.locator(".fixture-frame").evaluate((frame) => {
    frame.scrollTo({ top: frame.scrollHeight, behavior: "smooth" });
  });
  await pause(900);

  for (const selector of [
    ".editor__h1",
    ".e-b",
    ".e-i",
    ".e-s",
    ".e-c",
    ".fixture-link",
    ".fixture-mention",
    ".editor__quote",
    ".editor__ul",
    ".editor__pre",
    ".fixture-embed",
    ".editor__table-wrap",
  ]) {
    try {
      await page
        .locator(selector)
        .first()
        .waitFor({ state: "visible", timeout: 10_000 });
    } catch (error) {
      console.error(`missing demo feature ${selector}`);
      console.error(await editor.innerHTML());
      throw error;
    }
  }

  await caption("Transaction-based · accessible · extensible");
  await pause(1_200);
  await context.close();
  await browser.close();
  browser = undefined;

  const videoPath = await video.path();
  await run("mise", [
    "exec",
    "--",
    "ffmpeg",
    "-y",
    "-ss",
    trimStart.toFixed(3),
    "-i",
    videoPath,
    "-filter_complex",
    "[0:v]fps=10,scale=900:-1:flags=lanczos,split[s0][s1];[s0]palettegen=max_colors=128[p];[s1][p]paletteuse=dither=bayer:bayer_scale=3",
    "-loop",
    "0",
    pendingOutput,
  ]);
  await rename(pendingOutput, output);
  const { size } = await stat(output);
  console.log(`wrote ${output} (${(size / 1024 / 1024).toFixed(1)} MiB)`);
} finally {
  try {
    if (browser) await browser.close();
  } finally {
    if (server.exitCode === null) {
      server.kill("SIGTERM");
      await Promise.race([once(server, "exit"), pause(5_000)]);
      if (server.exitCode === null) server.kill("SIGKILL");
    }
  }
}
