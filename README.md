# dioxus-editor

A pluggable rich-text editor for Dioxus applications with a transaction-based
document model, Markdown input/output, decorator rendering, history, keyboard
commands, and a contenteditable Dioxus view.

> [!WARNING]
> The 2.x line targets Dioxus 0.8 while Dioxus itself is in alpha.

The crate exposes CSS class names but does **not** ship CSS; applications own
all editor styling.

## Quick start

```toml
[dependencies]
dioxus-editor = { git = "https://github.com/danielkov/dioxus-editor", tag = "v2.0.0-alpha.1" }
```

```rust
use dioxus::prelude::*;
use dioxus_editor::{plugins, use_editor, EditorConfig, EditorView, Schema};

#[component]
fn App() -> Element {
    let editor = use_editor(|| {
        EditorConfig::new(Schema::new())
            .with_plugin(Box::new(plugins::DefaultKeymap))
            .with_plugin(Box::new(plugins::History::new()))
            .with_plugin(Box::new(plugins::MarkdownShortcuts::new()))
    });

    rsx! { EditorView { editor, aria_label: "Article body" } }
}
```

By default, Enter splits the current block. Supplying `on_submit` opts into
submit-on-Enter; Shift+Enter always splits the block.

## Compatibility and version policy

| dioxus-editor | Dioxus | Minimum Rust | Status |
|---|---|---|---|
| 1.x | >=0.7.10, <0.8 | 1.88 | Maintained |
| 2.x | >=0.8, <0.9 | 1.88 | Maintained |

Major editor versions track Dioxus compatibility. The maintained 1.x/Dioxus
0.7 line is available from the
[`1.x` branch](https://github.com/danielkov/dioxus-editor/tree/1.x).

This repository does not publish the crate to crates.io. Use a Git dependency
and pin a tag or revision for reproducible builds.

## License

Licensed under either [Apache License 2.0](LICENSE-APACHE) or
[MIT](LICENSE-MIT), at your option.
