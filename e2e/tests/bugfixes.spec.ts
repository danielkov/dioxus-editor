// Regression coverage for three editor bugs:
//
//   1. Backspace in an empty bullet/ordered list should clear the list
//      (lift the lone item back to a paragraph) instead of no-oping.
//   2. Cmd+Backspace inside a list_item should remove the entire line +
//      its bullet, not just the line's text content.
//   3. The `block_embed` decorator (stand-in for a block embed /
//      file cards) must render as a block element at root level — never
//      as a nested span inside the surrounding paragraph.

import { test, expect } from "@playwright/test";
import {
  expectDump,
  freshEditor,
  typeText,
} from "./helpers";

test.describe("backspace in empty list clears the list", () => {
  test("immediately after `- ` shortcut", async ({ page }) => {
    // `- ` creates a bullet_list whose only item has an empty text child.
    // The single Backspace press must outdent that lone item to a fresh
    // paragraph — anything else strands the user inside an unreachable
    // empty bullet.
    await freshEditor(page);
    await typeText(page, "- ");
    await expectDump(page, "(doc (bullet_list (list_item (text \"\"))))");
    await page.keyboard.press("Backspace");
    await expectDump(page, "(doc (paragraph))");
    // DOM mirror: no list element should remain on screen.
    await expect(page.locator(".editor ul")).toHaveCount(0);
    await expect(page.locator(".editor ol")).toHaveCount(0);
  });

  test("after typing then deleting the last char in a single bullet", async ({
    page,
  }) => {
    // Type `- a`, delete the `a`, then press Backspace again. The empty
    // text node may have been pruned, leaving an element-anchored caret —
    // the second Backspace still needs to clear the list.
    await freshEditor(page);
    await typeText(page, "- a");
    await expectDump(
      page,
      "(doc (bullet_list (list_item (text \"a\"))))",
    );
    await page.keyboard.press("Backspace");
    await expectDump(page, "(doc (bullet_list (list_item)))");
    await page.keyboard.press("Backspace");
    await expectDump(page, "(doc (paragraph))");
    await expect(page.locator(".editor ul")).toHaveCount(0);
  });

  test("works the same for ordered lists", async ({ page }) => {
    await freshEditor(page);
    await typeText(page, "1. ");
    await expectDump(
      page,
      "(doc (ordered_list (list_item (text \"\"))))",
    );
    await page.keyboard.press("Backspace");
    await expectDump(page, "(doc (paragraph))");
    await expect(page.locator(".editor ol")).toHaveCount(0);
  });
});

test.describe("backspace in empty blockquote / code_block demotes to paragraph", () => {
  test("empty blockquote demotes on Backspace", async ({ page }) => {
    // Markdown shortcut leaves an empty text inside the blockquote; this
    // is the same state the toolbar toggle reaches a moment later.
    await freshEditor(page);
    await typeText(page, "> ");
    await expectDump(page, "(doc (blockquote (text \"\")))");
    await page.keyboard.press("Backspace");
    await expectDump(page, "(doc (paragraph))");
  });

  test("empty code_block demotes on Backspace", async ({ page }) => {
    await freshEditor(page);
    await page.locator("#toggle-code-block").click();
    await expectDump(page, "(doc (code_block))");
    await page.keyboard.press("Backspace");
    await expectDump(page, "(doc (paragraph))");
  });
});

test.describe("Cmd+Backspace clears the list line", () => {
  test("removes a middle list item entirely", async ({ page }) => {
    // Cmd+Backspace surfaces as the `deleteSoftLineBackward` /
    // `deleteHardLineBackward` inputType in `beforeinput`. Inside a
    // list_item that maps to "remove the whole line including its
    // marker" — the bullet is part of the line.
    await freshEditor(page);
    await typeText(page, "- one");
    await page.keyboard.press("Shift+Enter");
    await typeText(page, "two");
    await page.keyboard.press("Shift+Enter");
    await typeText(page, "three");
    // Caret is in "three" — move caret into "two" mid-text and fire the
    // cmd+backspace shortcut. We dispatch a `beforeinput` event with the
    // right inputType because Playwright's `Meta+Backspace` does not
    // emit the inputType reliably across browsers.
    await page.evaluate(() => {
      const items = document.querySelectorAll<HTMLLIElement>(
        ".editor ul > li",
      );
      const text = items[1]?.querySelector("span")?.firstChild;
      if (!text) throw new Error("expected 3 list items with text spans");
      const sel = window.getSelection();
      const r = document.createRange();
      r.setStart(text, 2);
      r.collapse(true);
      sel?.removeAllRanges();
      sel?.addRange(r);
    });
    await expect.poll(async () => await deleteToLineStart(page)).toBe(true);
    await expectDump(
      page,
      "(doc (bullet_list (list_item (text \"one\")) (list_item (text \"three\"))))",
    );
  });

  test("removes the only list item and collapses the list", async ({
    page,
  }) => {
    await freshEditor(page);
    await typeText(page, "- only");
    await expect.poll(async () => await deleteToLineStart(page)).toBe(true);
    await expectDump(page, "(doc (paragraph))");
    await expect(page.locator(".editor ul")).toHaveCount(0);
  });

  test("inside a blockquote removes the entire block", async ({ page }) => {
    await freshEditor(page);
    await typeText(page, "> hello");
    await expect.poll(async () => await deleteToLineStart(page)).toBe(true);
    await expectDump(page, "(doc (paragraph))");
  });

  test("inside a code_block removes the entire block", async ({ page }) => {
    await freshEditor(page);
    await page.locator("#toggle-code-block").click();
    await typeText(page, "code");
    await expect.poll(async () => await deleteToLineStart(page)).toBe(true);
    await expectDump(page, "(doc (paragraph))");
  });

  test("inside a paragraph still clears to start of line, not removes the block", async ({
    page,
  }) => {
    // Outside of a list, cmd+backspace keeps the standard semantics —
    // delete from caret back to the start of the current block.
    await freshEditor(page);
    await typeText(page, "hello world");
    // Caret currently at end of "hello world". Move to offset 6 (between
    // "hello " and "world").
    await page.evaluate(() => {
      const span = document.querySelector(".editor > p > span");
      const text = span?.firstChild;
      if (!text) throw new Error("expected one text span in paragraph");
      const sel = window.getSelection();
      const r = document.createRange();
      r.setStart(text, 6);
      r.collapse(true);
      sel?.removeAllRanges();
      sel?.addRange(r);
    });
    await expect.poll(async () => await deleteToLineStart(page)).toBe(true);
    await expectDump(page, "(doc (paragraph (text \"world\")))");
  });
});

test.describe("arrow navigation reaches every empty list item", () => {
  test("ArrowUp from last empty bullet lands in the prior empty bullet, not skipping it", async ({
    page,
  }) => {
    // The bug: two consecutive empty `<li>` rendered with only a layout
    // placeholder are not navigable by ArrowUp/Down — Chromium's caret
    // traversal skips elements without an editable anchor. The fix is to
    // render a real `<br>` placeholder inside empty leaf blocks; this
    // test pins that behavior down so a later "CSS-only" attempt at the
    // same problem regresses loudly.
    await freshEditor(page);
    await typeText(page, "- one");
    await page.keyboard.press("Shift+Enter");
    await typeText(page, "two");
    await page.keyboard.press("Shift+Enter");
    await page.keyboard.press("Shift+Enter");
    // Now in the fourth empty bullet. ArrowUp must land in the third
    // empty bullet, not jump over to "two".
    await page.keyboard.press("ArrowUp");
    const landingItem = await page.evaluate(() => {
      const sel = window.getSelection();
      const lis = document.querySelectorAll(".editor ul > li");
      for (let i = 0; i < lis.length; i++) {
        if (lis[i].contains(sel?.anchorNode || null)) return i;
      }
      return -1;
    });
    expect(landingItem).toBe(2);
  });
});

test.describe("backspace in empty list_item splits/lifts at position", () => {
  test("empty middle bullet → list splits into two lists around a paragraph", async ({
    page,
  }) => {
    await freshEditor(page);
    await typeText(page, "- one");
    await page.keyboard.press("Shift+Enter");
    await page.keyboard.press("Shift+Enter");
    await typeText(page, "three");
    // Currently in item 3 ("three"). Move caret to the empty middle
    // item (item 2).
    await page.evaluate(() => {
      const lis = document.querySelectorAll(".editor ul > li");
      const r = document.createRange();
      r.setStart(lis[1], 0);
      r.collapse(true);
      const sel = window.getSelection()!;
      sel.removeAllRanges();
      sel.addRange(r);
    });
    await page.keyboard.press("Backspace");
    await expectDump(
      page,
      '(doc (bullet_list (list_item (text "one"))) (paragraph) (bullet_list (list_item (text "three"))))',
    );
  });

  test("empty trailing bullet → trailing paragraph, caret on the new line", async ({
    page,
  }) => {
    await freshEditor(page);
    await typeText(page, "- one");
    await page.keyboard.press("Shift+Enter");
    await typeText(page, "two");
    await page.keyboard.press("Shift+Enter");
    // Caret in fresh empty 3rd item.
    await page.keyboard.press("Backspace");
    await expectDump(
      page,
      '(doc (bullet_list (list_item (text "one")) (list_item (text "two"))) (paragraph))',
    );
  });
});

test.describe("toggle list only affects the caret's line", () => {
  test("toggle bullet on item 3 of a 3-item list lifts only item 3", async ({
    page,
  }) => {
    await freshEditor(page);
    await typeText(page, "- one");
    await page.keyboard.press("Shift+Enter");
    await typeText(page, "two");
    await page.keyboard.press("Shift+Enter");
    await typeText(page, "three");
    // Caret in item 3. Click the bullet toolbar via the fixture button.
    await page.locator("#toggle-bullet-list").click();
    await expectDump(
      page,
      '(doc (bullet_list (list_item (text "one")) (list_item (text "two"))) (paragraph (text "three")))',
    );
  });

  test("toggle bullet on middle item splits the list around a paragraph", async ({
    page,
  }) => {
    await freshEditor(page);
    await typeText(page, "- one");
    await page.keyboard.press("Shift+Enter");
    await typeText(page, "two");
    await page.keyboard.press("Shift+Enter");
    await typeText(page, "three");
    // Move caret into "two".
    await page.evaluate(() => {
      const lis = document.querySelectorAll(".editor ul > li");
      const r = document.createRange();
      const text = lis[1].querySelector("span")?.firstChild;
      r.setStart(text!, 0);
      r.collapse(true);
      const sel = window.getSelection()!;
      sel.removeAllRanges();
      sel.addRange(r);
    });
    await page.locator("#toggle-bullet-list").click();
    await expectDump(
      page,
      '(doc (bullet_list (list_item (text "one"))) (paragraph (text "two")) (bullet_list (list_item (text "three"))))',
    );
  });
});

test.describe("select-all + backspace clears any block to a clean paragraph", () => {
  test("from inside a bullet list", async ({ page }) => {
    await freshEditor(page);
    await typeText(page, "- one");
    await page.keyboard.press("Shift+Enter");
    await typeText(page, "two");
    await page.evaluate(() => {
      const ed = document.querySelector(".editor")!;
      const sel = window.getSelection()!;
      const r = document.createRange();
      r.selectNodeContents(ed);
      sel.removeAllRanges();
      sel.addRange(r);
    });
    await page.keyboard.press("Backspace");
    await expectDump(page, "(doc (paragraph))");
  });

  test("from inside a blockquote", async ({ page }) => {
    await freshEditor(page);
    await typeText(page, "> hello");
    await page.evaluate(() => {
      const ed = document.querySelector(".editor")!;
      const sel = window.getSelection()!;
      const r = document.createRange();
      r.selectNodeContents(ed);
      sel.removeAllRanges();
      sel.addRange(r);
    });
    await page.keyboard.press("Backspace");
    await expectDump(page, "(doc (paragraph))");
  });

  test("from inside a code block", async ({ page }) => {
    await freshEditor(page);
    await page.locator("#toggle-code-block").click();
    await typeText(page, "code");
    await page.evaluate(() => {
      const ed = document.querySelector(".editor")!;
      const sel = window.getSelection()!;
      const r = document.createRange();
      r.selectNodeContents(ed);
      sel.removeAllRanges();
      sel.addRange(r);
    });
    await page.keyboard.press("Backspace");
    await expectDump(page, "(doc (paragraph))");
  });

  test("range across two list items merges them", async ({ page }) => {
    // The mid-list cross-item delete previously bailed because the
    // simple cross-block routine required both endpoints to be direct
    // children of doc.root. With the nested routine in place, selecting
    // across two items joins them into one item.
    await freshEditor(page);
    await typeText(page, "- abcd");
    await page.keyboard.press("Shift+Enter");
    await typeText(page, "wxyz");
    await page.evaluate(() => {
      const ed = document.querySelector(".editor")!;
      const spans = ed.querySelectorAll("li > span");
      const a = spans[0].firstChild!;
      const b = spans[1].firstChild!;
      const r = document.createRange();
      r.setStart(a, 2);
      r.setEnd(b, 2);
      const sel = window.getSelection()!;
      sel.removeAllRanges();
      sel.addRange(r);
    });
    await page.keyboard.press("Backspace");
    // Both items partially intersected → kind survives. Items 1 and 2
    // merge across the deleted line break, matching common editor behavior
    // for cross-paragraph delete.
    await expectDump(
      page,
      '(doc (bullet_list (list_item (text "abyz"))))',
    );
  });
});

test.describe("backspace at element-anchored caret", () => {
  test("after pruning a trailing text node, backspace deletes one char per press from the previous sibling", async ({
    page,
  }) => {
    // Repro skeleton: a paragraph with three text siblings, where
    // deleting through the last one (via plain Backspace) leaves the
    // caret element-anchored at the boundary of two surviving spans.
    // Subsequent Backspace used to remove an entire sibling text node
    // per press; now it must trim one char at a time.
    await freshEditor(page);
    await typeText(page, "ab `cd` ef");
    await expectDump(
      page,
      '(doc (paragraph (text "ab ") (text :fmt C "cd") (text " ef")))',
    );
    // Three Backspaces drain " ef" (last char → 1-char node → prune).
    // The caret lands element-anchored right after the surviving "cd"
    // code span.
    await page.keyboard.press("Backspace");
    await page.keyboard.press("Backspace");
    await page.keyboard.press("Backspace");
    await expectDump(
      page,
      '(doc (paragraph (text "ab ") (text :fmt C "cd")))',
    );
    // Next Backspace must trim "cd" → "c", not delete the whole code
    // span — that's the regression.
    await page.keyboard.press("Backspace");
    await expectDump(
      page,
      '(doc (paragraph (text "ab ") (text :fmt C "c")))',
    );
    // One more Backspace prunes the 1-char code span; the next two
    // shave the trailing space and "b" off "ab " one at a time.
    await page.keyboard.press("Backspace");
    await expectDump(page, '(doc (paragraph (text "ab ")))');
    await page.keyboard.press("Backspace");
    await expectDump(page, '(doc (paragraph (text "ab")))');
    await page.keyboard.press("Backspace");
    await expectDump(page, '(doc (paragraph (text "a")))');
  });
});

test.describe("inline marks toggle pending-format mode", () => {
  test("cmd+b with no selection marks the next typed run bold", async ({
    page,
  }) => {
    await freshEditor(page);
    await typeText(page, "plain");
    await page.keyboard.press("ControlOrMeta+b");
    await typeText(page, "bold");
    await expectDump(
      page,
      '(doc (paragraph (text "plain") (text :fmt B "bold")))',
    );
  });

  test("pending bold persists across shift+enter", async ({ page }) => {
    await freshEditor(page);
    await page.keyboard.press("ControlOrMeta+b");
    await typeText(page, "first");
    await page.keyboard.press("Shift+Enter");
    await typeText(page, "second");
    await expectDump(
      page,
      '(doc (paragraph (text :fmt B "first")) (paragraph (text :fmt B "second")))',
    );
  });

  test("toggling off pending clears subsequent typing to plain", async ({
    page,
  }) => {
    await freshEditor(page);
    await page.keyboard.press("ControlOrMeta+b");
    await typeText(page, "bold");
    await page.keyboard.press("ControlOrMeta+b");
    await typeText(page, "plain");
    await expectDump(
      page,
      '(doc (paragraph (text :fmt B "bold") (text "plain")))',
    );
  });
});

test.describe("block decorators render as block-level siblings", () => {
  test("inserting into an empty editor lays out as <div> at root", async ({
    page,
  }) => {
    await freshEditor(page);
    await page.locator("#insert-block-embed").click();
    // Wait for the model to settle so the structural assertion below
    // doesn't race against a mid-dispatch render.
    await expectDump(page, "(doc [block_embed] (paragraph))");
    // The DOM must have NO inline span/paragraph wrapping around the
    // decorator — it sits as a direct child of `.editor`.
    await expect(page.locator(".editor > .editor__decorator--block")).toHaveCount(1);
    await expect(page.locator(".editor > .editor__decorator--inline")).toHaveCount(0);
    // Bonus: the surrounding tag must be `<div>` not `<span>` so the
    // browser doesn't insert a phantom paragraph break.
    expect(
      await page.locator(".editor > .editor__decorator--block").evaluate(
        (el) => el.tagName.toLowerCase(),
      ),
    ).toBe("div");
  });

  test("inserting after text places the decorator after the paragraph, never inside it", async ({
    page,
  }) => {
    await freshEditor(page);
    await typeText(page, "before");
    await page.locator("#insert-block-embed").click();
    await expectDump(
      page,
      "(doc (paragraph (text \"before\")) [block_embed] (paragraph))",
    );
    // Critical regression — a `<div>` inside `<p>` would be reparented
    // by the browser, so assert NO decorator is rendered inside any
    // paragraph.
    await expect(page.locator(".editor p .editor__decorator")).toHaveCount(0);
    await expect(page.locator(".editor > .editor__decorator--block")).toHaveCount(1);
  });

  test("backspace at start of paragraph below removes the decorator", async ({
    page,
  }) => {
    await freshEditor(page);
    await typeText(page, "before");
    await page.locator("#insert-block-embed").click();
    // After insert, caret sits in the trailing empty paragraph. The
    // browser exposes that caret as Element-anchored; Backspace must
    // detect the preceding decorator and remove it.
    await page.keyboard.press("Backspace");
    await expectDump(page, "(doc (paragraph (text \"before\")) (paragraph))");
    await expect(page.locator(".editor > .editor__decorator--block")).toHaveCount(0);
  });
});

/**
 * Dispatch a `deleteSoftLineBackward` beforeinput event directly on the
 * editor element. macOS's Cmd+Backspace fires this inputType in real
 * Chrome; Playwright's `Meta+Backspace` does not reliably reach the
 * `beforeinput` handler under Linux/headless CI, so we synthesize the
 * event to keep the test deterministic across platforms.
 */
async function deleteToLineStart(page: import("@playwright/test").Page): Promise<boolean> {
  return await page.evaluate(() => {
    const ed = document.querySelector(".editor");
    if (!ed) return false;
    const ev = new InputEvent("beforeinput", {
      bubbles: true,
      cancelable: true,
      inputType: "deleteSoftLineBackward",
      data: null,
    });
    ed.dispatchEvent(ev);
    return true;
  });
}
