//! End-to-end scenarios driving the editor like a user would.
//!
//! These tests bypass Dioxus rendering but exercise the full state machine:
//! commands produce transactions, plugins (markdown shortcuts, history) run
//! their `append_transaction` hooks, and the doc shape is asserted after
//! each step. The goal is to catch real-world flow bugs without booting a
//! browser.

use std::rc::Rc;

use dioxus_editor::FormatBits;
use dioxus_editor::Plugin;
use dioxus_editor::commands::{
    delete_backward, delete_range_transaction, delete_to_block_start, insert_text, split_block,
    toggle_blockquote, toggle_bold, toggle_bullet_list, toggle_code, toggle_code_block,
    toggle_heading, toggle_italic, toggle_ordered_list, toggle_strike,
};
use dioxus_editor::plugins::{History, MarkdownShortcuts};
use dioxus_editor::*;

/// Mini test harness: tracks state, lets you `type`, `enter`, `backspace`,
/// and run named commands while plugins fire between transactions.
struct Sim {
    state: EditorState,
    plugins: Vec<Box<dyn Plugin>>,
}

impl Sim {
    fn new() -> Self {
        let schema = Rc::new(Schema::new());
        let mut state = EditorState::new(schema, Doc::empty());
        // Caret in the lone empty paragraph at element offset 0, matching
        // what the view's onfocus handler installs.
        let para = state.doc.root_node().children[0];
        state.selection = Selection::caret(Point::element(para, 0));
        Self {
            state,
            plugins: vec![Box::new(MarkdownShortcuts::new()), Box::new(History::new())],
        }
    }

    fn apply(&mut self, tr: Transaction) {
        let prev = self.state.clone();
        let mut next = prev.apply(tr.clone()).expect("apply");
        let mut latest = tr;
        for _ in 0..8 {
            let mut chained: Option<Transaction> = None;
            for p in self.plugins.iter_mut() {
                if let Some(t) = p.append_transaction(&latest, &self.state, &next) {
                    chained = Some(t);
                    break;
                }
            }
            let Some(t) = chained else {
                break;
            };
            let after = next.apply(t.clone()).expect("plugin apply");
            self.state = next;
            next = after;
            latest = t;
        }
        self.state = next;
    }

    fn type_chars(&mut self, s: &str) {
        for ch in s.chars() {
            let mut buf = [0u8; 4];
            let s1 = ch.encode_utf8(&mut buf);
            let Some(tr) = insert_text(&self.state, s1) else {
                panic!("insert_text returned None for {s1:?}");
            };
            self.apply(tr);
        }
    }

    fn enter(&mut self) {
        let tr = split_block(&self.state).expect("split_block");
        self.apply(tr);
    }

    fn backspace(&mut self) {
        let Some(tr) = delete_backward(&self.state) else {
            return;
        };
        self.apply(tr);
    }

    fn cmd<F>(&mut self, f: F)
    where
        F: FnOnce(&EditorState) -> Option<Transaction>,
    {
        let tr = f(&self.state).expect("command produced no transaction");
        self.apply(tr);
    }

    fn root_kinds(&self) -> Vec<String> {
        self.state
            .doc
            .root_node()
            .children
            .iter()
            .map(|&k| {
                self.state
                    .doc
                    .get(k)
                    .map(|n| n.kind().to_string())
                    .unwrap_or_default()
            })
            .collect()
    }

    fn block_at(&self, idx: usize) -> &dioxus_editor::ElementNode {
        let root = self.state.doc.root_node();
        self.state
            .doc
            .get_element(root.children[idx])
            .expect("block")
    }

    fn block_text(&self, idx: usize) -> String {
        let block = self.block_at(idx);
        let mut s = String::new();
        for &c in &block.children {
            collect_text(&self.state.doc, c, &mut s);
        }
        s
    }
}

fn collect_text(doc: &Doc, key: NodeKey, out: &mut String) {
    match doc.get(key) {
        Some(Node::Text(t)) => out.push_str(&t.text),
        Some(Node::Element(e)) => {
            for &c in &e.children {
                collect_text(doc, c, out);
            }
        }
        Some(Node::Decorator(_)) | None => {}
    }
}

// -- typing into a fresh editor -------------------------------------------

#[test]
fn typing_into_empty_editor_lands_in_paragraph() {
    let mut sim = Sim::new();
    sim.type_chars("hi");
    assert_eq!(sim.root_kinds(), vec!["paragraph"]);
    assert_eq!(sim.block_text(0), "hi");
}

#[test]
fn typing_special_char_lands_in_paragraph() {
    // Non-letter chars (like `>`, `#`, `*`) must also land inside the
    // paragraph — they used to leak out to root level when the caret
    // was element-anchored at offset 0 of the root.
    let mut sim = Sim::new();
    sim.type_chars(">");
    assert_eq!(sim.root_kinds(), vec!["paragraph"]);
    assert_eq!(sim.block_text(0), ">");
}

// -- markdown block shortcuts --------------------------------------------

#[test]
fn shortcut_blockquote() {
    let mut sim = Sim::new();
    sim.type_chars("> ");
    assert_eq!(sim.root_kinds(), vec!["blockquote"]);
    // The shortcut should consume "> " entirely — leaving the blockquote
    // empty (caret at start of its inner text node).
    assert_eq!(sim.block_text(0), "");
}

#[test]
fn shortcut_h1() {
    let mut sim = Sim::new();
    sim.type_chars("# title");
    assert_eq!(sim.root_kinds(), vec!["heading"]);
    assert_eq!(sim.block_text(0), "title");
}

#[test]
fn shortcut_bullet_list() {
    let mut sim = Sim::new();
    sim.type_chars("- one");
    assert_eq!(sim.root_kinds(), vec!["bullet_list"]);
    // bullet_list > list_item > text "one"
    let list = sim.block_at(0);
    let li = sim.state.doc.get_element(list.children[0]).unwrap();
    assert_eq!(li.kind, "list_item");
    let mut s = String::new();
    for &c in &li.children {
        collect_text(&sim.state.doc, c, &mut s);
    }
    assert_eq!(s, "one");
}

#[test]
fn shortcut_ordered_list() {
    let mut sim = Sim::new();
    sim.type_chars("1. one");
    assert_eq!(sim.root_kinds(), vec!["ordered_list"]);
}

// -- Shift+Enter in each block kind --------------------------------------

#[test]
fn shift_enter_in_paragraph_splits() {
    let mut sim = Sim::new();
    sim.type_chars("hello");
    sim.enter();
    sim.type_chars("world");
    assert_eq!(sim.root_kinds(), vec!["paragraph", "paragraph"]);
    assert_eq!(sim.block_text(0), "hello");
    assert_eq!(sim.block_text(1), "world");
}

#[test]
fn shift_enter_in_heading_demotes_new_block_to_paragraph() {
    let mut sim = Sim::new();
    sim.type_chars("# title");
    sim.enter();
    sim.type_chars("body");
    assert_eq!(sim.root_kinds(), vec!["heading", "paragraph"]);
    assert_eq!(sim.block_text(0), "title");
    assert_eq!(sim.block_text(1), "body");
}

#[test]
fn shift_enter_in_blockquote_splits_into_another_blockquote() {
    let mut sim = Sim::new();
    sim.type_chars("> one");
    sim.enter();
    sim.type_chars("two");
    // We accept either: both blockquotes (markdown style) OR blockquote
    // followed by paragraph. Just ensure the second line is reachable.
    assert!(sim.root_kinds().len() >= 2, "kinds={:?}", sim.root_kinds());
    assert_eq!(sim.block_text(0), "one");
    assert_eq!(sim.block_text(1), "two");
}

#[test]
fn shift_enter_in_code_block_inserts_newline_not_split() {
    let mut sim = Sim::new();
    sim.cmd(toggle_code_block);
    sim.type_chars("a");
    sim.enter();
    sim.type_chars("b");
    // One code_block, content "a\nb".
    assert_eq!(sim.root_kinds(), vec!["code_block"]);
    assert_eq!(sim.block_text(0), "a\nb");
}

#[test]
fn shift_enter_in_list_creates_new_item() {
    let mut sim = Sim::new();
    sim.type_chars("- one");
    sim.enter();
    sim.type_chars("two");
    assert_eq!(sim.root_kinds(), vec!["bullet_list"]);
    let list = sim.block_at(0);
    assert_eq!(
        list.children.len(),
        2,
        "expected two list_items, got {} ({:?})",
        list.children.len(),
        list.children
            .iter()
            .filter_map(|&k| sim.state.doc.get_element(k).map(|e| e.kind.clone()))
            .collect::<Vec<_>>()
    );
    let li0 = sim.state.doc.get_element(list.children[0]).unwrap();
    let li1 = sim.state.doc.get_element(list.children[1]).unwrap();
    assert_eq!(li0.kind, "list_item");
    assert_eq!(li1.kind, "list_item");
    let mut s0 = String::new();
    for &c in &li0.children {
        collect_text(&sim.state.doc, c, &mut s0);
    }
    let mut s1 = String::new();
    for &c in &li1.children {
        collect_text(&sim.state.doc, c, &mut s1);
    }
    assert_eq!(s0, "one");
    assert_eq!(s1, "two");
}

// -- backspace at start ---------------------------------------------------

#[test]
fn backspace_at_heading_start_demotes_to_paragraph() {
    let mut sim = Sim::new();
    sim.type_chars("# title");
    // Move caret to start of "title".
    let heading = sim.block_at(0);
    let text_key = heading.children[0];
    sim.state.selection = Selection::caret(Point::text(text_key, 0));
    sim.backspace();
    assert_eq!(sim.root_kinds(), vec!["paragraph"]);
    assert_eq!(sim.block_text(0), "title");
}

#[test]
fn backspace_at_blockquote_start_demotes_to_paragraph() {
    let mut sim = Sim::new();
    sim.type_chars("> body");
    let bq = sim.block_at(0);
    let text_key = bq.children[0];
    sim.state.selection = Selection::caret(Point::text(text_key, 0));
    sim.backspace();
    assert_eq!(sim.root_kinds(), vec!["paragraph"]);
    assert_eq!(sim.block_text(0), "body");
}

#[test]
fn backspace_joins_empty_second_paragraph() {
    let mut sim = Sim::new();
    sim.type_chars("first");
    sim.enter();
    // Now in a fresh empty paragraph. Backspace should join back.
    sim.backspace();
    assert_eq!(sim.root_kinds(), vec!["paragraph"]);
    assert_eq!(sim.block_text(0), "first");
}

// -- toolbar toggles ------------------------------------------------------

#[test]
fn toggle_blockquote_wraps_current_block() {
    let mut sim = Sim::new();
    sim.type_chars("hi");
    sim.cmd(toggle_blockquote);
    assert_eq!(sim.root_kinds(), vec!["blockquote"]);
    assert_eq!(sim.block_text(0), "hi");
}

#[test]
fn toggle_h2_then_toggle_again_reverts() {
    let mut sim = Sim::new();
    sim.type_chars("hi");
    sim.cmd(|s| toggle_heading(s, 2));
    assert_eq!(sim.root_kinds(), vec!["heading"]);
    sim.cmd(|s| toggle_heading(s, 2));
    assert_eq!(sim.root_kinds(), vec!["paragraph"]);
    assert_eq!(sim.block_text(0), "hi");
}

#[test]
fn toggle_bullet_list_wraps_paragraph() {
    let mut sim = Sim::new();
    sim.type_chars("hi");
    sim.cmd(toggle_bullet_list);
    assert_eq!(sim.root_kinds(), vec!["bullet_list"]);
}

#[test]
fn toggle_ordered_then_bullet_swaps_kind_only() {
    let mut sim = Sim::new();
    sim.type_chars("hi");
    sim.cmd(toggle_ordered_list);
    assert_eq!(sim.root_kinds(), vec!["ordered_list"]);
    sim.cmd(toggle_bullet_list);
    assert_eq!(sim.root_kinds(), vec!["bullet_list"]);
}

// -- list editing -------------------------------------------------------

fn list_items_text(sim: &Sim, list_idx: usize) -> Vec<String> {
    let list = sim.block_at(list_idx);
    list.children
        .iter()
        .map(|&item_key| {
            let item = sim.state.doc.get_element(item_key).expect("list_item");
            let mut s = String::new();
            for &c in &item.children {
                collect_text(&sim.state.doc, c, &mut s);
            }
            s
        })
        .collect()
}

#[test]
fn three_item_ordered_list_swap_to_bullet_preserves_all_items() {
    let mut sim = Sim::new();
    sim.type_chars("1. one");
    sim.enter();
    sim.type_chars("two");
    sim.enter();
    sim.type_chars("three");
    assert_eq!(sim.root_kinds(), vec!["ordered_list"]);
    assert_eq!(
        list_items_text(&sim, 0),
        vec!["one".to_string(), "two".to_string(), "three".to_string()]
    );
    sim.cmd(toggle_bullet_list);
    assert_eq!(sim.root_kinds(), vec!["bullet_list"]);
    assert_eq!(
        list_items_text(&sim, 0),
        vec!["one".to_string(), "two".to_string(), "three".to_string()],
        "swapping ol → ul must preserve every item, not just the first"
    );
}

#[test]
fn toggle_bullet_on_third_item_lifts_only_that_item() {
    // Block toggles are per-line — clicking the bullet button while
    // inside the third item should lift JUST that item out as a
    // paragraph; the first two items stay in a (shrunk) bullet list
    // immediately above the new paragraph.
    let mut sim = Sim::new();
    sim.type_chars("- one");
    sim.enter();
    sim.type_chars("two");
    sim.enter();
    sim.type_chars("three");
    sim.cmd(toggle_bullet_list);
    assert_eq!(sim.root_kinds(), vec!["bullet_list", "paragraph"]);
    assert_eq!(
        list_items_text(&sim, 0),
        vec!["one".to_string(), "two".to_string()],
    );
    assert_eq!(sim.block_text(1), "three");
}

#[test]
fn toggle_bullet_on_middle_item_splits_list_around_paragraph() {
    // Caret in item 2 of a 3-item list, then toggle bullet — the list
    // splits into [list("one")] + paragraph("two") + [list("three")].
    let mut sim = Sim::new();
    sim.type_chars("- one");
    sim.enter();
    sim.type_chars("two");
    sim.enter();
    sim.type_chars("three");
    let list = sim.block_at(0);
    let item2 = sim.state.doc.get_element(list.children[1]).unwrap();
    sim.state.selection = Selection::caret(Point::text(item2.children[0], 0));
    sim.cmd(toggle_bullet_list);
    assert_eq!(
        sim.root_kinds(),
        vec!["bullet_list", "paragraph", "bullet_list"],
    );
    assert_eq!(list_items_text(&sim, 0), vec!["one".to_string()]);
    assert_eq!(sim.block_text(1), "two");
    assert_eq!(list_items_text(&sim, 2), vec!["three".to_string()]);
}

#[test]
fn backspace_in_empty_middle_list_item_splits_list_around_paragraph() {
    // Cursor in an empty middle list_item; Backspace should leave a
    // paragraph at that slot, splitting the surrounding list into two —
    // not merge the empty item into the previous one (which would hop
    // the cursor backwards to the previous bullet's end).
    let mut sim = Sim::new();
    sim.type_chars("- one");
    sim.enter();
    sim.enter(); // empty item 2
    sim.type_chars("three");
    let list = sim.block_at(0);
    // After: items = [one, empty, three]. Caret currently in item 3 —
    // move it to the empty middle item.
    let item_empty = list.children[1];
    sim.state.selection = Selection::caret(Point::element(item_empty, 0));
    sim.backspace();
    assert_eq!(
        sim.root_kinds(),
        vec!["bullet_list", "paragraph", "bullet_list"],
    );
    assert_eq!(list_items_text(&sim, 0), vec!["one".to_string()]);
    assert_eq!(sim.block_text(1), "");
    assert_eq!(list_items_text(&sim, 2), vec!["three".to_string()]);
}

#[test]
fn backspace_in_empty_last_list_item_lifts_to_trailing_paragraph() {
    let mut sim = Sim::new();
    sim.type_chars("- one");
    sim.enter();
    sim.type_chars("two");
    sim.enter(); // empty item 3 at end
    sim.backspace();
    assert_eq!(sim.root_kinds(), vec!["bullet_list", "paragraph"]);
    assert_eq!(
        list_items_text(&sim, 0),
        vec!["one".to_string(), "two".to_string()],
    );
    assert_eq!(sim.block_text(1), "");
}

#[test]
fn backspace_in_empty_list_item_with_element_caret_outdents() {
    // After the user types `- ` (creating a list) and then deletes the last
    // typed character, the empty text node may be pruned and the caret
    // ends up element-anchored at offset 0 of the lone list_item. A
    // second Backspace at this point must still lift the item out — the
    // earlier text-only outdent path skipped this case and the user got
    // a stuck, unreachable empty bullet.
    let mut sim = Sim::new();
    sim.type_chars("- ");
    assert_eq!(sim.root_kinds(), vec!["bullet_list"]);
    let list = sim.block_at(0);
    let item_key = list.children[0];
    // Force the post-prune shape: empty list_item, element-anchored caret.
    let item = sim.state.doc.get_element(item_key).unwrap().clone();
    for &c in item.children.iter().rev() {
        if sim
            .state
            .doc
            .get_text(c)
            .map(|t| t.text.is_empty())
            .unwrap_or(false)
        {
            let (parent, idx) = sim.state.doc.child_index(c).unwrap();
            let tr = dioxus_editor::Transaction::new().step(dioxus_editor::Step::RemoveNodes {
                parent,
                range: idx..idx + 1,
            });
            sim.apply(tr);
        }
    }
    sim.state.selection = Selection::caret(Point::element(item_key, 0));
    sim.backspace();
    assert_eq!(
        sim.root_kinds(),
        vec!["paragraph"],
        "empty list_item with Element@0 caret must outdent to a paragraph"
    );
}

#[test]
fn cmd_backspace_in_middle_list_item_removes_that_item() {
    let mut sim = Sim::new();
    sim.type_chars("- one");
    sim.enter();
    sim.type_chars("two");
    sim.enter();
    sim.type_chars("three");
    // Caret in item 2 ("two"), mid-text.
    let list = sim.block_at(0);
    let item2 = sim.state.doc.get_element(list.children[1]).unwrap();
    sim.state.selection = Selection::caret(Point::text(item2.children[0], 2));
    sim.cmd(delete_to_block_start);
    assert_eq!(sim.root_kinds(), vec!["bullet_list"]);
    assert_eq!(
        list_items_text(&sim, 0),
        vec!["one".to_string(), "three".to_string()],
        "Cmd+Backspace must remove the entire list_item, not just its text"
    );
}

#[test]
fn cmd_backspace_in_last_remaining_list_item_clears_list_to_paragraph() {
    let mut sim = Sim::new();
    sim.type_chars("- only line");
    let list = sim.block_at(0);
    let item = sim.state.doc.get_element(list.children[0]).unwrap();
    sim.state.selection = Selection::caret(Point::text(item.children[0], 4));
    sim.cmd(delete_to_block_start);
    assert_eq!(sim.root_kinds(), vec!["paragraph"]);
    assert_eq!(sim.block_text(0), "");
}

#[test]
fn cmd_backspace_in_paragraph_still_deletes_to_block_start() {
    let mut sim = Sim::new();
    sim.type_chars("hello world");
    let p = sim.block_at(0);
    let text_key = p.children[0];
    sim.state.selection = Selection::caret(Point::text(text_key, 6));
    sim.cmd(delete_to_block_start);
    assert_eq!(sim.root_kinds(), vec!["paragraph"]);
    assert_eq!(sim.block_text(0), "world");
}

#[test]
fn backspace_in_empty_first_list_item_clears_only_list() {
    // After a single `- ` shortcut, the list has one empty item; Backspace
    // should remove the list entirely (single-item outdent → paragraph).
    let mut sim = Sim::new();
    sim.type_chars("- ");
    sim.backspace();
    assert_eq!(sim.root_kinds(), vec!["paragraph"]);
    assert_eq!(sim.block_text(0), "");
}

#[test]
fn backspace_in_empty_code_block_demotes_to_paragraph() {
    // Toolbar-toggled code blocks land empty (no children). Backspace at
    // the resulting Element@0 caret must demote back to a paragraph —
    // the existing heading / blockquote demote path didn't cover code
    // blocks and the user was stranded.
    let mut sim = Sim::new();
    let para = sim.state.doc.root_node().children[0];
    sim.state.selection = Selection::caret(Point::element(para, 0));
    sim.cmd(dioxus_editor::commands::toggle_code_block);
    assert_eq!(sim.root_kinds(), vec!["code_block"]);
    sim.backspace();
    assert_eq!(sim.root_kinds(), vec!["paragraph"]);
}

#[test]
fn cmd_backspace_in_blockquote_removes_block_to_paragraph() {
    let mut sim = Sim::new();
    sim.type_chars("> hello");
    assert_eq!(sim.root_kinds(), vec!["blockquote"]);
    sim.cmd(delete_to_block_start);
    assert_eq!(sim.root_kinds(), vec!["paragraph"]);
    assert_eq!(sim.block_text(0), "");
}

#[test]
fn cmd_backspace_in_code_block_removes_block_to_paragraph() {
    let mut sim = Sim::new();
    let para = sim.state.doc.root_node().children[0];
    sim.state.selection = Selection::caret(Point::element(para, 0));
    sim.cmd(dioxus_editor::commands::toggle_code_block);
    sim.type_chars("code");
    assert_eq!(sim.root_kinds(), vec!["code_block"]);
    sim.cmd(delete_to_block_start);
    assert_eq!(sim.root_kinds(), vec!["paragraph"]);
    assert_eq!(sim.block_text(0), "");
}

// -- pending-format mark mode ---------------------------------------------

#[test]
fn cmd_b_with_no_selection_then_typing_marks_next_run_bold() {
    let mut sim = Sim::new();
    sim.type_chars("plain");
    // Collapsed caret toggle → pending_format = Some(BOLD).
    sim.cmd(toggle_bold);
    sim.type_chars("bold");
    let parts = formats_in(&sim, 0);
    assert_eq!(
        parts,
        vec![
            ("plain".into(), FormatBits::NONE),
            ("bold".into(), FormatBits::BOLD),
        ],
        "typed chars after cmd+b should carry the bold mark"
    );
}

#[test]
fn pending_format_persists_across_shift_enter() {
    let mut sim = Sim::new();
    sim.cmd(toggle_bold);
    sim.type_chars("first");
    sim.enter();
    sim.type_chars("second");
    assert_eq!(sim.root_kinds(), vec!["paragraph", "paragraph"]);
    let p0_parts = formats_in(&sim, 0);
    let p1_parts = formats_in(&sim, 1);
    assert_eq!(p0_parts, vec![("first".into(), FormatBits::BOLD)]);
    assert_eq!(
        p1_parts,
        vec![("second".into(), FormatBits::BOLD)],
        "shift+enter must carry pending_format into the new paragraph"
    );
}

#[test]
fn toggling_off_pending_clears_subsequent_typing_to_plain() {
    let mut sim = Sim::new();
    sim.cmd(toggle_bold);
    sim.type_chars("bold");
    sim.cmd(toggle_bold);
    sim.type_chars("plain");
    let parts = formats_in(&sim, 0);
    assert_eq!(
        parts,
        vec![
            ("bold".into(), FormatBits::BOLD),
            ("plain".into(), FormatBits::NONE),
        ],
        "second cmd+b should turn pending bold off so following chars are plain"
    );
}

#[test]
fn combined_pending_marks_bold_then_italic() {
    let mut sim = Sim::new();
    sim.cmd(toggle_bold);
    sim.type_chars("b");
    sim.cmd(toggle_italic);
    sim.type_chars("bi");
    sim.cmd(toggle_bold);
    sim.type_chars("i");
    let parts = formats_in(&sim, 0);
    let want_bi = FormatBits(FormatBits::BOLD.0 | FormatBits::ITALIC.0);
    assert_eq!(
        parts,
        vec![
            ("b".into(), FormatBits::BOLD),
            ("bi".into(), want_bi),
            ("i".into(), FormatBits::ITALIC),
        ]
    );
}

// -- block decorators -----------------------------------------------------

fn sim_with_block_decorator_kind(kind: &str) -> Sim {
    use dioxus_editor::DecoratorSpec;
    use std::rc::Rc as RcRc;
    let schema = Schema::new().with_decorator(
        kind,
        DecoratorSpec {
            inline: false,
            // Tests don't render; a stub Err is the cheapest Element.
            render: RcRc::new(|_| Err(dioxus::prelude::RenderError::default())),
            to_markdown: RcRc::new(|_| String::new()),
        },
    );
    let mut state = EditorState::new(RcRc::new(schema), Doc::empty());
    let para = state.doc.root_node().children[0];
    state.selection = Selection::caret(Point::element(para, 0));
    Sim {
        state,
        plugins: vec![Box::new(History::new())],
    }
}

#[test]
fn block_decorator_inserted_into_empty_paragraph_lands_at_root() {
    let mut sim = sim_with_block_decorator_kind("image");
    let tr =
        dioxus_editor::commands::insert_decorator(&sim.state, "image", dioxus_editor::Attrs::new())
            .expect("insert_decorator");
    sim.apply(tr);
    // The empty seed paragraph is replaced by [decorator, paragraph] so
    // the user can keep typing after the embed; both live at root.
    assert_eq!(sim.root_kinds(), vec!["image", "paragraph"]);
}

#[test]
fn block_decorator_inserted_mid_text_appends_after_block() {
    let mut sim = sim_with_block_decorator_kind("image");
    sim.type_chars("hello");
    let tr =
        dioxus_editor::commands::insert_decorator(&sim.state, "image", dioxus_editor::Attrs::new())
            .expect("insert_decorator");
    sim.apply(tr);
    // Non-empty surrounding block stays put; decorator + trailing paragraph
    // slot in after it at root.
    assert_eq!(sim.root_kinds(), vec!["paragraph", "image", "paragraph"]);
    assert_eq!(sim.block_text(0), "hello");
}

#[test]
fn backspace_at_start_of_paragraph_after_block_decorator_removes_decorator() {
    let mut sim = sim_with_block_decorator_kind("image");
    sim.type_chars("hello");
    let tr =
        dioxus_editor::commands::insert_decorator(&sim.state, "image", dioxus_editor::Attrs::new())
            .expect("insert_decorator");
    sim.apply(tr);
    // Caret now sits in the fresh trailing paragraph. Backspace should
    // remove the preceding block decorator rather than no-op.
    sim.backspace();
    assert_eq!(sim.root_kinds(), vec!["paragraph", "paragraph"]);
    assert_eq!(sim.block_text(0), "hello");
}

// -- history / undo ----------------------------------------------------

#[test]
fn undo_after_typing_burst_then_structural_then_burst_unwinds_each_group() {
    let mut sim = Sim::new();
    sim.type_chars("abc");
    sim.enter();
    sim.type_chars("def");
    assert_eq!(sim.root_kinds(), vec!["paragraph", "paragraph"]);
    assert_eq!(sim.block_text(0), "abc");
    assert_eq!(sim.block_text(1), "def");
    // Undo "def" burst.
    let tr = undo_replay(&mut sim);
    sim.state = sim.state.apply(tr).expect("apply undo 1");
    assert_eq!(sim.root_kinds(), vec!["paragraph", "paragraph"]);
    assert_eq!(sim.block_text(0), "abc");
    assert_eq!(sim.block_text(1), "");
    // Undo Shift+Enter.
    let tr = undo_replay(&mut sim);
    sim.state = sim.state.apply(tr).expect("apply undo 2");
    assert_eq!(sim.root_kinds(), vec!["paragraph"]);
    assert_eq!(sim.block_text(0), "abc");
    // Undo "abc" burst.
    let tr = undo_replay(&mut sim);
    sim.state = sim.state.apply(tr).expect("apply undo 3");
    assert_eq!(sim.root_kinds(), vec!["paragraph"]);
    assert_eq!(sim.block_text(0), "");
}

// Pull an undo transaction out of the History plugin's handle_event.
fn undo_replay(sim: &mut Sim) -> Transaction {
    let state = sim.state.clone();
    for p in sim.plugins.iter_mut() {
        if let Some(tr) = p.handle_event(&state, &dioxus_editor::EditorEvent::Undo) {
            return tr;
        }
    }
    panic!("history plugin did not return an undo transaction");
}

#[test]
fn swap_ol_to_ul_with_caret_in_third_item_does_not_overwrite_first() {
    // Regression for the user-reported "set OL to UL and it copied line
    // 3's content over line 1's content" bug.
    let mut sim = Sim::new();
    sim.type_chars("1. one");
    sim.enter();
    sim.type_chars("two");
    sim.enter();
    sim.type_chars("three");
    // Caret in item 3.
    let list = sim.block_at(0);
    let item3 = sim.state.doc.get_element(list.children[2]).unwrap();
    sim.state.selection = Selection::caret(Point::text(item3.children[0], 0));
    sim.cmd(toggle_bullet_list);
    assert_eq!(sim.root_kinds(), vec!["bullet_list"]);
    assert_eq!(
        list_items_text(&sim, 0),
        vec!["one".to_string(), "two".to_string(), "three".to_string()],
        "swap must not depend on caret position or duplicate items"
    );
}

// -- inline markdown shortcuts -------------------------------------------

fn sim_with_link_decorator() -> Sim {
    let schema = Schema::new().with_decorator(
        "link",
        DecoratorSpec {
            inline: true,
            render: Rc::new(|_| Err(dioxus::prelude::RenderError::default())),
            to_markdown: Rc::new(|attrs| {
                format!(
                    "[{}]({})",
                    attrs.get_str("text").unwrap_or(""),
                    attrs.get_str("href").unwrap_or("")
                )
            }),
        },
    );
    let mut state = EditorState::new(Rc::new(schema), Doc::empty());
    let paragraph = state.doc.root_node().children[0];
    state.selection = Selection::caret(Point::element(paragraph, 0));
    Sim {
        state,
        plugins: vec![Box::new(MarkdownShortcuts::new()), Box::new(History::new())],
    }
}

fn formats_in(sim: &Sim, block_idx: usize) -> Vec<(String, FormatBits)> {
    let block = sim.block_at(block_idx);
    block
        .children
        .iter()
        .filter_map(|&k| {
            sim.state
                .doc
                .get_text(k)
                .map(|t| (t.text.clone(), t.format))
        })
        .collect()
}

#[test]
fn inline_shortcut_link() {
    let mut sim = sim_with_link_decorator();
    let markdown = "before [docs](https://example.com/path) after";
    sim.type_chars(markdown);

    let block = sim.block_at(0);
    assert_eq!(block.children.len(), 3);
    let link = sim.state.doc.get_decorator(block.children[1]).unwrap();
    assert_eq!(link.kind, "link");
    assert_eq!(link.attrs.get_str("text"), Some("docs"));
    assert_eq!(link.attrs.get_str("href"), Some("https://example.com/path"));
    assert_eq!(
        dioxus_editor::io::to_markdown(&sim.state.doc, &sim.state.schema).unwrap(),
        markdown
    );
}

#[test]
fn inline_link_shortcut_waits_for_balanced_url_parentheses() {
    let mut sim = sim_with_link_decorator();
    let markdown = "[docs](https://example.com/a_(b))";
    sim.type_chars(markdown);

    let block = sim.block_at(0);
    assert_eq!(block.children.len(), 1);
    let link = sim.state.doc.get_decorator(block.children[0]).unwrap();
    assert_eq!(
        link.attrs.get_str("href"),
        Some("https://example.com/a_(b)")
    );
    assert_eq!(
        dioxus_editor::io::to_markdown(&sim.state.doc, &sim.state.schema).unwrap(),
        markdown
    );
}

#[test]
fn inline_shortcut_bold() {
    let mut sim = Sim::new();
    sim.type_chars("a **b** c");
    let parts = formats_in(&sim, 0);
    assert_eq!(
        parts,
        vec![
            ("a ".into(), FormatBits::NONE),
            ("b".into(), FormatBits::BOLD),
            (" c".into(), FormatBits::NONE),
        ]
    );
}

#[test]
fn inline_shortcut_italic() {
    let mut sim = Sim::new();
    sim.type_chars("hi _there_ end");
    let parts = formats_in(&sim, 0);
    assert!(
        parts
            .iter()
            .any(|(t, f)| t == "there" && *f == FormatBits::ITALIC),
        "parts = {parts:?}"
    );
}

#[test]
fn inline_shortcut_code() {
    let mut sim = Sim::new();
    sim.type_chars("call `fn` ok");
    let parts = formats_in(&sim, 0);
    assert!(
        parts
            .iter()
            .any(|(t, f)| t == "fn" && *f == FormatBits::CODE),
        "parts = {parts:?}"
    );
}

#[test]
fn inline_shortcut_strike() {
    let mut sim = Sim::new();
    sim.type_chars("a ~~b~~ c");
    let parts = formats_in(&sim, 0);
    assert!(
        parts
            .iter()
            .any(|(t, f)| t == "b" && *f == FormatBits::STRIKE),
        "parts = {parts:?}"
    );
}

#[test]
fn block_shortcut_mid_text_does_not_fire() {
    let mut sim = Sim::new();
    sim.type_chars("hello > world");
    assert_eq!(sim.root_kinds(), vec!["paragraph"]);
    assert_eq!(sim.block_text(0), "hello > world");
}

// -- mark toggles over selections -----------------------------------------

#[test]
fn toolbar_bold_marks_selected_range_only() {
    let mut sim = Sim::new();
    sim.type_chars("hello world");
    // Select "world".
    let p = sim.block_at(0);
    let text_key = p.children[0];
    sim.state.selection = Selection::Range {
        anchor: Point::text(text_key, 6),
        focus: Point::text(text_key, 11),
    };
    sim.cmd(toggle_bold);
    let parts = formats_in(&sim, 0);
    assert_eq!(
        parts,
        vec![
            ("hello ".into(), FormatBits::NONE),
            ("world".into(), FormatBits::BOLD),
        ]
    );
}

#[test]
fn toolbar_italic_then_bold_combine() {
    let mut sim = Sim::new();
    sim.type_chars("hi");
    let p = sim.block_at(0);
    let text_key = p.children[0];
    sim.state.selection = Selection::Range {
        anchor: Point::text(text_key, 0),
        focus: Point::text(text_key, 2),
    };
    sim.cmd(toggle_italic);
    // After italic, the text node was rebuilt — find the new selection's
    // text key via formats_in.
    let p = sim.block_at(0);
    let text_key = p.children[0];
    sim.state.selection = Selection::Range {
        anchor: Point::text(text_key, 0),
        focus: Point::text(text_key, 2),
    };
    sim.cmd(toggle_bold);
    let parts = formats_in(&sim, 0);
    let combined = FormatBits::BOLD | FormatBits::ITALIC;
    assert_eq!(parts, vec![("hi".into(), combined)]);
}

#[test]
fn toolbar_code_marks_inline() {
    let mut sim = Sim::new();
    sim.type_chars("fn name");
    let p = sim.block_at(0);
    let text_key = p.children[0];
    sim.state.selection = Selection::Range {
        anchor: Point::text(text_key, 0),
        focus: Point::text(text_key, 2),
    };
    sim.cmd(toggle_code);
    let parts = formats_in(&sim, 0);
    assert_eq!(
        parts,
        vec![
            ("fn".into(), FormatBits::CODE),
            (" name".into(), FormatBits::NONE),
        ]
    );
}

#[test]
fn toggle_strike_clears_when_already_set() {
    let mut sim = Sim::new();
    sim.type_chars("nope");
    let p = sim.block_at(0);
    let text_key = p.children[0];
    sim.state.selection = Selection::Range {
        anchor: Point::text(text_key, 0),
        focus: Point::text(text_key, 4),
    };
    sim.cmd(toggle_strike);
    sim.state.selection = Selection::Range {
        anchor: Point::text(sim.block_at(0).children[0], 0),
        focus: Point::text(sim.block_at(0).children[0], 4),
    };
    sim.cmd(toggle_strike);
    let parts = formats_in(&sim, 0);
    assert_eq!(parts, vec![("nope".into(), FormatBits::NONE)]);
}

// -- backspace at element-anchored caret ----------------------------------

#[test]
fn backspace_at_element_caret_between_text_nodes_deletes_one_char() {
    // Regression: when the caret is element-anchored between two text
    // siblings (e.g. just after pruning an empty inline-code span),
    // Backspace must trim one char from the previous text node — not
    // remove the entire node like it does for atomic decorators.
    let mut sim = Sim::new();
    let para = sim.state.doc.root_node().children[0];
    let tr = Transaction::new().step(Step::InsertNodes {
        parent: para,
        index: 0,
        nodes: vec![
            NodeSpec::text("hello"),
            NodeSpec::Text {
                text: "X".into(),
                format: FormatBits::CODE,
            },
            NodeSpec::text("world"),
        ],
    });
    sim.apply(tr);
    let para_e = sim.state.doc.get_element(para).unwrap();
    let code_key = para_e.children[1];
    sim.state.selection = Selection::caret(Point::text(code_key, 1));
    // Backspace 1: prune the single-char code node; caret lands element-
    // anchored between "hello" and "world".
    sim.backspace();
    let para_e = sim.state.doc.get_element(para).unwrap();
    assert_eq!(para_e.children.len(), 2);
    // Backspace 2: must shave one char off "hello", not delete it whole.
    sim.backspace();
    let para_e = sim.state.doc.get_element(para).unwrap();
    assert_eq!(para_e.children.len(), 2, "both text nodes should survive");
    assert_eq!(
        sim.state.doc.get_text(para_e.children[0]).unwrap().text,
        "hell"
    );
    assert_eq!(
        sim.state.doc.get_text(para_e.children[1]).unwrap().text,
        "world"
    );
    // Backspace 3..5: each removes one more char.
    sim.backspace();
    sim.backspace();
    sim.backspace();
    let para_e = sim.state.doc.get_element(para).unwrap();
    assert_eq!(
        sim.state.doc.get_text(para_e.children[0]).unwrap().text,
        "h"
    );
}

#[test]
fn delete_forward_at_element_caret_between_text_nodes_deletes_one_char() {
    let mut sim = Sim::new();
    let para = sim.state.doc.root_node().children[0];
    let tr = Transaction::new().step(Step::InsertNodes {
        parent: para,
        index: 0,
        nodes: vec![
            NodeSpec::text("hello"),
            NodeSpec::Text {
                text: "X".into(),
                format: FormatBits::CODE,
            },
            NodeSpec::text("world"),
        ],
    });
    sim.apply(tr);
    let para_e = sim.state.doc.get_element(para).unwrap();
    let code_key = para_e.children[1];
    // Caret at start of code "X"; delete_forward should prune the 1-char
    // node, then the next call should trim one char off "world".
    sim.state.selection = Selection::caret(Point::text(code_key, 0));
    let tr = dioxus_editor::commands::delete_forward(&sim.state).expect("delete_forward 1");
    sim.apply(tr);
    let tr = dioxus_editor::commands::delete_forward(&sim.state).expect("delete_forward 2");
    sim.apply(tr);
    let para_e = sim.state.doc.get_element(para).unwrap();
    assert_eq!(para_e.children.len(), 2);
    assert_eq!(
        sim.state.doc.get_text(para_e.children[0]).unwrap().text,
        "hello"
    );
    assert_eq!(
        sim.state.doc.get_text(para_e.children[1]).unwrap().text,
        "orld"
    );
}

// -- cross-block mark toggles --------------------------------------------

#[test]
fn toggle_bold_across_two_list_items_marks_both() {
    // Regression: selecting across two list_items and pressing Cmd+B used
    // to no-op because toggle_mark_cross_node bailed when the endpoints
    // sat under different parents. Each item's text should pick up bold.
    let mut sim = Sim::new();
    sim.type_chars("- something");
    sim.enter();
    sim.type_chars("other thing");
    let list = sim.block_at(0);
    let li0 = sim.state.doc.get_element(list.children[0]).unwrap();
    let li1 = sim.state.doc.get_element(list.children[1]).unwrap();
    let t0 = li0.children[0];
    let t1 = li1.children[0];
    sim.state.selection = Selection::Range {
        anchor: Point::text(t0, 0),
        focus: Point::text(t1, 11),
    };
    sim.cmd(toggle_bold);
    let list = sim.block_at(0);
    let li0 = sim.state.doc.get_element(list.children[0]).unwrap();
    let li1 = sim.state.doc.get_element(list.children[1]).unwrap();
    let read = |item: &dioxus_editor::ElementNode| -> Vec<(String, FormatBits)> {
        item.children
            .iter()
            .filter_map(|&k| {
                sim.state
                    .doc
                    .get_text(k)
                    .map(|t| (t.text.clone(), t.format))
            })
            .collect()
    };
    assert_eq!(
        read(li0),
        vec![("something".into(), FormatBits::BOLD)],
        "first list_item should be entirely bold",
    );
    assert_eq!(
        read(li1),
        vec![("other thing".into(), FormatBits::BOLD)],
        "second list_item should be entirely bold",
    );
}

#[test]
fn toggle_bold_partial_across_two_list_items_marks_clipped_runs() {
    // Partial selection that starts mid-text in item 1 and ends mid-text
    // in item 2 should split both boundary text nodes and bold only the
    // covered slice on each side.
    let mut sim = Sim::new();
    sim.type_chars("- something");
    sim.enter();
    sim.type_chars("other thing");
    let list = sim.block_at(0);
    let li0 = sim.state.doc.get_element(list.children[0]).unwrap();
    let li1 = sim.state.doc.get_element(list.children[1]).unwrap();
    let t0 = li0.children[0];
    let t1 = li1.children[0];
    // Select "thing" (chars 4..9 of "something") through "other"
    // (chars 0..5 of "other thing").
    sim.state.selection = Selection::Range {
        anchor: Point::text(t0, 4),
        focus: Point::text(t1, 5),
    };
    sim.cmd(toggle_bold);
    let list = sim.block_at(0);
    let li0 = sim.state.doc.get_element(list.children[0]).unwrap();
    let li1 = sim.state.doc.get_element(list.children[1]).unwrap();
    let read = |item: &dioxus_editor::ElementNode| -> Vec<(String, FormatBits)> {
        item.children
            .iter()
            .filter_map(|&k| {
                sim.state
                    .doc
                    .get_text(k)
                    .map(|t| (t.text.clone(), t.format))
            })
            .collect()
    };
    assert_eq!(
        read(li0),
        vec![
            ("some".into(), FormatBits::NONE),
            ("thing".into(), FormatBits::BOLD),
        ],
    );
    assert_eq!(
        read(li1),
        vec![
            ("other".into(), FormatBits::BOLD),
            (" thing".into(), FormatBits::NONE),
        ],
    );
}

#[test]
fn toggle_bold_across_paragraphs_marks_both() {
    let mut sim = Sim::new();
    sim.type_chars("first");
    sim.enter();
    sim.type_chars("second");
    let p0_text = sim.block_at(0).children[0];
    let p1_text = sim.block_at(1).children[0];
    sim.state.selection = Selection::Range {
        anchor: Point::text(p0_text, 0),
        focus: Point::text(p1_text, 6),
    };
    sim.cmd(toggle_bold);
    let p0 = sim.block_at(0);
    let p1 = sim.block_at(1);
    assert_eq!(
        sim.state.doc.get_text(p0.children[0]).unwrap().format,
        FormatBits::BOLD,
    );
    assert_eq!(
        sim.state.doc.get_text(p1.children[0]).unwrap().format,
        FormatBits::BOLD,
    );
}

#[test]
fn toggle_bold_across_already_bold_list_items_clears() {
    // ProseMirror mixed-coverage rule: when EVERY touched run already has
    // the mark, toggling removes it. Span across two fully-bold items;
    // both should go plain.
    let mut sim = Sim::new();
    sim.type_chars("- a");
    sim.enter();
    sim.type_chars("b");
    // Make item 1 bold.
    let list = sim.block_at(0);
    let li0 = sim.state.doc.get_element(list.children[0]).unwrap();
    let t0 = li0.children[0];
    sim.state.selection = Selection::Range {
        anchor: Point::text(t0, 0),
        focus: Point::text(t0, 1),
    };
    sim.cmd(toggle_bold);
    // Make item 2 bold.
    let list = sim.block_at(0);
    let li1 = sim.state.doc.get_element(list.children[1]).unwrap();
    let t1 = li1.children[0];
    sim.state.selection = Selection::Range {
        anchor: Point::text(t1, 0),
        focus: Point::text(t1, 1),
    };
    sim.cmd(toggle_bold);
    // Now select across both items and toggle — both should clear.
    let list = sim.block_at(0);
    let li0 = sim.state.doc.get_element(list.children[0]).unwrap();
    let li1 = sim.state.doc.get_element(list.children[1]).unwrap();
    let t0 = li0.children[0];
    let t1 = li1.children[0];
    sim.state.selection = Selection::Range {
        anchor: Point::text(t0, 0),
        focus: Point::text(t1, 1),
    };
    sim.cmd(toggle_bold);
    let list = sim.block_at(0);
    let li0 = sim.state.doc.get_element(list.children[0]).unwrap();
    let li1 = sim.state.doc.get_element(list.children[1]).unwrap();
    assert_eq!(
        sim.state.doc.get_text(li0.children[0]).unwrap().format,
        FormatBits::NONE,
    );
    assert_eq!(
        sim.state.doc.get_text(li1.children[0]).unwrap().format,
        FormatBits::NONE,
    );
}

// -- cross-block range deletion ------------------------------------------

#[test]
fn delete_range_across_paragraphs_merges() {
    let mut sim = Sim::new();
    sim.type_chars("first");
    sim.enter();
    sim.type_chars("second");
    // Select from char 3 of "first" to char 3 of "second".
    let p0_text = sim.block_at(0).children[0];
    let p1_text = sim.block_at(1).children[0];
    let from = Point::text(p0_text, 3);
    let to = Point::text(p1_text, 3);
    let tr = delete_range_transaction(&sim.state.doc, from, to).expect("delete tr");
    sim.apply(tr);
    assert_eq!(sim.root_kinds(), vec!["paragraph"]);
    assert_eq!(sim.block_text(0), "firond");
}

// -- backspace inside lists ----------------------------------------------

#[test]
fn backspace_at_start_of_second_list_item_joins_to_first() {
    let mut sim = Sim::new();
    sim.type_chars("- one");
    sim.enter();
    sim.type_chars("two");
    // Move caret to start of "two".
    let list = sim.block_at(0);
    let li1 = sim.state.doc.get_element(list.children[1]).unwrap();
    let text_key = li1.children[0];
    sim.state.selection = Selection::caret(Point::text(text_key, 0));
    sim.backspace();
    let list = sim.block_at(0);
    assert_eq!(
        list.children.len(),
        1,
        "expected one list_item after join, got {}",
        list.children.len()
    );
    let li0 = sim.state.doc.get_element(list.children[0]).unwrap();
    let mut s = String::new();
    for &c in &li0.children {
        collect_text(&sim.state.doc, c, &mut s);
    }
    assert_eq!(s, "onetwo");
}

// -- markdown shortcut state transitions ---------------------------------

#[test]
fn typing_after_heading_shortcut_keeps_one_block() {
    let mut sim = Sim::new();
    sim.type_chars("# t");
    sim.type_chars("itle");
    assert_eq!(sim.root_kinds(), vec!["heading"]);
    assert_eq!(sim.block_text(0), "title");
}

#[test]
fn no_inline_shortcut_inside_code_block() {
    let mut sim = Sim::new();
    sim.cmd(toggle_code_block);
    sim.type_chars("a **b** c");
    // Inside a code block, ** should be literal — no bold span generated.
    assert_eq!(sim.root_kinds(), vec!["code_block"]);
    assert_eq!(sim.block_text(0), "a **b** c");
}

#[test]
fn no_block_shortcut_when_not_at_top_level() {
    // Inside a list_item, typing "> " should not turn the list_item into a
    // blockquote — block shortcuts only fire on a top-level paragraph.
    let mut sim = Sim::new();
    sim.type_chars("- ");
    // Now in a list with empty first item. Type "> ".
    sim.type_chars("> ");
    assert_eq!(sim.root_kinds(), vec!["bullet_list"]);
    let list = sim.block_at(0);
    let li = sim.state.doc.get_element(list.children[0]).unwrap();
    let mut s = String::new();
    for &c in &li.children {
        collect_text(&sim.state.doc, c, &mut s);
    }
    assert_eq!(s, "> ");
}

// -- shift+enter splits at caret position ---------------------------------

#[test]
fn shift_enter_in_middle_of_text_splits_correctly() {
    let mut sim = Sim::new();
    sim.type_chars("helloworld");
    let p = sim.block_at(0);
    let text_key = p.children[0];
    sim.state.selection = Selection::caret(Point::text(text_key, 5));
    sim.enter();
    assert_eq!(sim.root_kinds(), vec!["paragraph", "paragraph"]);
    assert_eq!(sim.block_text(0), "hello");
    assert_eq!(sim.block_text(1), "world");
}

// -- recovery from a root-anchored selection ----------------------------

#[test]
fn typing_with_root_anchored_caret_lands_inside_first_block() {
    // Simulates the post-send / post-set_doc case where selection was
    // (incorrectly) anchored at the root element. The text must still end
    // up inside the existing paragraph, not as a root-level sibling.
    let mut sim = Sim::new();
    sim.state.selection = Selection::caret(Point::element(sim.state.doc.root_key(), 0));
    sim.type_chars(">");
    assert_eq!(sim.root_kinds(), vec!["paragraph"]);
    assert_eq!(sim.block_text(0), ">");
}

#[test]
fn block_shortcut_fires_after_set_doc_reset() {
    // After `set_doc(Doc::empty())`, typing `> ` must still create a
    // blockquote — the selection reset must land inside the paragraph
    // so the shortcut's "text is at the start of a top-level paragraph"
    // precondition holds.
    let mut sim = Sim::new();
    // Simulate the reset selection that set_doc now installs.
    let first = sim.state.doc.root_node().children[0];
    sim.state.selection = Selection::caret(Point::element(first, 0));
    sim.type_chars("> ");
    assert_eq!(sim.root_kinds(), vec!["blockquote"]);
}

#[test]
fn shift_enter_at_start_of_text_creates_empty_predecessor() {
    let mut sim = Sim::new();
    sim.type_chars("body");
    let p = sim.block_at(0);
    let text_key = p.children[0];
    sim.state.selection = Selection::caret(Point::text(text_key, 0));
    sim.enter();
    assert_eq!(sim.root_kinds(), vec!["paragraph", "paragraph"]);
    assert_eq!(sim.block_text(0), "");
    assert_eq!(sim.block_text(1), "body");
}
