import { expect, test } from "@playwright/test";
import { expectDump, freshEditor, selDump, typeText } from "./helpers";

// insertReplacementText — autocorrect / spellcheck replacements. Trusted
// events name the replaced word via `getTargetRanges()`; synthetic events
// can't carry target ranges, so these specs cover the fallback path where
// the current model selection is replaced.

function dispatchReplacement(text: string) {
  return (el: Element, data: string) => {
    el.dispatchEvent(
      new InputEvent("beforeinput", {
        inputType: "insertReplacementText",
        data,
        bubbles: true,
        cancelable: true,
      }),
    );
  };
}

test.describe("replacement text", () => {
  test.beforeEach(async ({ page }) => {
    await freshEditor(page);
  });

  test("replacement over a selection replaces exactly the selection", async ({
    page,
  }) => {
    await typeText(page, "teh mistake");
    for (let i = 0; i < 7; i++) {
      await page.keyboard.press("Shift+ArrowLeft");
    }
    await expect.poll(() => selDump(page)).toMatch(/^range\(/);

    await page
      .locator(".editor")
      .evaluate(dispatchReplacement("correction"), "correction");
    await expectDump(page, '(doc (paragraph (text "teh correction")))');
  });

  test("replacement at a collapsed caret inserts the text", async ({ page }) => {
    await typeText(page, "helo");
    await page.locator(".editor").evaluate(dispatchReplacement("!"), "!");
    await expectDump(page, '(doc (paragraph (text "helo!")))');
  });
});
