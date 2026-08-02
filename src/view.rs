//! Dioxus view + reconciler.
//!
//! [`EditorView`] is the single component the host mounts. It renders the
//! current `Doc` to a contenteditable surface, listens for editing events,
//! routes them through the plugin pipeline as transactions, and after every
//! state change writes the model selection back to the DOM. Decorator
//! rendering is delegated to the schema's registered renderers — the view
//! only owns the outer wrapper (`contenteditable="false"`, `data-key`, the
//! click-to-select handler).
//!
//! Host applications style the `editor` root (`editor--empty` while the
//! document has no content), the `editor__*` block classes, and the text-run
//! classes: every text span carries `e-t`, plus `e-b` (bold), `e-i`
//! (italic), `e-s` (strike), and `e-c` (inline code) for its active
//! formats. This crate does not ship CSS — see `docs/styling.md` for the
//! full class reference and the required host rules.

use dioxus::html::{Key, Modifiers};
use dioxus::prelude::*;

#[cfg(target_arch = "wasm32")]
use crate::commands::insert_text;
use crate::model::{Doc, Node, NodeKey};
use crate::plugin::EditorEvent;
use crate::selection::{Point, Selection};
use crate::state::{DispatchError, EditorHandle};

#[component]
pub fn EditorView(
    editor: EditorHandle,
    #[props(default = "Start writing…".to_string())] placeholder: String,
    #[props(default = "Rich text editor".to_string())] aria_label: String,
    #[props(default)] on_submit: Option<EventHandler<Doc>>,
    #[props(default)] on_error: Option<EventHandler<DispatchError>>,
) -> Element {
    editor.set_error_handler(on_error);
    #[cfg(target_arch = "wasm32")]
    {
        editor.dom_binding().borrow_mut().on_submit = on_submit;
    }
    let state_sig = editor.state_signal();
    let state = state_sig.read().clone();
    let doc = state.doc.clone();
    let is_empty = doc.is_empty();

    let editor_for_keys = editor.clone();
    let editor_for_paste = editor.clone();
    let editor_for_drop = editor.clone();
    let editor_for_focus = editor.clone();
    let editor_for_compose_start = editor.clone();
    let editor_for_compose_end = editor.clone();
    #[cfg(target_arch = "wasm32")]
    let editor_for_beforeinput = editor.clone();
    let _ = &editor_for_compose_start;
    // Provide the handle through context so nested DecoratorSlot
    // components can resolve schema renderers.
    use_context_provider(|| editor.clone());

    // Sync the DOM selection from the model after every render.
    {
        let editor = editor.clone();
        use_effect(use_reactive!(|state_sig| {
            let editor = editor.clone();
            let _ = state_sig; // touch for reactivity
            #[cfg(target_arch = "wasm32")]
            wasm::apply_model_selection_to_dom(&editor);
            #[cfg(not(target_arch = "wasm32"))]
            let _ = editor;
        }));
    }

    rsx! {
        div {
            class: if is_empty { "editor editor--empty" } else { "editor" },
            "data-placeholder": "{placeholder}",
            contenteditable: "true",
            spellcheck: "true",
            role: "textbox",
            "aria-multiline": "true",
            "aria-label": "{aria_label}",
            tabindex: "0",
            onkeydown: move |e: Event<KeyboardData>| {
                handle_keydown(&editor_for_keys, &on_submit, e);
            },
            onfocus: move |_| {
                let state = editor_for_focus.read_state();
                if matches!(state.selection, Selection::None)
                    && let Some((key, kind, off)) =
                        crate::plugins::last_leaf(&state.doc)
                    {
                        editor_for_focus.set_selection(Selection::caret(Point { key, offset: off, kind }));
                    }
            },
            oncompositionstart: move |_| {
                #[cfg(target_arch = "wasm32")]
                wasm::set_composing(true);
            },
            oncompositionend: move |e: Event<CompositionData>| {
                #[cfg(target_arch = "wasm32")]
                wasm::set_composing(false);
                let text = e.data.data();
                if !text.is_empty()
                    && let Some(tr) =
                        crate::commands::insert_text(&editor_for_compose_end.read_state(), &text)
                    {
                        editor_for_compose_end.report_internal(editor_for_compose_end.dispatch(tr));
                    }
            },
            // The browser's `selectionchange` only fires on `document`,
            // so an element-bound `onselectionchange` here would never
            // run. `wasm::install_selectionchange_listener` (called from
            // `onmounted`) attaches at the document level instead.
            onpaste: move |e: Event<ClipboardData>| {
                handle_paste(&editor_for_paste, e);
            },
            ondragover: move |e| {
                e.prevent_default();
            },
            ondrop: move |e: Event<DragData>| {
                e.prevent_default();
                handle_drop(&editor_for_drop, e);
            },
            onmounted: move |e| {
                #[cfg(target_arch = "wasm32")]
                wasm::attach_beforeinput(&editor_for_beforeinput, e.data());
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let _ = e;
                }
            },

            DocBody { doc: doc.clone() }
        }
    }
}

// -- rendering ------------------------------------------------------------

#[component]
fn DocBody(doc: Doc) -> Element {
    let root = doc.root_node();
    rsx! {
        for &child in root.children.iter() {
            RenderNode { doc: doc.clone(), node_key: child }
        }
    }
}

#[component]
fn RenderNode(doc: Doc, node_key: NodeKey) -> Element {
    let Some(node) = doc.get(node_key) else {
        return rsx! {};
    };
    match node {
        Node::Element(e) => {
            render_element(doc.clone(), e.key, &e.kind, &e.attrs_clone(), &e.children)
        }
        Node::Text(t) => {
            let cls = format!("e-t{}", t.format.css_class_suffix());
            // Empty text nodes still need a slot so the caret has somewhere
            // to land — render a zero-width-space marker the user never
            // sees, but the browser does.
            let text = if t.text.is_empty() {
                "\u{200B}".to_string()
            } else {
                t.text.clone()
            };
            rsx! {
                span {
                    class: "{cls}",
                    "data-key": "{t.key}",
                    "{text}"
                }
            }
        }
        Node::Decorator(d) => {
            // Resolve renderer through the state held by the schema. The
            // doc itself doesn't carry the schema; render_decorator is a
            // separate component that takes the schema from context.
            rsx! {
                DecoratorSlot { kind: d.kind.clone(), node_key: d.key, attrs: d.attrs_clone() }
            }
        }
    }
}

fn render_element(
    doc: Doc,
    key: NodeKey,
    kind: &str,
    attrs: &crate::attrs::Attrs,
    children: &[NodeKey],
) -> Element {
    let key_str = key.to_string();
    let render_children = |doc: Doc, kids: Vec<NodeKey>| -> Element {
        rsx! {
            for &c in kids.iter() {
                RenderNode { doc: doc.clone(), node_key: c }
            }
        }
    };
    let children_vec = children.to_vec();
    // Empty leaf blocks need a real DOM anchor (a `<br>`) inside their
    // editable region — not just visual height from CSS. Chromium's caret
    // traversal walks editable text / `<br>` nodes; an empty `<li>` with
    // only a layout placeholder is skipped by ArrowUp/Down, so two
    // consecutive empty bullets become unreachable from the keyboard. A
    // pseudo-element with `content` doesn't help here because it sits
    // outside the editable tree. The same `<br>` also gives the empty
    // block a baseline line-box so a fresh click lands a visible caret.
    let needs_br = children.is_empty()
        || (children.len() == 1
            && doc
                .get_text(children[0])
                .map(|t| t.text.is_empty())
                .unwrap_or(false));
    // True when the block's last text child ends in a `\n` — covered by
    // a CSS `::after` ZWSP on `.editor__code`, no DOM placeholder needed.
    let _ = needs_br; // referenced below per-kind
    match kind {
        "paragraph" => rsx! {
            p {
                class: "editor__p",
                "data-key": "{key_str}",
                if needs_br {
                    br {}
                } else {
                    {render_children(doc.clone(), children_vec)}
                }
            }
        },
        "heading" => {
            let level = attrs.get_int("level").unwrap_or(1).clamp(1, 6);
            let cls = format!("editor__h editor__h{level}");
            let kids = render_children(doc.clone(), children_vec);
            // Dioxus rsx doesn't allow dynamic element names — pick the
            // tag at compile time and fall back to `<h1>` styling via class.
            match level {
                1 => {
                    rsx! { h1 { class: "{cls}", "data-key": "{key_str}", if needs_br { br {} } else { {kids} } } }
                }
                2 => {
                    rsx! { h2 { class: "{cls}", "data-key": "{key_str}", if needs_br { br {} } else { {kids} } } }
                }
                3 => {
                    rsx! { h3 { class: "{cls}", "data-key": "{key_str}", if needs_br { br {} } else { {kids} } } }
                }
                4 => {
                    rsx! { h4 { class: "{cls}", "data-key": "{key_str}", if needs_br { br {} } else { {kids} } } }
                }
                5 => {
                    rsx! { h5 { class: "{cls}", "data-key": "{key_str}", if needs_br { br {} } else { {kids} } } }
                }
                _ => {
                    rsx! { h6 { class: "{cls}", "data-key": "{key_str}", if needs_br { br {} } else { {kids} } } }
                }
            }
        }
        "blockquote" => rsx! {
            blockquote {
                class: "editor__quote",
                "data-key": "{key_str}",
                if needs_br {
                    br {}
                } else {
                    {render_children(doc.clone(), children_vec)}
                }
            }
        },
        "code_block" => {
            let lang = attrs.get_str("lang").unwrap_or("").to_string();
            rsx! {
                pre {
                    class: "editor__pre",
                    "data-key": "{key_str}",
                    "data-lang": "{lang}",
                    code {
                        class: "editor__code",
                        if needs_br {
                            br {}
                        } else {
                            {render_children(doc.clone(), children_vec)}
                        }
                    }
                }
            }
        }
        "bullet_list" => rsx! {
            ul { class: "editor__ul", "data-key": "{key_str}", {render_children(doc.clone(), children_vec)} }
        },
        "ordered_list" => rsx! {
            ol { class: "editor__ol", "data-key": "{key_str}", {render_children(doc.clone(), children_vec)} }
        },
        "list_item" => rsx! {
            li {
                class: "editor__li",
                "data-key": "{key_str}",
                if needs_br {
                    br {}
                } else {
                    {render_children(doc.clone(), children_vec)}
                }
            }
        },
        "table" => rsx! {
            TableElement { doc: doc.clone(), node_key: key, attrs: attrs.clone() }
        },
        "table_row" => {
            let is_header = attrs.get_bool("header").unwrap_or(false);
            let cls = if is_header {
                "editor__tr editor__tr--header"
            } else {
                "editor__tr"
            };
            rsx! {
                tr { class: "{cls}", "data-key": "{key_str}", {render_children(doc.clone(), children_vec)} }
            }
        }
        "table_cell" => rsx! {
            TableCellEl {
                doc: doc.clone(),
                node_key: key,
                needs_br: needs_br,
            }
        },
        _ => rsx! {
            div { class: "editor__block", "data-key": "{key_str}", {render_children(doc.clone(), children_vec)} }
        },
    }
}

#[component]
fn DecoratorSlot(kind: String, node_key: NodeKey, attrs: crate::attrs::Attrs) -> Element {
    let handle = use_context::<EditorHandle>();
    let state = handle.read_state();
    let spec = state.schema.decorator(&kind);
    let Some(spec) = spec else {
        return rsx! {
            span {
                class: "editor__decorator editor__decorator--unknown",
                "data-key": "{node_key}",
                "[unknown decorator: {kind}]"
            }
        };
    };
    let inline = spec.inline;
    let inner = (spec.render)(&attrs);
    let key_str = node_key.to_string();
    let selected = matches!(&state.selection, Selection::Node(k) if *k == node_key);

    let mut classes = String::from("editor__decorator");
    classes.push_str(if inline {
        " editor__decorator--inline"
    } else {
        " editor__decorator--block"
    });
    if selected {
        classes.push_str(" editor__decorator--selected");
    }

    let handle_click = {
        let handle = handle.clone();
        move |e: Event<MouseData>| {
            e.stop_propagation();
            handle.set_selection(Selection::Node(node_key));
        }
    };
    let handle_remove = {
        let handle = handle.clone();
        move |e: Event<MouseData>| {
            e.stop_propagation();
            if let Some((parent, idx)) = handle.read_state().doc.child_index(node_key) {
                let tr = crate::step::Transaction::new()
                    .step(crate::step::Step::RemoveNodes {
                        parent,
                        range: idx..idx + 1,
                    })
                    .select(Selection::caret(Point {
                        key: parent,
                        offset: idx,
                        kind: crate::selection::PointKind::Element,
                    }));
                handle.report_internal(handle.dispatch(tr));
            }
        }
    };

    let controls = rsx! {
        button {
            class: "editor__decorator-remove",
            r#type: "button",
            title: "Remove",
            "aria-label": "Remove {kind}",
            onkeydown: move |e: Event<KeyboardData>| e.stop_propagation(),
            onmousedown: move |e: Event<MouseData>| { e.prevent_default(); },
            onclick: handle_remove,
            "×"
        }
    };

    if inline {
        rsx! {
            span {
                class: "{classes}",
                contenteditable: "false",
                "data-key": "{key_str}",
                "data-kind": "{kind}",
                onclick: handle_click,
                {inner}
                {controls}
            }
        }
    } else {
        rsx! {
            div {
                class: "{classes}",
                contenteditable: "false",
                "data-key": "{key_str}",
                "data-kind": "{kind}",
                onclick: handle_click,
                {inner}
                {controls}
            }
        }
    }
}

/// Shared open-popover state across a table's cells. Every cell consumes
/// this context; the single signal guarantees opening one cell's menu
/// closes any other that was already open.
#[derive(Clone, Copy)]
struct TableMenu {
    open: Signal<Option<NodeKey>>,
}

#[component]
fn TableElement(doc: Doc, node_key: NodeKey, attrs: crate::attrs::Attrs) -> Element {
    let key_str = node_key.to_string();
    let aligns = attrs.get_str("align").unwrap_or("").to_string();
    let handle = use_context::<EditorHandle>();

    let menu_open = use_signal::<Option<NodeKey>>(|| None);
    use_context_provider(|| TableMenu { open: menu_open });

    let children_vec = doc
        .get_element(node_key)
        .map(|e| e.children.clone())
        .unwrap_or_default();
    let row_count = children_vec.len();
    let col_count = children_vec
        .iter()
        .filter_map(|&k| doc.get_element(k))
        .map(|r| r.children.len())
        .max()
        .unwrap_or(0);

    let body = rsx! {
        for &c in children_vec.iter() {
            RenderNode { doc: doc.clone(), node_key: c }
        }
    };

    let h_add_col = handle.clone();
    let h_add_row = handle.clone();

    rsx! {
        div {
            class: "editor__table-wrap",
            "data-rows": "{row_count}",
            "data-cols": "{col_count}",
            table {
                class: "editor__table",
                "data-key": "{key_str}",
                "data-align": "{aligns}",
                tbody { {body} }
            }
            button {
                class: "editor__table-add editor__table-add--col",
                r#type: "button",
                contenteditable: "false",
                    title: "Click to add a new column",
                "aria-label": "Add column",
                onkeydown: move |e: Event<KeyboardData>| e.stop_propagation(),
                onmousedown: move |e: Event<MouseData>| { e.prevent_default(); },
                onclick: move |e: Event<MouseData>| {
                    e.stop_propagation();
                    if let Some(tr) =
                        crate::commands::append_column(&h_add_col.read_state(), node_key)
                    {
                        h_add_col.report_internal(h_add_col.dispatch(tr));
                    }
                },
                span { class: "editor__table-add-icon", "+" }
            }
            button {
                class: "editor__table-add editor__table-add--row",
                r#type: "button",
                contenteditable: "false",
                    title: "Click to add a new row",
                "aria-label": "Add row",
                onkeydown: move |e: Event<KeyboardData>| e.stop_propagation(),
                onmousedown: move |e: Event<MouseData>| { e.prevent_default(); },
                onclick: move |e: Event<MouseData>| {
                    e.stop_propagation();
                    if let Some(tr) =
                        crate::commands::append_row(&h_add_row.read_state(), node_key)
                    {
                        h_add_row.report_internal(h_add_row.dispatch(tr));
                    }
                },
                span { class: "editor__table-add-icon", "+" }
            }
        }
    }
}

#[component]
fn TableCellEl(doc: Doc, node_key: NodeKey, needs_br: bool) -> Element {
    let handle = use_context::<EditorHandle>();
    let menu = use_context::<TableMenu>();
    let mut menu_open = menu.open;
    let key_str = node_key.to_string();

    let attrs = doc
        .get_element(node_key)
        .map(|e| e.attrs.clone())
        .unwrap_or_default();
    let _ = attrs;
    let children_vec = doc
        .get_element(node_key)
        .map(|e| e.children.clone())
        .unwrap_or_default();

    let header_row = doc
        .parent(node_key)
        .and_then(|p| doc.get_element(p))
        .map(|r| r.attrs.get_bool("header").unwrap_or(false))
        .unwrap_or(false);
    let (col_idx, table_align) = doc
        .parent(node_key)
        .and_then(|row_key| {
            let row = doc.get_element(row_key)?;
            let pos = row.children.iter().position(|&c| c == node_key)?;
            let table_key = doc.parent(row_key)?;
            let align = doc
                .get_element(table_key)?
                .attrs
                .get_str("align")
                .unwrap_or("")
                .to_string();
            Some((pos, align))
        })
        .unwrap_or((0, String::new()));
    let align = table_align
        .split(',')
        .nth(col_idx)
        .unwrap_or("none")
        .to_string();
    let style = match align.as_str() {
        "left" => "text-align: left;",
        "center" => "text-align: center;",
        "right" => "text-align: right;",
        _ => "",
    };

    let is_open = menu_open() == Some(node_key);

    let on_menu_click = move |e: Event<MouseData>| {
        e.stop_propagation();
        if menu_open() == Some(node_key) {
            menu_open.set(None);
        } else {
            menu_open.set(Some(node_key));
        }
    };

    let popover = if is_open {
        rsx! {
            TableMenuPopover {
                cell_key: node_key,
                handle: handle.clone(),
                close: menu_open,
            }
        }
    } else {
        rsx! {}
    };

    let inner = rsx! {
        if needs_br {
            br {}
        } else {
            for &c in children_vec.iter() {
                RenderNode { doc: doc.clone(), node_key: c }
            }
        }
        button {
            class: "editor__cell-menu",
            r#type: "button",
            contenteditable: "false",
            title: "Cell actions",
            "aria-label": "Cell actions",
            "aria-haspopup": "dialog",
            "aria-expanded": "{is_open}",
            "data-cell-menu": "{key_str}",
            onkeydown: move |e: Event<KeyboardData>| e.stop_propagation(),
            onmousedown: move |e: Event<MouseData>| { e.prevent_default(); },
            onclick: on_menu_click,
            "⋯"
        }
        {popover}
    };

    if header_row {
        rsx! {
            th {
                class: "editor__th",
                "data-key": "{key_str}",
                style: "{style}",
                {inner}
            }
        }
    } else {
        rsx! {
            td {
                class: "editor__td",
                "data-key": "{key_str}",
                style: "{style}",
                {inner}
            }
        }
    }
}

#[component]
fn TableMenuPopover(
    cell_key: NodeKey,
    handle: EditorHandle,
    close: Signal<Option<NodeKey>>,
) -> Element {
    // Plant the caret into the popover's cell so caret-based commands
    // (`insert_row_above`, `delete_column`, …) see this cell as the
    // operative position. Closes the popover on completion. Returning
    // an `EventHandler<MouseData>` lets us share one builder across many
    // button definitions.
    let action = move |cmd: fn(&crate::state::EditorState) -> Option<crate::step::Transaction>| {
        let handle = handle.clone();
        let mut close = close;
        move |e: Event<MouseData>| {
            e.stop_propagation();
            handle.set_selection(crate::selection::Selection::caret(
                crate::selection::Point::element(cell_key, 0),
            ));
            let state = handle.read_state();
            if let Some(tr) = cmd(&state) {
                handle.report_internal(handle.dispatch(tr));
            }
            close.set(None);
        }
    };

    let close_backdrop = {
        let mut close = close;
        move |_: Event<MouseData>| close.set(None)
    };

    // A host container may clip the cell's stacking context with
    // `overflow-y: auto`, so an `absolute` popover gets cut off. Mount
    // the popover at viewport coordinates instead — compute the cell's
    // position once on mount and pin the popover there with `position:
    // fixed`. Starts `visibility: hidden` to avoid a one-frame flash
    // in the top-left before the script runs.
    let on_popover_mounted = move |_e: Event<MountedData>| {
        #[cfg(target_arch = "wasm32")]
        position_popover(cell_key, &_e);
    };
    let on_popover_keydown = {
        let mut close = close;
        move |e: Event<KeyboardData>| {
            e.stop_propagation();
            if e.key() == Key::Escape {
                e.prevent_default();
                close.set(None);
                #[cfg(target_arch = "wasm32")]
                focus_cell_menu(cell_key);
            }
        }
    };

    rsx! {
        div {
            class: "editor__table-popover-backdrop",
            contenteditable: "false",
            onmousedown: move |e: Event<MouseData>| { e.prevent_default(); },
            onclick: close_backdrop,
        }
        div {
            class: "editor__table-popover",
            contenteditable: "false",
            role: "dialog",
            "aria-label": "Cell actions",
            onmounted: on_popover_mounted,
            onkeydown: on_popover_keydown,
            onmousedown: move |e: Event<MouseData>| { e.prevent_default(); },
            div { class: "editor__table-popover-section",
                div { class: "editor__table-popover-label", "Row" }
                button {
                    class: "editor__table-popover-item",
                    r#type: "button",
                    contenteditable: "false",
                            onmousedown: move |e: Event<MouseData>| { e.prevent_default(); },
                    onclick: action(crate::commands::insert_row_above),
                    "Insert above"
                }
                button {
                    class: "editor__table-popover-item",
                    r#type: "button",
                    contenteditable: "false",
                            onmousedown: move |e: Event<MouseData>| { e.prevent_default(); },
                    onclick: action(crate::commands::insert_row_below),
                    "Insert below"
                }
                button {
                    class: "editor__table-popover-item",
                    r#type: "button",
                    contenteditable: "false",
                            onmousedown: move |e: Event<MouseData>| { e.prevent_default(); },
                    onclick: action(crate::commands::duplicate_row),
                    "Duplicate"
                }
                button {
                    class: "editor__table-popover-item editor__table-popover-item--danger",
                    r#type: "button",
                    contenteditable: "false",
                            onmousedown: move |e: Event<MouseData>| { e.prevent_default(); },
                    onclick: action(crate::commands::delete_row),
                    "Delete row"
                }
            }
            div { class: "editor__table-popover-section",
                div { class: "editor__table-popover-label", "Column" }
                button {
                    class: "editor__table-popover-item",
                    r#type: "button",
                    contenteditable: "false",
                            onmousedown: move |e: Event<MouseData>| { e.prevent_default(); },
                    onclick: action(crate::commands::insert_column_before),
                    "Insert left"
                }
                button {
                    class: "editor__table-popover-item",
                    r#type: "button",
                    contenteditable: "false",
                            onmousedown: move |e: Event<MouseData>| { e.prevent_default(); },
                    onclick: action(crate::commands::insert_column_after),
                    "Insert right"
                }
                button {
                    class: "editor__table-popover-item",
                    r#type: "button",
                    contenteditable: "false",
                            onmousedown: move |e: Event<MouseData>| { e.prevent_default(); },
                    onclick: action(crate::commands::duplicate_column),
                    "Duplicate"
                }
                button {
                    class: "editor__table-popover-item editor__table-popover-item--danger",
                    r#type: "button",
                    contenteditable: "false",
                            onmousedown: move |e: Event<MouseData>| { e.prevent_default(); },
                    onclick: action(crate::commands::delete_column),
                    "Delete column"
                }
            }
            div { class: "editor__table-popover-section",
                div { class: "editor__table-popover-label", "Cell" }
                button {
                    class: "editor__table-popover-item",
                    r#type: "button",
                    contenteditable: "false",
                            onmousedown: move |e: Event<MouseData>| { e.prevent_default(); },
                    onclick: action(crate::commands::clear_cell),
                    "Clear contents"
                }
                button {
                    class: "editor__table-popover-item editor__table-popover-item--danger",
                    r#type: "button",
                    contenteditable: "false",
                            onmousedown: move |e: Event<MouseData>| { e.prevent_default(); },
                    onclick: action(crate::commands::delete_table),
                    "Delete table"
                }
            }
        }
    }
}

// -- event handling ------------------------------------------------------

fn handle_keydown(
    editor: &EditorHandle,
    on_submit: &Option<EventHandler<Doc>>,
    e: Event<KeyboardData>,
) {
    let key = e.key();
    let mods = e.modifiers();
    let has_mod = mods.contains(Modifiers::CONTROL) || mods.contains(Modifiers::META);
    let has_shift = mods.contains(Modifiers::SHIFT);
    let _has_alt = mods.contains(Modifiers::ALT);

    // Composition guard — let browser handle IME-driven typing.
    #[cfg(target_arch = "wasm32")]
    if wasm::is_composing(editor) {
        return;
    }

    match &key {
        Key::Enter if !has_shift => {
            e.prevent_default();
            if let Some(on_submit) = on_submit {
                on_submit.call(editor.doc());
            } else if let Some(tr) = crate::commands::split_block(&editor.read_state()) {
                editor.report_internal(editor.dispatch(tr));
            }
        }
        Key::Enter => {
            // Shift+Enter (or Alt+Enter) inserts a line break by splitting
            // the current block. We handle it directly here rather than
            // relying on `beforeinput`'s `insertLineBreak` because Chrome
            // on macOS sometimes fires `insertParagraph` for both flavors
            // of Enter inside a generic contenteditable.
            e.prevent_default();
            if let Some(tr) = crate::commands::split_block(&editor.read_state()) {
                editor.report_internal(editor.dispatch(tr));
            }
        }
        Key::Backspace | Key::Delete => {
            // Modifier-aware deletes are routed through `beforeinput`'s
            // delete* inputTypes — those carry the user's intended scope
            // (word, line, character) better than reading modifiers off
            // a keydown.
        }
        Key::Character(_) if !has_mod => {
            // Plain character input lands via `beforeinput`. Letting
            // Dioxus's keydown path handle it would drop non-BMP
            // characters (emoji, surrogate pairs) and lose paste/IME
            // composition that doesn't surface as discrete key presses.
        }
        Key::Character(s) if has_mod => {
            // History navigation is handled by the History plugin.
            let lower = s.to_lowercase();
            if lower == "z" {
                e.prevent_default();
                let event = if has_shift {
                    EditorEvent::Redo
                } else {
                    EditorEvent::Undo
                };
                editor.report_internal(editor.handle_event(event));
                return;
            }
            if lower == "y" && !has_shift {
                e.prevent_default();
                editor.report_internal(editor.handle_event(EditorEvent::Redo));
                return;
            }
            // Other modifier shortcuts — translate to canonical `Mod-…`
            // key string and ask the registered keymap to fire.
            let mut canonical = String::from("Mod");
            if has_shift {
                canonical.push_str("-Shift");
            }
            canonical.push('-');
            canonical.push_str(&lower);
            if editor
                .report_internal(editor.run_key(&canonical))
                .unwrap_or(false)
            {
                e.prevent_default();
            }
        }
        Key::ArrowLeft | Key::ArrowRight | Key::ArrowUp | Key::ArrowDown => {
            // Let the browser move the caret; selection-sync happens via the
            // selectionchange listener attached at mount.
        }
        Key::Tab if crate::commands::table_context(&editor.read_state()).is_some() => {
            // Tab inside a table cell hops to the next cell; Shift+Tab
            // walks back. Tab past the last cell appends a new row. Outside
            // a table we leave Tab alone so the browser can shift focus.
            e.prevent_default();
            let state = editor.read_state();
            let tr = if has_shift {
                crate::commands::move_to_prev_cell(&state)
            } else {
                crate::commands::move_to_next_cell(&state)
            };
            if let Some(tr) = tr {
                editor.report_internal(editor.dispatch(tr));
            }
        }
        _ => {}
    }
}

/// Handle a paste. The editor manages plain-text paste itself; file
/// pastes are left alone so an outer wrapper (e.g. an upload
/// pipeline) can pick them up during event bubbling. We always
/// preventDefault so the browser doesn't write directly into the DOM —
/// our model owns content insertion.
fn handle_paste(editor: &EditorHandle, e: Event<ClipboardData>) {
    #[cfg(target_arch = "wasm32")]
    {
        let clipboard = e.data().data_transfer();
        if !clipboard.files().is_empty() {
            // Files present — let bubbling deliver them to the host.
            // Prevent the browser from also pasting any concurrent text
            // representation into the contenteditable.
            e.prevent_default();
            return;
        }
        if let Some(text) = clipboard.get_as_text()
            && !text.is_empty()
        {
            e.prevent_default();
            let state = editor.read_state();
            // A pasted URL becomes a link node: over a non-empty
            // selection it wraps the selected text (the pasted URL is
            // the href); otherwise the URL itself is the link label.
            let trimmed = text.trim();
            if state.schema.has_decorator("link") && crate::autolink::looks_like_url(trimmed) {
                let linked = match &state.selection {
                    Selection::Range { anchor, focus } if anchor != focus => {
                        crate::commands::wrap_selection_as_link(&state, trimmed)
                    }
                    _ => crate::commands::insert_link(&state, trimmed, trimmed),
                };
                if let Some(tr) = linked {
                    editor.report_internal(editor.dispatch(tr));
                    return;
                }
            }
            if let Some(tr) = insert_text(&state, &text) {
                editor.report_internal(editor.dispatch(tr));
            }
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (editor, e);
    }
}

fn handle_drop(editor: &EditorHandle, e: Event<DragData>) {
    #[cfg(target_arch = "wasm32")]
    {
        let raw = e.data();
        let Some(ev) = raw.downcast::<web_sys::DragEvent>() else {
            return;
        };
        let Some(dt) = ev.data_transfer() else {
            return;
        };
        if let Some(files) = dt.files()
            && files.length() > 0
        {
            // Same as paste: bubble to the host's upload pipeline.
            return;
        }
        if let Ok(text) = dt.get_data("text/plain")
            && !text.is_empty()
            && let Some(tr) = insert_text(&editor.read_state(), &text)
        {
            editor.report_internal(editor.dispatch(tr));
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (editor, e);
    }
}

// Convenience extension so `ElementNode` survives borrow-checker pressure
// when its data is fanned into multiple `rsx!` invocations during render.
impl crate::model::ElementNode {
    fn attrs_clone(&self) -> crate::attrs::Attrs {
        self.attrs.clone()
    }
}
impl crate::model::DecoratorNode {
    fn attrs_clone(&self) -> crate::attrs::Attrs {
        self.attrs.clone()
    }
}

#[cfg(target_arch = "wasm32")]
fn focus_cell_menu(cell_key: NodeKey) {
    use wasm_bindgen::JsCast;
    if let Some(button) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| {
            document
                .query_selector(&format!("[data-cell-menu=\"{}\"]", cell_key))
                .ok()
                .flatten()
        })
        .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
    {
        let _ = button.focus();
    }
}

#[cfg(target_arch = "wasm32")]
fn position_popover(cell_key: NodeKey, e: &Event<MountedData>) {
    use wasm_bindgen::JsCast;
    let data = e.data();
    let Some(pop) = data.downcast::<web_sys::Element>() else {
        return;
    };
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    let Some(cell) = doc
        .query_selector(&format!("[data-key=\"{}\"]", cell_key))
        .ok()
        .flatten()
    else {
        return;
    };
    let cell_rect = cell.get_bounding_client_rect();
    let pop_rect = pop.get_bounding_client_rect();
    let Some(win) = web_sys::window() else {
        return;
    };
    let vw = win
        .inner_width()
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let vh = win
        .inner_height()
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let margin = 6.0;
    let pop_w = pop_rect.width().max(160.0);
    let pop_h = pop_rect.height().max(60.0);

    // Default: open below the cell, right-aligned with its right edge so
    // the popover anchors near the `⋯` button. Flip above when the
    // viewport bottom would clip it; shift horizontally so it never
    // overflows either edge.
    let mut top = cell_rect.bottom() + 4.0;
    if top + pop_h > vh - margin {
        top = (cell_rect.top() - pop_h - 4.0).max(margin);
    }
    let mut left = cell_rect.right() - pop_w;
    if left + pop_w > vw - margin {
        left = vw - pop_w - margin;
    }
    if left < margin {
        left = margin;
    }
    let html = pop.dyn_ref::<web_sys::HtmlElement>().map(|h| h.style());
    if let Some(style) = html {
        let _ = style.set_property("top", &format!("{}px", top));
        let _ = style.set_property("left", &format!("{}px", left));
        let _ = style.set_property("visibility", "visible");
    }
}

// -- WASM glue: selection mapping + IME flag ------------------------------

/// Per-instance DOM binding for an editor: its contenteditable root
/// element plus the document-level `selectionchange` listener installed
/// for it. Held behind an `Rc<RefCell<…>>` on the [`EditorHandle`] so
/// every mounted editor owns its own root — multiple instances coexist
/// without colliding on a shared global root, which
/// would otherwise pin both editors' caret/selection mapping to whichever
/// one mounted last.
#[cfg(target_arch = "wasm32")]
#[derive(Default)]
pub(crate) struct DomBinding {
    root: Option<web_sys::Element>,
    beforeinput_listener: Option<wasm::BeforeInputListener>,
    cut_listener: Option<wasm::CutListener>,
    sel_listener: Option<wasm::SelListener>,
    on_submit: Option<EventHandler<Doc>>,
}

#[cfg(target_arch = "wasm32")]
impl DomBinding {
    pub(crate) fn root(&self) -> Option<web_sys::Element> {
        self.root.clone()
    }
}

#[cfg(target_arch = "wasm32")]
mod wasm {
    use std::cell::Cell;
    use std::rc::Rc;

    use dioxus::html::MountedData;
    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;

    use crate::model::{Doc, Node, NodeKey};
    use crate::selection::{Point, PointKind, Selection};
    use crate::state::EditorHandle;

    thread_local! {
        static COMPOSING: Cell<bool> = const { Cell::new(false) };
    }

    pub struct BeforeInputListener {
        element: web_sys::Element,
        closure: Closure<dyn FnMut(web_sys::InputEvent)>,
    }

    impl Drop for BeforeInputListener {
        fn drop(&mut self) {
            let _ = self.element.remove_event_listener_with_callback(
                "beforeinput",
                self.closure.as_ref().unchecked_ref(),
            );
        }
    }

    pub struct CutListener {
        element: web_sys::Element,
        closure: Closure<dyn FnMut(web_sys::ClipboardEvent)>,
    }

    impl Drop for CutListener {
        fn drop(&mut self) {
            let _ = self
                .element
                .remove_event_listener_with_callback("cut", self.closure.as_ref().unchecked_ref());
        }
    }

    pub struct SelListener {
        closure: Closure<dyn FnMut(web_sys::Event)>,
    }

    impl Drop for SelListener {
        fn drop(&mut self) {
            if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
                let _ = doc.remove_event_listener_with_callback(
                    "selectionchange",
                    self.closure.as_ref().unchecked_ref(),
                );
            }
        }
    }

    fn editor_root(handle: &EditorHandle) -> Option<web_sys::Element> {
        handle.dom_binding().borrow().root()
    }

    pub fn is_composing(_handle: &EditorHandle) -> bool {
        COMPOSING.with(|c| c.get())
    }

    pub fn set_composing(v: bool) {
        COMPOSING.with(|c| c.set(v));
    }

    /// Attach `beforeinput` to the editor element on mount. `beforeinput`
    /// is the canonical channel for ALL text input — typed characters
    /// (including non-BMP / emoji), IME accept, autocorrect, paste,
    /// form-style autofill. We attach directly because `beforeinput` needs
    /// access to browser-specific `InputEvent::input_type`. The closure is
    /// retained in the per-editor DOM binding and removed when the binding is
    /// replaced or dropped.
    pub fn attach_beforeinput(handle: &EditorHandle, data: Rc<MountedData>) {
        let Some(elem) = data.downcast::<web_sys::Element>() else {
            return;
        };
        let elem: web_sys::Element = elem.clone();
        handle.dom_binding().borrow_mut().root = Some(elem.clone());

        let handle_bi = handle.clone();
        let on_beforeinput = Closure::wrap(Box::new(move |e: web_sys::InputEvent| {
            // Composition is owned by the IME — let it run its course;
            // we'll reflect the final text via the compositionend handler.
            if COMPOSING.with(|c| c.get()) {
                return;
            }
            let input_type = e.input_type();
            match input_type.as_str() {
                "insertText" | "insertReplacementText" | "insertFromYank" | "insertFromDrop" => {
                    if let Some(text) = e.data() {
                        if !text.is_empty() {
                            e.prevent_default();
                            if let Some(tr) =
                                crate::commands::insert_text(&handle_bi.read_state(), &text)
                            {
                                handle_bi.report_internal(handle_bi.dispatch(tr));
                            }
                        } else {
                            // Some browsers omit `data` for empty text;
                            // still preventDefault to keep the DOM clean.
                            e.prevent_default();
                        }
                    }
                }
                "insertParagraph" => {
                    e.prevent_default();
                    let on_submit = handle_bi.dom_binding().borrow().on_submit;
                    if let Some(on_submit) = on_submit {
                        on_submit.call(handle_bi.doc());
                    } else if let Some(tr) = crate::commands::split_block(&handle_bi.read_state()) {
                        handle_bi.report_internal(handle_bi.dispatch(tr));
                    }
                }
                "insertLineBreak" => {
                    e.prevent_default();
                    if let Some(tr) = crate::commands::split_block(&handle_bi.read_state()) {
                        handle_bi.report_internal(handle_bi.dispatch(tr));
                    }
                }
                "deleteContentBackward" => {
                    e.prevent_default();
                    if let Some(tr) = crate::commands::delete_backward(&handle_bi.read_state()) {
                        handle_bi.report_internal(handle_bi.dispatch(tr));
                    }
                }
                "deleteContentForward" => {
                    e.prevent_default();
                    if let Some(tr) = crate::commands::delete_forward(&handle_bi.read_state()) {
                        handle_bi.report_internal(handle_bi.dispatch(tr));
                    }
                }
                "deleteByCut" => {
                    // Cut is fully serviced by the native `cut` listener
                    // (clipboard write + model delete, with preventDefault).
                    // A browser that still fires this mirror must not delete
                    // again — just keep the DOM from mutating.
                    e.prevent_default();
                }
                "deleteByDrag" => {
                    // Move-source half of an internal drag: remove the
                    // dragged range from the model.
                    e.prevent_default();
                    if let Some(tr) = crate::commands::delete_backward(&handle_bi.read_state()) {
                        handle_bi.report_internal(handle_bi.dispatch(tr));
                    }
                }
                "deleteWordBackward" => {
                    e.prevent_default();
                    if let Some(tr) = crate::commands::delete_word_backward(&handle_bi.read_state())
                    {
                        handle_bi.report_internal(handle_bi.dispatch(tr));
                    }
                }
                "deleteWordForward" => {
                    e.prevent_default();
                    if let Some(tr) = crate::commands::delete_word_forward(&handle_bi.read_state())
                    {
                        handle_bi.report_internal(handle_bi.dispatch(tr));
                    }
                }
                "deleteSoftLineBackward" | "deleteHardLineBackward" => {
                    e.prevent_default();
                    if let Some(tr) =
                        crate::commands::delete_to_block_start(&handle_bi.read_state())
                    {
                        handle_bi.report_internal(handle_bi.dispatch(tr));
                    }
                }
                "deleteSoftLineForward" | "deleteHardLineForward" => {
                    e.prevent_default();
                    if let Some(tr) = crate::commands::delete_to_block_end(&handle_bi.read_state())
                    {
                        handle_bi.report_internal(handle_bi.dispatch(tr));
                    }
                }
                "formatBold" => {
                    e.prevent_default();
                    if let Some(tr) = crate::commands::toggle_bold(&handle_bi.read_state()) {
                        handle_bi.report_internal(handle_bi.dispatch(tr));
                    }
                }
                "formatItalic" => {
                    e.prevent_default();
                    if let Some(tr) = crate::commands::toggle_italic(&handle_bi.read_state()) {
                        handle_bi.report_internal(handle_bi.dispatch(tr));
                    }
                }
                "insertFromPaste" | "insertFromPasteAsQuotation" => {
                    // Paste is handled by the Dioxus `onpaste` handler on
                    // the editor (which routes text vs. files). The
                    // `beforeinput` mirror still fires here; preventDefault
                    // so the browser doesn't double-insert.
                    e.prevent_default();
                }
                _ => {
                    // Unknown inputType — for safety, prevent the default
                    // so the browser can't quietly mutate the DOM behind
                    // the model's back.
                    e.prevent_default();
                }
            }
        }) as Box<dyn FnMut(_)>);
        let _ = elem.add_event_listener_with_callback(
            "beforeinput",
            on_beforeinput.as_ref().unchecked_ref(),
        );
        handle.dom_binding().borrow_mut().beforeinput_listener = Some(BeforeInputListener {
            element: elem.clone(),
            closure: on_beforeinput,
        });

        attach_cut(handle, &elem);
        install_selectionchange_listener(handle);
    }

    /// Wire up Cmd/Ctrl+X. The `beforeinput` `deleteByCut` mirror can't reach
    /// the clipboard, so we attach a native `cut` listener: copy the current
    /// selection text onto the clipboard, suppress the browser's own DOM
    /// mutation, then remove
    /// the selected range through the model's delete path. Without this,
    /// every cut hit the `beforeinput` catch-all that only preventDefaulted
    /// — so Cmd+X did nothing.
    fn attach_cut(handle: &EditorHandle, elem: &web_sys::Element) {
        let handle_cut = handle.clone();
        let on_cut = Closure::wrap(Box::new(move |e: web_sys::ClipboardEvent| {
            if COMPOSING.with(|c| c.get()) {
                return;
            }
            let Some(win) = web_sys::window() else {
                return;
            };
            let selected = win
                .get_selection()
                .ok()
                .flatten()
                .map(|s| s.to_string().as_string().unwrap_or_default())
                .unwrap_or_default();
            if selected.is_empty() {
                return;
            }
            if let Some(cb) = e.clipboard_data() {
                let _ = cb.set_data("text/plain", &selected);
            }
            // Own the mutation: stop the browser from editing the
            // contenteditable directly, then mirror the delete into the
            // model so DOM and model stay in lockstep.
            e.prevent_default();
            if let Some(tr) = crate::commands::delete_backward(&handle_cut.read_state()) {
                handle_cut.report_internal(handle_cut.dispatch(tr));
            }
        }) as Box<dyn FnMut(_)>);
        let _ = elem.add_event_listener_with_callback("cut", on_cut.as_ref().unchecked_ref());
        handle.dom_binding().borrow_mut().cut_listener = Some(CutListener {
            element: elem.clone(),
            closure: on_cut,
        });
    }

    /// The browser's `selectionchange` event fires on `document` only —
    /// element-level binding never fires (Dioxus's `onselectionchange`
    /// attribute is bound element-level so it can't see arrow-key or
    /// click-driven caret moves). We install a document listener per
    /// editor instance and store it on the handle's `DomBinding`. The
    /// closure short-circuits when the changed selection isn't inside this
    /// editor's own root, so multiple mounted instances only react to their
    /// own caret moves — the
    /// listener is dropped with the handle when its component unmounts.
    fn install_selectionchange_listener(handle: &EditorHandle) {
        let handle_cb = handle.clone();
        let closure = Closure::wrap(Box::new(move |_e: web_sys::Event| {
            if let Some(sel) = read_dom_selection(&handle_cb) {
                handle_cb.set_selection(sel.clone());
                handle_cb.report_internal(
                    handle_cb.handle_event(crate::plugin::EditorEvent::SelectionChange(sel)),
                );
            }
        }) as Box<dyn FnMut(_)>);
        if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
            let _ = doc.add_event_listener_with_callback(
                "selectionchange",
                closure.as_ref().unchecked_ref(),
            );
        }
        // Replacing the slot drops any previous closure, whose `Drop` impl
        // removes its listener.
        handle.dom_binding().borrow_mut().sel_listener = Some(SelListener { closure });
    }

    pub fn read_dom_selection(handle: &EditorHandle) -> Option<Selection> {
        let win = web_sys::window()?;
        let sel = win.get_selection().ok().flatten()?;
        if sel.range_count() == 0 {
            return None;
        }
        let anchor_node = sel.anchor_node()?;
        let focus_node = sel.focus_node()?;
        let root = editor_root(handle)?;
        if !node_in_root(&anchor_node, &root) || !node_in_root(&focus_node, &root) {
            return None;
        }
        let anchor_offset = sel.anchor_offset() as usize;
        let focus_offset = sel.focus_offset() as usize;
        let anchor_point = point_from_dom(handle, &anchor_node, anchor_offset)?;
        let focus_point = point_from_dom(handle, &focus_node, focus_offset)?;
        Some(Selection::Range {
            anchor: anchor_point,
            focus: focus_point,
        })
    }

    fn node_in_root(node: &web_sys::Node, root: &web_sys::Element) -> bool {
        let root_node: &web_sys::Node = root.unchecked_ref();
        root_node.contains(Some(node))
    }

    fn point_from_dom(
        handle: &EditorHandle,
        dom_node: &web_sys::Node,
        offset: usize,
    ) -> Option<Point> {
        // Cmd+A, mouse drags into the editor padding, and the browser's
        // own "select all" sometimes leave the selection anchored on a
        // node that has no `data-key` — the contenteditable root itself
        // (no key by design) or a wrapper between blocks. Descending into
        // the actual leaf at `offset` finds the keyed node that selection
        // really points at and gives us a usable model Point.
        let (resolved_node, resolved_offset) = normalize_to_keyed(dom_node, offset);
        let (key, kind) = nearest_keyed_ancestor(&resolved_node)?;
        let state = handle.read_state();
        let model_node = state.doc.get(key)?;
        match (model_node, kind) {
            (Node::Text(t), _) => {
                // DOM offsets count UTF-16 code units; our model offsets
                // count chars (code points). Convert so a click between
                // an emoji and the next char doesn't land mid-surrogate
                // in the model (which then breaks backspace's "remove
                // char before caret" semantics).
                let char_offset = dom_utf16_to_char_offset(&t.text, resolved_offset);
                Some(Point::text(key, char_offset))
            }
            (Node::Element(e), _) => {
                let len = e.children.len();
                Some(Point::element(key, resolved_offset.min(len)))
            }
            (Node::Decorator(_), _) => Some(Point::element(
                state.doc.parent(key).unwrap_or(state.doc.root_key()),
                if resolved_offset > 0 { 1 } else { 0 },
            )),
        }
    }

    /// Convert a DOM text offset (counted in UTF-16 code units, the way
    /// `Range.setStart` and `Selection.anchorOffset` interpret positions
    /// inside a text node) into a char-index into the model's `String`
    /// (counted in Unicode code points). Without this conversion, a
    /// click between an astral-plane emoji and the next char lands on
    /// the wrong model offset and downstream commands (Backspace,
    /// SplitText, ReplaceText) misbehave near surrogate pairs.
    fn dom_utf16_to_char_offset(text: &str, dom_offset: usize) -> usize {
        let mut utf16_count = 0usize;
        for (char_idx, ch) in text.chars().enumerate() {
            if utf16_count >= dom_offset {
                return char_idx;
            }
            utf16_count += ch.len_utf16();
        }
        text.chars().count()
    }

    /// Inverse of `dom_utf16_to_char_offset` — used when writing the
    /// model selection back to the DOM. `set_base_and_extent` and the
    /// `Range` API count text positions in UTF-16 code units.
    fn char_to_dom_utf16_offset(text: &str, char_offset: usize) -> usize {
        let mut utf16_count = 0usize;
        for (idx, ch) in text.chars().enumerate() {
            if idx >= char_offset {
                return utf16_count;
            }
            utf16_count += ch.len_utf16();
        }
        utf16_count
    }

    /// Walk down from `(node, offset)` to a usable leaf. When the
    /// starting anchor sits on the unkeyed editor root (the
    /// contenteditable wrapper) — as it does for `Cmd+A` /
    /// `selectNodeContents` / mouse drags into padding — we keep
    /// descending past any keyed wrapper until we land on a text node, so
    /// the resulting model `Point` is text-anchored and the selection
    /// machinery has a real char offset to work with. Anchors that
    /// already sit inside a keyed element are returned as-is — those are
    /// already meaningful at the model layer.
    fn normalize_to_keyed(node: &web_sys::Node, offset: usize) -> (web_sys::Node, usize) {
        // If the starting node is already inside a keyed element, leave
        // it alone: the original semantic (text char offset or element
        // child index) is exactly what point_from_dom wants.
        let started_unkeyed =
            nearest_keyed_ancestor(node).is_none() && node.node_type() != web_sys::Node::TEXT_NODE;
        if !started_unkeyed {
            return (node.clone(), offset);
        }
        // Started on an unkeyed wrapper (the editor root). Walk into the
        // child indicated by `offset` and recurse down to the deepest
        // descendant text node — accumulating an end-of-content offset
        // when we step past the last child of any level.
        let mut cur = node.clone();
        let mut off = offset;
        loop {
            if cur.node_type() == web_sys::Node::TEXT_NODE {
                return (cur, off);
            }
            let children = cur.child_nodes();
            let n = children.length() as usize;
            if n == 0 {
                return (cur, off);
            }
            let (idx, want_end) = if off < n { (off, false) } else { (n - 1, true) };
            let Some(child) = children.item(idx as u32) else {
                return (cur, off);
            };
            off = if want_end {
                match child.node_type() {
                    t if t == web_sys::Node::TEXT_NODE => {
                        child.node_value().map(|s| s.chars().count()).unwrap_or(0)
                    }
                    _ => child
                        .dyn_ref::<web_sys::Element>()
                        .map(|el| el.child_nodes().length() as usize)
                        .unwrap_or(0),
                }
            } else {
                0
            };
            cur = child;
        }
    }

    fn nearest_keyed_ancestor(node: &web_sys::Node) -> Option<(NodeKey, PointKind)> {
        let mut cur: Option<web_sys::Node> = Some(node.clone());
        // If the starting node is a text node, the "keyed ancestor" is its
        // parent span (PointKind::Text).
        let starting_was_text = node.node_type() == web_sys::Node::TEXT_NODE;
        while let Some(n) = cur {
            if let Some(el) = n.dyn_ref::<web_sys::Element>()
                && let Some(key_attr) = el.get_attribute("data-key")
                && let Ok(k) = key_attr.parse::<NodeKey>()
            {
                let kind = if starting_was_text {
                    PointKind::Text
                } else {
                    PointKind::Element
                };
                return Some((k, kind));
            }
            cur = n.parent_node();
        }
        None
    }

    pub fn apply_model_selection_to_dom(handle: &EditorHandle) {
        let state = handle.read_state();
        let sel = state.selection.clone();
        let Selection::Range { anchor, focus } = sel else {
            return;
        };
        let Some(root) = editor_root(handle) else {
            return;
        };
        let doc = &state.doc;
        let Some((a_node, a_off)) = locate_dom_point(&root, doc, anchor) else {
            return;
        };
        let Some((f_node, f_off)) = locate_dom_point(&root, doc, focus) else {
            return;
        };
        let Some(win) = web_sys::window() else {
            return;
        };
        let Some(sel_obj) = win.get_selection().ok().flatten() else {
            return;
        };
        let _ = sel_obj.set_base_and_extent(&a_node, a_off as u32, &f_node, f_off as u32);
    }

    fn locate_dom_point(
        root: &web_sys::Element,
        doc: &Doc,
        point: Point,
    ) -> Option<(web_sys::Node, usize)> {
        let target = find_keyed(root, point.key)?;
        match point.kind {
            PointKind::Text => {
                // Caller wants a text caret inside the span. Grab the
                // first text-DOM-node child and translate the model's
                // char offset into the DOM's UTF-16 offset (they differ
                // around astral-plane characters / emoji).
                let child = target.first_child()?;
                let text = child.node_value().unwrap_or_default();
                let utf16_off = char_to_dom_utf16_offset(&text, point.offset);
                Some((child, utf16_off))
            }
            PointKind::Element => {
                // Element anchor: place inside the element at child index
                // `point.offset`. The element's children may include the
                // span we render for empty paragraphs (a single `<br/>`);
                // clamp.
                let target_node: web_sys::Node = target.unchecked_into();
                let child_count = target_node.child_nodes().length() as usize;
                let off = point.offset.min(child_count);
                let _ = doc;
                Some((target_node, off))
            }
        }
    }

    fn find_keyed(root: &web_sys::Element, key: NodeKey) -> Option<web_sys::Element> {
        let selector = format!("[data-key=\"{}\"]", key);
        root.query_selector(&selector).ok().flatten()
    }
}
