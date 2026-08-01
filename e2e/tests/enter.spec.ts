import { expect, test } from "@playwright/test";
import { beforeInput, expectDump, freshEditor, selDump, typeText } from "./helpers";

test("Enter splits a block by default", async ({ page }) => {
  await freshEditor(page);
  await typeText(page, "first");
  await page.keyboard.press("Enter");
  await typeText(page, "second");
  await expectDump(page, '(doc (paragraph (text "first")) (paragraph (text "second")))');
});

test("Enter deletes a cross-paragraph selection before splitting", async ({ page }) => {
  await freshEditor(page);
  await typeText(page, "first");
  await page.keyboard.press("Enter");
  await typeText(page, "second");
  await page.evaluate(() => {
    const spans = document.querySelectorAll(".editor p span");
    const start = spans[0].firstChild;
    const end = spans[1].firstChild;
    if (!start || !end) return;
    const selection = window.getSelection();
    const range = document.createRange();
    range.setStart(start, 3);
    range.setEnd(end, 3);
    selection?.removeAllRanges();
    selection?.addRange(range);
  });
  await expect.poll(() => selDump(page)).toMatch(/^range\(/);

  await page.keyboard.press("Enter");
  await expectDump(page, '(doc (paragraph (text "fir")) (paragraph (text "ond")))');
});

test("raw insertParagraph splits in default mode", async ({ page }) => {
  await freshEditor(page);
  await typeText(page, "first");
  await beforeInput(page, "insertParagraph");
  await typeText(page, "second");
  await expectDump(page, '(doc (paragraph (text "first")) (paragraph (text "second")))');
});

test("on_submit opts into submit-on-Enter", async ({ page }) => {
  await freshEditor(page);
  await typeText(page, "content");
  await page.getByLabel("Submit on Enter").check();
  await page.getByRole("textbox", { name: "Rich text editor" }).click();
  await page.keyboard.press("Enter");
  await expect(page.getByLabel("Submission count")).toHaveText("1");
  await expectDump(page, '(doc (paragraph (text "content")))');
});

test("raw insertParagraph submits in explicit mode", async ({ page }) => {
  await freshEditor(page);
  await typeText(page, "content");
  await page.getByLabel("Submit on Enter").check();
  await beforeInput(page, "insertParagraph");
  await expect(page.getByLabel("Submission count")).toHaveText("1");
  await expectDump(page, '(doc (paragraph (text "content")))');
});
