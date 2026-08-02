import { test } from "@playwright/test";
import { expectDump, freshEditor, typeText } from "./helpers";

// Cut — the editor writes the selection to the clipboard itself, keeps
// the browser from mutating the contenteditable, and deletes the range
// through the model.

test.describe("cut", () => {
  test.beforeEach(async ({ page }) => {
    await freshEditor(page);
  });

  test("Mod+X deletes the selection through the model", async ({ page }) => {
    await typeText(page, "hello world");
    for (let i = 0; i < 5; i++) {
      await page.keyboard.press("Shift+ArrowLeft");
    }
    await page.keyboard.press("ControlOrMeta+x");
    await expectDump(page, '(doc (paragraph (text "hello ")))');
  });

  test("cut text lands on the clipboard and pastes back", async ({ page }) => {
    await typeText(page, "hello world");
    for (let i = 0; i < 5; i++) {
      await page.keyboard.press("Shift+ArrowLeft");
    }
    await page.keyboard.press("ControlOrMeta+x");
    await expectDump(page, '(doc (paragraph (text "hello ")))');

    await page.keyboard.press("ControlOrMeta+v");
    await expectDump(page, '(doc (paragraph (text "hello world")))');
  });

  test("Mod+X with a collapsed caret is a no-op", async ({ page }) => {
    await typeText(page, "hello");
    await page.keyboard.press("ControlOrMeta+x");
    await expectDump(page, '(doc (paragraph (text "hello")))');
  });
});
