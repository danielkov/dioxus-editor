# Styling

`dioxus-editor` does not include CSS. The view renders semantic class names and data attributes. The host application controls all visual styles.

This page specifies the styling interface. It lists each class, its HTML element, and the CSS rules necessary for correct editor operation.

Class names are part of the public API. Semantic versioning applies to class names as it applies to other symbols.

For a small working stylesheet, refer to [`fixture/assets/fixture.css`](../fixture/assets/fixture.css). This stylesheet supports the kitchen-sink application in the end-to-end test suite.

## Required rules

The following two rules are necessary. All other rules are optional styles.

```css
.editor {
  /* The model can store trailing spaces and consecutive spaces.
   * The default `white-space: normal` rule combines these spaces.
   * Then, the DOM caret positions do not agree with the document model.
   * `pre-wrap` keeps each space and permits long lines to wrap. */
  white-space: pre-wrap;
  word-break: break-word;
}

.editor__code::after {
  /* A code block can end with `\n`.
   * The final newline needs a line box.
   * Without this line box, the empty line closes and the caret cannot enter it.
   * This pseudo-element adds an invisible, measurable zero-width space. */
  content: "\200B";
}
```

## The root

```html
<div
  class="editor"
  role="textbox"
  aria-multiline="true"
  contenteditable="true"
  data-placeholder="Start writing…"
></div>
```

| Class           | When                                       |
| --------------- | ------------------------------------------ |
| `editor`        | Always present on the contenteditable root |
| `editor--empty` | Present when the document has no content   |

The `placeholder` property is copied to `data-placeholder`. Host CSS renders the placeholder.

Position the placeholder as an absolute element. This position prevents the pseudo-element from moving the caret from the first line.

```css
.editor {
  position: relative;
}

.editor[data-placeholder]:empty::before,
.editor--empty[data-placeholder]::before {
  content: attr(data-placeholder);
  color: #999;
  pointer-events: none;
  position: absolute;
  top: 0;
  left: 0;
}
```

## Blocks

Each block element has a `data-key` attribute. This attribute contains the node key from the document model.

Keys are stable during one session. Keys can change after a reload. Use keys for debugging only. Do not use keys for styling.

| Class                                     | Element         | Notes                                                                                         |
| ----------------------------------------- | --------------- | --------------------------------------------------------------------------------------------- |
| `editor__p`                               | `<p>`           | Paragraph                                                                                     |
| `editor__h` + `editor__h1` … `editor__h6` | `<h1>` … `<h6>` | Both classes are present. Use `editor__h` for shared rules and `editor__h{n}` for each level. |
| `editor__quote`                           | `<blockquote>`  |                                                                                               |
| `editor__pre`                             | `<pre>`         | Code block wrapper. Its `data-lang` contains the block language and can be empty.             |
| `editor__code`                            | `<code>`        | Contains the text and is inside `editor__pre`                                                 |
| `editor__ul` / `editor__ol`               | `<ul>` / `<ol>` |                                                                                               |
| `editor__li`                              | `<li>`          |                                                                                               |
| `editor__block`                           | `<div>`         | Fallback wrapper for element types that do not have a specialized view                        |

Empty blocks contain a `<br>` placeholder. This placeholder lets the caret enter the block. It does not require a style.

## Text runs

Inline text renders as spans with short class names. These nodes occur frequently and change after each keystroke. Short names decrease the payload size.

Each text run has the base class `e-t`. It also has one flag for each active format. You can combine the flags.

```html
<span class="e-t e-b e-i" data-key="…">bold italic</span>
```

| Class | Format                |
| ----- | --------------------- |
| `e-t` | Every text run (base) |
| `e-b` | Bold                  |
| `e-i` | Italic                |
| `e-s` | Strikethrough         |
| `e-c` | Inline code           |

```css
.e-b {
  font-weight: 700;
}
.e-i {
  font-style: italic;
}
.e-s {
  text-decoration: line-through;
}
.e-c {
  font-family: ui-monospace, monospace;
  background: #f0f0f0;
}
```

## Decorators

Decorators include links, embeds, mentions, and all items registered through `DecoratorSpec`.

Each decorator renders in a wrapper that the crate supplies. Inline decorators use a `<span>`. Block decorators use a `<div>`.

Each wrapper has `contenteditable="false"`. The `data-kind` attribute contains the registered decorator name.

| Class                                                    | When                                                                  |
| -------------------------------------------------------- | --------------------------------------------------------------------- |
| `editor__decorator`                                      | Present on each decorator wrapper                                     |
| `editor__decorator--inline` / `editor__decorator--block` | Selected by `DecoratorSpec::inline`                                   |
| `editor__decorator--selected`                            | Present when the decorator has node selection                         |
| `editor__decorator--unknown`                             | Present when the schema does not register the document decorator type |
| `editor__decorator-remove`                               | Present on the `×` remove button in each wrapper                      |

The host controls the result of the schema `render` function. The classes in the table apply only to the wrapper.

Style `editor__decorator-remove` as necessary. For example, show the button only when the wrapper has hover or `--selected` state.

If your user interface controls removal, apply `display: none` to the button.

## Tables

```text
div.editor__table-wrap          ← data-rows, data-cols
├─ table.editor__table          ← data-key, data-align (per-column, comma-separated)
│  └─ tbody
│     └─ tr.editor__tr (first row: editor__tr editor__tr--header)
│        └─ th.editor__th / td.editor__td   ← data-key, inline text-align
│           ├─ …cell content…
│           ├─ button.editor__cell-menu     ← the "⋯" per-cell menu trigger
│           └─ …popover, when open…
├─ button.editor__table-add.editor__table-add--col
└─ button.editor__table-add.editor__table-add--row
       └─ span.editor__table-add-icon
```

Each cell has an inline `text-align` style. Host CSS does not need to control cell alignment.

The cell menu opens a popover.

| Class                                | Element                          |
| ------------------------------------ | -------------------------------- |
| `editor__table-popover-backdrop`     | Full-viewport click-away layer   |
| `editor__table-popover`              | Popover panel                    |
| `editor__table-popover-section`      | One group (Row / Column / Cell)  |
| `editor__table-popover-label`        | Group heading                    |
| `editor__table-popover-item`         | Action button                    |
| `editor__table-popover-item--danger` | Modifier for destructive actions |

The `editor__cell-menu` and `editor__table-add` buttons do not have styles.

If you do not want interactive table controls, apply `display: none` to these buttons. Keyboard navigation continues to operate. Press Tab to move between cells.

## Data attributes

| Attribute                 | Where                      | Value                                     |
| ------------------------- | -------------------------- | ----------------------------------------- |
| `data-key`                | Every model-backed element | Node key for the session and debugging    |
| `data-placeholder`        | Editor root                | The `placeholder` property                |
| `data-kind`               | Decorator wrappers         | Registered decorator name                 |
| `data-lang`               | `editor__pre`              | Code block language                       |
| `data-rows` / `data-cols` | `editor__table-wrap`       | Current table dimensions                  |
| `data-align`              | `editor__table`            | Comma-separated alignment for each column |
| `data-cell-menu`          | `editor__cell-menu`        | Key of the applicable cell                |
