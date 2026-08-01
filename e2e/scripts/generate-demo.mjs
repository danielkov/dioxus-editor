import { spawn } from "node:child_process";
import { once } from "node:events";
import { mkdir, rename, rm, stat } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { chromium } from "@playwright/test";

const root = fileURLToPath(new URL("../..", import.meta.url));
const artifacts = fileURLToPath(new URL("../../target/demo-gif", import.meta.url));
const output = fileURLToPath(new URL("../../docs/editor-demo.gif", import.meta.url));
const pendingOutput = fileURLToPath(new URL("../../target/demo-gif/editor-demo.gif", import.meta.url));
const port = 18084;
const url = `http://127.0.0.1:${port}`;
const pause = (ms = 400) => new Promise((resolve) => setTimeout(resolve, ms));

function run(command, args, options = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { cwd: root, stdio: "inherit", ...options });
    child.once("error", reject);
    child.once("exit", (code) =>
      code === 0 ? resolve() : reject(new Error(`${command} exited with ${code}`)),
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
await mkdir(fileURLToPath(new URL("../../docs", import.meta.url)), { recursive: true });

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
    viewport: { width: 1120, height: 760 },
    colorScheme: "light",
    recordVideo: { dir: artifacts, size: { width: 1120, height: 760 } },
  });
  const recordingStartedAt = Date.now();
  const page = await context.newPage();
  const video = page.video();
  await page.goto(url, { waitUntil: "domcontentloaded" });
  await page.locator("#fixture-root > h1").waitFor({ state: "visible", timeout: 120_000 });

  await page.addStyleTag({
    content: `
      body { padding: 32px; background: linear-gradient(135deg, #f7f4ff, #eef7ff); }
      #fixture-root { max-width: 980px; }
      #fixture-root > h1 { color: #5b34da; font-size: 18px; letter-spacing: .12em; }
      #fixture-root > label, #fixture-root > output, #state-dump, #selection-dump { display: none; }
      .fixture-frame { height: 500px; overflow: auto; border: 1px solid #d8d1ef; border-radius: 14px; padding: 24px 28px; box-shadow: 0 18px 50px rgba(45, 30, 90, .12); }
      .editor { min-height: 450px; font-size: 17px; line-height: 1.65; }
      .editor__h1 { color: #32188f; font-size: 30px; }
      .editor__quote { border-left-color: #7656df; background: #f7f4ff; padding: 8px 14px; }
      .editor__pre, .e-tC, .e-c { background: #171426; color: #f3efff; }
      .editor__link { color: #5432c7; text-decoration: underline; }
      .editor__table { width: 100%; border-collapse: collapse; }
      .editor__table td { min-width: 120px; height: 34px; border: 1px solid #d8d1ef; padding: 5px 8px; }
      .fixture-embed { margin: 8px 0; padding: 14px; border: 1px dashed #7656df; border-radius: 8px; color: #5432c7; background: #faf8ff; }
      .editor__decorator-remove { display: none; }
      .fixture-actions { display: flex; flex-wrap: wrap; gap: 7px; margin-top: 14px; }
      .fixture-actions button { border: 1px solid #d8d1ef; border-radius: 999px; background: white; color: #4a358f; padding: 7px 12px; font: inherit; }
      #demo-caption { position: fixed; top: 28px; right: 32px; z-index: 20; padding: 9px 14px; border-radius: 999px; color: white; background: #5b34da; box-shadow: 0 8px 24px rgba(45, 30, 90, .2); font: 600 14px/1.2 -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }
    `,
  });
  await page.evaluate(() => {
    document.querySelector("#fixture-root > h1").textContent = "Dioxus Editor";
    const caption = document.createElement("div");
    caption.id = "demo-caption";
    caption.textContent = "Rich text, entirely in Rust";
    document.body.append(caption);
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
  await type("A **fast**, _composable_ editor with ~~fragile~~ predictable state and `typed commands`.");
  await enter();

  await caption("Schema-backed links");
  await type("Explore [Dioxus](https://dioxuslabs.com) without leaving Rust.");
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
  await page.getByRole("button", { name: "insert block embed", exact: true }).click();
  await pause(550);

  await caption("Editable tables");
  await page.getByRole("button", { name: "insert table", exact: true }).click();
  await pause(900);

  for (const selector of [
    ".editor__h1",
    ".e-tB, .e-b",
    ".e-tI, .e-i",
    ".e-tS, .e-s",
    ".e-tC, .e-c",
    ".editor__link",
    ".editor__quote",
    ".editor__ul",
    ".editor__pre",
    ".fixture-embed",
    ".editor__table-wrap",
  ]) {
    try {
      await page.locator(selector).first().waitFor({ state: "visible", timeout: 10_000 });
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
