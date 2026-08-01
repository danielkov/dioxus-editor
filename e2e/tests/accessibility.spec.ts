import { expect, test } from "@playwright/test";
import { expectDump, freshEditor } from "./helpers";

test("decorator removal is keyboard operable", async ({ page }) => {
  await freshEditor(page);
  await page.getByRole("button", { name: "insert block embed" }).click();

  const remove = page.getByRole("button", { name: "Remove block_embed" });
  await remove.focus();
  await page.keyboard.press("Enter");

  await expect(remove).toHaveCount(0);
  await expectDump(page, "(doc (paragraph))");
});

test("table controls and cell popover are keyboard operable", async ({ page }) => {
  await freshEditor(page);
  await page.getByRole("button", { name: "insert table" }).click();

  const addColumn = page.getByRole("button", { name: "Add column" });
  await addColumn.focus();
  await page.keyboard.press("Enter");
  await expect(page.locator(".editor__table-wrap")).toHaveAttribute("data-cols", "3");

  const menu = page.getByRole("button", { name: "Cell actions" }).first();
  await menu.focus();
  await page.keyboard.press("Enter");
  await expect(menu).toHaveAttribute("aria-expanded", "true");
  await expect(page.getByRole("dialog", { name: "Cell actions" })).toBeVisible();

  await page.keyboard.press("Tab");
  await expect(page.getByRole("button", { name: "Insert above" })).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog", { name: "Cell actions" })).toHaveCount(0);
  await expect(menu).toBeFocused();
  await expect(menu).toHaveAttribute("aria-expanded", "false");
});

test("an internal keymap dispatch failure reaches EditorView on_error", async ({ page }) => {
  await freshEditor(page);
  await page.keyboard.press("ControlOrMeta+Shift+f");
  await expect(page.getByLabel("Editor error")).toContainText("transaction failed");
  await expectDump(page, "(doc (paragraph))");
});
