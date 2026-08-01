import { test, expect, Page } from "@playwright/test";

// -- helpers --------------------------------------------------------------

async function freshEditor(page: Page) {
  await page.goto("/");
  // The fixture mounts on the body; click into the editor so it has focus
  // and the model installs a caret in the first block.
  await page.locator(".editor").click();
  // Wait until the structural dump shows the empty initial doc.
  await expect(page.locator("#state-dump")).toHaveText(/^\(doc \(paragraph\)\)$/);
}

async function dump(page: Page): Promise<string> {
  return (await page.locator("#state-dump").textContent()) ?? "";
}

async function typeText(page: Page, text: string) {
  // Always go through the keyboard so beforeinput fires the same way as a
  // real user — `locator.fill` would set DOM text directly and bypass the
  // model.
  await page.keyboard.type(text, { delay: 5 });
}

/**
 * Move the caret to offset 0 of the text node that currently holds the
 * caret. Doing this via key presses is unreliable across platforms (macOS
 * `Home` and `Cmd+Left` do not trigger inside a contenteditable in
 * headless Chromium), so we set the DOM selection directly — the editor's
 * `selectionchange` listener mirrors it back into the model.
 */
/**
 * Pin the caret to offset 0 of the text node it currently sits in. Sets
 * the DOM selection directly on the same text DOM node (avoids the
 * arrow-key approach, which would cross the block boundary on the first
 * extra ArrowLeft and silently land the caret in the previous block).
 * Then waits for the model dump to reflect the exact same key + offset.
 */
async function caretToTextStart(page: Page) {
  const dump = (await page.locator("#selection-dump").textContent()) ?? "";
  const m = /caret\((\d+),\s*Text,\s*\d+\)/.exec(dump);
  if (!m) {
    throw new Error(`caretToTextStart: expected a Text caret in ${dump}`);
  }
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
  await expect
    .poll(async () => (await page.locator("#selection-dump").textContent()) ?? "")
    .toBe(`caret(${key}, Text, 0)`);
}

// -- typing into a fresh editor ------------------------------------------

test.describe("typing", () => {
  test("plain text lands inside the lone paragraph", async ({ page }) => {
    await freshEditor(page);
    await typeText(page, "hello");
    await expect(page.locator("#state-dump")).toHaveText('(doc (paragraph (text "hello")))');
    // DOM mirror: a single span inside one paragraph.
    expect(await page.locator(".editor > *").count()).toBe(1);
    expect(await page.locator(".editor > p > span").innerText()).toBe("hello");
  });

  test("special characters do not leak to the root", async ({ page }) => {
    await freshEditor(page);
    await typeText(page, ">");
    await expect(page.locator("#state-dump")).toHaveText('(doc (paragraph (text ">")))');
    expect(await page.locator(".editor > p").count()).toBe(1);
    expect(await page.locator(".editor > span").count()).toBe(0);
  });

  test("split + typing → continues inside the new paragraph", async ({
    page,
  }) => {
    // Regression-adjacent: the very next character after a structural
    // change must land inside a block, not as a root-level sibling.
    await freshEditor(page);
    await typeText(page, "first");
    await page.keyboard.press("Shift+Enter");
    await typeText(page, "second");
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text "first")) (paragraph (text "second")))',
    );
  });
});

// -- markdown shortcuts ---------------------------------------------------

test.describe("markdown block shortcuts", () => {
  test("> space converts paragraph to blockquote", async ({ page }) => {
    await freshEditor(page);
    await typeText(page, "> hello");
    await expect(page.locator("#state-dump")).toHaveText('(doc (blockquote (text "hello")))');
  });

  test("# space promotes to h1", async ({ page }) => {
    await freshEditor(page);
    await typeText(page, "# title");
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (heading :level 1 (text "title")))',
    );
  });

  test("## space promotes to h2", async ({ page }) => {
    await freshEditor(page);
    await typeText(page, "## sub");
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (heading :level 2 (text "sub")))',
    );
  });

  test("- space wraps in bullet list", async ({ page }) => {
    await freshEditor(page);
    await typeText(page, "- one");
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (bullet_list (list_item (text "one"))))',
    );
  });

  test("1. space wraps in ordered list", async ({ page }) => {
    await freshEditor(page);
    await typeText(page, "1. step");
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (ordered_list (list_item (text "step"))))',
    );
  });

  test("block shortcut mid-text does not fire", async ({ page }) => {
    await freshEditor(page);
    await typeText(page, "hello > world");
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text "hello > world")))',
    );
  });
});

test.describe("markdown inline shortcuts", () => {
  test("**bold**", async ({ page }) => {
    await freshEditor(page);
    await typeText(page, "a **b** c");
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text "a ") (text :fmt B "b") (text " c")))',
    );
  });

  test("_italic_", async ({ page }) => {
    await freshEditor(page);
    await typeText(page, "a _b_ c");
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text "a ") (text :fmt I "b") (text " c")))',
    );
  });

  test("`code`", async ({ page }) => {
    await freshEditor(page);
    await typeText(page, "call `fn` x");
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text "call ") (text :fmt C "fn") (text " x")))',
    );
  });

  test("~~strike~~", async ({ page }) => {
    await freshEditor(page);
    await typeText(page, "a ~~b~~ c");
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text "a ") (text :fmt S "b") (text " c")))',
    );
  });
});

// -- Shift+Enter in each block kind --------------------------------------

test.describe("shift+enter", () => {
  test("splits paragraph", async ({ page }) => {
    await freshEditor(page);
    await typeText(page, "hello");
    await page.keyboard.press("Shift+Enter");
    await typeText(page, "world");
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text "hello")) (paragraph (text "world")))',
    );
  });

  test("inside heading demotes new block to paragraph", async ({ page }) => {
    await freshEditor(page);
    await typeText(page, "# title");
    await page.keyboard.press("Shift+Enter");
    await typeText(page, "body");
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (heading :level 1 (text "title")) (paragraph (text "body")))',
    );
  });

  test("inside blockquote splits into another blockquote", async ({
    page,
  }) => {
    await freshEditor(page);
    await typeText(page, "> one");
    await page.keyboard.press("Shift+Enter");
    await typeText(page, "two");
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (blockquote (text "one")) (blockquote (text "two")))',
    );
  });

  test("inside list creates a new list item, not a new list", async ({
    page,
  }) => {
    // Regression: split_block used to split the outer `bullet_list`
    // rather than the inner `list_item`, producing two adjacent lists.
    await freshEditor(page);
    await typeText(page, "- one");
    await page.keyboard.press("Shift+Enter");
    await typeText(page, "two");
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (bullet_list (list_item (text "one")) (list_item (text "two"))))',
    );
  });

  test("inside heading promotes new block to paragraph (no toolbar in fixture)", async ({
    page,
  }) => {
    // Code-block Shift+Enter newline behavior is exercised by the Rust
    // scenarios (the fixture has no toolbar to toggle code_block). Here
    // we pin down the closely-related heading → paragraph demotion that
    // shares the split_block code path.
    await freshEditor(page);
    await typeText(page, "# code");
    await page.keyboard.press("Shift+Enter");
    await typeText(page, "more");
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (heading :level 1 (text "code")) (paragraph (text "more")))',
    );
  });
});

// -- backspace behaviour --------------------------------------------------

test.describe("backspace", () => {
  test("at end of text deletes the previous character", async ({ page }) => {
    await freshEditor(page);
    await typeText(page, "hello");
    await page.keyboard.press("Backspace");
    await expect(page.locator("#state-dump")).toHaveText('(doc (paragraph (text "hell")))');
  });

  test("at start of heading demotes to paragraph", async ({ page }) => {
    await freshEditor(page);
    await typeText(page, "# title");
    // Move caret to start.
    await caretToTextStart(page);
    await page.keyboard.press("Backspace");
    await expect(page.locator("#state-dump")).toHaveText('(doc (paragraph (text "title")))');
  });

  test("at start of blockquote demotes to paragraph", async ({ page }) => {
    await freshEditor(page);
    await typeText(page, "> body");
    await caretToTextStart(page);
    await page.keyboard.press("Backspace");
    await expect(page.locator("#state-dump")).toHaveText('(doc (paragraph (text "body")))');
  });

  test("joins empty second paragraph back to the first", async ({ page }) => {
    await freshEditor(page);
    await typeText(page, "first");
    await page.keyboard.press("Shift+Enter");
    await page.keyboard.press("Backspace");
    await expect(page.locator("#state-dump")).toHaveText('(doc (paragraph (text "first")))');
  });

  test("at start of second list item joins items, not list", async ({
    page,
  }) => {
    await freshEditor(page);
    await typeText(page, "- one");
    await page.keyboard.press("Shift+Enter");
    await typeText(page, "two");
    await caretToTextStart(page);
    await page.keyboard.press("Backspace");
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (bullet_list (list_item (text "onetwo"))))',
    );
  });

  test("series of edits produces exactly the expected tree", async ({
    page,
  }) => {
    // The "extra row" / "bricked input" symptom presented as a text node
    // sitting as a direct child of `(doc …)`. Pin down the full expected
    // tree after a non-trivial sequence — any drift (including a stray
    // root text node) breaks the assertion exactly. The trailing block
    // is a second blockquote (not a list) because block-level shortcuts
    // only fire inside a top-level paragraph; "- " typed inside a
    // blockquote stays literal.
    await freshEditor(page);
    await typeText(page, "abc");
    await page.keyboard.press("Shift+Enter");
    await typeText(page, "> quoted");
    await page.keyboard.press("Shift+Enter");
    await typeText(page, "- item");
    await page.keyboard.press("Backspace");
    await page.keyboard.press("Backspace");
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text "abc")) (blockquote (text "quoted")) (blockquote (text "- it")))',
    );
  });
});

// -- range selection -----------------------------------------------------

/**
 * Programmatically extend the DOM selection across `[start, end)` chars
 * within the current text node, then wait for the model dump to mirror the
 * range exactly.
 */
async function selectChars(page: Page, start: number, end: number) {
  const dumpBefore =
    (await page.locator("#selection-dump").textContent()) ?? "";
  const m = /caret\((\d+),\s*Text,\s*\d+\)/.exec(dumpBefore);
  // The selection-dump format for ranges is
  //   `range(<key>/Text/<anchor> -> <key>/Text/<focus>)`.
  // We know the current text-node key from the caret form, so we can
  // assert the exact string after extending the selection.
  if (!m) throw new Error(`expected a caret in dump, got: ${dumpBefore}`);
  const key = m[1];
  await page.evaluate(
    ([s, e]) => {
      const sel = window.getSelection();
      if (!sel || sel.rangeCount === 0) return;
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
    .poll(async () => (await page.locator("#selection-dump").textContent()) ?? "")
    .toBe(`range(${key}/Text/${start} -> ${key}/Text/${end})`);
}

test.describe("range selection", () => {
  test("backspace on a range inside a paragraph deletes the range", async ({
    page,
  }) => {
    await freshEditor(page);
    await typeText(page, "hello world");
    await selectChars(page, 0, 5);
    await page.keyboard.press("Backspace");
    await expect(page.locator("#state-dump")).toHaveText('(doc (paragraph (text " world")))');
  });

  test("backspace on a range inside a blockquote deletes the range", async ({
    page,
  }) => {
    await freshEditor(page);
    await typeText(page, "> quoted text here");
    await selectChars(page, 7, 11);
    await page.keyboard.press("Backspace");
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (blockquote (text "quoted  here")))',
    );
  });

  test("delete on a range inside a paragraph deletes the range", async ({
    page,
  }) => {
    await freshEditor(page);
    await typeText(page, "abcdef");
    await selectChars(page, 1, 5);
    await page.keyboard.press("Delete");
    await expect(page.locator("#state-dump")).toHaveText('(doc (paragraph (text "af")))');
  });

  test("typing into a range replaces it", async ({ page }) => {
    await freshEditor(page);
    await typeText(page, "old text");
    await selectChars(page, 0, 3);
    await typeText(page, "new");
    await expect(page.locator("#state-dump")).toHaveText('(doc (paragraph (text "new text")))');
  });

  test("backspace on a cross-block range merges the two blocks", async ({
    page,
  }) => {
    await freshEditor(page);
    await typeText(page, "first");
    await page.keyboard.press("Shift+Enter");
    await typeText(page, "second");
    // Select from char 3 of "first" to char 3 of "second".
    // Snapshot the keys of the two text nodes for an exact selection-dump
    // assertion. The dump prints node keys; learning them lets the test
    // be deterministic instead of regex-shaped.
    const keys = await page.evaluate(() => {
      const ps = document.querySelectorAll(".editor p span");
      return [
        ps[0].getAttribute("data-key"),
        ps[1].getAttribute("data-key"),
      ] as [string, string];
    });
    await page.evaluate(() => {
      const ps = document.querySelectorAll(".editor p span");
      const a = ps[0].firstChild!;
      const b = ps[1].firstChild!;
      const sel = window.getSelection();
      const r = document.createRange();
      r.setStart(a, 3);
      r.setEnd(b, 3);
      sel?.removeAllRanges();
      sel?.addRange(r);
    });
    await expect
      .poll(async () => (await page.locator("#selection-dump").textContent()) ?? "")
      .toBe(`range(${keys[0]}/Text/3 -> ${keys[1]}/Text/3)`);
    await page.keyboard.press("Backspace");
    await expect(page.locator("#state-dump")).toHaveText('(doc (paragraph (text "firond")))');
  });

  test("backspace on a fully selected blockquote + paragraph leaves one empty paragraph", async ({
    page,
  }) => {
    // Mirrors the user's reported "selected lines + Backspace" scenario.
    // Build: blockquote "hello there" + paragraph "second line".
    await freshEditor(page);
    await typeText(page, "> hello there");
    await page.keyboard.press("Shift+Enter");
    await typeText(page, "more");
    // The Shift+Enter created a second blockquote with "more". Demote it
    // to a paragraph: move caret to start of "more" and Backspace once.
    await page.evaluate(() => {
      const blocks = document.querySelectorAll(".editor blockquote");
      const t = blocks[1].querySelector("span")?.firstChild;
      if (!t) return;
      const sel = window.getSelection();
      const r = document.createRange();
      r.setStart(t, 0);
      r.collapse(true);
      sel?.removeAllRanges();
      sel?.addRange(r);
    });
    await expect
      .poll(async () => (await page.locator("#selection-dump").textContent()) ?? "")
      .toMatch(/^caret\(\d+, Text, 0\)$/);
    await page.keyboard.press("Backspace");
    // Sanity check the setup before the real assertion.
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (blockquote (text "hello there")) (paragraph (text "more")))',
    );
    // Now select from start of blockquote text to end of paragraph text
    // and Backspace — the two should collapse into one blockquote.
    await page.evaluate(() => {
      const first = document
        .querySelector(".editor blockquote span")
        ?.firstChild;
      const last = document.querySelector(".editor p span")?.firstChild;
      if (!first || !last) return;
      const sel = window.getSelection();
      const r = document.createRange();
      r.setStart(first, 0);
      r.setEnd(last, last.nodeValue?.length ?? 0);
      sel?.removeAllRanges();
      sel?.addRange(r);
    });
    await expect
      .poll(async () => (await page.locator("#selection-dump").textContent()) ?? "")
      .toMatch(/^range\(/);
    await page.keyboard.press("Backspace");
    // Per the "no residual block kind for fully-consumed nodes" rule, an
    // emptied blockquote is replaced by a paragraph — not preserved as
    // an empty blockquote the user can't dismiss without an extra
    // keypress.
    await expect(page.locator("#state-dump")).toHaveText("(doc (paragraph))");
  });

  test("select-all + backspace empties the editor to a single empty paragraph", async ({
    page,
  }) => {
    await freshEditor(page);
    await typeText(page, "line one");
    await page.keyboard.press("Shift+Enter");
    await typeText(page, "line two");
    await page.evaluate(() => {
      const ed = document.querySelector(".editor")!;
      const sel = window.getSelection();
      const r = document.createRange();
      r.selectNodeContents(ed);
      sel?.removeAllRanges();
      sel?.addRange(r);
    });
    await page.keyboard.press("Backspace");
    await expect(page.locator("#state-dump")).toHaveText("(doc (paragraph))");
  });
});

// -- console hygiene -----------------------------------------------------

test("no unexpected console errors during a normal session", async ({
  page,
}) => {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  page.on("console", (msg) => {
    if (msg.type() === "error") errors.push(msg.text());
  });
  await freshEditor(page);
  await typeText(page, "hello ");
  await typeText(page, "**world**");
  await page.keyboard.press("Shift+Enter");
  await typeText(page, "> quoted");
  await page.keyboard.press("Shift+Enter");
  await typeText(page, "- item");
  await page.keyboard.press("Backspace");
  await page.keyboard.press("Shift+Enter");
  await typeText(page, "next");
  expect(errors, errors.join("\n")).toEqual([]);
});
