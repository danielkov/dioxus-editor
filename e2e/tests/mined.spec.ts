import { test, expect } from "@playwright/test";

import {
  beforeInput,
  caretToTextOffset,
  caretToTextStart,
  freshEditor,
  selDump,
  selectAll,
  selectBlockChars,
  selectChars,
  typeText,
} from "./helpers";

// Tests in this file are translations of behaviors verified by mature
// open-source rich-text editor test suites — adapted to assert the
// equivalent behavior against this editor's surface. Each `test.describe`
// block cites which library/file the behavior was drawn from, and each
// test name describes the verified behavior. No code was copied; we use
// only the *catalog* of behaviors and our own assertion code.
//
// Source libraries (their tests served as the catalog):
//   - ProseMirror: prosemirror-commands, prosemirror-history,
//     prosemirror-inputrules, prosemirror-transform.
//   - Lexical (Meta): packages/lexical/__tests__/unit/LexicalSelection,
//     LexicalEditor, LexicalHistory, LexicalMarkdown, LexicalListPlugin,
//     packages/lexical-list/__tests__/unit/formatList.
//   - Slate: packages/slate/test/transforms/{delete,insertText,
//     splitNodes,setNodes,mergeNodes} fixtures.
//   - Tiptap: packages/core/__tests__ and the extension-{bold,list,
//     code-block-lowlight} suites.

const mod = process.platform === "darwin" ? "Meta" : "Control";

// ---------------------------------------------------------------------------
// Inline marks — drawn from ProseMirror toggleMark tests, Tiptap
// extension-bold/extension-italic tests, Lexical LexicalSelection toggle
// format tests.
// ---------------------------------------------------------------------------

test.describe("marks: toggle on selection", () => {
  test("Cmd+B applies bold to the selected range only", async ({ page }) => {
    await freshEditor(page);
    await typeText(page, "hello world");
    await selectChars(page, 6, 11);
    await page.keyboard.press(`${mod}+b`);
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text "hello ") (text :fmt B "world")))',
    );
  });

  test("Cmd+I applies italic to the selected range only", async ({ page }) => {
    await freshEditor(page);
    await typeText(page, "hello world");
    await selectChars(page, 6, 11);
    await page.keyboard.press(`${mod}+i`);
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text "hello ") (text :fmt I "world")))',
    );
  });

  test("Cmd+E applies inline code to the selected range only", async ({
    page,
  }) => {
    await freshEditor(page);
    await typeText(page, "say foo");
    await selectChars(page, 4, 7);
    await page.keyboard.press(`${mod}+e`);
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text "say ") (text :fmt C "foo")))',
    );
  });

  test("Cmd+Shift+S applies strike to the selected range only", async ({
    page,
  }) => {
    await freshEditor(page);
    await typeText(page, "nope yes");
    await selectChars(page, 0, 4);
    await page.keyboard.press(`${mod}+Shift+s`);
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text :fmt S "nope") (text " yes")))',
    );
  });

  test("Cmd+B on an already-bold range removes the mark", async ({ page }) => {
    await freshEditor(page);
    await typeText(page, "abc");
    await selectChars(page, 0, 3);
    await page.keyboard.press(`${mod}+b`);
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text :fmt B "abc")))',
    );
    // Re-select the new (rebuilt) text node and toggle off.
    await selectChars(page, 0, 3);
    await page.keyboard.press(`${mod}+b`);
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text "abc")))',
    );
  });

  test("Bold + italic stack as composable marks", async ({ page }) => {
    await freshEditor(page);
    await typeText(page, "abc");
    await selectChars(page, 0, 3);
    await page.keyboard.press(`${mod}+b`);
    await selectChars(page, 0, 3);
    await page.keyboard.press(`${mod}+i`);
    // BI = both flags set.
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text :fmt BI "abc")))',
    );
  });

  test("Toggle bold on a mixed range applies bold everywhere", async ({
    page,
  }) => {
    // Drawn from ProseMirror toggleMark mixed-coverage semantics.
    await freshEditor(page);
    await typeText(page, "ab cd");
    // Bold just "ab" — this splits the paragraph's text into two spans.
    await selectChars(page, 0, 2);
    await page.keyboard.press(`${mod}+b`);
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text :fmt B "ab") (text " cd")))',
    );
    // Now select across both spans (block-level char range) and toggle —
    // mixed coverage normalizes to "all bold", not "all plain".
    await selectBlockChars(page, 0, 5);
    await page.keyboard.press(`${mod}+b`);
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text :fmt B "ab cd")))',
    );
  });

  test("Cmd+B across two list items marks both items bold", async ({
    page,
  }) => {
    // Regression: selecting across two list_items via Cmd+A used to
    // no-op on Cmd+B because the cross-node mark path bailed when the
    // endpoints sat under different parents.
    await freshEditor(page);
    await typeText(page, "- something");
    await page.keyboard.press("Shift+Enter");
    await typeText(page, "other thing");
    await selectAll(page);
    await page.keyboard.press(`${mod}+b`);
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (bullet_list (list_item (text :fmt B "something")) (list_item (text :fmt B "other thing"))))',
    );
  });

  test("Cmd+B across two paragraphs marks both paragraphs bold", async ({
    page,
  }) => {
    await freshEditor(page);
    await typeText(page, "first");
    await page.keyboard.press("Shift+Enter");
    await typeText(page, "second");
    await selectAll(page);
    await page.keyboard.press(`${mod}+b`);
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text :fmt B "first")) (paragraph (text :fmt B "second")))',
    );
  });
});

// ---------------------------------------------------------------------------
// Backspace boundaries — drawn from ProseMirror joinBackward, Lexical
// LexicalSelection backspace tests, Slate delete fixtures.
// ---------------------------------------------------------------------------

test.describe("backspace: boundaries", () => {
  test("Backspace at document start is a no-op", async ({ page }) => {
    await freshEditor(page);
    await page.keyboard.press("Backspace");
    await expect(page.locator("#state-dump")).toHaveText("(doc (paragraph))");
  });

  test("Backspace after a single character empties the editor to a clean paragraph", async ({
    page,
  }) => {
    // ProseMirror: an empty paragraph has no text children — the doc is
    // `(doc (paragraph))`. A leftover `(text "")` is an artifact our
    // editor currently leaves behind; this assertion targets the clean
    // form so we can talk about whether to coalesce.
    await freshEditor(page);
    await typeText(page, "a");
    await page.keyboard.press("Backspace");
    await expect(page.locator("#state-dump")).toHaveText("(doc (paragraph))");
  });

  test("Backspace deletes a whole emoji grapheme as one unit", async ({
    page,
  }) => {
    // Drawn from Lexical + Slate emoji boundary tests. The emoji is a
    // surrogate pair (UTF-16); deletion must remove the whole code
    // point, not leave a lone surrogate half behind.
    await freshEditor(page);
    await typeText(page, "a😀b");
    // Char offset 2 = between 😀 (char index 1) and b (char index 2).
    await caretToTextOffset(page, 2);
    await page.keyboard.press("Backspace");
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text "ab")))',
    );
  });

  test("Backspace at start of empty paragraph removes the paragraph", async ({
    page,
  }) => {
    await freshEditor(page);
    await typeText(page, "first");
    await page.keyboard.press("Shift+Enter");
    // Caret in fresh empty paragraph.
    await page.keyboard.press("Backspace");
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text "first")))',
    );
  });
});

// ---------------------------------------------------------------------------
// Delete (forward) — drawn from ProseMirror joinForward + Lexical
// deleteContent tests.
// ---------------------------------------------------------------------------

test.describe("delete (forward): boundaries", () => {
  test("Delete at end of document is a no-op", async ({ page }) => {
    await freshEditor(page);
    await typeText(page, "abc");
    await page.keyboard.press("Delete");
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text "abc")))',
    );
  });

  test("Delete at end of paragraph merges next paragraph", async ({ page }) => {
    await freshEditor(page);
    await typeText(page, "abc");
    await page.keyboard.press("Shift+Enter");
    await typeText(page, "def");
    // Move caret to end of "abc".
    await page.evaluate(() => {
      const ps = document.querySelectorAll(".editor p span");
      const t = ps[0].firstChild!;
      const sel = window.getSelection()!;
      const r = document.createRange();
      r.setStart(t, t.nodeValue!.length);
      r.collapse(true);
      sel.removeAllRanges();
      sel.addRange(r);
    });
    await page.keyboard.press("Delete");
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text "abcdef")))',
    );
  });

  test("Delete forward deletes a whole emoji grapheme", async ({ page }) => {
    await freshEditor(page);
    await typeText(page, "a😀b");
    // Char offset 1 = between a (index 0) and 😀 (index 1).
    await caretToTextOffset(page, 1);
    await page.keyboard.press("Delete");
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text "ab")))',
    );
  });
});

// ---------------------------------------------------------------------------
// Modifier deletes — drawn from Lexical deleteWord/deleteSoftLine,
// Slate deleteWord fixtures, ProseMirror command tests.
// ---------------------------------------------------------------------------

test.describe("modifier delete intents", () => {
  test("deleteSoftLineBackward clears to the block start", async ({ page }) => {
    await freshEditor(page);
    await typeText(page, "hello there");
    await beforeInput(page, "deleteSoftLineBackward");
    await expect(page.locator("#state-dump")).toHaveText("(doc (paragraph))");
  });

  test("deleteSoftLineBackward does NOT cross into the previous block", async ({
    page,
  }) => {
    await freshEditor(page);
    await typeText(page, "first");
    await page.keyboard.press("Shift+Enter");
    await typeText(page, "second");
    await beforeInput(page, "deleteSoftLineBackward");
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text "first")) (paragraph))',
    );
  });

  test("deleteWordBackward deletes the preceding word and its trailing space", async ({
    page,
  }) => {
    await freshEditor(page);
    await typeText(page, "hello world");
    await beforeInput(page, "deleteWordBackward");
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text "hello ")))',
    );
  });

  test("deleteWordForward deletes the next word and the whitespace before it", async ({
    page,
  }) => {
    await freshEditor(page);
    await typeText(page, "hello world");
    await caretToTextOffset(page, 5); // between "hello" and " world"
    await beforeInput(page, "deleteWordForward");
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text "hello")))',
    );
  });
});

// ---------------------------------------------------------------------------
// Block-type changes via demotion — drawn from ProseMirror setBlockType,
// Lexical Markdown shortcut tests.
// ---------------------------------------------------------------------------

test.describe("block-type demotion via Backspace", () => {
  test("Backspace at start of heading demotes to paragraph and keeps text", async ({
    page,
  }) => {
    await freshEditor(page);
    await typeText(page, "# title");
    await caretToTextStart(page);
    await page.keyboard.press("Backspace");
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text "title")))',
    );
  });

  test("Backspace at start of blockquote demotes to paragraph and keeps text", async ({
    page,
  }) => {
    await freshEditor(page);
    await typeText(page, "> quoted");
    await caretToTextStart(page);
    await page.keyboard.press("Backspace");
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text "quoted")))',
    );
  });
});

// ---------------------------------------------------------------------------
// Lists — drawn from Lexical LexicalListPlugin / formatList tests,
// ProseMirror list commands, Tiptap extension-list.
// ---------------------------------------------------------------------------

test.describe("lists", () => {
  test("Shift+Enter on a non-empty list item creates a sibling item", async ({
    page,
  }) => {
    await freshEditor(page);
    await typeText(page, "- one");
    await page.keyboard.press("Shift+Enter");
    await typeText(page, "two");
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (bullet_list (list_item (text "one")) (list_item (text "two"))))',
    );
  });

  test("Backspace at start of second list item merges items in place", async ({
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

  test("Backspace at start of first list item lifts content to a paragraph", async ({
    page,
  }) => {
    // ProseMirror + Lexical: Backspace at offset 0 of a single-item list
    // unwraps the list to a paragraph (Tab/Shift+Tab not required for
    // this — Backspace is the canonical "outdent at start" gesture).
    // Our editor currently no-ops here; failure surfaces the gap.
    await freshEditor(page);
    await typeText(page, "- only");
    await caretToTextStart(page);
    await page.keyboard.press("Backspace");
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text "only")))',
    );
  });

  test("Two consecutive Shift+Enter inside a list adds two empty items", async ({
    page,
  }) => {
    await freshEditor(page);
    await typeText(page, "- one");
    await page.keyboard.press("Shift+Enter");
    await page.keyboard.press("Shift+Enter");
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (bullet_list (list_item (text "one")) (list_item) (list_item)))',
    );
  });
});

// ---------------------------------------------------------------------------
// Input rule edges — drawn from prosemirror-inputrules tests, Lexical
// Markdown shortcut tests, Tiptap markPasteRule.
// ---------------------------------------------------------------------------

test.describe("input rule edges", () => {
  test("Block shortcut does not fire mid-word", async ({ page }) => {
    await freshEditor(page);
    await typeText(page, "see > here");
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text "see > here")))',
    );
  });

  test("Block shortcut does not fire inside a heading", async ({ page }) => {
    // ProseMirror inputrules: block-prefix rules only fire when the
    // parent is the "default" textblock (paragraph). Typing `> ` inside
    // a heading must stay literal.
    await freshEditor(page);
    await typeText(page, "# title");
    await caretToTextStart(page);
    await typeText(page, "> ");
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (heading :level 1 (text "> title")))',
    );
  });

  test("Inline shortcut requires a non-whitespace body", async ({ page }) => {
    // ProseMirror's "no-op on empty body" semantics.
    await freshEditor(page);
    await typeText(page, "a **   ** c");
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text "a **   ** c")))',
    );
  });

  test("Markdown shortcut counts as one undo step", async ({ page }) => {
    // ProseMirror inputrule undo behavior.
    await freshEditor(page);
    await typeText(page, "# t");
    await typeText(page, "itle");
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (heading :level 1 (text "title")))',
    );
    // First undo collapses the "itle" typing burst.
    await page.keyboard.press(`${mod}+z`);
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (heading :level 1 (text "")))',
    );
  });

  test("`code` collapses to inline-code mark on the body", async ({ page }) => {
    await freshEditor(page);
    await typeText(page, "call `fn` ok");
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text "call ") (text :fmt C "fn") (text " ok")))',
    );
  });

  test("Bold + italic + strike + code applied in sequence stack", async ({
    page,
  }) => {
    await freshEditor(page);
    await typeText(page, "**a** _b_ ~~c~~ `d`");
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text :fmt B "a") (text " ") (text :fmt I "b") (text " ") (text :fmt S "c") (text " ") (text :fmt C "d")))',
    );
  });
});

// ---------------------------------------------------------------------------
// History edges — drawn from prosemirror-history newGroupDelay tests,
// Lexical LexicalHistory.test.tsx (canUndo/canRedo, CLEAR_HISTORY,
// maxDepth).
// ---------------------------------------------------------------------------

test.describe("history", () => {
  test("New edit after undo clears the redo stack", async ({ page }) => {
    await freshEditor(page);
    await typeText(page, "abc");
    await page.keyboard.press(`${mod}+z`);
    // Type something else — the redo stack must be wiped.
    await typeText(page, "xyz");
    // Now Cmd+Shift+Z should NOT bring back "abc".
    await page.keyboard.press(`${mod}+Shift+z`);
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text "xyz")))',
    );
  });

  test("Format toggle is its own undo step", async ({ page }) => {
    // From Lexical's "different transaction types get separate undo
    // entries" tests.
    await freshEditor(page);
    await typeText(page, "abc");
    await selectChars(page, 0, 3);
    await page.keyboard.press(`${mod}+b`);
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text :fmt B "abc")))',
    );
    await page.keyboard.press(`${mod}+z`); // undo bold
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text "abc")))',
    );
    await page.keyboard.press(`${mod}+z`); // undo "abc" burst
    await expect(page.locator("#state-dump")).toHaveText("(doc (paragraph))");
  });

  test("Input rule transform is its own undo step (separate from typing)", async ({
    page,
  }) => {
    // ProseMirror inputrules: undo immediately after a rule fires
    // reverts the structural transform but keeps the trigger text.
    // Three Cmd+Z's should unwind: typed-after, the rule, the trigger.
    await freshEditor(page);
    await typeText(page, "# title");
    await page.keyboard.press(`${mod}+z`); // undo "title" burst
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (heading :level 1 (text "")))',
    );
    await page.keyboard.press(`${mod}+z`); // undo the input rule
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text "# ")))',
    );
    await page.keyboard.press(`${mod}+z`); // undo "# " typing burst
    await expect(page.locator("#state-dump")).toHaveText("(doc (paragraph))");
  });

  test("Multiple typing bursts each undo separately", async ({ page }) => {
    await freshEditor(page);
    await typeText(page, "abc");
    await page.keyboard.press("Shift+Enter"); // structural — breaks burst
    await typeText(page, "def");
    await page.keyboard.press(`${mod}+z`);
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text "abc")) (paragraph))',
    );
    await page.keyboard.press(`${mod}+z`);
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text "abc")))',
    );
    await page.keyboard.press(`${mod}+z`);
    await expect(page.locator("#state-dump")).toHaveText("(doc (paragraph))");
  });

  test("Cmd+Shift+Z redoes the most recent undo", async ({ page }) => {
    await freshEditor(page);
    await typeText(page, "abc");
    await page.keyboard.press(`${mod}+z`);
    await page.keyboard.press(`${mod}+Shift+z`);
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text "abc")))',
    );
  });
});

// ---------------------------------------------------------------------------
// Selection — drawn from Lexical LexicalSelection.test.ts (move, normalize,
// anchor/focus, default cursor placement).
// ---------------------------------------------------------------------------

test.describe("selection edge cases", () => {
  test("Select-all then typing replaces every block with one paragraph", async ({
    page,
  }) => {
    await freshEditor(page);
    await typeText(page, "line one");
    await page.keyboard.press("Shift+Enter");
    await typeText(page, "line two");
    await selectAll(page);
    await typeText(page, "x");
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text "x")))',
    );
  });

  test("Backwards selection (focus < anchor) still mirrors to the model", async ({
    page,
  }) => {
    // Lexical's "selection.anchor / selection.focus directionality" tests.
    await freshEditor(page);
    await typeText(page, "hello");
    const d = await selDump(page);
    const key = /caret\((\d+),/.exec(d)![1];
    await page.evaluate(() => {
      const text = document.querySelector(".editor p span")!.firstChild!;
      const sel = window.getSelection()!;
      sel.removeAllRanges();
      sel.setBaseAndExtent(text, 4, text, 1);
    });
    await expect
      .poll(() => selDump(page))
      .toBe(`range(${key}/Text/4 -> ${key}/Text/1)`);
    await page.keyboard.press("Backspace");
    // Range deletion is direction-agnostic: chars 1-4 (the "ell" segment)
    // are removed regardless of anchor/focus order.
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text "ho")))',
    );
  });

  test("Default caret on a fresh empty doc is inside the first block", async ({
    page,
  }) => {
    // ProseMirror state-init test: cursor must land at start of first
    // textblock, never at the document root.
    await freshEditor(page);
    const d = await selDump(page);
    // Whatever the key, it MUST NOT be the doc root (key 1).
    expect(d).not.toMatch(/^caret\(1,/);
  });
});

// ---------------------------------------------------------------------------
// Range deletion across structures — drawn from Slate fixtures
// transforms/delete/{across-blocks,across-marks,depth-3-blocks}.
// ---------------------------------------------------------------------------

test.describe("cross-structure range deletion", () => {
  test("Backspace on a range that ends inside a mark preserves outside marks", async ({
    page,
  }) => {
    await freshEditor(page);
    await typeText(page, "a **bold** end");
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text "a ") (text :fmt B "bold") (text " end")))',
    );
    // Select from char 2 ("a ") through char 4 ("a *bo*"-ish span) and
    // delete. Drag from offset 2 of text " end" backward into bold node.
    // We use programmatic selection because we can't rely on key keys.
    await page.evaluate(() => {
      const spans = document.querySelectorAll(".editor p span");
      const a = spans[0].firstChild!; // "a "
      const b = spans[1].firstChild!; // "bold"
      const sel = window.getSelection()!;
      const r = document.createRange();
      r.setStart(a, 2); // end of "a "
      r.setEnd(b, 2); // middle of "bold"
      sel.removeAllRanges();
      sel.addRange(r);
    });
    await page.keyboard.press("Backspace");
    // Surviving structure: "a " + "ld" (bold) + " end".
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text "a ") (text :fmt B "ld") (text " end")))',
    );
  });

  test("Backspace on a range across paragraph + paragraph merges + coalesces", async ({
    page,
  }) => {
    await freshEditor(page);
    await typeText(page, "first");
    await page.keyboard.press("Shift+Enter");
    await typeText(page, "second");
    await page.evaluate(() => {
      const spans = document.querySelectorAll(".editor p span");
      const a = spans[0].firstChild!;
      const b = spans[1].firstChild!;
      const sel = window.getSelection()!;
      const r = document.createRange();
      r.setStart(a, 3);
      r.setEnd(b, 3);
      sel.removeAllRanges();
      sel.addRange(r);
    });
    await page.keyboard.press("Backspace");
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text "firond")))',
    );
  });

  test("Backspace on a range across blockquote + paragraph collapses to one block", async ({
    page,
  }) => {
    await freshEditor(page);
    await typeText(page, "> hello there");
    await page.keyboard.press("Shift+Enter");
    await typeText(page, "more");
    // Demote second block to a paragraph.
    await caretToTextStart(page);
    await page.keyboard.press("Backspace");
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (blockquote (text "hello there")) (paragraph (text "more")))',
    );
    // Select across the two and Backspace.
    await page.evaluate(() => {
      const first = document
        .querySelector(".editor blockquote span")!
        .firstChild!;
      const last = document.querySelector(".editor p span")!.firstChild!;
      const sel = window.getSelection()!;
      const r = document.createRange();
      r.setStart(first, 0);
      r.setEnd(last, last.nodeValue!.length);
      sel.removeAllRanges();
      sel.addRange(r);
    });
    await expect.poll(() => selDump(page)).toMatch(/^range\(/);
    await page.keyboard.press("Backspace");
    // No residual block kind for fully-consumed nodes — the blockquote
    // emptied by this delete collapses to a paragraph.
    await expect(page.locator("#state-dump")).toHaveText("(doc (paragraph))");
  });
});

// ---------------------------------------------------------------------------
// Paste — drawn from Tiptap transformPastedHTML / paste rule tests and
// ProseMirror clipboard tests.
// ---------------------------------------------------------------------------

test.describe("paste", () => {
  test("Empty paste is a safe no-op (no error, no model change)", async ({
    page,
  }) => {
    // Even an "empty paste" path that doesn't actually deliver data must
    // not throw. Use the dispatched event form — Blink's data-stripping
    // is sufficient to verify our handler bails cleanly on empty input.
    await freshEditor(page);
    await typeText(page, "x");
    await page.evaluate(() => {
      const ed = document.querySelector(".editor") as HTMLElement;
      const dt = new DataTransfer();
      dt.setData("text/plain", "");
      const ev = new ClipboardEvent("paste", {
        bubbles: true,
        cancelable: true,
        clipboardData: dt,
      });
      ed.dispatchEvent(ev);
    });
    await expect(page.locator("#state-dump")).toHaveText(
      '(doc (paragraph (text "x")))',
    );
  });
});
