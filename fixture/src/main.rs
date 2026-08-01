//! Standalone editor fixture — minimal page mounting `EditorView` plus a
//! deterministic structural dump element (`#state-dump`) that Playwright
//! tests read to assert against the document model without poking into
//! private internals.

use std::rc::Rc;

use dioxus::prelude::*;

use dioxus_editor::plugins::{DefaultKeymap, History, MarkdownShortcuts};
use dioxus_editor::*;

fn main() {
    dioxus::launch(App);
}

struct FailingPlugin;

impl Plugin for FailingPlugin {
    fn keymap(&self) -> Vec<KeyBinding> {
        vec![KeyBinding {
            keys: "Mod-Shift-f".into(),
            command: |_| {
                Some(Transaction::new().step(Step::SetAttr {
                    key: NodeKey::MAX,
                    name: "failure".into(),
                    value: None,
                }))
            },
        }]
    }
}

#[component]
fn App() -> Element {
    let handle = use_editor(|| {
        // Register a stub block decorator for insertion/removal coverage.
        let schema = Schema::new()
            .with_decorator(
                "block_embed",
                DecoratorSpec {
                    inline: false,
                    render: Rc::new(|attrs: &Attrs| {
                        let label = attrs.get_str("label").unwrap_or("[embed]").to_string();
                        rsx! { div { class: "fixture-embed", "{label}" } }
                    }),
                    to_markdown: Rc::new(|_| String::new()),
                },
            )
            .with_decorator(
                "link",
                DecoratorSpec {
                    inline: true,
                    render: Rc::new(|attrs: &Attrs| {
                        let href = attrs.get_str("href").unwrap_or_default();
                        let label = attrs.get_str("text").unwrap_or(href).to_string();
                        if href.starts_with("https://") || href.starts_with("http://") {
                            rsx! {
                                a {
                                    class: "editor__link",
                                    href: "{href}",
                                    target: "_blank",
                                    rel: "noopener noreferrer",
                                    "{label}"
                                }
                            }
                        } else {
                            rsx! { span { class: "editor__link editor__link--unsafe", "{label}" } }
                        }
                    }),
                    to_markdown: Rc::new(|attrs| {
                        let href = attrs.get_str("href").unwrap_or_default();
                        let label = attrs.get_str("text").unwrap_or(href);
                        format!("[{}]({})", label, href)
                    }),
                },
            );
        EditorConfig::new(schema)
            .with_plugin(Box::new(DefaultKeymap))
            .with_plugin(Box::new(History::new()))
            .with_plugin(Box::new(MarkdownShortcuts::new()))
            .with_plugin(Box::new(FailingPlugin))
    });
    let mut submit_mode = use_signal(|| false);
    let mut submit_count = use_signal(|| 0_u32);
    let mut last_error = use_signal(String::new);
    let state_sig = handle.state_signal();
    let state = state_sig.read().clone();
    let dump = dump_doc(&state.doc);
    let sel = dump_selection(&state.selection);

    let handle_insert = handle.clone();
    let on_insert_embed = move |_| {
        let state = handle_insert.read_state();
        let attrs = Attrs::new().with("label", "[embed]");
        if let Some(tr) = dioxus_editor::commands::insert_decorator(&state, "block_embed", attrs) {
            let _ = handle_insert.dispatch(tr);
        }
    };
    let h_bq = handle.clone();
    let h_code = handle.clone();
    let h_ul = handle.clone();
    let h_ol = handle.clone();
    let h_h1 = handle.clone();
    let h_table = handle.clone();

    rsx! {
        document::Link { rel: "stylesheet", href: asset!("/assets/fixture.css") }
        div { id: "fixture-root",
            h1 { "editor fixture" }
            label {
                input {
                    r#type: "checkbox",
                    checked: submit_mode(),
                    onchange: move |event| submit_mode.set(event.checked()),
                }
                "Submit on Enter"
            }
            output { "aria-label": "Submission count", "{submit_count}" }
            div { class: "fixture-frame",
                if submit_mode() {
                    EditorView {
                        editor: handle.clone(),
                        placeholder: "Start writing…".to_string(),
                        aria_label: "Rich text editor".to_string(),
                        on_submit: move |_| submit_count += 1,
                        on_error: move |error: DispatchError| last_error.set(error.to_string()),
                    }
                } else {
                    EditorView {
                        editor: handle.clone(),
                        placeholder: "Start writing…".to_string(),
                        aria_label: "Rich text editor".to_string(),
                        on_error: move |error: DispatchError| last_error.set(error.to_string()),
                    }
                }
            }
            div { class: "fixture-actions",
                button {
                    id: "toggle-blockquote",
                    onmousedown: move |e: Event<MouseData>| e.prevent_default(),
                    onclick: move |_| {
                        let s = h_bq.read_state();
                        if let Some(tr) = dioxus_editor::commands::toggle_blockquote(&s) {
                            let _ = h_bq.dispatch(tr);
                        }
                    },
                    "blockquote"
                }
                button {
                    id: "toggle-code-block",
                    onmousedown: move |e: Event<MouseData>| e.prevent_default(),
                    onclick: move |_| {
                        let s = h_code.read_state();
                        if let Some(tr) = dioxus_editor::commands::toggle_code_block(&s) {
                            let _ = h_code.dispatch(tr);
                        }
                    },
                    "code block"
                }
                button {
                    id: "toggle-bullet-list",
                    onmousedown: move |e: Event<MouseData>| e.prevent_default(),
                    onclick: move |_| {
                        let s = h_ul.read_state();
                        if let Some(tr) = dioxus_editor::commands::toggle_bullet_list(&s) {
                            let _ = h_ul.dispatch(tr);
                        }
                    },
                    "ul"
                }
                button {
                    id: "toggle-ordered-list",
                    onmousedown: move |e: Event<MouseData>| e.prevent_default(),
                    onclick: move |_| {
                        let s = h_ol.read_state();
                        if let Some(tr) = dioxus_editor::commands::toggle_ordered_list(&s) {
                            let _ = h_ol.dispatch(tr);
                        }
                    },
                    "ol"
                }
                button {
                    id: "toggle-h1",
                    onmousedown: move |e: Event<MouseData>| e.prevent_default(),
                    onclick: move |_| {
                        let s = h_h1.read_state();
                        if let Some(tr) = dioxus_editor::commands::toggle_heading(&s, 1) {
                            let _ = h_h1.dispatch(tr);
                        }
                    },
                    "h1"
                }
                button { id: "insert-block-embed", onclick: on_insert_embed, "insert block embed" }
                button {
                    id: "insert-table",
                    onclick: move |_| {
                        let state = h_table.read_state();
                        if let Some(tr) = dioxus_editor::commands::insert_table(&state, 2, 2) {
                            let _ = h_table.dispatch(tr);
                        }
                    },
                    "insert table"
                }
            }
            pre { id: "state-dump", "aria-label": "Document state", "{dump}" }
            pre { id: "selection-dump", "aria-label": "Selection state", "{sel}" }
            output { "aria-label": "Editor error", "{last_error}" }
        }
    }
}

// -- dump helpers ---------------------------------------------------------

fn dump_doc(doc: &Doc) -> String {
    let mut out = String::new();
    dump_node(doc, doc.root_key(), &mut out);
    out
}

fn dump_node(doc: &Doc, key: NodeKey, out: &mut String) {
    let Some(node) = doc.get(key) else {
        return;
    };
    match node {
        Node::Element(e) => {
            out.push('(');
            out.push_str(&e.kind);
            if let Some(level) = e.attrs.get_int("level") {
                out.push_str(" :level ");
                out.push_str(&level.to_string());
            }
            for &c in &e.children {
                out.push(' ');
                dump_node(doc, c, out);
            }
            out.push(')');
        }
        Node::Text(t) => {
            out.push_str("(text");
            if t.format.0 != 0 {
                out.push_str(" :fmt ");
                out.push_str(&format_to_str(t.format));
            }
            out.push_str(" \"");
            for ch in t.text.chars() {
                match ch {
                    '\\' => out.push_str("\\\\"),
                    '"' => out.push_str("\\\""),
                    '\n' => out.push_str("\\n"),
                    c => out.push(c),
                }
            }
            out.push_str("\")");
        }
        Node::Decorator(d) => {
            out.push('[');
            out.push_str(&d.kind);
            out.push(']');
        }
    }
}

fn format_to_str(f: FormatBits) -> String {
    let mut s = String::new();
    if f.contains(FormatBits::BOLD) {
        s.push('B');
    }
    if f.contains(FormatBits::ITALIC) {
        s.push('I');
    }
    if f.contains(FormatBits::STRIKE) {
        s.push('S');
    }
    if f.contains(FormatBits::CODE) {
        s.push('C');
    }
    if s.is_empty() {
        s.push('-');
    }
    s
}

fn dump_selection(sel: &Selection) -> String {
    match sel {
        Selection::None => "none".to_string(),
        Selection::Node(k) => format!("node({})", k),
        Selection::Range { anchor, focus } => {
            if anchor == focus {
                format!(
                    "caret({}, {:?}, {})",
                    anchor.key, anchor.kind, anchor.offset
                )
            } else {
                format!(
                    "range({}/{:?}/{} -> {}/{:?}/{})",
                    anchor.key, anchor.kind, anchor.offset, focus.key, focus.kind, focus.offset
                )
            }
        }
    }
}
