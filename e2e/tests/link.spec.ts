import { expect, test } from "@playwright/test";
import { expectDump, freshEditor, selDump, typeText } from "./helpers";

// Link paste — a pasted URL becomes a `link` node. With a non-empty
// selection the selected text is wrapped (the URL becomes the href);
// otherwise the URL itself is the visible label. These drive a real
// keyboard paste so the editor's `onpaste` handler reads genuine
// clipboard data (a synthetic ClipboardEvent is stripped of its data by
// Blink, so it can't exercise this path).

test.describe("link paste", () => {
  test.beforeEach(async ({ page, context }) => {
    await context.grantPermissions(["clipboard-read", "clipboard-write"]);
    await freshEditor(page);
  });

  async function setClipboard(page: import("@playwright/test").Page, text: string) {
    await page.evaluate((t) => navigator.clipboard.writeText(t), text);
  }

  async function paste(page: import("@playwright/test").Page) {
    const editor = page.getByRole("textbox", { name: "Rich text editor" });
    if (!(await editor.evaluate((element) => element === document.activeElement))) {
      await editor.click();
    }
    await page.keyboard.press("ControlOrMeta+V");
  }

  test("pasting a bare URL inserts a link node labelled with the URL", async ({
    page,
  }) => {
    await setClipboard(page, "https://example.com");
    await paste(page);

    await expectDump(page, "(doc (paragraph [link]))");
    const link = page.getByRole("link", { name: "https://example.com" });
    await expect(link).toHaveAttribute("href", "https://example.com");
  });

  test("pasting a URL over selected text wraps the text in a link", async ({
    page,
  }) => {
    await typeText(page, "click here");
    await expectDump(page, '(doc (paragraph (text "click here")))');
    // Select the whole label.
    await page.evaluate(() => {
      const ed = document.querySelector(".editor")!;
      const sel = window.getSelection();
      const r = document.createRange();
      r.selectNodeContents(ed);
      sel?.removeAllRanges();
      sel?.addRange(r);
    });
    await expect.poll(() => selDump(page)).toMatch(/^range\(/);

    await setClipboard(page, "https://example.com/docs");
    await paste(page);

    await expectDump(page, "(doc (paragraph [link]))");
    const link = page.getByRole("link", { name: "click here" });
    await expect(link).toHaveAttribute("href", "https://example.com/docs");
  });

  test("pasting a bare www host links via https", async ({ page }) => {
    await setClipboard(page, "www.example.com");
    await paste(page);

    await expectDump(page, "(doc (paragraph [link]))");
    const link = page.getByRole("link", { name: "www.example.com" });
    await expect(link).toHaveAttribute("href", "https://www.example.com");
  });

  test("pasting ordinary text stays plain text", async ({ page }) => {
    await setClipboard(page, "just some words");
    await paste(page);

    await expectDump(page, '(doc (paragraph (text "just some words")))');
  });
});
