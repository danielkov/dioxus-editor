import { expect, Page } from "@playwright/test";

/**
 * Mount a fresh editor and return with the caret seated at the start of
 * the empty paragraph. The fixture is keyed by the model — the empty
 * tree dump is the cue that focus has settled.
 */
export async function freshEditor(page: Page) {
  await page.goto("/", { waitUntil: "domcontentloaded" });
  await page.getByRole("textbox", { name: "Rich text editor" }).click();
  await expect(page.getByLabel("Document state")).toHaveText("(doc (paragraph))");
}

/** Read the structural model dump (one-shot — racy if a render is pending). */
export async function dump(page: Page): Promise<string> {
  return (await page.getByLabel("Document state").textContent()) ?? "";
}

/** Polling-based "the model is exactly this" assertion. */
export async function expectDump(page: Page, expected: string) {
  await expect(page.getByLabel("Document state")).toHaveText(expected);
}

/** Read the model selection dump. */
export async function selDump(page: Page): Promise<string> {
  return (await page.getByLabel("Selection state").textContent()) ?? "";
}

/**
 * Type via the real keyboard so `beforeinput` fires the same way as a
 * real user. `locator.fill` sets DOM text directly and bypasses the
 * editor model, so we never use it here.
 */
export async function typeText(page: Page, text: string) {
  await page.keyboard.type(text, { delay: 5 });
}

/** Dispatch a browser editing intent without relying on OS key mappings. */
export async function beforeInput(page: Page, inputType: string) {
  await page.getByRole("textbox", { name: "Rich text editor" }).evaluate((editor, type) => {
    editor.dispatchEvent(
      new InputEvent("beforeinput", {
        bubbles: true,
        cancelable: true,
        inputType: type,
      }),
    );
  }, inputType);
}

/**
 * Pin the caret to offset 0 of the text node the model says it's in.
 * Sets the DOM selection directly on that exact text DOM node — using
 * arrow keys would silently overshoot past the block boundary on the
 * first extra ArrowLeft.
 */
export async function caretToTextStart(page: Page) {
  const d = await selDump(page);
  const m = /caret\((\d+),\s*Text,\s*\d+\)/.exec(d);
  if (!m) throw new Error(`caretToTextStart: expected a Text caret, got ${d}`);
  const key = m[1];
  await page.evaluate((k) => {
    const span = document.querySelector<HTMLElement>(`span[data-key="${k}"]`);
    const text = span?.firstChild;
    if (!text) return;
    const sel = window.getSelection();
    const r = document.createRange();
    r.setStart(text, 0);
    r.collapse(true);
    sel?.removeAllRanges();
    sel?.addRange(r);
  }, key);
  await expect.poll(() => selDump(page)).toBe(`caret(${key}, Text, 0)`);
}

/**
 * Pin the caret to a CHAR offset within the text node the model says it's
 * currently in. The DOM Range API works in UTF-16 code units; this helper
 * translates from the editor's char-counted model offsets to the DOM's
 * UTF-16 offsets so emoji / astral-plane characters are handled the same
 * way the model treats them.
 */
export async function caretToTextOffset(page: Page, charOffset: number) {
  const d = await selDump(page);
  const m = /caret\((\d+),\s*Text,\s*\d+\)/.exec(d);
  if (!m) throw new Error(`caretToTextOffset: expected a Text caret, got ${d}`);
  const key = m[1];
  await page.evaluate(
    ([k, want]) => {
      const span = document.querySelector<HTMLElement>(
        `span[data-key="${k}"]`,
      );
      const text = span?.firstChild;
      if (!text) return;
      const s = text.nodeValue ?? "";
      let utf16 = 0;
      let chars = 0;
      for (const ch of s) {
        if (chars >= (want as number)) break;
        utf16 += ch.length; // surrogate pair → 2, otherwise → 1
        chars += 1;
      }
      const sel = window.getSelection();
      const r = document.createRange();
      r.setStart(text, utf16);
      r.collapse(true);
      sel?.removeAllRanges();
      sel?.addRange(r);
    },
    [key, charOffset],
  );
  await expect
    .poll(() => selDump(page))
    .toBe(`caret(${key}, Text, ${charOffset})`);
}

/**
 * Extend the selection across `[start, end)` characters inside the
 * current text node. Asserts the exact range dump form before returning.
 */
export async function selectChars(page: Page, start: number, end: number) {
  // Accept either a caret or a range as the starting state — toolbar-
  // command tests already leave the dump as `range(...)` from a prior
  // selection, but we want to re-select against the same text node key.
  const d = await selDump(page);
  const m =
    /caret\((\d+),\s*Text,\s*\d+\)/.exec(d) ||
    /range\((\d+)\/Text\/\d+\s*->\s*\d+\/Text\/\d+\)/.exec(d);
  if (!m) throw new Error(`selectChars: expected a Text caret/range, got ${d}`);
  const key = m[1];
  await page.evaluate(
    ([s, e]) => {
      const sel = window.getSelection();
      if (!sel) return;
      let node: Node | null = sel.anchorNode;
      while (node && node.nodeType !== Node.TEXT_NODE) {
        node = (node as HTMLElement).firstChild;
      }
      if (!node) return;
      const range = document.createRange();
      range.setStart(node, s);
      range.setEnd(node, e);
      sel.removeAllRanges();
      sel.addRange(range);
    },
    [start, end],
  );
  await expect
    .poll(() => selDump(page))
    .toBe(`range(${key}/Text/${start} -> ${key}/Text/${end})`);
}

/**
 * Select `[start, end)` chars across the *first* block in the editor,
 * walking through child text nodes as needed. Use this when a previous
 * operation has split a single text node into several (e.g. after a
 * mark toggle creates a formatted span between two plain spans).
 */
export async function selectBlockChars(
  page: Page,
  start: number,
  end: number,
) {
  await page.evaluate(
    ([s, e]) => {
      const block = document.querySelector(
        ".editor > p, .editor > h1, .editor > h2, .editor > h3, .editor > h4, .editor > h5, .editor > h6, .editor > blockquote, .editor > pre, .editor > ul > li, .editor > ol > li",
      );
      if (!block) return;
      let acc = 0;
      let startNode: Node | null = null;
      let startOff = 0;
      let endNode: Node | null = null;
      let endOff = 0;
      for (const span of block.querySelectorAll("span")) {
        const text = (span as HTMLElement).firstChild;
        if (!text || text.nodeType !== Node.TEXT_NODE) continue;
        const len = (text.nodeValue ?? "").length;
        if (startNode === null && acc + len >= (s as number)) {
          startNode = text;
          startOff = (s as number) - acc;
        }
        if (endNode === null && acc + len >= (e as number)) {
          endNode = text;
          endOff = (e as number) - acc;
        }
        acc += len;
        if (startNode && endNode) break;
      }
      if (!startNode || !endNode) return;
      const sel = window.getSelection();
      const r = document.createRange();
      r.setStart(startNode, startOff);
      r.setEnd(endNode, endOff);
      sel?.removeAllRanges();
      sel?.addRange(r);
    },
    [start, end],
  );
  await expect.poll(() => selDump(page)).toMatch(/^range\(/);
}

/**
 * Select all editor content using the contenteditable root. Bypasses
 * Cmd+A on macOS which is unreliable inside headless Chromium for
 * Dioxus's contenteditable.
 */
export async function selectAll(page: Page) {
  await page.evaluate(() => {
    const ed = document.querySelector(".editor")!;
    const sel = window.getSelection();
    const r = document.createRange();
    r.selectNodeContents(ed);
    sel?.removeAllRanges();
    sel?.addRange(r);
  });
  await expect.poll(() => selDump(page)).toMatch(/^range\(/);
}
