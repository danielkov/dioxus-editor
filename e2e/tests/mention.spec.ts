import { expect, test } from "@playwright/test";
import { expectDump, freshEditor, typeText } from "./helpers";

// Mention picker — the fixture registers an inline `mention` decorator
// and opens a caret-anchored popup while the caret sits inside an
// `@query` run. Picking an entry replaces `@query` with the decorator.

test.describe("mention picker", () => {
  test.beforeEach(async ({ page }) => {
    await freshEditor(page);
  });

  test("typing @ opens the picker and filters as you type", async ({ page }) => {
    await typeText(page, "Ping @");
    const popup = page.locator(".fixture-mention-popup");
    await expect(popup).toBeVisible();
    await expect(popup.locator(".fixture-mention-item")).toHaveCount(4);

    await typeText(page, "f");
    await expect(popup.locator(".fixture-mention-item")).toHaveCount(2);

    await typeText(page, "err");
    await expect(popup.locator(".fixture-mention-item")).toHaveCount(1);
    await expect(popup.locator(".fixture-mention-item")).toContainText("@ferris");
  });

  test("a query with no matches closes the picker", async ({ page }) => {
    await typeText(page, "@zzz");
    await expect(page.locator(".fixture-mention-popup")).toBeHidden();
  });

  test("mid-word @ never opens the picker", async ({ page }) => {
    await typeText(page, "mail user@fer");
    await expect(page.locator(".fixture-mention-popup")).toBeHidden();
  });

  test("@ at the start of a block opens the picker", async ({ page }) => {
    await typeText(page, "@a");
    await expect(page.locator(".fixture-mention-popup")).toBeVisible();
    await expect(page.locator(".fixture-mention-item")).toHaveCount(1);
    await expect(page.locator(".fixture-mention-item")).toContainText("@ada");
  });

  test("picking only replaces the query at the caret, not an identical earlier substring", async ({
    page,
  }) => {
    // Leave a literal "@fe" behind (space closes the picker without a
    // pick), then start a second, identical query and pick from it.
    await typeText(page, "@fe stays, @fe");
    await page
      .locator(".fixture-mention-item")
      .filter({ hasText: "@ferris" })
      .click();

    await expectDump(
      page,
      '(doc (paragraph (text "@fe stays, ") [mention] (text " ")))',
    );
  });

  test("clicking a result replaces the query with a mention decorator", async ({
    page,
  }) => {
    await typeText(page, "Ping @fe");
    await page
      .locator(".fixture-mention-item")
      .filter({ hasText: "@ferris" })
      .click();

    await expectDump(page, '(doc (paragraph (text "Ping ") [mention] (text " ")))');
    await expect(page.locator(".fixture-mention")).toHaveText("@ferris");
    await expect(page.locator(".fixture-mention-popup")).toBeHidden();

    // Typing continues after the inserted trailing space.
    await typeText(page, "for review");
    await expectDump(
      page,
      '(doc (paragraph (text "Ping ") [mention] (text " for review")))',
    );
  });
});
